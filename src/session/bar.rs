//! When the toolbar shows, where, and how opaque it is (port of the toolbar zones in
//! OverlayController.swift). The pixels come from `toolbar::Toolbar`; the surface it
//! sits on belongs to the backend.

use std::time::{Duration, Instant};

use crate::{
    config::Config,
    raster::{self, Frame},
    theme,
    toolbar::{Action, Toolbar},
};

/// How long the toolbar stays up after the pointer turns on.
pub const REVEAL_ON_START: Duration = Duration::from_secs(3);
/// Gap between the bar and the bottom of the screen (logical px).
const BOTTOM_GAP: i32 = 14;
/// How long the bar waits for the desktop's theme (asked at launch) before using the
/// dark look. The bar first appears after the pointer's first frame, so the answer is
/// almost always in by then.
const THEME_WAIT: Duration = Duration::from_millis(150);
/// Fade durations (seconds), as on macOS.
const FADE_IN: f32 = 0.15;
const FADE_OUT: f32 = 0.35;
/// The reveal zone: the bar widened by this much on each side…
const ZONE_SIDE: f32 = 80.0;
/// …and extended this far above it and below it.
const ZONE_ABOVE: f32 = 100.0;
const ZONE_BELOW: f32 = 10.0;

/// A linear change of a value over time.
#[derive(Clone, Copy, Debug)]
struct Fade {
    from: f32,
    to: f32,
    start: Instant,
    secs: f32,
}

impl Fade {
    fn value(&self, now: Instant) -> f32 {
        let t = (now.saturating_duration_since(self.start).as_secs_f32() / self.secs).min(1.0);
        self.from + (self.to - self.from) * t
    }

    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start).as_secs_f32() >= self.secs
    }
}

/// What the backend should do with the toolbar's surface this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarView {
    /// Where it goes (logical px) and its buffer size (device px).
    pub frame: Frame,
    /// Show the surface (false: hide it).
    pub mapped: bool,
    /// The pixels changed: render with `Session::render_bar` and upload.
    pub render: bool,
    /// Opacity to apply, 0..=1.
    pub alpha: f32,
}

#[derive(Default)]
pub(super) struct Bar {
    /// Built lazily for the current output's scale; None after a reset.
    toolbar: Option<Toolbar>,
    pos: (i32, i32),
    revealed: bool,
    alpha: f32,
    fade: Option<Fade>,
    stale: bool,
    reveal_until: Option<Instant>,
    shown_once: bool,
    /// Built but not rendered yet.
    fresh: bool,
}

impl Bar {
    /// Forget the toolbar (new output, new scale or new pins); it's rebuilt when needed.
    pub fn reset(&mut self) {
        self.toolbar = None;
        self.revealed = false;
        self.alpha = 0.0;
        self.fade = None;
    }

    /// Hide at once (click-for-real: the bar can't be clicked, so don't show it).
    pub fn hide(&mut self) {
        self.revealed = false;
        self.alpha = 0.0;
        self.fade = None;
        if let Some(t) = self.toolbar.as_mut() {
            t.hover(None);
        }
    }

    pub fn sync(&mut self, cfg: &Config) {
        if let Some(t) = self.toolbar.as_mut() {
            t.set_current(&cfg.hand);
            t.set_sound(cfg.sound_on());
            self.stale = true;
        }
    }

    pub fn refresh(&mut self) {
        self.stale = true;
    }

    /// Point (logical px, output-local) relative to the bar, while it's showing.
    fn local(&self, at: (f32, f32)) -> Option<(f32, f32)> {
        self.toolbar.as_ref().filter(|_| self.revealed)?;
        Some((at.0 - self.pos.0 as f32, at.1 - self.pos.1 as f32))
    }

    /// Hover feedback; true if the bar needs drawing again.
    pub fn hover(&mut self, at: Option<(f32, f32)>) -> bool {
        let local = at.and_then(|p| self.local(p));
        let Some(t) = self.toolbar.as_mut() else { return false };
        let changed = t.hover(local);
        self.stale |= changed;
        changed
    }

