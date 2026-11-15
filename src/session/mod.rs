//! The pointer's state machine, shared by the Wayland and X11 backends.
//!
//! Sans-IO: a backend turns protocol events into calls here (positions in logical px,
//! relative to the output the pointer is on), asks what to draw, and does the drawing
//! and the protocol plumbing itself. Everything a user can see behave the same on every
//! display server lives here: taps, drawing, fades, the toolbar, click-for-real and
//! scrolling, IPC commands. That also makes it testable without a display.
//!
//! Zero idle cost depends on two things here: `next_deadline()` is None when nothing is
//! animating (the backend then sleeps in poll() with no timeout), and `plan_arm()`
//! returns None for frames that would look the same as the last one.

mod bar;
mod ink;

pub use bar::BarView;
pub use ink::{FadeBy, Ink, InkStroke};

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use crate::{
    config::{self, Config},
    hand::{self, Image, Meta, Sprite},
    ink::{Stroke, Style},
    ipc::Cmd,
    pose::{Arm, Pose},
    raster::{self, Frame, Laser, Plan},
    sound::{self, Choice, TapSound},
    stats::Stats,
    toolbar::Action,
};

/// Drag distance (logical px) before a press turns into drawing, as on macOS.
pub const DRAW_AFTER: f32 = 4.0;
/// How long input goes to the apps underneath after a scroll (when the backend can't
/// replay the scroll itself).
pub const SCROLL_THROUGH: Duration = Duration::from_millis(700);
/// Timer step while something fades (~60 Hz).
pub const FADE_TICK: Duration = Duration::from_millis(16);
/// Shortest wait before a drawing fades (seconds).
const MIN_FADE_DELAY: f32 = 0.5;
/// Ink line width as a fraction of the output height, and its minimum (logical px).
const INK_WIDTH: f32 = 0.0055;
const INK_MIN_WIDTH: f32 = 3.0;
/// Laser trails: the bright core's width as a fraction of the ink width (the glow adds
/// the rest), and how long the trail stays after the drag ends, seconds.
const LASER_CORE: f32 = 0.6;
const LASER_FADE_DELAY: f32 = 0.7;
/// Ink tile edge (logical px), before rounding up to whole device pixels.
const INK_TILE: i32 = 128;

/// Edge of an ink tile in logical px at `scale`: a whole number of device pixels.
pub fn ink_tile(scale: f32) -> i32 {
    let u = raster::pixel_unit(scale, 1.0);
    (INK_TILE + u - 1) / u * u
}

/// What the display server can do.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    /// Can show the soft drop shadow (needs translucent windows).
    pub shadow: bool,
    /// How ink fades reach the screen.
    pub ink_fade: FadeBy,
    /// Can fade the toolbar in and out (else it just appears and disappears).
    pub bar_fade: bool,
}

/// Mouse buttons the pointer reacts to. Others (side buttons) are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// What the pointer shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Look {
    Laser,
    Hand(PathBuf),
}

impl Look {
    /// The one resolver for hand names: "laser", a built-in or custom hand id, or a PNG.
    pub fn resolve(name: &str) -> Result<Look, String> {
        if name == config::LASER_ID {
            Ok(Look::Laser)
        } else {
            hand::resolve(name).map(Look::Hand)
        }
    }
}

/// Why input goes to the apps underneath instead of the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Passthrough {
    /// It doesn't: the pointer catches every click.
    Off,
    /// For a moment after a scroll.
    Scroll,
    /// Click for real (IPC or a held key): the real cursor, no hand.
    Real,
}

/// The output the pointer is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    /// Backend-chosen identity: a change means another output.
    pub id: u64,
    /// Logical size.
    pub w: i32,
    pub h: i32,
    /// Device px per logical px.
    pub scale: f32,
}

/// One frame of the arm or laser to draw.
pub enum ArmJob {
    Hand {
        pose: Pose,
        plan: Plan,
    },
    /// `frame` is None when the dot is off screen.
    Laser {
        laser: Laser,
        frame: Option<Frame>,
    },
}

