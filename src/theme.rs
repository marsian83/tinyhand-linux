//! The desktop's light/dark preference and accent colour, read from the XDG desktop
//! portal (`org.freedesktop.appearance`). GNOME, KDE Plasma, COSMIC and the GTK portal
//! backend used by most wlroots desktops answer it.
//!
//! The portal is asked once, on a background thread started at launch, so a slow or
//! missing portal never delays the pointer. Whoever needs the answer waits briefly for
//! it and otherwise falls back to the dark look.

use std::{
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// sRGB, 0..1. `None` when the desktop has no accent colour set.
    pub accent: Option<[f32; 3]>,
}

impl Theme {
    /// What the bar looks like when the desktop says nothing.
    pub const FALLBACK: Theme = Theme { dark: true, accent: None };
}

static THEME: OnceLock<Theme> = OnceLock::new();

/// Start asking the portal. Call once, early.
pub fn start() {
    if std::env::var_os("TINYHAND_THEME").is_some() {
        // Forced from the environment (testing, screenshots): no D-Bus at all.
        let _ = THEME.set(parse_override(&std::env::var("TINYHAND_THEME").unwrap_or_default()));
        return;
    }
    let spawned = thread::Builder::new().name("theme".into()).spawn(|| {
        let _ = THEME.set(read_portal().unwrap_or(Theme::FALLBACK));
    });
    if spawned.is_err() {
        let _ = THEME.set(Theme::FALLBACK);
    }
}

/// The theme, waiting up to `wait` for the portal's answer.
pub fn current(wait: Duration) -> Theme {
    let until = Instant::now() + wait;
    loop {
        if let Some(t) = THEME.get() {
            return *t;
        }
        if Instant::now() >= until {
            return Theme::FALLBACK;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

/// `TINYHAND_THEME=light|dark[,#rrggbb]`.
fn parse_override(v: &str) -> Theme {
    let mut parts = v.split(',');
    let dark = parts.next().map(str::trim) != Some("light");
    let accent = parts.next().and_then(|hex| {
        let n = u32::from_str_radix(hex.trim().trim_start_matches('#'), 16).ok()?;
        Some([(n >> 16) as u8, (n >> 8) as u8, n as u8].map(|c| f32::from(c) / 255.0))
    });
    Theme { dark, accent }
}

fn read_portal() -> zbus::Result<Theme> {
    use zbus::zvariant::OwnedValue;

    let conn = zbus::blocking::Connection::session()?;
    let read = |key: &str| -> zbus::Result<OwnedValue> {
        conn.call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Settings"),
            "ReadOne",
            &("org.freedesktop.appearance", key),
        )?
        .body()
        .deserialize::<OwnedValue>()
    };

    // 0 = no preference, 1 = dark, 2 = light. No preference keeps the dark look.
    let light = read("color-scheme").ok().and_then(|v| u32::try_from(v).ok()) == Some(2);
    // An (r, g, b) triple of doubles; values outside 0..1 mean "not set".
    let accent = read("accent-color")
        .ok()
        .and_then(|v| <(f64, f64, f64)>::try_from(v).ok())
        .map(|(r, g, b)| [r, g, b])
        .filter(|c| c.iter().all(|v| (0.0..=1.0).contains(v)))
        .map(|c| c.map(|v| v as f32));
    Ok(Theme { dark: !light, accent })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_parses() {
        assert_eq!(parse_override("dark"), Theme::FALLBACK);
        let t = parse_override("light, #3584e4");
        assert!(!t.dark);
        let a = t.accent.unwrap();
        assert!((a[0] - 0x35 as f32 / 255.0).abs() < 1e-6 && (a[2] - 0xe4 as f32 / 255.0).abs() < 1e-6);
    }
}
