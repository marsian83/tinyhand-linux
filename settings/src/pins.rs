//! Toolbar pins (`toolbar = …` in the config): at most `MAX` pointers, in the user's order.
//! Same rules as the macOS app.

/// The toolbar has room for this many pointers (Preferences.maxPinned on macOS).
pub const MAX: usize = 8;

#[derive(Debug, PartialEq)]
pub enum Refused {
    /// The toolbar already has `MAX` pointers.
    Full,
    /// That was the last pointer; the toolbar is turned off on the Drawing page instead.
    Last,
}

impl Refused {
    pub fn message(&self, title: &str) -> String {
        match self {
            Refused::Full => format!("The toolbar is full. Unpin a pointer to add {title}."),
            Refused::Last => {
                "Keep at least one pointer in the toolbar. You can turn the toolbar off under Drawing.".into()
            }
        }
    }
}

/// Pin `id` at the end, or unpin it. Returns whether it's pinned now.
pub fn toggle(pins: &mut Vec<String>, id: &str) -> Result<bool, Refused> {
    if let Some(i) = pins.iter().position(|p| p == id) {
        if pins.len() == 1 {
            return Err(Refused::Last);
        }
        pins.remove(i);
        return Ok(false);
    }
    if pins.len() >= MAX {
        return Err(Refused::Full);
    }
    pins.push(id.to_string());
    Ok(true)
}

/// Pin a newly made hand before a trailing laser dot, if there's room. Returns whether it
/// was pinned.
pub fn add_new(pins: &mut Vec<String>, id: &str) -> bool {
    if pins.len() >= MAX || pins.iter().any(|p| p == id) {
        return false;
    }
    let at = pins.len() - usize::from(pins.last().is_some_and(|p| p == "laser"));
    pins.insert(at, id.to_string());
    true
}

/// Move `id` to position `to` (clamped), keeping the others in order.
pub fn move_to(pins: &mut Vec<String>, id: &str, to: usize) {
    if let Some(from) = pins.iter().position(|p| p == id) {
        let p = pins.remove(from);
        pins.insert(to.min(pins.len()), p);
    }
}

/// Drop a hand that no longer exists. An emptied toolbar gets `fallback` instead, so
/// removing a hand never turns the toolbar off.
pub fn forget(pins: &mut Vec<String>, id: &str, fallback: &[String]) {
    let had = !pins.is_empty();
    pins.retain(|p| p != id);
    if had && pins.is_empty() {
        pins.extend(fallback.iter().filter(|p| *p != id).cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn toggling_keeps_order_and_limits() {
        let mut p = v(&["lavender", "coral", "laser"]);
        assert_eq!(toggle(&mut p, "robot"), Ok(true));
        assert_eq!(p, v(&["lavender", "coral", "laser", "robot"]));
        assert_eq!(toggle(&mut p, "coral"), Ok(false));
        assert_eq!(p, v(&["lavender", "laser", "robot"]));
        let mut full = v(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        assert_eq!(toggle(&mut full, "i"), Err(Refused::Full));
        let mut one = v(&["a"]);
        assert_eq!(toggle(&mut one, "a"), Err(Refused::Last));
        assert_eq!(one, v(&["a"]));
    }

    #[test]
    fn new_hands_go_before_the_laser() {
        let mut p = v(&["lavender", "laser"]);
        assert!(add_new(&mut p, "custom-1"));
        assert_eq!(p, v(&["lavender", "custom-1", "laser"]));
        assert!(!add_new(&mut p, "custom-1"), "already pinned");
        let mut q = v(&["lavender"]);
        assert!(add_new(&mut q, "custom-2"));
        assert_eq!(q, v(&["lavender", "custom-2"]));
    }

    #[test]
    fn moving_and_forgetting() {
        let mut p = v(&["a", "b", "c"]);
        move_to(&mut p, "a", 2);
        assert_eq!(p, v(&["b", "c", "a"]));
        move_to(&mut p, "a", 0);
        assert_eq!(p, v(&["a", "b", "c"]));
        move_to(&mut p, "c", 99);
        assert_eq!(p, v(&["a", "b", "c"]));
        let mut only = v(&["custom-1"]);
        forget(&mut only, "custom-1", &v(&["lavender", "custom-1", "laser"]));
        assert_eq!(only, v(&["lavender", "laser"]));
        let mut off = Vec::new();
        forget(&mut off, "custom-1", &v(&["lavender"]));
        assert!(off.is_empty(), "an off toolbar stays off");
    }
}