/// Rounded state of the last frame drawn: equal keys look identical, so skip the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameKey {
    Hand([i32; 7]),
    Laser([i32; 3]),
}

pub struct Session<T> {
    pub cfg: Config,
    caps: Caps,
    look: Look,
    /// The hand's decoded image, kept only until the first sprite is built from it.
    pending: Option<(Image, Meta)>,
    sprite: Option<Sprite>,
    /// A sprite size that failed to build (don't retry it every frame).
    sprite_failed: Option<u32>,
    arm: Arm,
    sound: TapSound,
    out: Option<Output>,
    cursor: Option<(f32, f32)>,
    last_step: Option<Instant>,
    last_key: Option<FrameKey>,
    arm_shown: bool,
    /// Something visible changed: draw a frame.
    pub dirty: bool,
    /// The pointer should turn off.
    pub exit: bool,
    real: bool,
    scroll_until: Option<Instant>,
    passthrough: Passthrough,
    /// Where the left button went down (logical px), while it's down.
    down: Option<(f32, f32)>,
    pub ink: Ink<T>,
    bar: bar::Bar,
    pub stats: Stats,
}

/// Decode a hand image and find its parts.
fn load(path: &std::path::Path) -> Result<(Image, Meta), String> {
    let img = hand::load_png(path)?;
    let meta =
        hand::measure(&img).ok_or_else(|| format!("{}: can't find a hand on a stick in this image", path.display()))?;
    Ok((img, meta))
}

impl<T> Session<T> {
    /// Fails if the configured hand can't be found or isn't a usable hand image.
    pub fn new(cfg: Config, caps: Caps) -> Result<Self, String> {
        let look = Look::resolve(&cfg.hand)?;
        let pending = match &look {
            Look::Hand(path) => Some(load(path)?),
            Look::Laser => None,
        };
        let mut sound = TapSound::new(Choice::or_default(&cfg.tap_sound));
        sound.set_enabled(cfg.sound_on());
        Ok(Session {
            arm: Arm::new(cfg.size, cfg.shadow && caps.shadow),
            cfg,
            caps,
            look,
            pending,
            sprite: None,
            sprite_failed: None,
            sound,
            out: None,
            cursor: None,
            last_step: None,
            last_key: None,
            arm_shown: false,
            dirty: true,
            exit: false,
            real: false,
            scroll_until: None,
            passthrough: Passthrough::Off,
            down: None,
            ink: Ink::default(),
            bar: bar::Bar::default(),
            stats: Stats::default(),
        })
    }

    pub fn look(&self) -> &Look {
        &self.look
    }

    pub fn output(&self) -> Option<Output> {
        self.out
    }

    pub fn passthrough(&self) -> Passthrough {
        self.passthrough
    }

    /// The pointer is on `out` (call on every change, including size and scale).
    /// Moving to another output clears the drawing, as on macOS.
    pub fn set_output(&mut self, out: Output) {
        match self.out {
            Some(o) if o == out => return,
            Some(o) if o.id == out.id => {
                if o.scale != out.scale {
                    self.bar.reset();
                }
            }
            _ => {
                self.ink.clear();
                self.down = None;
                self.bar.reset();
                self.arm.snap();
            }
        }
        self.out = Some(out);
        self.redraw_arm();
    }

    /// The output went away.
    pub fn lost_output(&mut self) {
        self.out = None;
        self.cursor = None;
        self.down = None;
        self.ink.clear();
        self.bar.reset();
        self.hide_arm();
    }

    // ------------------------------------------------------------------ input

    pub fn motion(&mut self, at: (f32, f32)) {
        if self.cursor != Some(at) {
            self.cursor = Some(at);
            self.dirty = true;
        }
        if self.bar.hover(Some(at)) {
            self.dirty = true;
        }
        if let Some(d) = self.down {
            if !self.ink.is_drawing() && (at.0 - d.0).hypot(at.1 - d.1) >= DRAW_AFTER {
                self.begin_stroke(d);
            }
            self.ink.extend(at);
        }
    }

