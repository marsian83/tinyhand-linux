//! Every built-in hand loads and measures into a plausible hand on a stick.

use tinyhand::{config, hand};

#[test]
fn every_built_in_hand_measures() {
    for (id, _, _) in config::HANDS {
        let path = hand::find(id).unwrap_or_else(|| panic!("hand_{id}.png not found"));
        let img = hand::load_png(&path).unwrap();
        let m = hand::measure(&img).unwrap_or_else(|| panic!("{id}: can't measure"));
        // Top to bottom: fingertip, wrist, cuff, end of the stick.
        assert!(
            m.hand_top <= m.tip && m.tip < m.wrist && m.wrist <= m.cuff_end && m.cuff_end < m.rod_end,
            "{id}: {m:?}"
        );
        assert!(m.rod_end <= 1.0 && m.hand_top >= 0.0, "{id}: {m:?}");
        assert!(m.rod_min_x < m.rod_x && m.rod_x < m.rod_max_x, "{id}: rod centre outside the rod: {m:?}");
        assert!(m.hand_min_x <= m.tip_x && m.tip_x <= m.hand_max_x, "{id}: fingertip outside the hand: {m:?}");
        // The stick is much narrower than the hand.
        assert!(m.rod_max_x - m.rod_min_x < (m.hand_max_x - m.hand_min_x) / 2.0, "{id}: {m:?}");
    }
}
