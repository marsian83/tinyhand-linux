//! X11 backend (Xfce, Cinnamon, MATE, i3, KDE on X11…), also the fallback on GNOME
//! Wayland through XWayland (GNOME has no wlr-layer-shell).
//!
//! The trick, as on Wayland: while the pointer is on, one full-screen override-redirect
//! window is the input sink. On a real X server it is an InputOnly window (no pixels, no
//! memory); under XWayland it has to be a transparent InputOutput window, because the
//! Wayland compositor only routes pointer input to X windows that have a surface.
//! Clicks land on it, so apps underneath never see them; the cursor is hidden with
//! XFixes (the server shows it again by itself when we disconnect, however we exit).
//!
//! Drawing: the arm, its shadow, the ink and the toolbar are small override-redirect
//! ARGB windows with an empty input shape, stacked above the sink. Pixels go up with
//! MIT-SHM when it's there. The shadow is rendered at quarter resolution and scaled up
//! by the server with XRender (bilinear). Without a compositing manager there's no alpha,
//! so windows get a 1-bit bounding shape instead and the shadow is skipped.
//!
//! Idle costs nothing: the loop blocks in poll() on the X connection and the control
//! socket, with a timeout only while something animates.
//!
//! Behaviour lives in `session`; this module is X11 plumbing.

mod display;
mod draw;
mod shm;

