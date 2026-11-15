//! Drawing a frame: the arm (tiles and shadow), dirty ink tiles and the toolbar, each in
//! its own window, with pixels uploaded through the staging area.

use std::time::Instant;

use x11rb::{
    connection::Connection as _,
    protocol::{
        render::{self, ConnectionExt as _},
        xproto::{ConnectionExt as _, Pixmap},
    },
    NONE,
};

use super::{
    display::{Display, Win},
    App, Geom, X11Ext, XResult, PARKED,
};
use crate::{
    config,
    hand::Sprite,
    ink::Key,
    pose::Pose,
    raster::{self, Frame},
    session::{self, ArmJob, BarView, Passthrough},
};

/// The quarter-resolution shadow pixmap and its (bilinear, scaling) picture.
struct ShadowSrc {
    pix: Pixmap,
    pict: render::Picture,
    /// (buffer w, buffer h, window w, window h) it was made for.
    size: (usize, usize, u16, u16),
}

/// Our drawing windows, apart from ink tiles (those belong to the strokes).
#[derive(Default)]
pub(super) struct Scene {
    pub tiles: Vec<Win>,
    pub shadow: Option<Win>,
    shadow_src: Option<ShadowSrc>,
    pub bar: Option<Win>,
    /// Opacity last set on the bar window (NaN: none yet).
    bar_alpha: f32,
    /// The shadow before upscaling, when XRender can't scale it.
    scratch: Vec<u32>,
}

impl Scene {
    pub fn windows_mut(&mut self) -> impl Iterator<Item = &mut Win> {
        self.tiles.iter_mut().chain(self.shadow.iter_mut()).chain(self.bar.iter_mut())
    }

    pub fn arm_windows(&self) -> impl Iterator<Item = &Win> {
        self.tiles.iter().chain(self.shadow.iter())
    }

    /// Park the arm's windows (hide them without unmapping).
    pub fn park_arm(&mut self, dpy: &Display) -> XResult<()> {
        for w in self.tiles.iter_mut().chain(self.shadow.iter_mut()) {
            dpy.place(w, PARKED)?;
        }
        Ok(())
    }

    pub fn park_bar(&mut self, dpy: &Display) -> XResult<()> {
        if let Some(w) = self.bar.as_mut() {
            dpy.place(w, PARKED)?;
        }
        Ok(())
    }

    pub fn destroy(&mut self, dpy: &mut Display) {
        for w in self.tiles.drain(..).chain(self.shadow.take()).chain(self.bar.take()) {
            dpy.destroy_win(w);
        }
        if let Some(s) = self.shadow_src.take() {
            let _ = dpy.conn.render_free_picture(s.pict);
            let _ = dpy.conn.free_pixmap(s.pix);
        }
    }

    fn ensure_tiles(&mut self, dpy: &mut Display, n: usize) -> XResult<()> {
        while self.tiles.len() < n {
            self.tiles.push(dpy.new_win(false)?);
        }
        Ok(())
    }
}

/// Where logical frames land on the root window: the monitor's origin and the scale.
#[derive(Clone, Copy)]
struct Place {
    x: i16,
    y: i16,
    scale: f32,
}

impl Place {
    /// Device-pixel position and size of a logical frame.
    fn geom(&self, f: &Frame) -> Geom {
        let s = self.scale;
        let clamp16 = |v: f32| v.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        let size = |v: i32| (v as f32 * s).round().clamp(1.0, 65535.0) as u16;
        (clamp16(self.x as f32 + f.x as f32 * s), clamp16(self.y as f32 + f.y as f32 * s), size(f.w), size(f.h))
    }

    /// Like `geom`, but exactly the buffer's size (frames are snapped so they match).
    fn buffer_geom(&self, f: &Frame) -> Geom {
        let (x, y, _, _) = self.geom(f);
        (x, y, f.bw as u16, f.bh as u16)
    }
}

impl App {
    fn place(&self) -> Place {
        let m = self.mons[self.mon];
        Place { x: m.x, y: m.y, scale: self.scale }
    }

