//! Wayland backend (wlr-layer-shell: KDE, Hyprland, Sway, niri, river, COSMIC, labwc…).
//!
//! The trick: while the pointer is on, a full-screen layer surface on the Overlay layer
//! *is* the input sink. Clicks land on us (so apps underneath never see them), the cursor
//! position arrives as ordinary pointer events, and we hide the cursor by setting a null
//! cursor on our own surface. No global hooks, no root, no polling.
//!
//! "Click for real" and scroll pass-through work by emptying the overlay's input region,
//! so the compositor routes input to the apps underneath until we restore it.
//!
//! Memory: the full-screen surface is a single transparent pixel stretched with
//! wp_viewporter. The arm, ink and toolbar are small subsurfaces (see raster.rs, ink.rs),
//! redrawn only when the pointer moves or an animation is running. When nothing moves,
//! the process sleeps in poll() with no timeout.
//!
//! Behaviour lives in `session`; this file is protocol plumbing.

mod handlers;
mod surface;

use std::{os::fd::AsRawFd, time::Instant};

use smithay_client_toolkit::{
    compositor::{CompositorState, FrameCallbackData},
    output::OutputState,
    registry::RegistryState,
    seat::SeatState,
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface},
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm,
    },
    subcompositor::SubcompositorState,
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_surface},
    Connection, Proxy, QueueHandle,
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, wp_fractional_scale_v1::WpFractionalScaleV1,
    },
    viewporter::client::wp_viewport::WpViewport,
};

use crate::{
    backend::Error,
    config::{self, Config},
    ipc,
    raster::{self, Frame},
    session::{self, ArmJob, Button, Caps, FadeBy, Output, Passthrough, Session},
};
use surface::{Kit, Sub};

/// Linux input event codes for the buttons the pointer reacts to.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

fn button(code: u32) -> Option<Button> {
    match code {
        BTN_LEFT => Some(Button::Left),
        BTN_RIGHT => Some(Button::Right),
        BTN_MIDDLE => Some(Button::Middle),
        _ => None,
    }
}

/// One output: a full-screen overlay surface that catches input while the pointer is on.
struct Screen {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    viewport: WpViewport,
    frac: Option<WpFractionalScaleV1>,
    scale: f32,
    /// Logical size, from the layer surface's configure.
    size: (i32, i32),
    blank: Option<Buffer>,
}

impl Drop for Screen {
    fn drop(&mut self) {
        // The LayerSurface destroys itself.
        self.viewport.destroy();
        if let Some(f) = &self.frac {
            f.destroy();
        }
    }
}

/// What's drawn on the output the pointer is on: the arm (its shadow below its tiles)
/// and the toolbar. Ink tiles belong to the strokes in the session.
struct Scene {
    parent: wl_surface::WlSurface,
    shadow: Sub,
    tiles: Vec<Sub>,
    bar: Option<Sub>,
    /// Alpha last applied to the bar (NaN: none yet).
    bar_alpha: f32,
    frame_pending: bool,
}

struct App {
    // Fields drop in order: the subsurfaces (scene, ink) before the screens they're on.
    session: Session<Sub>,
    scene: Option<Scene>,
    screens: Vec<Screen>,
    pointer: Option<wl_pointer::WlPointer>,

    kit: Kit,
    pool: SlotPool,
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    layer_shell: LayerShell,
    shm: Shm,
    frac_mgr: Option<WpFractionalScaleManagerV1>,
}

/// User data for protocol objects that have no events we care about.
struct Quiet;
/// Fractional scale object for one layer surface.
struct FracData(wl_surface::WlSurface);

