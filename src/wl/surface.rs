//! Small subsurfaces with their own buffers, and the globals needed to make them.

use smithay_client_toolkit::{
    compositor::{CompositorState, Region},
    shm::slot::{Buffer, SlotPool},
    subcompositor::SubcompositorState,
};
use wayland_client::{
    protocol::{wl_shm, wl_subsurface, wl_surface},
    QueueHandle,
};
use wayland_protocols::wp::{
    alpha_modifier::v1::client::{
        wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1, wp_alpha_modifier_v1::WpAlphaModifierV1,
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use super::{App, Quiet};
use crate::raster::Frame;

/// The globals that surfaces are made from.
pub(super) struct Kit {
    pub compositor: CompositorState,
    pub subcomp: SubcompositorState,
    pub viewporter: WpViewporter,
    /// wp_alpha_modifier_v1: fade surfaces without redrawing them (optional).
    pub alpha_mgr: Option<WpAlphaModifierV1>,
}

impl Kit {
    /// Catch input on `surface` (`catch`), or let it all fall through.
    pub fn set_input(&self, surface: &wl_surface::WlSurface, catch: bool) {
        if catch {
            surface.set_input_region(None);
        } else if let Ok(empty) = Region::new(&self.compositor) {
            surface.set_input_region(Some(empty.wl_region()));
        }
    }

    /// A subsurface of `parent` that never takes input (clicks fall through to the
    /// full-screen surface underneath), optionally with a compositor-side alpha.
    pub fn new_sub(&self, qh: &QueueHandle<App>, parent: &wl_surface::WlSurface, with_alpha: bool) -> Sub {
        let (sub, surface) = self.subcomp.create_subsurface(parent.clone(), qh);
        self.set_input(&surface, false);
        let viewport = self.viewporter.get_viewport(&surface, qh, Quiet);
        let alpha = self.alpha_mgr.as_ref().filter(|_| with_alpha).map(|m| m.get_surface(&surface, qh, Quiet));
        Sub { surface, sub, viewport, alpha, buffer: None, mapped: false }
    }
}

/// One subsurface (an arm tile, the shadow, an ink tile or the toolbar) with its own
/// small buffer.
pub(super) struct Sub {
    pub surface: wl_surface::WlSurface,
    pub sub: wl_subsurface::WlSubsurface,
    viewport: WpViewport,
    pub alpha: Option<WpAlphaModifierSurfaceV1>,
    buffer: Option<Buffer>,
    pub mapped: bool,
}

impl Sub {
    /// Render into a fresh buffer and show it at `f` (logical px; the buffer is f.bw×f.bh).
    pub fn present(&mut self, pool: &mut SlotPool, f: &Frame, render: impl FnOnce(&mut [u32])) {
        let Ok((buffer, canvas)) =
            pool.create_buffer(f.bw as i32, f.bh as i32, f.bw as i32 * 4, wl_shm::Format::Argb8888)
        else {
            return;
        };
        // SAFETY: any bit pattern is a valid u32.
        let (head, px, _) = unsafe { canvas.align_to_mut::<u32>() };
        // sctk aligns slots to 64 bytes; checked rather than assumed.
        assert!(head.is_empty(), "shm buffer is not 4-byte aligned");
        let px = &mut px[..f.bw * f.bh];
        render(px);
        if buffer.attach_to(&self.surface).is_err() {
            return;
        }
        self.surface.damage_buffer(0, 0, f.bw as i32, f.bh as i32);
        self.viewport.set_destination(f.w, f.h);
        self.sub.set_position(f.x, f.y);
        self.surface.commit();
        self.buffer = Some(buffer);
        self.mapped = true;
    }

    pub fn unmap(&mut self) {
        if self.mapped {
            self.surface.attach(None, 0, 0);
            self.surface.commit();
            self.buffer = None;
            self.mapped = false;
        }
    }

    /// Compositor-side opacity (needs the alpha modifier). Takes effect on commit.
    pub fn set_alpha(&self, a: f32) -> bool {
        let Some(m) = &self.alpha else { return false };
        m.set_multiplier((a.clamp(0.0, 1.0) as f64 * u32::MAX as f64) as u32);
        true
    }
}

impl Drop for Sub {
    fn drop(&mut self) {
        if let Some(a) = &self.alpha {
            a.destroy();
        }
        self.viewport.destroy();
        self.sub.destroy();
        self.surface.destroy();
    }
}
