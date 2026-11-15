//! Keybinding snippets for the desktop we're running on. Wayland doesn't let apps grab
//! global keys, so `tinyhand setup` and the settings window both show the lines to add
//! to the desktop's own config. Nothing here edits any file.

use std::path::Path;

/// Key bindings for one desktop.
#[derive(Clone, Debug, PartialEq)]
pub struct Bindings {
    /// Which desktop this is for, e.g. "Hyprland" or "GNOME".
    pub desktop: &'static str,
    /// What to do with `code`, starting in lower case: "add to ~/.config/sway/config:".
    pub how: String,
    /// Lines to paste or run. Empty when the desktop is set up by clicking (KDE).
    pub code: String,
    /// A note to show after the code, if any.
    pub note: Option<&'static str>,
}

const INTRO: &str = "\
TinyHand runs only while it's on: bind keys to these commands.

  tinyhand toggle      pointer on/off      (suggested: Ctrl+Alt+H)
  tinyhand clear       clear the drawing   (suggested: Ctrl+Alt+X)
  tinyhand real on|off click for real while a key is held (or `tinyhand real` to toggle)
";

/// Bindings for the current session (from `XDG_CURRENT_DESKTOP` and compositor sockets).
pub fn detect() -> Bindings {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    detect_with(&env, &Path::new(&env("HOME")).join(".config/hypr/hyprland.lua"))
}

/// The full text `tinyhand setup` prints: the commands, then the detected desktop's lines.
pub fn snippet() -> String {
    let b = detect();
    let mut out = format!("{INTRO}\n{}: {}\n", b.desktop, b.how);
    if !b.code.is_empty() {
        out += &format!("\n{}\n", b.code);
    }
    if let Some(note) = b.note {
        out += &format!("\n{note}\n");
    }
    out += &format!("\nSettings: {} (key = value; see the README).\n", crate::config::Config::path().display());
    out
}

/// `env` reads an environment variable ("" if unset); `hypr_lua` is where a Lua
/// Hyprland config would be.
fn detect_with(env: &dyn Fn(&str) -> String, hypr_lua: &Path) -> Bindings {
    let desktop = env("XDG_CURRENT_DESKTOP").to_lowercase();
    let b = |desktop, how: &str, code: &str| Bindings { desktop, how: how.into(), code: code.into(), note: None };
    if !env("HYPRLAND_INSTANCE_SIGNATURE").is_empty() || desktop.contains("hyprland") {
        if hypr_lua.exists() {
            b("Hyprland (Lua config)", "add to ~/.config/hypr/hyprland.lua:", HYPRLAND_LUA)
        } else {
            b("Hyprland", "add to ~/.config/hypr/hyprland.conf:", HYPRLAND_CONF)
        }
    } else if !env("SWAYSOCK").is_empty() || desktop.contains("sway") {
        b("Sway", "add to ~/.config/sway/config:", SWAY)
    } else if !env("NIRI_SOCKET").is_empty() || desktop.contains("niri") {
        b("niri", "add inside `binds { }` in ~/.config/niri/config.kdl:", NIRI)
    } else if desktop.contains("river") {
        b("river", "add to your init (usually ~/.config/river/init):", RIVER)
    } else if desktop.contains("kde") {
        b("KDE Plasma", KDE, "")
    } else if desktop.contains("gnome") {
        Bindings {
            desktop: "GNOME",
            how: "TinyHand runs through XWayland here (GNOME has no layer-shell). Run in a terminal:".into(),
            code: gnome(),
            note: Some("This replaces any existing list of custom shortcuts; if you have others, merge them by hand."),
        }
    } else {
        b("i3 / other X11 window managers", "add to your config, e.g. ~/.config/i3/config:", I3)
    }
}

