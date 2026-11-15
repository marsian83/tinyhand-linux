//! The drawings on screen: finished strokes (fading out on a timer) and the one being
//! drawn. Generic over `T`, the backend's handle for one ink tile on screen (a Wayland
//! subsurface, an X11 window), so the backend keeps its surfaces next to the strokes
//! they show.
//!
//! When a stroke goes away its surfaces move to a graveyard; the backend empties it with
//! `take_dead()` and destroys them (X11 needs the connection for that; on Wayland,
//! dropping is enough).

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::ink::{Key, Stroke};

/// How long a drawing takes to fade out once it starts.
const FADE: f32 = 0.6;

pub struct InkStroke<T> {
    pub stroke: Stroke,
    /// One surface per tile that has been shown.
    pub surfaces: HashMap<Key, T>,
    dirty: Vec<Key>,
    /// When the fade-out starts (None: never, or still being drawn).
    fade_at: Option<Instant>,
    /// Current opacity, 1 until the fade starts.
    pub alpha: f32,
    alpha_changed: bool,
}

impl<T> InkStroke<T> {
    fn new(stroke: Stroke, dirty: Vec<Key>) -> Self {
        InkStroke { stroke, surfaces: HashMap::new(), dirty, fade_at: None, alpha: 1.0, alpha_changed: false }
    }

    /// Tiles to draw again, each once, in order.
    pub fn take_dirty(&mut self) -> Vec<Key> {
        let mut keys = std::mem::take(&mut self.dirty);
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// Ask for a tile to be drawn again (e.g. after an X11 exposure).
    pub fn mark_dirty(&mut self, key: Key) {
        self.dirty.push(key);
    }

    /// The new alpha, if it changed since the last call (for compositor-side fading).
    pub fn take_alpha_change(&mut self) -> Option<f32> {
        std::mem::take(&mut self.alpha_changed).then_some(self.alpha)
    }

    fn fading(&self, now: Instant) -> bool {
        self.fade_at.is_some_and(|t| t <= now)
    }
}

/// How fades reach the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadeBy {
    /// Tiles are drawn once; the compositor applies `alpha` (Wayland alpha modifier).
    Compositor,
    /// Every tile is drawn again with the new alpha.
    Redraw,
}

pub struct Ink<T> {
    done: Vec<InkStroke<T>>,
    drawing: Option<InkStroke<T>>,
    dead: Vec<T>,
}

impl<T> Default for Ink<T> {
    fn default() -> Self {
        Ink { done: Vec::new(), drawing: None, dead: Vec::new() }
    }
}

impl<T> Ink<T> {
    pub fn is_drawing(&self) -> bool {
        self.drawing.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.done.is_empty() && self.drawing.is_none()
    }

    /// Strokes on screen, oldest first; the one being drawn (if any) last.
    pub fn strokes(&self) -> impl Iterator<Item = &InkStroke<T>> {
        self.done.iter().chain(self.drawing.iter())
    }

    pub fn strokes_mut(&mut self) -> impl Iterator<Item = &mut InkStroke<T>> {
        self.done.iter_mut().chain(self.drawing.iter_mut())
    }

    pub fn has_dirty(&self) -> bool {
        self.strokes().any(|s| !s.dirty.is_empty())
    }

    /// Surfaces of strokes that are gone, for the backend to destroy.
    pub fn take_dead(&mut self) -> Vec<T> {
        std::mem::take(&mut self.dead)
    }

    pub(super) fn begin(&mut self, mut stroke: Stroke, at: (f32, f32)) {
        self.end(None);
        let dirty = stroke.begin(at);
        self.drawing = Some(InkStroke::new(stroke, dirty));
    }

    pub(super) fn extend(&mut self, at: (f32, f32)) {
        if let Some(s) = self.drawing.as_mut() {
            let more = s.stroke.extend(at);
            s.dirty.extend(more);
        }
    }

    /// Finish the stroke being drawn; it starts fading at `fade_at` (None: never).
    pub(super) fn end(&mut self, fade_at: Option<Instant>) -> bool {
        let Some(mut s) = self.drawing.take() else { return false };
        let more = s.stroke.end();
        s.dirty.extend(more);
        s.fade_at = fade_at;
        self.done.push(s);
        true
    }

    pub(super) fn clear(&mut self) -> bool {
        let had = !self.is_empty();
        for s in self.done.drain(..).chain(self.drawing.take()) {
            self.dead.extend(s.surfaces.into_values());
        }
        had
    }

    /// Advance fades to `now`. True if anything visible changed.
    pub(super) fn step(&mut self, now: Instant, by: FadeBy) -> bool {
        let mut changed = false;
        for s in &mut self.done {
            let Some(at) = s.fade_at.filter(|&t| t <= now) else { continue };
            let t = ((now - at).as_secs_f32() / FADE).min(1.0);
            s.alpha = 1.0 - t * t; // ease-in, like the macOS fade
            changed = true;
            match by {
                FadeBy::Compositor => s.alpha_changed = true,
                FadeBy::Redraw => s.dirty = s.surfaces.keys().copied().collect(),
            }
        }
        if changed {
            let (gone, keep): (Vec<_>, Vec<_>) = self.done.drain(..).partition(|s| s.alpha <= 0.0);
            self.done = keep;
            for s in gone {
                self.dead.extend(s.surfaces.into_values());
            }
        }
        changed
    }

    /// When the next fade step is due, if any: a fade's start, or `tick` from now while
    /// one is running.
    pub(super) fn next_deadline(&self, now: Instant, tick: Duration) -> Option<Instant> {
        self.done.iter().filter_map(|s| s.fade_at).map(|t| if t <= now { now + tick } else { t }).min()
    }

    pub(super) fn fading(&self, now: Instant) -> bool {
        self.done.iter().any(|s| s.fading(now))
    }
}