use std::{
    collections::HashSet,
    fmt,
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

use x11rb::{
    connection::Connection as _,
    protocol::{
        xfixes::ConnectionExt as _,
        xproto::{
            ConfigureWindowAux, ConnectionExt as _, KeyButMask, Mapping, Window, BUTTON_PRESS_EVENT,
            BUTTON_RELEASE_EVENT,
        },
        xtest::ConnectionExt as _,
        Event,
    },
    NONE,
};

use crate::{
    backend::Error,
    config::Config,
    ipc,
    session::{Button, Caps, FadeBy, Output, Passthrough, Session},
};
use display::{Display, Monitor, Win};

/// While Alt holds click-for-real, re-check the modifier this often (in case a raw key
/// release got lost, e.g. XWayland losing keyboard focus).
const ALT_RECHECK: Duration = Duration::from_millis(250);
/// Unused windows are parked here (1×1, off screen) instead of being unmapped, so
/// compositors don't play map/unmap fades on them.
const PARKED: Geom = (-1, -1, 1, 1);
/// Minimum gap between defensive restacks (another always-on-top window could fight us).
const RESTACK_GAP: Duration = Duration::from_millis(100);
/// Frame pacing when the refresh rate is unknown.
const DEFAULT_HZ: f64 = 60.0;

type XResult<T> = Result<T, String>;
/// Window geometry in root device px: (x, y, w, h).
type Geom = (i16, i16, u16, u16);

/// `?` for X11 (and syscall) errors: message prefixed with "X11".
trait X11Ext<T> {
    fn x11(self) -> XResult<T>;
}
impl<T, E: fmt::Display> X11Ext<T> for Result<T, E> {
    fn x11(self) -> XResult<T> {
        self.map_err(|e| format!("X11: {e}"))
    }
}

/// The full-screen window that catches input while the pointer is on.
struct Sink {
    win: Window,
    on: bool,
    cursor_hidden: bool,
}

/// Click-for-real while Alt is held: seen as XI2 raw key events (reliable even though we
/// never have keyboard focus), or as the modifier state on pointer motion.
#[derive(Default)]
struct Alt {
    /// Keycodes that act as Mod1.
    mod1: Vec<u32>,
    /// Mod1 keys down right now (XI2).
    keys: HashSet<u32>,
    /// Mod1 was down at the last motion event.
    motion: bool,
    /// When to ask the server whether it's really still down.
    check: Option<Instant>,
}

impl Alt {
    fn held(&self) -> bool {
        !self.keys.is_empty() || self.motion
    }
}

struct App {
    dpy: Display,
    session: Session<Win>,
    sink: Sink,
    scene: draw::Scene,
    mons: Vec<Monitor>,
    mon: usize,
    scale: f32,
    frame_gap: Duration,
    next_frame: Instant,
    alt: Alt,
    last_restack: Option<Instant>,
    errors: u32,
}

pub fn run(cfg: Config, server: &ipc::Server) -> Result<(), Error> {
    let mut app = App::open(cfg)?;
    let res = app.main_loop(server);
    app.close();
    res.map_err(Error::Failed)
}

fn button(detail: u8) -> Option<Button> {
    match detail {
        1 => Some(Button::Left),
        2 => Some(Button::Middle),
        3 => Some(Button::Right),
        _ => None,
    }
}

impl App {
    fn open(cfg: Config) -> Result<App, Error> {
        let mut dpy = Display::open()?;
        let sink = dpy.create_sink()?;
        dpy.watch_root()?;
        let scale = dpy.xft_scale();
        if cfg.stats {
            eprintln!(
                "x11: xwayland={} composited={} shm={} render={} {:?} scale={scale}",
                dpy.xwayland,
                dpy.composited,
                dpy.staging.is_shm(),
                dpy.pictfmt.is_some(),
                dpy.ext,
            );
        }
        let caps = Caps { shadow: dpy.composited, ink_fade: FadeBy::Redraw, bar_fade: dpy.composited };
        let session = Session::new(cfg, caps)?;
        let alt = Alt { mod1: dpy.mod1_keys()?, ..Alt::default() };
        let mut app = App {
            dpy,
            session,
            sink: Sink { win: sink, on: false, cursor_hidden: false },
            scene: draw::Scene::default(),
            mons: Vec::new(),
            mon: 0,
            scale,
            frame_gap: Duration::from_secs_f64(1.0 / DEFAULT_HZ),
            next_frame: Instant::now(),
            alt,
            last_restack: None,
            errors: 0,
        };
        app.refresh_monitors()?;
        app.alt.motion = app.dpy.pointer()?.1;
        app.apply_passthrough(Passthrough::Off)?;
        app.sync_passthrough()?;
        app.dpy.conn.flush().x11()?;
        Ok(app)
    }

    /// Restore the cursor and release server resources (the server would anyway).
    fn close(&mut self) {
        if self.sink.cursor_hidden {
            let _ = self.dpy.conn.xfixes_show_cursor(self.dpy.root);
        }
        let _ = self.dpy.conn.destroy_window(self.sink.win);
        self.scene.destroy(&mut self.dpy);
        self.session.command(ipc::Cmd::Clear);
        self.reap();
        self.dpy.staging.free(&self.dpy.conn);
        let _ = self.dpy.conn.flush();
    }

    // ------------------------------------------------------------------ loop

    fn main_loop(&mut self, server: &ipc::Server) -> XResult<()> {
        let xfd = self.dpy.conn.stream().as_raw_fd();
        loop {
            self.tick()?;
            self.dpy.conn.flush().x11()?;
            // Last thing before blocking: make sure x11rb has no events queued internally.
            let mut got = false;
            while let Some((ev, seq)) = self.dpy.conn.poll_for_event_with_sequence().x11()? {
                got = true;
                self.event(ev, seq)?;
            }
            if self.session.exit {
                return Ok(());
            }
            if got {
                continue;
            }
            let [_, control] = ipc::wait([xfd, server.fd()], self.next_timeout()).map_err(|e| format!("poll: {e}"))?;
            if control {
                for cmd in server.drain() {
                    self.session.command(cmd);
                }
            }
            if self.session.exit {
                return Ok(());
            }
        }
    }

    /// Timers, click-for-real, cleanup, and a frame if one is due.
    fn tick(&mut self) -> XResult<()> {
        let now = Instant::now();
        self.session.tick(now);
        if self.alt.check.is_some_and(|t| t <= now) {
            // Is Alt really still down? (A lost raw release would leave us stuck.)
            self.alt.check = None;
            if !self.dpy.pointer()?.1 {
                self.alt.keys.clear();
                self.alt.motion = false;
            }
        }
        self.sync_passthrough()?;
        self.reap();
        if self.dpy.restack && self.last_restack.is_none_or(|t| now - t >= RESTACK_GAP) {
            self.restack()?;
        }
        if self.session.wants_frame(now) && !self.dpy.staging.busy(now) && now >= self.next_frame {
            self.draw(now)?;
            self.next_frame = now + self.frame_gap;
        }
        if !self.session.wants_frame(now) {
            // Idle: forget the clock so the next motion doesn't see a huge step.
            self.session.rest();
        }
        Ok(())
    }

    fn next_timeout(&mut self) -> Option<Duration> {
        let now = Instant::now();
        let busy = self.dpy.staging.busy(now);
        let frame = (self.session.wants_frame(now) && !busy).then_some(self.next_frame);
        let restack = self.dpy.restack.then(|| self.last_restack.map_or(now, |t| t + RESTACK_GAP));
        [self.session.next_deadline(now), frame, self.dpy.staging.busy_until(), self.alt.check, restack]
            .into_iter()
            .flatten()
            .min()
            .map(|t| t.saturating_duration_since(now))
    }

    /// Destroy the windows of ink that's gone.
    fn reap(&mut self) {
        for w in self.session.ink.take_dead() {
            self.dpy.destroy_win(w);
        }
    }

    // ------------------------------------------------------------------ events

    fn event(&mut self, ev: Event, seq: u64) -> XResult<()> {
        match ev {
            Event::MotionNotify(e) if e.event == self.sink.win => {
                let at = self.focus(e.root_x, e.root_y);
                self.session.motion(at);
                // Without XI2 motion is how we see Alt; with it, motion only sees it go
                // down (the raw release is the reliable "up").
                let alt = u16::from(e.state) & u16::from(KeyButMask::MOD1) != 0;
                if alt != self.alt.motion && (!self.dpy.ext.xi2 || alt) {
                    self.alt.motion = alt;
                }
            }
            Event::ButtonPress(e) if e.event == self.sink.win => {
                let at = self.focus(e.root_x, e.root_y);
                match (button(e.detail), e.detail) {
                    (Some(b), _) => self.session.press(b, at),
                    (None, 4..=7) => self.scroll(e.detail)?,
                    _ => self.session.motion(at),
                }
            }
            Event::ButtonRelease(e) if e.event == self.sink.win => {
                let at = self.focus(e.root_x, e.root_y);
                self.session.motion(at);
                if let Some(b) = button(e.detail) {
                    self.session.release(b);
                }
            }
            Event::XinputRawKeyPress(e) => {
                if self.alt.mod1.contains(&e.detail) {
                    self.alt.keys.insert(e.detail);
                }
            }
            Event::XinputRawKeyRelease(e) => {
                if self.alt.keys.remove(&e.detail) {
                    self.alt.motion = false;
                }
            }
            Event::MappingNotify(e) if e.request == Mapping::MODIFIER => self.alt.mod1 = self.dpy.mod1_keys()?,
            Event::ShmCompletion(_) => self.dpy.staging.done(),
            Event::Expose(e) => self.exposed(e.window, seq),
            Event::MapNotify(e) if !self.dpy.ours.contains(&e.window) => self.dpy.restack = true,
            Event::ConfigureNotify(e) if self.dpy.ours.contains(&e.window) => {
                // Keep our geometry cache honest if something (a WM) moved/resized it.
                let g = (e.x, e.y, e.width, e.height);
                let ink = self.session.ink.strokes_mut().flat_map(|s| s.surfaces.values_mut());
                for w in self.scene.windows_mut().chain(ink).filter(|w| w.id == e.window) {
                    w.geom = g;
                }
            }
            Event::ConfigureNotify(e) if self.dpy.ours.contains(&e.above_sibling) => {
                // Someone stacked a window right above one of ours: get back on top.
                self.dpy.restack = true;
            }
            Event::RandrScreenChangeNotify(_) => {
                self.refresh_monitors()?;
                let (w, h) = self.dpy.root_size()?;
                let aux = ConfigureWindowAux::new().x(0).y(0).width(w as u32).height(h as u32);
                self.dpy.conn.configure_window(self.sink.win, &aux).x11()?;
                self.session.redraw_arm();
            }
            Event::Error(e) => {
                self.errors += 1;
                if self.errors <= 5 {
                    eprintln!("tinyhand: X11 error {:?} (request {}.{})", e.error_kind, e.major_opcode, e.minor_opcode);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// One of our windows lost its pixels: draw them again (unless we already have).
    fn exposed(&mut self, win: Window, seq: u64) {
        // Exposures from before the last repaint, or of parked windows, are old news.
        let stale = |w: &Win| seq < w.seq || w.geom == PARKED;
        if let Some(w) = self.scene.arm_windows().find(|w| w.id == win) {
            if !stale(w) {
                self.session.redraw_arm();
            }
            return;
        }
        if let Some(w) = self.scene.bar.as_ref().filter(|w| w.id == win) {
            if !stale(w) {
                self.session.redraw_bar();
            }
            return;
        }
        for st in self.session.ink.strokes_mut() {
            if let Some((&k, w)) = st.surfaces.iter().find(|(_, w)| w.id == win) {
                if seq >= w.seq {
                    st.mark_dirty(k);
                }
                return;
            }
        }
    }

    /// The pointer is at root (x, y): tell the session which monitor that is, and return
    /// the position on it in logical px.
    fn focus(&mut self, x: i16, y: i16) -> (f32, f32) {
        self.mon = self.mon_at(x, y);
        let m = self.mons[self.mon];
        let s = self.scale;
        let (w, h) = ((m.w as f32 / s).round() as i32, (m.h as f32 / s).round() as i32);
        self.session.set_output(Output { id: m.id(), w, h, scale: s });
        ((x - m.x) as f32 / s, (y - m.y) as f32 / s)
    }

    /// Wheel: replay it to the app underneath with XTEST while the sink steps aside.
    /// Under XWayland, XTEST may route back through the compositor (to us again), so do
    /// what the Wayland backend does instead: let the next wheel events through for a bit.
    fn scroll(&mut self, button: u8) -> XResult<()> {
        if !self.session.cfg.scroll_through {
            return Ok(());
        }
        if self.dpy.ext.xtest && !self.dpy.xwayland {
            let c = &self.dpy.conn;
            c.unmap_window(self.sink.win).x11()?;
            c.xtest_fake_input(BUTTON_PRESS_EVENT, button, 0, NONE, 0, 0, 0).x11()?;
            c.xtest_fake_input(BUTTON_RELEASE_EVENT, button, 0, NONE, 0, 0, 0).x11()?;
            // Requests are handled in order and the server processes queued input between
            // requests, so the fake wheel lands on the app before we map again.
            c.map_window(self.sink.win).x11()?;
        } else {
            self.session.scroll_through(Instant::now());
        }
        Ok(())
    }

    // ------------------------------------------------------------------ passthrough

    fn sync_passthrough(&mut self) -> XResult<()> {
        let held = self.alt.held();
        if !held {
            self.alt.check = None;
        } else if self.alt.check.is_none() {
            self.alt.check = Some(Instant::now() + ALT_RECHECK);
        }
        match self.session.update_passthrough(held) {
            Some(mode) => self.apply_passthrough(mode),
            None => Ok(()),
        }
    }

    fn apply_passthrough(&mut self, mode: Passthrough) -> XResult<()> {
        let c = &self.dpy.conn;
        let on = mode == Passthrough::Off;
        if on != self.sink.on {
            self.sink.on = on;
            if on {
                c.map_window(self.sink.win).x11()?;
                self.dpy.restack = true;
            } else {
                c.unmap_window(self.sink.win).x11()?;
            }
        }
        match mode {
            Passthrough::Off => {
                if self.dpy.ext.xfixes && !self.sink.cursor_hidden {
                    c.xfixes_hide_cursor(self.dpy.root).x11()?;
                    self.sink.cursor_hidden = true;
                }
                let ((x, y), _) = self.dpy.pointer()?;
                let at = self.focus(x, y);
                self.session.motion(at);
            }
            // The hand stays while scroll events go through (as on macOS).
            Passthrough::Scroll => {}
            Passthrough::Real => {
                // Click for real: the real cursor, no hand.
                if self.sink.cursor_hidden {
                    c.xfixes_show_cursor(self.dpy.root).x11()?;
                    self.sink.cursor_hidden = false;
                }
                self.scene.park_arm(&self.dpy)?;
                self.scene.park_bar(&self.dpy)?;
                self.session.hide_arm();
            }
        }
        Ok(())
    }

    /// Keep: sink at the top of everything, then ink, the toolbar, shadow, arm tiles.
    fn restack(&mut self) -> XResult<()> {
        self.dpy.restack = false;
        self.last_restack = Some(Instant::now());
        let mut order = vec![self.sink.win];
        for s in self.session.ink.strokes() {
            let mut keys: Vec<_> = s.surfaces.keys().collect();
            keys.sort_unstable();
            order.extend(keys.into_iter().map(|k| s.surfaces[k].id));
        }
        order.extend(self.scene.bar.iter().map(|w| w.id));
        order.extend(self.scene.shadow.iter().map(|w| w.id));
        order.extend(self.scene.tiles.iter().map(|w| w.id));
        self.dpy.stack(&order)
    }

    // ------------------------------------------------------------------ monitors

    fn refresh_monitors(&mut self) -> XResult<()> {
        let (mons, hz) = self.dpy.monitors()?;
        self.frame_gap = Duration::from_secs_f64(1.0 / hz.unwrap_or(DEFAULT_HZ));
        self.mons = mons;
        let ((x, y), _) = self.dpy.pointer()?;
        self.focus(x, y);
        Ok(())
    }

    fn mon_at(&self, x: i16, y: i16) -> usize {
        let (x, y) = (x as i32, y as i32);
        let inside = |m: &Monitor| {
            x >= m.x as i32 && y >= m.y as i32 && x < m.x as i32 + m.w as i32 && y < m.y as i32 + m.h as i32
        };
        if let Some(i) = self.mons.iter().position(inside) {
            return i;
        }
        // In a gap between monitors: take the nearest.
        let dist = |m: &Monitor| {
            let dx = (m.x as i32 - x).max(x - (m.x as i32 + m.w as i32 - 1)).max(0);
            let dy = (m.y as i32 - y).max(y - (m.y as i32 + m.h as i32 - 1)).max(0);
            dx * dx + dy * dy
        };
        (0..self.mons.len()).min_by_key(|&i| dist(&self.mons[i])).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests;
