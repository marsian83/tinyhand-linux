//! The behaviour both backends share, driven without a display.

use super::*;

/// A tile "surface" for tests: just an id.
type S = u32;

const OUT: Output = Output { id: 1, w: 1800, h: 1125, scale: 1.0 };
const FRAME: Duration = Duration::from_micros(8_333);

fn cfg(hand: &str) -> Config {
    Config {
        hand: hand.into(),
        sound: false,
        laser_sound: false,
        toolbar: Vec::new(),
        fade_seconds: 3.0,
        ..Config::default()
    }
}

fn caps() -> Caps {
    Caps { shadow: true, ink_fade: FadeBy::Redraw, bar_fade: false }
}

fn session(hand: &str) -> Session<S> {
    let mut s = Session::new(cfg(hand), caps()).unwrap();
    s.set_output(OUT);
    s.motion((900.0, 500.0));
    s
}

/// Give every dirty tile of every stroke a surface, as a backend would when drawing.
fn show_ink(s: &mut Session<S>) {
    let mut id = 100;
    for st in s.ink.strokes_mut() {
        for k in st.take_dirty() {
            st.surfaces.entry(k).or_insert_with(|| {
                id += 1;
                id
            });
        }
    }
}

/// Run frames like a backend at 120 Hz until nothing wants one (or give up).
fn settle(s: &mut Session<S>, mut now: Instant) -> Instant {
    for _ in 0..2000 {
        s.tick(now);
        if !s.wants_frame(now) {
            return now;
        }
        s.dirty = false;
        let _ = s.plan_arm(now);
        show_ink(s);
        now += FRAME;
    }
    panic!("never settled");
}

#[test]
fn a_short_drag_taps_and_a_long_one_draws() {
    let mut s = session("laser");
    s.press(Button::Left, (100.0, 100.0));
    s.motion((103.0, 100.0));
    assert!(!s.ink.is_drawing(), "3 px is still a tap");
    s.motion((104.0, 100.0));
    assert!(s.ink.is_drawing(), "4 px starts drawing");
    s.motion((150.0, 120.0));
    assert!(s.ink.has_dirty());
    s.release(Button::Left);
    assert!(!s.ink.is_drawing());
    assert_eq!(s.ink.strokes().count(), 1, "the finished stroke stays on screen");
}

#[test]
fn only_the_left_button_draws() {
    let mut s = session("laser");
    s.press(Button::Right, (100.0, 100.0));
    s.motion((200.0, 100.0));
    assert!(!s.ink.is_drawing());
    s.release(Button::Right);
    assert!(s.ink.is_empty());
}

#[test]
fn click_for_real_ends_the_drawing_and_hides_the_hand() {
    let mut s = session("laser");
    s.press(Button::Left, (100.0, 100.0));
    s.motion((200.0, 100.0));
    assert!(s.ink.is_drawing());
    s.command(Cmd::Real(Some(true)));
    assert_eq!(s.update_passthrough(false), Some(Passthrough::Real));
    assert!(!s.ink.is_drawing(), "the stroke ended");
    assert_eq!(s.ink.strokes().count(), 1, "but stays on screen");
    assert_eq!(s.update_passthrough(false), None, "no change, nothing to do");
    // A held key does the same, and letting go of everything turns the pointer back on.
    s.command(Cmd::Real(Some(false)));
    assert_eq!(s.update_passthrough(true), None, "still real while the key is held");
    assert_eq!(s.update_passthrough(false), Some(Passthrough::Off));
    assert!(s.dirty);
}

#[test]
fn a_scroll_lets_input_through_for_a_moment() {
    let mut s = session("laser");
    let t0 = Instant::now();
    assert!(s.scroll_through(t0));
    assert_eq!(s.update_passthrough(false), Some(Passthrough::Scroll));
    assert_eq!(s.next_deadline(t0), Some(t0 + SCROLL_THROUGH));
    s.tick(t0 + SCROLL_THROUGH);
    assert_eq!(s.update_passthrough(false), Some(Passthrough::Off));

    let mut off = Session::<S>::new(Config { scroll_through: false, ..cfg("laser") }, caps()).unwrap();
    assert!(!off.scroll_through(t0));
}

