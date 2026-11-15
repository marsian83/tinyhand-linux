//! TinyHand for Linux: a tiny hand on a stick that points at things on your screen.
//! Unofficial port of TinyHand for macOS by Kamran Ahmed (MIT).
//!
//! The library holds everything the `tinyhand` pointer and the settings window share.
//! The pointer itself: `session` (behaviour, the same everywhere) on top of `pose`,
//! `raster`, `ink` and `toolbar` (pure drawing), driven by one of the display backends
//! `wl` or `x11` (chosen in `backend`).

pub mod backend;
pub mod bench;
pub mod config;
pub mod hand;
pub mod handmaker;
pub mod ink;
pub mod ipc;
pub mod pose;
pub mod raster;
pub mod session;
pub mod setup;
pub mod sound;
pub mod stats;
pub mod theme;
pub mod toolbar;
pub mod wl;
pub mod x11;
