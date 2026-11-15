//! Shortcuts: Linux desktops don't let apps grab global keys, so this page shows the
//! commands and the lines to add to the desktop's own key bindings (`tinyhand setup`).

use std::path::PathBuf;

use adw::prelude::*;
use gtk::gio;
use tinyhand::setup;

use crate::widgets::{code_block, copy, icon_button, sentence, toast};

/// (what it does, command, suggested key as a GTK accelerator).
const COMMANDS: &[(&str, &str, &str)] = &[
    ("Turn the pointer on or off", "tinyhand toggle", "<Control><Alt>h"),
    ("Clear the drawing", "tinyhand clear", "<Control><Alt>x"),
    ("Click for real", "tinyhand real on / off, or tinyhand real to toggle", ""),
];

pub fn page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .name("shortcuts")
        .title("Shortcuts")
        .icon_name("input-keyboard-symbolic")
        .build();
    page.add(&commands_group());
    page.add(&bindings_group(&setup::detect()));
    page
}

fn commands_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Commands")
        .description("TinyHand runs only while it's on. Bind these commands to keys in your desktop's settings.")
        .build();
    for (title, command, key) in COMMANDS {
        let row = adw::ActionRow::builder().title(*title).subtitle(*command).subtitle_selectable(true).build();
        if !key.is_empty() {
            row.add_suffix(&gtk::ShortcutLabel::builder().accelerator(*key).valign(gtk::Align::Center).build());
        }
        if *command == "tinyhand toggle" {
            row.add_suffix(&try_button());
        }
        group.add(&row);
    }
    group
}

/// Runs `tinyhand toggle`: the pointer comes on, and pressing it again turns it off.
fn try_button() -> gtk::Button {
    let button = gtk::Button::builder()
        .label("Try it")
        .valign(gtk::Align::Center)
        .tooltip_text("Turn the pointer on (or off again) now")
        .build();
    button.connect_clicked(|b| {
        let tinyhand = tinyhand_path();
        let argv = [tinyhand.as_os_str(), "toggle".as_ref()];
        if let Err(e) = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
            toast(b, &format!("Couldn't run tinyhand: {e}"));
        }
    });
    button
}

/// `tinyhand` next to this program, else whatever is on PATH.
fn tinyhand_path() -> PathBuf {
    let beside = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("tinyhand")));
    beside.filter(|p| p.is_file()).unwrap_or_else(|| "tinyhand".into())
}

/// The lines for the desktop we're running on, with a copy button.
fn bindings_group(b: &setup::Bindings) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(b.desktop).description(sentence(&b.how)).build();
    if !b.code.is_empty() {
        let button = icon_button("edit-copy-symbolic", "_Copy");
        button.add_css_class("flat");
        let code = b.code.clone();
        button.connect_clicked(move |b| copy(b, &code, "Key bindings"));
        group.set_header_suffix(Some(&button));
        group.add(&code_block(&b.code));
    }
    if let Some(note) = b.note {
        let label = gtk::Label::builder()
            .label(note)
            .wrap(true)
            .xalign(0.0)
            .margin_top(12)
            .css_classes(["dim-label", "caption"])
            .build();
        group.add(&label);
    }
    group
}
