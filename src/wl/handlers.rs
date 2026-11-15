//! Wayland protocol event handlers: they update the session and the screens, and the
//! main loop draws afterwards (`App::update`).

use std::time::Instant;

use smithay_client_toolkit::{
    compositor::CompositorHandler,
    delegate_dispatch2, delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Proxy, QueueHandle,
};
use wayland_protocols::wp::{
    alpha_modifier::v1::client::{
        wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1, wp_alpha_modifier_v1::WpAlphaModifierV1,
    },
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use super::{button, App, FracData, Quiet};

impl App {
    fn set_scale(&mut self, surface: &wl_surface::WlSurface, scale: f32) {
        if let Some(i) = self.screen_index(surface) {
            self.screens[i].scale = scale;
            self.refresh_output(i);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        // Integer scale, only used without wp_fractional_scale.
        if self.frac_mgr.is_none() {
            self.set_scale(surface, factor as f32);
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        let Some(scene) = self.scene.as_mut().filter(|s| &s.parent == surface) else { return };
        scene.frame_pending = false;
        if self.session.animating() {
            self.session.dirty = true;
        } else if !self.session.dirty {
            self.session.rest();
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_screen(qh, output);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if let Some(i) = self.screens.iter().position(|o| o.output == output) {
            self.remove_screen(i);
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(i) = self.screen_index(layer.wl_surface()) {
            self.remove_screen(i);
        }
        if self.screens.is_empty() {
            self.session.exit = true;
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        cfg: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(i) = self.screen_index(layer.wl_surface()) else { return };
        let size = (cfg.new_size.0 as i32, cfg.new_size.1 as i32);
        let sc = &mut self.screens[i];
        sc.size = size;
        if sc.blank.is_none() {
            // One transparent pixel, stretched to the whole output: a full-screen input
            // sink that costs 4 bytes of buffer memory.
            if let Ok((buffer, canvas)) = self.pool.create_buffer(1, 1, 4, wl_shm::Format::Argb8888) {
                canvas.fill(0);
                let _ = buffer.attach_to(sc.layer.wl_surface());
                sc.blank = Some(buffer);
            }
        }
        sc.viewport.set_destination(size.0.max(1), size.1.max(1));
        sc.layer.wl_surface().damage_buffer(0, 0, 1, 1);
        sc.layer.commit();
        self.refresh_output(i);
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seats.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for e in events {
            if self.screen_index(&e.surface).is_none() {
                continue;
            }
            let at = (e.position.0 as f32, e.position.1 as f32);
            match e.kind {
                PointerEventKind::Enter { serial } => {
                    // Hide the real cursor while it's over us.
                    pointer.set_cursor(serial, None, 0, 0);
                    self.focus(qh, &e.surface);
                    self.session.motion(at);
                }
                PointerEventKind::Motion { .. } => {
                    self.focus(qh, &e.surface);
                    self.session.motion(at);
                }
                PointerEventKind::Press { button: code, .. } => {
                    if let Some(b) = button(code) {
                        self.session.press(b, at);
                    }
                }
                PointerEventKind::Release { button: code, .. } => {
                    if let Some(b) = button(code) {
                        self.session.release(b);
                    }
                }
                PointerEventKind::Axis { .. } => {
                    // This scroll is lost, but the next ones go to the app underneath.
                    self.session.scroll_through(Instant::now());
                }
                PointerEventKind::Leave { .. } => self.session.leave(),
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

macro_rules! quiet {
    ($($t:ty),*) => {$(
        impl Dispatch2<$t, App> for Quiet {
            fn event(&self, _: &mut App, _: &$t, _: <$t as Proxy>::Event, _: &Connection, _: &QueueHandle<App>) {}
        }
    )*};
}
quiet!(WpViewporter, WpViewport, WpFractionalScaleManagerV1, WpAlphaModifierV1, WpAlphaModifierSurfaceV1);

impl Dispatch2<WpFractionalScaleV1, App> for FracData {
    fn event(
        &self,
        app: &mut App,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            app.set_scale(&self.0, scale as f32 / 120.0);
        }
    }
}

delegate_registry!(App);
delegate_dispatch2!(App);