    /// A button went down at `at`. On the toolbar it presses a button; elsewhere the hand
    /// taps (and the left button may start a drawing).
    pub fn press(&mut self, button: Button, at: (f32, f32)) {
        self.motion(at);
        if let Some(hit) = self.bar.click(at) {
            if let Some(action) = hit {
                self.bar_action(action);
            }
            return;
        }
        self.arm.press();
        self.sound.play();
        self.dirty = true;
        if button == Button::Left {
            self.down = Some(at);
        }
    }

    pub fn release(&mut self, button: Button) {
        self.arm.release();
        self.dirty = true;
        if button == Button::Left {
            self.down = None;
            self.end_stroke();
        }
    }

    /// The pointer left our surfaces.
    pub fn leave(&mut self) {
        if self.bar.hover(None) {
            self.dirty = true;
        }
    }

    /// A scroll the backend can't hand on: let input through for a moment, so the next
    /// notches reach the app underneath. False if scroll-through is off.
    pub fn scroll_through(&mut self, now: Instant) -> bool {
        if !self.cfg.scroll_through {
            return false;
        }
        self.scroll_until = Some(now + SCROLL_THROUGH);
        true
    }

    /// Work out whether input should go through to the apps underneath (`real_key`: the
    /// backend sees the click-for-real key held). Some(mode) when that changed; the
    /// backend then applies it. Leaving `Off` ends any tap or drawing in progress.
    pub fn update_passthrough(&mut self, real_key: bool) -> Option<Passthrough> {
        let want = if self.real || real_key {
            Passthrough::Real
        } else if self.scroll_until.is_some() {
            Passthrough::Scroll
        } else {
            Passthrough::Off
        };
        if want == self.passthrough {
            return None;
        }
        self.passthrough = want;
        if want == Passthrough::Off {
            self.last_step = None;
            self.redraw_arm();
        } else {
            self.down = None;
            self.end_stroke();
            self.arm.release();
            self.bar.hide();
        }
        Some(want)
    }

    // ------------------------------------------------------------------ commands

