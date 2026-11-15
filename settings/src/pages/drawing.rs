//! Drawing: ink colour, how long lines last, and the toolbar.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tinyhand::config::{Config, INK_COLOURS};

use crate::store::Store;

/// Fade choices, as in the macOS app: (label, seconds; 0 keeps lines until cleared).
const FADES: &[(&str, f32)] =
    &[("After 3 seconds", 3.0), ("After 6 seconds", 6.0), ("After 10 seconds", 10.0), ("Keep until cleared", 0.0)];

pub fn page(store: &Rc<Store>) -> adw::PreferencesPage {
    let page =
        adw::PreferencesPage::builder().name("drawing").title("Drawing").icon_name("document-edit-symbolic").build();
    let ink = adw::PreferencesGroup::builder()
        .title("Ink")
        .description("Hold the mouse button and drag to draw on the screen.")
        .build();
    ink.add(&ink_row(store));
    ink.add(&fade_row(store));
    page.add(&ink);
    let toolbar = adw::PreferencesGroup::builder().title("Toolbar").build();
    toolbar.add(&toolbar_row(store));
    page.add(&toolbar);
    page
}

fn ink_subtitle(ink: &str) -> String {
    match ink {
        "match" => "Matches each pointer.".into(),
        colour => format!("All pointers draw in {colour}."),
    }
}

/// "Match the pointer" plus the eight fixed colours, as round swatches.
fn ink_row(store: &Rc<Store>) -> adw::ActionRow {
    let current = store.get().ink;
    let row = adw::ActionRow::builder().title("Ink colour").subtitle(ink_subtitle(&current)).build();
    let swatches = gtk::Box::builder().spacing(6).valign(gtk::Align::Center).build();
    let mut first: Option<gtk::ToggleButton> = None;
    let choices = std::iter::once(("match", "Match the pointer")).chain(INK_COLOURS.iter().map(|(n, _)| (*n, *n)));
    for (name, title) in choices {
        let swatch = gtk::ToggleButton::builder()
            .tooltip_text(crate::widgets::sentence(title))
            .css_classes(["swatch", name])
            .valign(gtk::Align::Center)
            .active(current == name)
            .build();
        swatch.update_property(&[gtk::accessible::Property::Label(title)]);
        swatch.set_group(first.as_ref());
        first.get_or_insert_with(|| swatch.clone());
        swatch.connect_toggled(glib::clone!(
            #[strong]
            store,
            move |b| {
                if b.is_active() {
                    store.update(|c| c.ink = name.into());
                }
            }
        ));
        store.connect_changed(glib::clone!(
            #[weak]
            swatch,
            move |c| swatch.set_active(c.ink == name)
        ));
        swatches.append(&swatch);
    }
    store.connect_changed(glib::clone!(
        #[weak]
        row,
        move |c| row.set_subtitle(&ink_subtitle(&c.ink))
    ));
    row.add_suffix(&swatches);
    row
}

/// The fade choices, plus the configured time if it isn't one of them.
fn fade_choices(c: &Config) -> Vec<(String, f32)> {
    let mut all: Vec<(String, f32)> = FADES.iter().map(|(l, s)| (l.to_string(), *s)).collect();
    if !FADES.iter().any(|(_, s)| *s == c.fade_seconds) {
        all.insert(0, (format!("After {} seconds", c.fade_seconds), c.fade_seconds));
    }
    all
}

fn fade_row(store: &Rc<Store>) -> adw::ComboRow {
    let choices = fade_choices(&store.get());
    let labels: Vec<&str> = choices.iter().map(|(l, _)| l.as_str()).collect();
    let row = adw::ComboRow::builder()
        .title("Lines fade")
        .subtitle("Clear them any time with tinyhand clear (Ctrl+Alt+X).")
        .model(&gtk::StringList::new(&labels))
        .build();
    let index_of = |choices: &[(String, f32)], fade: f32| choices.iter().position(|(_, s)| *s == fade);
    row.set_selected(index_of(&choices, store.get().fade_seconds).unwrap_or(0) as u32);
    store.connect_changed(glib::clone!(
        #[weak]
        row,
        #[strong]
        choices,
        move |c| {
            if let Some(i) = index_of(&choices, c.fade_seconds) {
                row.set_selected(i as u32);
            }
        }
    ));
    row.connect_selected_notify(glib::clone!(
        #[strong]
        store,
        move |row| {
            if let Some((_, seconds)) = choices.get(row.selected() as usize) {
                let seconds = *seconds;
                store.update(|c| c.fade_seconds = seconds);
            }
        }
    ));
    row
}

fn toolbar_row(store: &Rc<Store>) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title("Show the toolbar")
        .subtitle("Your pinned pointers, sound and exit, at the bottom of the screen.")
        .active(!store.get().toolbar.is_empty())
        .build();
    row.connect_active_notify(glib::clone!(
        #[strong]
        store,
        move |row| {
            let pins = if row.is_active() { store.last_pins() } else { Vec::new() };
            store.update(|c| {
                // Turning it on keeps a toolbar that's already on as it is.
                if pins.is_empty() || c.toolbar.is_empty() {
                    c.toolbar = pins;
                }
            });
        }
    ));
    store.connect_changed(glib::clone!(
        #[weak]
        row,
        move |c| row.set_active(!c.toolbar.is_empty())
    ));
    row
}