    /// Draw whatever changed. Called when the session wants a frame and the server is
    /// done with the last one.
    pub(super) fn draw(&mut self, now: Instant) -> XResult<()> {
        self.session.dirty = false;
        let place = self.place();
        // During a timed scroll-through the hand stays (as on macOS); click-for-real
        // shows the real cursor instead.
        let arm = (self.session.passthrough() != Passthrough::Real).then(|| self.session.plan_arm(now)).flatten();
        let bar = self.session.bar_view(now);
        let ink: Vec<Vec<Key>> = self.session.ink.strokes_mut().map(|s| s.take_dirty()).collect();

        // Reserve the frame's pixels up front, so the shared segment is big enough
        // before anything is written into it.
        let mut total = 0;
        if let Some(job) = &arm {
            total += self.arm_pixels(job, place);
        }
        for (s, keys) in self.session.ink.strokes().zip(&ink) {
            total += keys.len() * s.stroke.tile_px() * s.stroke.tile_px();
        }
        if let Some(v) = bar.filter(|v| v.mapped && v.render) {
            total += v.frame.bw * v.frame.bh;
        }
        let t0 = Instant::now();
        if total > 0 {
            self.dpy.staging.reserve(&self.dpy.conn, total + 1)?;
            self.dpy.staging.alloc(1); // pixel 0: the completion marker's source
        }
        if let Some(job) = arm {
            self.draw_arm(job, place)?;
        }
        self.draw_ink(ink, place)?;
        self.draw_bar(bar, place)?;
        let took = t0.elapsed();
        if self.dpy.restack {
            self.restack()?;
        }
        if total > 0 {
            self.dpy.staging.finish(&self.dpy.conn, self.dpy.gc, now)?;
            if self.session.stats.record(took) && self.session.cfg.stats {
                eprintln!(
                    "{} staging={}KB strokes={} windows={}",
                    self.session.stats.summary(),
                    self.dpy.staging.kb(),
                    self.session.ink.strokes().count(),
                    self.dpy.ours.len()
                );
            }
        }
        Ok(())
    }

    /// Pixels the arm needs this frame (must match what `draw_arm` allocates).
    fn arm_pixels(&self, job: &ArmJob, place: Place) -> usize {
        match job {
            ArmJob::Laser { frame, .. } => frame.map_or(0, |f| f.bw * f.bh),
            ArmJob::Hand { plan, .. } => {
                let tiles: usize = plan.tiles.iter().map(|t| t.frame.bw * t.frame.bh).sum();
                let shadow = plan.shadow.map_or(0, |f| {
                    if self.dpy.pictfmt.is_some() {
                        f.bw * f.bh
                    } else {
                        // Scaled up to the window's size on our side.
                        let (_, _, w, h) = place.geom(&f);
                        w as usize * h as usize
                    }
                });
                tiles + shadow
            }
        }
    }

    fn draw_arm(&mut self, job: ArmJob, place: Place) -> XResult<()> {
        let App { dpy, session, scene, .. } = self;
        let Some(out) = session.output() else { return Ok(()) };
        match job {
            ArmJob::Laser { laser, frame } => {
                scene.ensure_tiles(dpy, 1)?;
                if let Some(s) = scene.shadow.as_mut() {
                    dpy.place(s, PARKED)?;
                }
                for w in scene.tiles.iter_mut().skip(1) {
                    dpy.place(w, PARKED)?;
                }
                match frame {
                    Some(f) => {
                        let at = dpy.staging.alloc(f.bw * f.bh);
                        laser.render(&f, config::LASER, dpy.staging.slice(at, f.bw * f.bh));
                        dpy.show(&mut scene.tiles[0], place.buffer_geom(&f), at, f.bw, f.bh)?;
                    }
                    None => dpy.place(&mut scene.tiles[0], PARKED)?,
                }
            }
            ArmJob::Hand { pose, plan } => {
                let Some(sprite) = session.sprite() else { return Ok(()) };
                let (out_h, scale) = (out.h as f32, out.scale);
                scene.ensure_tiles(dpy, plan.tiles.len())?;
                match plan.shadow {
                    Some(f) => draw_shadow(dpy, scene, &pose, sprite, &f, place, out_h, scale)?,
                    None => {
                        if let Some(s) = scene.shadow.as_mut() {
                            dpy.place(s, PARKED)?;
                        }
                    }
                }
                for (k, win) in scene.tiles.iter_mut().enumerate() {
                    match plan.tiles.get(k) {
                        Some(tile) => {
                            let f = tile.frame;
                            let at = dpy.staging.alloc(f.bw * f.bh);
                            raster::render_tile(&pose, sprite, tile, out_h, scale, dpy.staging.slice(at, f.bw * f.bh));
                            dpy.show(win, place.buffer_geom(&f), at, f.bw, f.bh)?;
                        }
                        None => dpy.place(win, PARKED)?,
                    }
                }
            }
        }
        Ok(())
    }

