//! Pick a display backend and run the pointer on it.
//!
//! Wayland with wlr-layer-shell first (KDE, Hyprland, Sway, niri, …); X11 for X sessions
//! and for GNOME, which has no layer-shell, through XWayland.
//! `TINYHAND_BACKEND=x11|wayland` forces one.

use std::fmt;

use crate::{config::Config, ipc, wl, x11};

/// Why a backend couldn't run the pointer.
#[derive(Debug)]
pub enum Error {
    /// This display server can't host the pointer; another backend may.
    Unsupported(String),
    /// Something went wrong while running.
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Unsupported(e) | Error::Failed(e) => f.write_str(e),
        }
    }
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Error::Failed(e)
    }
}

/// Run until the pointer is turned off. `server` is the control socket, already bound.
pub fn run(cfg: Config, server: &ipc::Server) -> Result<(), String> {
    let forced = std::env::var("TINYHAND_BACKEND").unwrap_or_default();
    let has = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if forced != "x11" && has("WAYLAND_DISPLAY") {
        match wl::run(cfg.clone(), server) {
            Err(Error::Unsupported(why)) if forced != "wayland" && has("DISPLAY") => {
                if cfg.stats {
                    eprintln!("tinyhand: {why}; using X11");
                }
            }
            other => return other.map_err(|e| e.to_string()),
        }
    }
    if has("DISPLAY") {
        return x11::run(cfg, server).map_err(|e| e.to_string());
    }
    Err("no Wayland (with wlr-layer-shell) or X11 display found".into())
}