pub fn run(cfg: Config, server: &ipc::Server) -> Result<(), Error> {
    let conn = Connection::connect_to_env().map_err(|e| Error::Unsupported(format!("no Wayland display: {e}")))?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
    let subcomp = SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh)
        .map_err(|e| format!("wl_subcompositor: {e}"))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|_| Error::Unsupported("this compositor has no wlr-layer-shell".into()))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| format!("wl_shm: {e}"))?;
    let viewporter = globals.bind(&qh, 1..=1, Quiet).map_err(|e| format!("wp_viewporter: {e}"))?;
    let frac_mgr = globals.bind(&qh, 1..=1, Quiet).ok();
    let alpha_mgr: Option<_> = globals.bind(&qh, 1..=1, Quiet).ok();
    let pool = SlotPool::new(256 * 1024, &shm).map_err(|e| e.to_string())?;

    let fades = alpha_mgr.is_some();
    let caps =
        Caps { shadow: true, ink_fade: if fades { FadeBy::Compositor } else { FadeBy::Redraw }, bar_fade: fades };
    let mut app = App {
        session: Session::new(cfg, caps)?,
        scene: None,
        screens: Vec::new(),
        pointer: None,
        kit: Kit { compositor, subcomp, viewporter, alpha_mgr },
        pool,
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        layer_shell,
        shm,
        frac_mgr,
    };

    // One loop for the Wayland socket, the control socket and (only while something is
    // animating) a timeout. Idle means poll() with no timeout: zero wakeups.
    loop {
        queue.dispatch_pending(&mut app).map_err(|e| e.to_string())?;
        app.update(&qh);
        if app.session.exit {
            return Ok(());
        }
        queue.flush().map_err(|e| e.to_string())?;
        let Some(guard) = queue.prepare_read() else { continue };
        let fd = guard.connection_fd().as_raw_fd();
        let now = Instant::now();
        let timeout = app.session.next_deadline(now).map(|t| t.saturating_duration_since(now));
        let [wayland, control] = ipc::wait([fd, server.fd()], timeout).map_err(|e| format!("poll: {e}"))?;
        if wayland {
            if let Err(e) = guard.read() {
                if !matches!(&e, wayland_client::backend::WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock)
                {
                    return Err(e.to_string().into());
                }
            }
        } else {
            drop(guard);
        }
        if control {
            for cmd in server.drain() {
                app.session.command(cmd);
            }
        }
    }
}

impl App {
    fn screen_index(&self, s: &wl_surface::WlSurface) -> Option<usize> {
        self.screens.iter().position(|o| o.layer.wl_surface() == s)
    }

    fn add_screen(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let surface = self.kit.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(qh, surface, Layer::Overlay, Some("tinyhand"), Some(&output));
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let viewport = self.kit.viewporter.get_viewport(layer.wl_surface(), qh, Quiet);
        let frac = self
            .frac_mgr
            .as_ref()
            .map(|m| m.get_fractional_scale(layer.wl_surface(), qh, FracData(layer.wl_surface().clone())));
        if self.session.passthrough() != Passthrough::Off {
            self.kit.set_input(layer.wl_surface(), false);
        }
        layer.commit();
        self.screens.push(Screen { output, layer, viewport, frac, scale: 1.0, size: (0, 0), blank: None });
    }

    /// An output went away (unplugged, or the compositor closed our surface on it).
    fn remove_screen(&mut self, i: usize) {
        let gone = self.screens.remove(i);
        if self.scene.as_ref().is_some_and(|s| &s.parent == gone.layer.wl_surface()) {
            self.scene = None;
            self.session.lost_output();
            drop(self.session.ink.take_dead());
        }
    }

    /// The pointer is over `surface`: put the arm on that output.
    fn focus(&mut self, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface) {
        let Some(i) = self.screen_index(surface) else { return };
        if self.scene.as_ref().is_none_or(|s| &s.parent != surface) {
            self.scene = None;
            let shadow = self.kit.new_sub(qh, surface, false);
            self.scene = Some(Scene {
                parent: surface.clone(),
                shadow,
                tiles: Vec::new(),
                bar: None,
                bar_alpha: f32::NAN,
                frame_pending: false,
            });
        }
        self.refresh_output(i);
    }