const HYPRLAND_LUA: &str = r#"hl.bind("CTRL + ALT + H", hl.dsp.exec_cmd("tinyhand toggle"), { description = "TinyHand pointer" })
hl.bind("CTRL + ALT + X", hl.dsp.exec_cmd("tinyhand clear"), { description = "TinyHand: clear drawing" })
-- Hold Left Alt to click for real (the key still reaches apps):
hl.bind("Alt_L", hl.dsp.exec_cmd("tinyhand real on"), { non_consuming = true })
hl.bind("ALT + Alt_L", hl.dsp.exec_cmd("tinyhand real off"), { non_consuming = true, release = true })
hl.layer_rule({ match = { namespace = "tinyhand" }, no_anim = true })"#;

const HYPRLAND_CONF: &str = "bind = CTRL ALT, H, exec, tinyhand toggle
bind = CTRL ALT, X, exec, tinyhand clear
# Hold Left Alt to click for real (n = the key still reaches apps, r = on release):
bindn = , Alt_L, exec, tinyhand real on
bindrn = ALT, Alt_L, exec, tinyhand real off
layerrule = noanim, tinyhand";

const SWAY: &str = "bindsym Ctrl+Mod1+h exec tinyhand toggle
bindsym Ctrl+Mod1+x exec tinyhand clear
bindsym Ctrl+Mod1+r exec tinyhand real";

const NIRI: &str = r#"Ctrl+Alt+H { spawn "tinyhand" "toggle"; }
Ctrl+Alt+X { spawn "tinyhand" "clear"; }
Ctrl+Alt+R { spawn "tinyhand" "real"; }"#;

const RIVER: &str = "riverctl map normal Control+Alt H spawn 'tinyhand toggle'
riverctl map normal Control+Alt X spawn 'tinyhand clear'
riverctl map normal Control+Alt R spawn 'tinyhand real'";

const KDE: &str = "open System Settings → Keyboard → Shortcuts → Add New → Command or Script, \
then add `tinyhand toggle` (Ctrl+Alt+H), `tinyhand clear` (Ctrl+Alt+X) and `tinyhand real` (Ctrl+Alt+R).";

const I3: &str = "bindsym Ctrl+Mod1+h exec tinyhand toggle
bindsym Ctrl+Mod1+x exec tinyhand clear
bindsym Ctrl+Mod1+r exec tinyhand real";

fn gnome() -> String {
    let base = "org.gnome.settings-daemon.plugins.media-keys";
    let path = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings";
    let mut out = format!("gsettings set {base} custom-keybindings \"['{path}/tinyhand0/', '{path}/tinyhand1/']\"");
    let keys =
        [("TinyHand", "tinyhand toggle", "<Control><Alt>h"), ("TinyHand clear", "tinyhand clear", "<Control><Alt>x")];
    for (i, (name, cmd, key)) in keys.iter().enumerate() {
        let schema = format!("{base}.custom-keybinding:{path}/tinyhand{i}/");
        out += &format!("\ngsettings set {schema} name '{name}'");
        out += &format!("\ngsettings set {schema} command '{cmd}'");
        out += &format!("\ngsettings set {schema} binding '{key}'");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn detect_in(vars: &[(&str, &str)]) -> Bindings {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        detect_with(&|k| vars.get(k).cloned().unwrap_or_default(), Path::new("/nonexistent/hyprland.lua"))
    }

    #[test]
    fn picks_the_desktop() {
        assert_eq!(detect_in(&[("HYPRLAND_INSTANCE_SIGNATURE", "x")]).code, HYPRLAND_CONF);
        assert_eq!(detect_in(&[("SWAYSOCK", "/run/sway")]).desktop, "Sway");
        assert_eq!(detect_in(&[("XDG_CURRENT_DESKTOP", "niri")]).desktop, "niri");
        assert_eq!(detect_in(&[("XDG_CURRENT_DESKTOP", "KDE")]).code, "");
        assert_eq!(detect_in(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")]).desktop, "GNOME");
        assert_eq!(detect_in(&[]).code, I3);
    }

    #[test]
    fn gnome_sets_both_shortcuts() {
        let g = gnome();
        assert_eq!(g.lines().count(), 7);
        assert!(g.contains("tinyhand1/ binding '<Control><Alt>x'"));
    }
}
