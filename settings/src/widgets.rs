//! Small widgets and helpers shared by the pages.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::store::Store;
use crate::thumbs::Thumbs;
use tinyhand::config::Config;

/// A switch row that edits one on/off setting and follows changes made elsewhere.
pub fn switch_row(
    store: &Rc<Store>,
    title: &str,
    subtitle: &str,
    get: fn(&Config) -> bool,
    set: fn(&mut Config, bool),
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).active(get(&store.get())).build();
    if !subtitle.is_empty() {
        row.set_subtitle(subtitle);
    }
    row.connect_active_notify(glib::clone!(
        #[strong]
        store,
        move |row| {
            let on = row.is_active();
            store.update(|c| set(c, on));
        }
    ));
    store.connect_changed(glib::clone!(
        #[weak]
        row,
        move |c| row.set_active(get(c))
    ));
    row
}

/// A hand's thumbnail (or the laser dot), `size` logical pixels square.
pub fn pointer_icon(id: &str, size: i32, thumbs: &Rc<Thumbs>) -> gtk::Widget {
    if id == "laser" {
        let dot = gtk::Box::builder().halign(gtk::Align::Center).valign(gtk::Align::Center).build();
        dot.add_css_class("laser-dot");
        let frame = gtk::Box::builder().width_request(size).height_request(size).build();
        dot.set_hexpand(true);
        frame.append(&dot);
        return frame.upcast();
    }
    // An Image (not a Picture) so the thumbnail is drawn at exactly `size`, whatever its
    // pixel size.
    let image = gtk::Image::builder().pixel_size(size).build();
    thumbs.show(id, &image);
    image.upcast()
}

/// Show a short message at the bottom of the settings window.
pub fn toast(widget: &impl IsA<gtk::Widget>, text: &str) {
    if let Some(window) = widget.root().and_downcast::<adw::PreferencesWindow>() {
        window.add_toast(adw::Toast::builder().title(glib::markup_escape_text(text)).timeout(4).build());
    }
}

/// Open a web page in the user's browser.
pub fn open_uri(widget: &impl IsA<gtk::Widget>, uri: &str) {
    let window = widget.root().and_downcast::<gtk::Window>();
    let shown = widget.clone().upcast::<gtk::Widget>();
    gtk::UriLauncher::new(uri).launch(window.as_ref(), gio::Cancellable::NONE, move |result| {
        if let Err(e) = result {
            if !e.matches(gtk::DialogError::Dismissed) {
                toast(&shown, &format!("Couldn't open the browser: {e}"));
            }
        }
    });
}

/// A button with an icon and a label.
pub fn icon_button(icon: &str, label: &str) -> gtk::Button {
    let content = adw::ButtonContent::builder().icon_name(icon).label(label).use_underline(true).build();
    gtk::Button::builder().child(&content).build()
}

/// Copy `text` to the clipboard and say so.
pub fn copy(widget: &impl IsA<gtk::Widget>, text: &str, what: &str) {
    widget.clipboard().set_text(text);
    toast(widget, &format!("{what} copied"));
}

/// Text in a monospace box that can be selected and scrolls sideways if it's wide.
pub fn code_block(text: &str) -> gtk::ScrolledWindow {
    let label = gtk::Label::builder().label(text).selectable(true).xalign(0.0).yalign(0.0).wrap(false).build();
    label.add_css_class("monospace");
    label.add_css_class("code");
    gtk::ScrolledWindow::builder()
        .child(&label)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .css_classes(["card", "code-card"])
        .build()
}

/// Capitalise the first letter ("add to …" → "Add to …").
pub fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sentence_capitalises() {
        assert_eq!(super::sentence("add to x:"), "Add to x:");
        assert_eq!(super::sentence(""), "");
    }
}