    /// Tell the session about screen `i`'s size and scale if the arm is on it.
    fn refresh_output(&mut self, i: usize) {
        let sc = &self.screens[i];
        if self.scene.as_ref().is_none_or(|s| &s.parent != sc.layer.wl_surface()) || sc.size.0 == 0 || sc.size.1 == 0 {
            return;
        }
        let id = sc.layer.wl_surface().id().protocol_id() as u64;
        self.session.set_output(Output { id, w: sc.size.0, h: sc.size.1, scale: sc.scale });
    }

    /// After a batch of events or commands: timers, pass-through, cleanup, and a frame
    /// if anything changed.
    fn update(&mut self, qh: &QueueHandle<Self>) {
        self.session.tick(Instant::now());
        if let Some(mode) = self.session.update_passthrough(false) {
            let catch = mode == Passthrough::Off;
            for sc in &self.screens {
                self.kit.set_input(sc.layer.wl_surface(), catch);
                sc.layer.commit();
            }
            if !catch {
                // Input now goes to the app underneath and the real cursor shows there.
                self.hide_scene();
            }
        }
        let dead = self.session.ink.take_dead();
        if !dead.is_empty() {
            drop(dead);
            if let Some(s) = &self.scene {
                s.parent.commit();
            }
        }
        if self.session.dirty {
            self.draw(qh);
        }
    }

    fn hide_scene(&mut self) {
        if let Some(s) = self.scene.as_mut() {
            s.shadow.unmap();
            s.tiles.iter_mut().chain(s.bar.as_mut()).for_each(Sub::unmap);
            s.parent.commit();
        }
        self.session.hide_arm();
    }

    /// Draw what changed: the arm (or laser), dirty ink tiles, the toolbar, fades.
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        if self.session.passthrough() != Passthrough::Off {
            return;
        }
        let Some(scene) = &self.scene else { return };
        if scene.frame_pending {
            return; // `dirty` stays set; the frame callback draws.
        }
        self.session.dirty = false;
        let now = Instant::now();
        let arm = self.draw_arm(qh, now);
        let ink = self.draw_ink(qh);
        let (bar, bar_rendered) = self.draw_bar(qh, now);
        let faded = self.apply_ink_alpha();
        let took = now.elapsed();

