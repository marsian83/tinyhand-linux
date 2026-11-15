//! Settings: built-in hands, the config file and command-line overrides.
//!
//! Config lives in `$XDG_CONFIG_HOME/tinyhand/config` (usually ~/.config/tinyhand/config)
//! as `key = value` lines. Defaults match the macOS app.

use std::path::{Path, PathBuf};

/// Built-in hands: (id, title, ink colour 0xRRGGBB). Same list and ink colours as macOS.
pub const HANDS: &[(&str, &str, u32)] = &[
    ("lavender", "Lavender toy", 0x8754F2),
    ("glove", "Cursor glove", 0x8754F2),
    ("coral", "Coral vinyl", 0xFF291F),
    ("plush", "Plush", 0x2F86F0),
    ("robot", "Robot", 0x16A87A),
    ("pink", "Bubblegum", 0xEE4F98),
    ("blue", "Sky", 0x2F86F0),
    ("mint", "Mint", 0x16A87A),
    ("lemon", "Lemon", 0xF2B705),
    ("black", "Midnight", 0x1D1D21),
    ("chrome", "Chrome", 0x6B7380),
    ("holo", "Holo", 0x8754F2),
    ("fuzzy", "Fuzzy", 0x8754F2),
    ("wood", "Wood", 0x9A5E2C),
    ("marble", "Marble", 0x6B7380),
    ("terrazzo", "Terrazzo", 0xF0561D),
    ("glass", "Glass", 0x1597E5),
    ("jelly", "Jelly", 0xA33FE6),
    ("pastel", "Pastel", 0xEE4F98),
    ("pixel", "Pixel", 0x1D1D21),
    ("lowpoly", "Low poly", 0x8754F2),
    ("knit", "Knit", 0x8754F2),
    ("skeleton", "Skeleton", 0x9A5E2C),
    ("astro", "Astronaut", 0x2F86F0),
    ("space", "Space", 0xFF291F),
    ("monster", "Monster", 0x3E9E2E),
    ("water", "Water", 0x1597E5),
    ("lava", "Lava", 0xF0561D),
    ("grass", "Grass", 0x3E9E2E),
    ("cactus", "Cactus", 0x3E9E2E),
    ("choco", "Chocolate", 0x9A5E2C),
    ("cloud", "Cloud", 0x2F86F0),
    ("mech", "Mech", 0xF0561D),
    ("neon", "Neon", 0xE0309A),
    ("leaf", "Leaf", 0x3E9E2E),
    ("cookie", "Gingerbread", 0x9A5E2C),
];

/// The laser dot's colour (Ink.red in the macOS app).
pub const LASER: u32 = 0xFF291F;

/// The hand id that shows the laser dot instead of a hand.
pub const LASER_ID: &str = "laser";

/// Display title of a built-in hand, a custom hand (its name) or the laser.
pub fn hand_title(id: &str) -> Option<String> {
    if id == LASER_ID {
        return Some("Laser dot".into());
    }
    if let Some(h) = HANDS.iter().find(|h| h.0 == id) {
        return Some(h.1.into());
    }
    if id.starts_with("custom-") {
        return crate::handmaker::get(id).map(|h| h.name);
    }
    None
}

/// The "match" ink colour of a built-in hand, a custom hand or the laser.
pub fn hand_ink(id: &str) -> Option<u32> {
    if id == LASER_ID {
        return Some(LASER);
    }
    if let Some(h) = HANDS.iter().find(|h| h.0 == id) {
        return Some(h.2);
    }
    if id.starts_with("custom-") {
        return crate::handmaker::get(id).map(|h| h.ink);
    }
    None
}

/// Every hand to choose from as (id, title): built-ins, then custom hands, then the laser.
pub fn all_hands() -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = HANDS.iter().map(|h| (h.0.to_string(), h.1.to_string())).collect();
    all.extend(crate::handmaker::list().into_iter().map(|h| (h.id, h.name)));
    all.push((LASER_ID.into(), "Laser dot".into()));
    all
}

/// Fixed ink colours, as in the macOS ink tray.
pub const INK_COLOURS: &[(&str, u32)] = &[
    ("red", 0xFF291F),
    ("orange", 0xFF8A00),
    ("yellow", 0xFFCC00),
    ("green", 0x30C25A),
    ("blue", 0x0A84FF),
    ("purple", 0x8754F2),
    ("white", 0xFFFFFF),
    ("black", 0x1D1D21),
];