    /// Upload dirty ink tiles (`keys`: per stroke, in `strokes_mut` order).
    fn draw_ink(&mut self, keys: Vec<Vec<Key>>, place: Place) -> XResult<()> {
        let App { dpy, session, .. } = self;
        let Some(out) = session.output() else { return Ok(()) };
        let edge = session::ink_tile(out.scale);
        for (st, keys) in session.ink.strokes_mut().zip(keys) {
            if keys.is_empty() {
                continue;
            }
            let pal = st.stroke.palette(st.alpha);
            let px = st.stroke.tile_px();
            for key in keys {
                let win = match st.surfaces.entry(key) {
                    std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                    std::collections::hash_map::Entry::Vacant(e) => e.insert(dpy.new_win(false)?),
                };
                let (x, y) = st.stroke.tile_origin(key);
                let f = Frame { x, y, w: edge, h: edge, bw: px, bh: px };
                let at = dpy.staging.alloc(px * px);
                st.stroke.render_tile_with(key, &pal, dpy.staging.slice(at, px * px));
                dpy.show(win, place.buffer_geom(&f), at, px, px)?;
            }
        }
        Ok(())
    }

    fn draw_bar(&mut self, view: Option<BarView>, place: Place) -> XResult<()> {
        let App { dpy, session, scene, .. } = self;
        let Some(v) = view.filter(|v| v.mapped) else { return scene.park_bar(dpy) };
        if scene.bar.is_none() {
            scene.bar = Some(dpy.new_win(false)?);
            scene.bar_alpha = f32::NAN;
        }
        let Some(win) = scene.bar.as_mut() else { return Ok(()) };
        if v.alpha != scene.bar_alpha && dpy.composited {
            dpy.set_opacity(win.id, v.alpha)?;
            scene.bar_alpha = v.alpha;
        }
        if v.render {
            let f = v.frame;
            let at = dpy.staging.alloc(f.bw * f.bh);
            session.render_bar(dpy.staging.slice(at, f.bw * f.bh));
            dpy.show(win, place.buffer_geom(&f), at, f.bw, f.bh)?;
        }
        Ok(())
    }
}

/// The shadow: rendered at quarter resolution, then scaled up to the window's size by
/// XRender (bilinear) if the server has it, else by us.
#[allow(clippy::too_many_arguments)]
fn draw_shadow(
    dpy: &mut Display,
    scene: &mut Scene,
    pose: &Pose,
    sprite: &Sprite,
    f: &Frame,
    place: Place,
    out_h: f32,
    scale: f32,
) -> XResult<()> {
    if scene.shadow.is_none() {
        scene.shadow = Some(dpy.new_win(true)?);
    }
    let g = place.geom(f);
    let (dw, dh) = (g.2, g.3);
    let n = f.bw * f.bh;
    let win = scene.shadow.as_mut().ok_or("no shadow window")?;
    if dpy.pictfmt.is_some() && win.pict != NONE {
        let at = dpy.staging.alloc(n);
        raster::render_shadow(pose, sprite, f, out_h, scale, dpy.staging.slice(at, n));
        let src = shadow_src(dpy, &mut scene.shadow_src, f.bw, f.bh, dw, dh)?;
        dpy.staging.put(&dpy.conn, dpy.gc, src.0, f.bw, f.bh, 0, 0, at)?;
        dpy.place(win, g)?;
        win.seq = dpy
            .conn
            .render_composite(render::PictOp::SRC, src.1, NONE, win.pict, 0, 0, 0, 0, 0, 0, dw, dh)
            .x11()?
            .sequence_number();
    } else {
        if scene.scratch.len() < n {
            scene.scratch.resize(n, 0);
        }
        raster::render_shadow(pose, sprite, f, out_h, scale, &mut scene.scratch[..n]);
        let (dw, dh) = (dw as usize, dh as usize);
        let at = dpy.staging.alloc(dw * dh);
        upscale(&scene.scratch[..n], f.bw, f.bh, dpy.staging.slice(at, dw * dh), dw, dh);
        dpy.place(win, g)?;
        win.seq = dpy.staging.put(&dpy.conn, dpy.gc, win.id, dw, dh, 0, 0, at)?;
    }
    Ok(())
}