        let Some(scene) = self.scene.as_mut() else { return };
        // Sync subsurfaces: everything above lands atomically with this parent commit.
        // Throttle by frame callbacks while drawing; alpha changes alone don't need one.
        if arm || ink || self.session.animating() {
            scene.parent.frame(qh, FrameCallbackData(scene.parent.clone()));
            scene.frame_pending = true;
            scene.parent.commit();
        } else if bar || faded {
            scene.parent.commit();
        }
        if (arm || ink || bar_rendered) && self.session.stats.record(took) && self.session.cfg.stats {
            eprintln!(
                "{} pool={}KB strokes={}",
                self.session.stats.summary(),
                self.pool.len() / 1024,
                self.session.ink.strokes().count()
            );
        }
    }

    fn draw_arm(&mut self, qh: &QueueHandle<Self>, now: Instant) -> bool {
        let App { session, scene, kit, pool, .. } = self;
        let (Some(scene), Some(out)) = (scene.as_mut(), session.output()) else { return false };
        let Some(job) = session.plan_arm(now) else { return false };
        let n = match &job {
            ArmJob::Hand { plan, .. } => plan.tiles.len(),
            ArmJob::Laser { .. } => 1,
        };
        while scene.tiles.len() < n {
            scene.tiles.push(kit.new_sub(qh, &scene.parent, false));
        }
        let (out_h, scale) = (out.h as f32, out.scale);
        match job {
            ArmJob::Laser { laser, frame } => {
                scene.shadow.unmap();
                scene.tiles.iter_mut().skip(1).for_each(Sub::unmap);
                match frame {
                    Some(f) => scene.tiles[0].present(pool, &f, |px| laser.render(&f, config::LASER, px)),
                    None => scene.tiles[0].unmap(),
                }
            }
            ArmJob::Hand { pose, plan } => {
                let Some(sprite) = session.sprite() else { return false };
                match plan.shadow {
                    Some(f) => {
                        scene.shadow.present(pool, &f, |px| raster::render_shadow(&pose, sprite, &f, out_h, scale, px))
                    }
                    None => scene.shadow.unmap(),
                }
                for (k, sub) in scene.tiles.iter_mut().enumerate() {
                    match plan.tiles.get(k) {
                        Some(tile) => sub.present(pool, &tile.frame, |px| {
                            raster::render_tile(&pose, sprite, tile, out_h, scale, px)
                        }),
                        None => sub.unmap(),
                    }
                }
            }
        }
        true
    }

    /// Upload dirty ink tiles. Ink sits below the arm.
    fn draw_ink(&mut self, qh: &QueueHandle<Self>) -> bool {
        let App { session, scene, kit, pool, .. } = self;
        let (Some(scene), Some(out)) = (scene.as_ref(), session.output()) else { return false };
        let tile = session::ink_tile(out.scale);
        // With the alpha modifier, fading is the compositor's job; otherwise bake it in.
        let by_compositor = kit.alpha_mgr.is_some();
        let mut changed = false;
        for st in session.ink.strokes_mut() {
            let keys = st.take_dirty();
            if keys.is_empty() {
                continue;
            }
            let alpha = st.alpha;
            let pal = st.stroke.palette(if by_compositor { 1.0 } else { alpha });
            let px = st.stroke.tile_px();
            for key in keys {
                let sub = st.surfaces.entry(key).or_insert_with(|| {
                    let sub = kit.new_sub(qh, &scene.parent, true);
                    sub.sub.place_below(&scene.shadow.surface);
                    sub.set_alpha(alpha);
                    sub
                });
                let (x, y) = st.stroke.tile_origin(key);
                let f = Frame { x, y, w: tile, h: tile, bw: px, bh: px };
                let stroke = &st.stroke;
                sub.present(pool, &f, |o| stroke.render_tile_with(key, &pal, o));
                changed = true;
            }
        }
        changed
    }

    /// Compositor-side ink fades: new alphas, no redraw.
    fn apply_ink_alpha(&mut self) -> bool {
        let mut changed = false;
        for st in self.session.ink.strokes_mut() {
            if let Some(a) = st.take_alpha_change() {
                for sub in st.surfaces.values() {
                    if sub.set_alpha(a) {
                        sub.surface.commit();
                    }
                }
                changed = true;
            }
        }
        changed
    }

    /// Show, hide, fade or redraw the toolbar. Returns (changed, rendered).
    fn draw_bar(&mut self, qh: &QueueHandle<Self>, now: Instant) -> (bool, bool) {
        let view = self.session.bar_view(now);
        let App { session, scene, kit, pool, .. } = self;
        let Some(scene) = scene.as_mut() else { return (false, false) };
        let Some(v) = view else {
            scene.bar_alpha = f32::NAN;
            return (scene.bar.take().is_some(), false);
        };
        if !v.mapped {
            return match scene.bar.as_mut() {
                Some(b) if b.mapped => {
                    b.unmap();
                    (true, false)
                }
                _ => (false, false),
            };
        }
        let (parent, shadow) = (&scene.parent, &scene.shadow.surface);
        let bar = scene.bar.get_or_insert_with(|| {
            let b = kit.new_sub(qh, parent, true);
            b.sub.place_below(shadow);
            b
        });
        let mut changed = false;
        if v.alpha != scene.bar_alpha {
            scene.bar_alpha = v.alpha;
            if bar.set_alpha(v.alpha) {
                changed = true;
                if !v.render {
                    bar.surface.commit();
                }
            }
        }
        if v.render {
            bar.present(pool, &v.frame, |px| session.render_bar(px));
            changed = true;
        }
        (changed, v.render)
    }
}