#[derive(Clone, Debug)]
pub struct Config {
    /// Hand id, a path to a PNG, or "laser".
    pub hand: String,
    /// Hand height as a fraction of the screen height (0.08–0.20).
    pub size: f32,
    pub shadow: bool,
    /// Tap sound on for hands / for the laser dot.
    pub sound: bool,
    pub laser_sound: bool,
    pub tap_sound: String,
    /// Seconds before a drawing fades; 0 keeps drawings until cleared.
    pub fade_seconds: f32,
    /// "match" (the hand's colour) or one of INK_COLOURS.
    pub ink: String,
    /// Let scroll wheel events through to the app underneath (Wayland).
    pub scroll_through: bool,
    /// Hands pinned to the toolbar (empty = no toolbar).
    pub toolbar: Vec<String>,
    pub stats: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hand: "lavender".into(),
            size: 0.13,
            shadow: true,
            sound: true,
            laser_sound: false,
            tap_sound: "thock".into(),
            fade_seconds: 3.0,
            ink: "match".into(),
            scroll_through: true,
            toolbar: ["lavender", "glove", "coral", "plush", "robot", "laser"].map(String::from).to_vec(),
            stats: false,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("tinyhand").join("config")
    }

    /// Defaults, then the config file (if any). Unknown keys are reported and ignored.
    pub fn load() -> Config {
        let mut c = Config::default();
        if let Ok(text) = std::fs::read_to_string(Self::path()) {
            for (n, line) in text.lines().enumerate() {
                let line = strip_comment(line);
                if line.is_empty() {
                    continue;
                }
                let Some((k, v)) = line.split_once('=') else {
                    eprintln!("tinyhand: config line {}: expected key = value", n + 1);
                    continue;
                };
                if let Err(e) = c.set(k.trim(), v.trim()) {
                    eprintln!("tinyhand: config line {}: {e}", n + 1);
                }
            }
        }
        c
    }

    pub fn set(&mut self, key: &str, v: &str) -> Result<(), String> {
        let flag = |v: &str| match v {
            "true" | "yes" | "on" | "1" => Ok(true),
            "false" | "no" | "off" | "0" => Ok(false),
            _ => Err(format!("{key}: expected true/false, got {v}")),
        };
        let num = |v: &str| {
            v.parse::<f32>().ok().filter(|n| n.is_finite()).ok_or_else(|| format!("{key}: expected a number, got {v}"))
        };
        match key {
            "hand" => self.hand = v.into(),
            "size" => self.size = num(v)?.clamp(0.08, 0.20),
            "shadow" => self.shadow = flag(v)?,
            "sound" => self.sound = flag(v)?,
            "laser_sound" => self.laser_sound = flag(v)?,
            "tap_sound" => self.tap_sound = v.into(),
            "fade_seconds" => self.fade_seconds = num(v)?.clamp(0.0, 3600.0),
            "ink" => {
                if v != "match" && !INK_COLOURS.iter().any(|(n, _)| *n == v) && parse_hex(v).is_none() {
                    return Err(format!("ink: unknown colour {v}"));
                }
                self.ink = v.into()
            }
            "scroll_through" => self.scroll_through = flag(v)?,
            "toolbar" => {
                self.toolbar = if matches!(v, "off" | "none" | "false") {
                    Vec::new()
                } else {
                    v.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect()
                }
            }
            "stats" => self.stats = flag(v)?,
            _ => return Err(format!("unknown setting {key}")),
        }
        Ok(())
    }

    /// Every saved setting as (key, value) in the config file's syntax.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let toolbar = if self.toolbar.is_empty() { "off".into() } else { self.toolbar.join(", ") };
        vec![
            ("hand", self.hand.clone()),
            ("size", format!("{:.3}", self.size)),
            ("shadow", self.shadow.to_string()),
            ("sound", self.sound.to_string()),
            ("laser_sound", self.laser_sound.to_string()),
            ("tap_sound", self.tap_sound.clone()),
            ("fade_seconds", format!("{}", self.fade_seconds)),
            ("ink", self.ink.clone()),
            ("scroll_through", self.scroll_through.to_string()),
            ("toolbar", toolbar),
        ]
    }

    /// Write the settings to the config file, keeping comments, order and unknown lines.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        write_atomic(&path, merge(&old, &self.entries()).as_bytes())
    }

    pub fn is_laser(&self) -> bool {
        self.hand == LASER_ID
    }

    /// Ink colour as 0xRRGGBB for the current hand and ink setting.
    pub fn ink_colour(&self) -> u32 {
        if let Some(c) = INK_COLOURS.iter().find(|(n, _)| *n == self.ink) {
            return c.1;
        }
        if let Some(c) = parse_hex(&self.ink) {
            return c;
        }
        hand_ink(&self.hand).unwrap_or(0x8754F2)
    }

    pub fn sound_on(&self) -> bool {
        if self.is_laser() {
            self.laser_sound
        } else {
            self.sound
        }
    }
}

