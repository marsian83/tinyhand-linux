//! Arm pose: a port of `ArmView.aim` from the macOS app.
//!
//! Coordinates are logical pixels, y up, origin at the bottom-left of the output,
//! exactly like AppKit, so the maths matches the original line for line.
//! The original eases per 120 Hz tick; here easing is time-based so it looks the
//! same at any refresh rate.

use crate::hand::Meta;

const GRIP_LEAN: f32 = 0.30;
const GRIP_DEPTH: f32 = 0.12;
const GRIP_FOLLOW: f32 = 0.08;
const PRESS_DECAY: f32 = 0.18;
const MIN_ROD: f32 = 0.05;
const LIFT: f32 = 0.06;
const TICK_HZ: f32 = 120.0;
/// The shadow's blur radius as a fraction of the lift.
const SHADOW_BLUR: f32 = 0.35;

/// How high the hand floats above the screen (logical px) at a given press (0..=1).
fn lift(out_h: f32, press: f32) -> f32 {
    LIFT * out_h * (1.0 - 0.8 * press)
}

/// The shadow's blur radius (logical px) at a given press. The precomputed shadow
/// textures (raster::shadow_sigmas) use the same formula.
pub fn shadow_radius(out_h: f32, press: f32) -> f32 {
    (lift(out_h, press) * SHADOW_BLUR).max(1.0) + 5.0 * press
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub grip: (f32, f32),
    pub rotation: f32,
    pub rod_len: f32,
    pub hand_pts: f32,
    pub width_pts: f32,
    pub lift: f32,
    pub press: f32,
    /// Shadow offset in logical pixels, y up (down-right of the hand).
    pub shadow_off: (f32, f32),
    pub shadow_radius: f32,
    pub shadow_opacity: f32,
}

impl Pose {
    /// Same rounding the original uses to skip redraws that would look identical.
    pub fn key(&self) -> [i32; 7] {
        let r = |v: f32| (v * 100.0).round() as i32;
        [
            r(self.rotation),
            r(self.rod_len),
            r(self.hand_pts),
            r(self.grip.0),
            r(self.lift),
            r(self.press),
            r(self.shadow_opacity),
        ]
    }
}

pub struct Arm {
    pub hand_frac: f32,
    pub shadow: bool,
    pub held: bool,
    press: f32,
    grip_x: Option<f32>,
    grip_target: Option<f32>,
}

impl Arm {
    pub fn new(hand_frac: f32, shadow: bool) -> Self {
        Arm { hand_frac, shadow, held: false, press: 0.0, grip_x: None, grip_target: None }
    }

    pub fn press(&mut self) {
        self.press = 1.0;
        self.held = true;
    }

    pub fn release(&mut self) {
        self.held = false;
    }

    /// Still animating (press springing back or grip catching up)?
    pub fn settling(&self) -> bool {
        if self.press > 0.002 {
            return true;
        }
        matches!((self.grip_x, self.grip_target), (Some(g), Some(t)) if (t - g).abs() > 0.5)
    }

    /// Reset the grip so it doesn't slide in from another output.
    pub fn snap(&mut self) {
        self.grip_x = None;
    }

    /// Advance only the press spring (for the laser dot, which has no arm).
    pub fn step_press(&mut self, dt: f32) -> f32 {
        if !self.held {
            self.press *= (1.0 - PRESS_DECAY).powf(dt.clamp(0.0, 0.1) * TICK_HZ);
        }
        self.grip_x = None;
        self.press
    }