/// The shadow source pixmap/picture for a bw×bh buffer shown at dw×dh.
fn shadow_src(
    dpy: &Display,
    cache: &mut Option<ShadowSrc>,
    bw: usize,
    bh: usize,
    dw: u16,
    dh: u16,
) -> XResult<(Pixmap, render::Picture)> {
    let size = (bw, bh, dw, dh);
    if let Some(s) = cache.as_ref().filter(|s| s.size == size) {
        return Ok((s.pix, s.pict));
    }
    let c = &dpy.conn;
    if let Some(old) = cache.take() {
        c.render_free_picture(old.pict).x11()?;
        c.free_pixmap(old.pix).x11()?;
    }
    let fmt = dpy.pictfmt.ok_or("X11: no ARGB picture format")?;
    let pix = c.generate_id().x11()?;
    c.create_pixmap(32, pix, dpy.root, bw as u16, bh as u16).x11()?;
    let pict = c.generate_id().x11()?;
    c.render_create_picture(pict, pix, fmt, &render::CreatePictureAux::new()).x11()?;
    c.render_set_picture_filter(pict, b"bilinear", &[]).x11()?;
    // The transform maps destination to source pixels.
    let fx = |v: f64| (v * 65536.0).round() as i32;
    let t = render::Transform {
        matrix11: fx(bw as f64 / dw as f64),
        matrix12: 0,
        matrix13: 0,
        matrix21: 0,
        matrix22: fx(bh as f64 / dh as f64),
        matrix23: 0,
        matrix31: 0,
        matrix32: 0,
        matrix33: fx(1.0),
    };
    c.render_set_picture_transform(pict, t).x11()?;
    *cache = Some(ShadowSrc { pix, pict, size });
    Ok((pix, pict))
}

/// Bilinear upscale of premultiplied ARGB (only used when XRender is missing).
pub(super) fn upscale(src: &[u32], sw: usize, sh: usize, dst: &mut [u32], dw: usize, dh: usize) {
    let (kx, ky) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    let get = |x: i32, y: i32| -> u32 {
        if x < 0 || y < 0 || x >= sw as i32 || y >= sh as i32 {
            0
        } else {
            src[y as usize * sw + x as usize]
        }
    };
    let lerp = |a: u32, b: u32, t: u32| -> u32 {
        let it = 256 - t;
        let rb = (((a & 0x00ff00ff) * it + (b & 0x00ff00ff) * t) >> 8) & 0x00ff00ff;
        let ag = (((a >> 8) & 0x00ff00ff) * it + ((b >> 8) & 0x00ff00ff) * t) & 0xff00ff00;
        rb | ag
    };
    for y in 0..dh {
        let v = (y as f32 + 0.5) * ky - 0.5;
        let (yi, ty) = (v.floor() as i32, ((v - v.floor()) * 256.0) as u32);
        for x in 0..dw {
            let u = (x as f32 + 0.5) * kx - 0.5;
            let (xi, tx) = (u.floor() as i32, ((u - u.floor()) * 256.0) as u32);
            let top = lerp(get(xi, yi), get(xi + 1, yi), tx);
            let bot = lerp(get(xi, yi + 1), get(xi + 1, yi + 1), tx);
            dst[y * dw + x] = lerp(top, bot, ty);
        }
    }
}