#[test]
fn drawings_fade_out_on_schedule() {
    let mut s = session("laser");
    s.cfg.fade_seconds = 1.0;
    s.press(Button::Left, (100.0, 100.0));
    s.motion((300.0, 200.0));
    let before = Instant::now();
    s.release(Button::Left);
    let after = Instant::now();
    show_ink(&mut s);
    let tiles = s.ink.strokes().next().unwrap().surfaces.len();
    assert!(tiles > 0);

    let at = s.next_deadline(after).expect("a fade is scheduled");
    assert!(at >= before + Duration::from_secs(1) && at <= after + Duration::from_secs(1));
    s.tick(at - Duration::from_millis(1));
    assert!(!s.ink.has_dirty(), "nothing changes before the fade starts");

    // Halfway: dimmer, every tile drawn again (FadeBy::Redraw), next step one tick away.
    let mid = at + Duration::from_millis(300);
    s.tick(mid);
    let st = s.ink.strokes().next().unwrap();
    assert!(st.alpha > 0.0 && st.alpha < 1.0, "alpha {}", st.alpha);
    assert!(s.ink.has_dirty());
    assert_eq!(s.next_deadline(mid), Some(mid + FADE_TICK));

    // Done: the stroke is gone and its surfaces are handed back for destroying.
    s.tick(at + Duration::from_millis(600));
    assert!(s.ink.is_empty());
    assert_eq!(s.ink.take_dead().len(), tiles);
    assert_eq!(s.next_deadline(at + Duration::from_secs(1)), None);
}

#[test]
fn compositor_fades_change_alpha_without_redrawing() {
    let mut s = Session::<S>::new(
        Config { fade_seconds: 0.5, ..cfg("laser") },
        Caps { ink_fade: FadeBy::Compositor, ..caps() },
    )
    .unwrap();
    s.set_output(OUT);
    s.press(Button::Left, (100.0, 100.0));
    s.motion((300.0, 200.0));
    s.release(Button::Left);
    show_ink(&mut s);
    let at = s.next_deadline(Instant::now()).unwrap();
    s.tick(at + Duration::from_millis(200));
    assert!(!s.ink.has_dirty());
    let st = s.ink.strokes_mut().next().unwrap();
    assert!(st.take_alpha_change().is_some_and(|a| a < 1.0));
    assert_eq!(st.take_alpha_change(), None);
}

#[test]
fn fade_zero_keeps_drawings() {
    let mut s = session("laser");
    s.cfg.fade_seconds = 0.0;
    s.press(Button::Left, (100.0, 100.0));
    s.motion((300.0, 200.0));
    s.release(Button::Left);
    assert_eq!(s.next_deadline(Instant::now()), None);
}

#[test]
fn idle_means_no_timer() {
    // The zero-wakeup promise: once everything has settled, nothing asks to wake up.
    let mut s = session("coral");
    let now = Instant::now();
    assert!(matches!(s.plan_arm(now), Some(ArmJob::Hand { .. })));
    let now = settle(&mut s, now + FRAME);
    assert_eq!(s.next_deadline(now), None);
    assert!(s.plan_arm(now + FRAME).is_none(), "an identical frame isn't drawn");

    // A tap animates for a while, then everything is quiet again.
    s.press(Button::Left, (900.0, 500.0));
    s.release(Button::Left);
    assert!(s.wants_frame(now));
    let now = settle(&mut s, now + FRAME);
    assert!(!s.animating());
    assert_eq!(s.next_deadline(now), None);
}