/// Replace each key's line in `old` with its new value and append the keys it lacks.
fn merge(old: &str, entries: &[(&str, String)]) -> String {
    let mut done = vec![false; entries.len()];
    let mut out = String::new();
    for line in old.lines() {
        let key = strip_comment(line).split_once('=').map(|(k, _)| k.trim());
        match key.and_then(|k| entries.iter().position(|(e, _)| *e == k)) {
            Some(i) if !done[i] => {
                done[i] = true;
                out += &format!("{} = {}\n", entries[i].0, entries[i].1);
            }
            Some(_) => {} // a duplicate key: the first line now holds the value
            None => {
                out += line;
                out.push('\n');
            }
        }
    }
    for (i, (k, v)) in entries.iter().enumerate() {
        if !done[i] {
            out += &format!("{k} = {v}\n");
        }
    }
    out
}

/// The line without its comment, trimmed. A comment is a whole line starting with `#`,
/// or ` # ...` after a value; a `#` inside a value (`ink = #00ff00`, a path) is kept.
fn strip_comment(line: &str) -> &str {
    let line = line.trim();
    if line.starts_with('#') {
        return "";
    }
    let b = line.as_bytes();
    let cut = (1..b.len())
        .find(|&i| b[i] == b'#' && b[i - 1].is_ascii_whitespace() && b.get(i + 1).is_none_or(u8::is_ascii_whitespace));
    cut.map_or(line, |i| line[..i].trim_end())
}

/// Write then rename (to a name only this process uses), so a running pointer never
/// reads half a file.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().ok_or(std::io::ErrorKind::InvalidInput)?;
    let tmp = dir.join(format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn parse_hex(v: &str) -> Option<u32> {
    let h = v.strip_prefix('#')?;
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_strip_but_hashes_in_values_stay() {
        assert_eq!(strip_comment("# a comment"), "");
        assert_eq!(strip_comment("  size = 0.1  # smaller"), "size = 0.1");
        assert_eq!(strip_comment("ink = #00ff00"), "ink = #00ff00");
        assert_eq!(strip_comment("ink = #00ff00 # green"), "ink = #00ff00");
        assert_eq!(strip_comment("hand = /tmp/my#hand.png"), "hand = /tmp/my#hand.png");
    }

    #[test]
    fn hex_ink_survives_save_and_load() {
        let mut c = Config::default();
        c.set("ink", "#00ff00").unwrap();
        let text = merge("# my settings\nink = red # was red\n", &c.entries());
        assert!(text.starts_with("# my settings\n"), "{text}");
        let mut back = Config::default();
        for line in text.lines().map(strip_comment).filter(|l| !l.is_empty()) {
            let (k, v) = line.split_once('=').unwrap();
            back.set(k.trim(), v.trim()).unwrap();
        }
        assert_eq!(back.ink_colour(), 0x00ff00);
    }

    #[test]
    fn parses_settings_and_rejects_bad_ones() {
        let mut c = Config::default();
        c.set("size", "0.5").unwrap();
        assert_eq!(c.size, 0.20, "size is clamped");
        c.set("ink", "#00ff00").unwrap();
        assert_eq!(c.ink_colour(), 0x00ff00);
        assert!(c.set("ink", "mauve").is_err());
        assert!(c.set("shadow", "maybe").is_err());
        assert!(c.set("nope", "1").is_err());
        assert!(c.set("fade_seconds", "inf").is_err(), "a non-finite fade would panic in Duration");
        assert!(c.set("size", "NaN").is_err());
        c.set("fade_seconds", "1e30").unwrap();
        assert_eq!(c.fade_seconds, 3600.0);
    }

    #[test]
    fn saving_keeps_comments_and_round_trips() {
        let c = Config { size: 0.1, toolbar: vec![], ..Default::default() };
        let text = merge("# my settings\nsize = 0.2 # big\nsize = 0.3\nfuture_key = 1\n", &c.entries());
        assert!(text.starts_with("# my settings\nsize = 0.100\nfuture_key = 1\n"), "{text}");
        assert_eq!(text.matches("size").count(), 1);
        let mut back = Config::default();
        for line in text.lines().skip(1).filter(|l| !l.starts_with("future")) {
            let (k, v) = line.split_once('=').unwrap();
            back.set(k.trim(), v.trim()).unwrap();
        }
        assert_eq!(back.size, 0.1);
        assert!(back.toolbar.is_empty());
    }

    #[test]
    fn ink_matches_the_hand_by_default() {
        let mut c = Config { hand: "robot".into(), ..Default::default() };
        assert_eq!(c.ink_colour(), 0x16A87A);
        c.hand = "laser".into();
        assert_eq!(c.ink_colour(), LASER);
        assert!(!c.sound_on(), "laser is silent by default, as on macOS");
    }

    #[test]
    fn every_builtin_hand_has_an_asset() {
        for (id, _, _) in HANDS {
            let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("assets/hands/hand_{id}.png"));
            assert!(p.exists(), "missing {}", p.display());
        }
    }
}