    pub fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Off => self.exit = true,
            Cmd::Clear => {
                self.ink.clear();
                self.dirty = true;
            }
            Cmd::Reload => self.reload(Config::load()),
            Cmd::Real(v) => self.real = v.unwrap_or(!self.real),
            Cmd::Hand(name) => {
                self.set_hand(&name);
            }
            Cmd::Sound(v) => {
                let on = v.unwrap_or(!self.cfg.sound_on());
                if self.look == Look::Laser {
                    self.cfg.laser_sound = on;
                } else {
                    self.cfg.sound = on;
                }
                self.sound.set_enabled(on);
                self.bar.sync(&self.cfg);
                self.dirty = true;
            }
            Cmd::TapSound(name) => match Choice::parse(&name) {
                Some(c) => {
                    self.cfg.tap_sound = name;
                    self.sound.set_choice(c);
                    self.sound.play();
                }
                None => eprintln!("tinyhand: unknown tap sound {name}"),
            },
        }
    }

    fn bar_action(&mut self, action: Action) {
        match action {
            Action::Hand(id) => self.command(Cmd::Hand(id)),
            Action::ToggleSound => self.command(Cmd::Sound(None)),
            Action::Exit => self.exit = true,
        }
    }

    /// Switch hands. The image is decoded and checked first: if that fails, the pointer
    /// keeps the hand it has (and says why on stderr).
    pub fn set_hand(&mut self, name: &str) -> bool {
        let loaded = Look::resolve(name).and_then(|look| match &look {
            Look::Hand(path) => load(path).map(|img| (look, Some(img))),
            Look::Laser => Ok((look, None)),
        });
        let (look, pending) = match loaded {
            Ok(l) => l,
            Err(e) => {
                eprintln!("tinyhand: {e}");
                return false;
            }
        };
        self.look = look;
        self.pending = pending;
        self.sprite = None;
        self.sprite_failed = None;
        self.cfg.hand = name.to_string();
        self.sound.set_enabled(self.cfg.sound_on());
        self.bar.sync(&self.cfg);
        self.redraw_arm();
        true
    }

    /// Apply a freshly loaded config (the settings window saved it).
    fn reload(&mut self, mut new: Config) {
        new.stats = self.cfg.stats;
        // Keep the current hand until the new one has loaded.
        let hand = std::mem::replace(&mut new.hand, self.cfg.hand.clone());
        let resize = new.size != self.cfg.size || new.shadow != self.cfg.shadow;
        self.cfg = new;
        self.arm.hand_frac = self.cfg.size;
        self.arm.shadow = self.cfg.shadow && self.caps.shadow;
        self.sound.set_choice(Choice::or_default(&self.cfg.tap_sound));
        self.sound.set_enabled(self.cfg.sound_on());
        self.bar.reset();
        if hand != self.cfg.hand {
            self.set_hand(&hand);
        } else if resize {
            // Size and shadow are baked into the sprite.
            self.sprite = None;
            self.sprite_failed = None;
        }
        self.redraw_arm();
    }

    // ------------------------------------------------------------------ timers

    /// Advance timers to `now` (call before drawing and after every wakeup). Sets `dirty`
    /// if something visible changed.
    pub fn tick(&mut self, now: Instant) {
        if self.scroll_until.is_some_and(|t| t <= now) {
            self.scroll_until = None;
        }
        if self.ink.step(now, self.caps.ink_fade) {
            self.dirty = true;
        }
        if self.bar.tick(now) {
            self.dirty = true;
        }
    }

    /// When `tick` next has something to do. None: nothing is waiting, so the backend can
    /// sleep until input arrives. (The arm springing back after a tap is paced by the
    /// backend's frame clock instead: see `animating`.)
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        [self.scroll_until, self.ink.next_deadline(now, FADE_TICK), self.bar.next_deadline(now, FADE_TICK)]
            .into_iter()
            .flatten()
            .min()
    }

    /// The arm is still moving after the last input (press springing back, grip
    /// catching up): keep drawing frames.
    pub fn animating(&self) -> bool {
        self.arm_shown && self.arm.settling()
    }

    /// Anything to draw now?
    pub fn wants_frame(&self, now: Instant) -> bool {
        self.dirty || self.animating() || self.ink.has_dirty() || self.ink.fading(now)
    }

    /// Nothing is moving: forget the clock so the next motion doesn't see a huge step.
    pub fn rest(&mut self) {
        self.last_step = None;
    }

    // ------------------------------------------------------------------ drawing

    /// Step the arm to `now` and say what to draw. None if nothing visible changed (or
    /// there's nothing to draw yet).
    pub fn plan_arm(&mut self, now: Instant) -> Option<ArmJob> {
        let (out, (cx, cy)) = (self.out?, self.cursor?);
        let dt = self.last_step.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
        self.last_step = Some(now);
        let out_h = out.h as f32;

        if self.look == Look::Laser {
            let press = self.arm.step_press(dt);
            let laser = Laser::new((cx, cy), out_h, self.cfg.size, press);
            if !self.new_key(FrameKey::Laser(laser.key())) {
                return None;
            }
            let frame = laser.frame(out.w, out.h, out.scale);
            return Some(ArmJob::Laser { laser, frame });
        }

        let hand_px = (self.cfg.size * out_h * out.scale).round() as u32;
        if !self.ensure_sprite(hand_px, out_h) {
            return None;
        }
        let sprite = self.sprite.as_ref()?;
        let pose = self.arm.step((cx, out_h - cy), out.w as f32, out_h, dt, &sprite.meta);
        if !self.new_key(FrameKey::Hand(pose.key())) {
            return None;
        }
        let plan = raster::plan(&pose, self.sprite.as_ref()?, out.w, out.h, out.scale);
        Some(ArmJob::Hand { pose, plan })
    }

    fn new_key(&mut self, key: FrameKey) -> bool {
        if self.last_key == Some(key) {
            return false;
        }
        self.last_key = Some(key);
        self.arm_shown = true;
        true
    }

    /// The sprite `plan_arm` planned with (for rendering its tiles).
    pub fn sprite(&self) -> Option<&Sprite> {
        self.sprite.as_ref()
    }

    /// Build the sprite at `hand_px`. A failure (the file vanished) keeps the old sprite,
    /// if there is one, rather than turning the pointer off mid-talk.
    fn ensure_sprite(&mut self, hand_px: u32, out_h: f32) -> bool {
        if self.sprite.as_ref().is_some_and(|s| s.hand_px == hand_px) || self.sprite_failed == Some(hand_px) {
            return self.sprite.is_some();
        }
        let Look::Hand(path) = &self.look else { return false };
        let loaded = match self.pending.take() {
            Some(l) => Ok(l),
            None => load(path),
        };
        match loaded {
            Ok((img, meta)) => {
                // Build, then drop the full-size image straight away.
                let mut s = Sprite::build(&img, meta, hand_px);
                if self.arm.shadow {
                    s.build_shadow(self.cfg.size * out_h, raster::shadow_sigmas(out_h));
                }
                self.sprite = Some(s);
                sound::trim_heap();
                true
            }
            Err(e) => {
                eprintln!("tinyhand: {e}");
                self.sprite_failed = Some(hand_px);
                self.sprite.is_some()
            }
        }
    }

    /// Draw the arm again next frame even if it hasn't moved (its surfaces were lost or
    /// moved).
    pub fn redraw_arm(&mut self) {
        self.last_key = None;
        self.dirty = true;
    }

    /// The backend hid the arm.
    pub fn hide_arm(&mut self) {
        self.arm_shown = false;
        self.last_key = None;
    }

    /// The toolbar for this frame (None: no toolbar; remove its surface).
    pub fn bar_view(&mut self, now: Instant) -> Option<BarView> {
        let out = self.out?;
        if self.passthrough != Passthrough::Off {
            return None;
        }
        self.bar.view(
            &self.cfg,
            (out.w, out.h, out.scale),
            self.cursor,
            self.ink.is_drawing(),
            self.caps.bar_fade,
            self.stats.frames > 0,
            now,
        )
    }

    /// Render the toolbar (`BarView::frame` buffer size).
    pub fn render_bar(&mut self, out: &mut [u32]) {
        if self.bar.render(out) {
            // The first render decoded the thumbnails' hand images.
            sound::trim_heap();
        }
    }

    /// Draw the toolbar again (its surface lost its pixels).
    pub fn redraw_bar(&mut self) {
        self.bar.refresh();
        self.dirty = true;
    }

    fn begin_stroke(&mut self, at: (f32, f32)) {
        let Some(out) = self.out else { return };
        let width = (out.h as f32 * INK_WIDTH).max(INK_MIN_WIDTH);
        let (style, width) = if self.cfg.is_laser() { (Style::Laser, width * LASER_CORE) } else { (Style::Ink, width) };
        let stroke = Stroke::styled(style, self.cfg.ink_colour(), width, out.scale, ink_tile(out.scale));
        self.ink.begin(stroke, at);
        self.dirty = true;
    }

    fn end_stroke(&mut self) {
        // A laser trail always fades soon after the drag ends; ink follows the setting.
        let delay = if self.cfg.is_laser() {
            Some(LASER_FADE_DELAY)
        } else {
            (self.cfg.fade_seconds > 0.0).then(|| self.cfg.fade_seconds.max(MIN_FADE_DELAY))
        };
        let fade_at = delay.map(|d| Instant::now() + Duration::from_secs_f32(d));
        if self.ink.end(fade_at) {
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests;
