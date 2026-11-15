//! TinyHand Settings: a GTK 4 / libadwaita window for the `tinyhand` config file.
//!
//! The pointer itself has no window and no daemon; this is a separate program that edits
//! `~/.config/tinyhand/config` and tells a running pointer to re-read it.

mod handmaker_flow;
mod pages;
mod pins;
mod store;
mod thumbs;
mod widgets;
mod window;

use adw::prelude::*;
use gtk::{gdk, glib};

const APP_ID: &str = "app.tinyhand.Settings";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(window::present);
    app.run()
}

fn load_css() {
    let Some(display) = gdk::Display::default() else { return };
    let css = gtk::CssProvider::new();
    css.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    gtk::Window::set_default_icon_name("tinyhand");
}