    /// A click at `at`: Some(action) if it hit a button, Some(None) if it hit the bar
    /// between buttons (swallowed), None if it missed the bar (it's a tap).
    pub fn click(&self, at: (f32, f32)) -> Option<Option<Action>> {
        let (lx, ly) = self.local(at)?;
        let t = self.toolbar.as_ref()?;
        let (bx, by, bw, bh) = t.body_rect();
        let inside = lx >= bx as f32 && ly >= by as f32 && lx < (bx + bw) as f32 && ly < (by + bh) as f32;
        inside.then(|| t.click((lx, ly)))
    }

    /// Lay out, reveal or hide, and say what to do with the surface.
    /// `ready`: the pointer has drawn its first frame (the bar's thumbnails never delay it).
    #[allow(clippy::too_many_arguments)]
    pub fn view(
        &mut self,
        cfg: &Config,
        (w, h, scale): (i32, i32, f32),
        cursor: Option<(f32, f32)>,
        drawing: bool,
        can_fade: bool,
        ready: bool,
        now: Instant,
    ) -> Option<BarView> {
        if cfg.toolbar.is_empty() || !ready {
            return None;
        }
        if self.toolbar.is_none() {
            let pinned: Vec<&str> = cfg.toolbar.iter().map(String::as_str).collect();
            self.toolbar = Some(Toolbar::new(&pinned, &cfg.hand, cfg.sound_on(), scale, theme::current(THEME_WAIT)));
            self.stale = true;
            self.fresh = true;
            if !self.shown_once {
                self.shown_once = true;
                self.reveal_until = Some(now + REVEAL_ON_START);
            }
        }
        let t = self.toolbar.as_ref()?;
        let (tw, th) = t.size();
        let (bw, bh) = t.buffer_size();
        // Centred at the bottom, on whole device pixels (the bar can grow: an unpinned
        // current hand gets its own button).
        let u = raster::pixel_unit(scale, 1.0);
        let pos = (((w - tw) / 2) / u * u, ((h - th - BOTTOM_GAP) / u * u).max(0));
        if pos != self.pos {
            self.pos = pos;
            self.stale = true;
        }
        let near = cursor.is_some_and(|(cx, cy)| {
            cx >= pos.0 as f32 - ZONE_SIDE
                && cx <= (pos.0 + tw) as f32 + ZONE_SIDE
                && cy >= pos.1 as f32 - ZONE_ABOVE
                && cy <= (pos.1 + th) as f32 + ZONE_BELOW
        });
        let want = !drawing && (near || self.reveal_until.is_some_and(|t| now < t));
        if want != self.revealed {
            self.revealed = want;
            let (to, secs) = if want { (1.0, FADE_IN) } else { (0.0, FADE_OUT) };
            self.fade = can_fade.then_some(Fade { from: self.alpha, to, start: now, secs });
            if !can_fade {
                self.alpha = to;
            }
            if want {
                self.stale = true;
            } else if let Some(t) = self.toolbar.as_mut() {
                t.hover(None);
            }
        }
        if let Some(f) = self.fade {
            self.alpha = f.value(now);
            if f.done(now) {
                self.fade = None;
            }
        }
        let mapped = self.revealed || self.alpha > 0.0;
        let render = mapped && self.stale;
        if render {
            self.stale = false;
        }
        Some(BarView { frame: Frame { x: pos.0, y: pos.1, w: tw, h: th, bw, bh }, mapped, render, alpha: self.alpha })
    }

    /// Render the toolbar; true the first time after it was built.
    pub fn render(&mut self, out: &mut [u32]) -> bool {
        if let Some(t) = self.toolbar.as_mut() {
            t.render(out);
        }
        std::mem::take(&mut self.fresh)
    }

    /// Timers: the end of the reveal-on-start window, and fade steps.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = self.fade.is_some();
        if self.reveal_until.is_some_and(|t| t <= now) {
            self.reveal_until = None;
            changed = true;
        }
        changed
    }

    pub fn next_deadline(&self, now: Instant, tick: Duration) -> Option<Instant> {
        let fade = self.fade.map(|_| now + tick);
        [self.reveal_until, fade].into_iter().flatten().min()
    }
}