#[test]
fn another_output_clears_the_drawing() {
    let mut s = session("laser");
    s.press(Button::Left, (100.0, 100.0));
    s.motion((300.0, 200.0));
    s.release(Button::Left);
    show_ink(&mut s);
    let tiles = s.ink.strokes().next().unwrap().surfaces.len();
    s.set_output(Output { scale: 1.0, ..OUT }); // same output, same size: nothing
    assert!(!s.ink.is_empty());
    s.set_output(Output { id: 2, ..OUT });
    assert!(s.ink.is_empty());
    assert_eq!(s.ink.take_dead().len(), tiles);
}

#[test]
fn clear_hands_back_every_surface() {
    let mut s = session("laser");
    for y in [100.0, 400.0] {
        s.press(Button::Left, (100.0, y));
        s.motion((300.0, y + 50.0));
        s.release(Button::Left);
    }
    show_ink(&mut s);
    let tiles: usize = s.ink.strokes().map(|st| st.surfaces.len()).sum();
    s.command(Cmd::Clear);
    assert!(s.ink.is_empty());
    assert_eq!(s.ink.take_dead().len(), tiles);
}

#[test]
fn a_bad_hand_keeps_the_current_one() {
    let mut s = session("laser");
    assert!(!s.set_hand("no-such-hand"));
    assert_eq!(*s.look(), Look::Laser);
    assert_eq!(s.cfg.hand, "laser");
    assert!(s.set_hand("robot"));
    assert!(matches!(s.look(), Look::Hand(_)));
    assert!(matches!(s.plan_arm(Instant::now()), Some(ArmJob::Hand { .. })));
    assert!(Session::<S>::new(cfg("no-such-hand"), caps()).is_err(), "startup fails loudly");
}

#[test]
fn reload_applies_new_settings_and_keeps_a_working_hand() {
    let mut s = session("coral");
    let _ = s.plan_arm(Instant::now());
    let px = s.sprite().unwrap().hand_px;
    s.reload(Config { size: 0.2, hand: "no-such-hand".into(), ..cfg("coral") });
    assert_eq!(s.cfg.hand, "coral", "a missing hand in the new config is skipped");
    let _ = s.plan_arm(Instant::now());
    assert!(s.sprite().unwrap().hand_px > px, "the sprite was rebuilt bigger");
}

#[test]
fn the_toolbar_takes_its_clicks() {
    let mut s =
        Session::<S>::new(Config { toolbar: vec!["coral".into(), "laser".into()], ..cfg("coral") }, caps()).unwrap();
    s.set_output(OUT);
    let now = Instant::now();
    assert!(s.bar_view(now).is_none(), "no toolbar before the first frame");
    s.stats.record(Duration::ZERO);
    let v = s.bar_view(now).expect("shown at start");
    assert!(v.mapped && v.render && v.alpha == 1.0);
    let f = v.frame;
    assert!(f.y + f.h <= OUT.h && f.x > 0 && f.x + f.w < OUT.w, "{f:?}");

    // The middle of the bar is a button or the bar itself: no tap either way.
    let mid = ((f.x + f.w / 2) as f32, (f.y + f.h / 2 - 4) as f32);
    s.press(Button::Left, mid);
    s.release(Button::Left);
    assert!(!s.arm.settling(), "a click on the bar isn't a tap");
    // Away from the bar it's a tap.
    s.press(Button::Left, (200.0, 200.0));
    assert!(s.arm.settling());
    s.release(Button::Left);

    // The reveal-on-start window ends; away from the bar it hides.
    s.motion((200.0, 200.0));
    s.tick(now + bar::REVEAL_ON_START);
    let v = s.bar_view(now + bar::REVEAL_ON_START).unwrap();
    assert!(!v.mapped);
    // Near it, it comes back.
    s.motion(((f.x + f.w / 2) as f32, (f.y - 20) as f32));
    assert!(s.bar_view(now + bar::REVEAL_ON_START).unwrap().mapped);
    // Click for real hides it.
    s.command(Cmd::Real(Some(true)));
    s.update_passthrough(false);
    assert!(s.bar_view(now + bar::REVEAL_ON_START).is_none());
}