    pub fn step(&mut self, cursor: (f32, f32), w: f32, h: f32, dt: f32, m: &Meta) -> Pose {
        let ticks = (dt.clamp(0.0, 0.1)) * TICK_HZ;
        let target_x = (cursor.0 + GRIP_LEAN * w).clamp(w * 0.15, w * 1.05);
        let follow = 1.0 - (1.0 - GRIP_FOLLOW).powf(ticks);
        let gx = self.grip_x.map_or(target_x, |g| g + (target_x - g) * follow);
        self.grip_x = Some(gx);
        self.grip_target = Some(target_x);
        let grip = (gx, -GRIP_DEPTH * h);

        if !self.held {
            self.press *= (1.0 - PRESS_DECAY).powf(ticks);
        }
        let press = self.press;

        let (dx, dy) = (cursor.0 - grip.0, cursor.1 - grip.1);
        let reach = dx.hypot(dy);
        let aim = dy.atan2(dx);

        let hand_pts = self.hand_frac * h * (1.0 - 0.06 * press);
        let s = hand_pts / (m.wrist - m.hand_top);
        let width_pts = s * m.aspect;
        let hand_tip_pts = (m.wrist - m.tip) * s;
        let tip_off_x = (m.tip_x - m.rod_x) * width_pts;

        let along = (reach * reach - tip_off_x * tip_off_x).max(0.0).sqrt();
        let rod_len = (along - hand_tip_pts).max(MIN_ROD * h);
        let tip_angle = (rod_len + hand_tip_pts).atan2(tip_off_x);
        let rotation = aim - tip_angle;

        let lift = lift(h, press);
        Pose {
            grip,
            rotation,
            rod_len,
            hand_pts,
            width_pts,
            lift,
            press,
            shadow_off: (lift * 0.6, -lift),
            shadow_radius: shadow_radius(h, press),
            shadow_opacity: if self.shadow { 0.26 + 0.02 * press } else { 0.0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta {
            aspect: 0.35,
            hand_top: 0.0,
            tip: 0.01,
            wrist: 0.3,
            rod_end: 1.0,
            tip_x: 0.3,
            rod_x: 0.5,
            hand_min_x: 0.0,
            hand_max_x: 1.0,
            rod_min_x: 0.45,
            rod_max_x: 0.55,
            cuff_end: 0.33,
            cuff_min_x: 0.3,
            cuff_max_x: 0.7,
        }
    }

    fn run(arm: &mut Arm, steps: usize, dt: f32, cursor: (f32, f32)) -> Pose {
        let m = meta();
        let mut p = arm.step(cursor, 1800.0, 1125.0, 0.0, &m);
        for _ in 0..steps {
            p = arm.step(cursor, 1800.0, 1125.0, dt, &m);
        }
        p
    }

    #[test]
    fn grip_and_press_settle() {
        let mut arm = Arm::new(0.13, true);
        run(&mut arm, 1, 1.0 / 120.0, (300.0, 600.0));
        arm.press();
        arm.release();
        assert!(arm.settling());
        let p = run(&mut arm, 600, 1.0 / 120.0, (1500.0, 200.0));
        assert!(!arm.settling(), "still moving after 5 s");
        assert!(p.press < 0.01);
    }

    #[test]
    fn easing_is_the_same_at_any_refresh_rate() {
        let at = |hz: f32| {
            let mut arm = Arm::new(0.13, true);
            run(&mut arm, 1, 1.0 / hz, (200.0, 500.0));
            arm.press();
            arm.release();
            run(&mut arm, (hz * 0.25) as usize, 1.0 / hz, (1400.0, 300.0))
        };
        let (a, b) = (at(60.0), at(240.0));
        assert!((a.grip.0 - b.grip.0).abs() < 2.0, "grip {} vs {}", a.grip.0, b.grip.0);
        assert!((a.press - b.press).abs() < 0.01, "press {} vs {}", a.press, b.press);
    }

    #[test]
    fn shadow_follows_the_press() {
        let h = 1125.0;
        assert!(shadow_radius(h, 1.0) != shadow_radius(h, 0.0));
        let mut arm = Arm::new(0.13, false);
        assert_eq!(run(&mut arm, 1, 0.01, (500.0, 500.0)).shadow_opacity, 0.0, "shadow off");
    }
}
