//! About: version, credits, links and "Reset all settings".

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use tinyhand::config::Config;

use crate::store::Store;
use crate::widgets::{open_uri, toast};

const ICON: &[u8] = include_bytes!("../../../packaging/icons/tinyhand.png");

/// (title, shown address, link).
const LINKS: &[(&str, &str, &str)] = &[
    ("Linux port", "github.com/marsian83/tinyhand-linux", "https://github.com/marsian83/tinyhand-linux"),
    ("Report a problem", "Issues on GitHub", "https://github.com/marsian83/tinyhand-linux/issues"),
    ("Original app", "tinyhand.app", "https://tinyhand.app"),
    ("License", "MIT", "https://github.com/marsian83/tinyhand-linux/blob/main/LICENSE"),
];

pub fn page(store: &Rc<Store>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().name("about").title("About").icon_name("help-about-symbolic").build();
    page.add(&header());
    let links = adw::PreferencesGroup::new();
    for (title, shown, uri) in LINKS {
        links.add(&link_row(title, shown, uri));
    }
    page.add(&links);
    page.add(&reset_group(store));
    page
}

fn header() -> adw::PreferencesGroup {
    let content =
        gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).halign(gtk::Align::Center).build();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON)) {
        let icon = gtk::Image::builder().paintable(&texture).pixel_size(96).margin_bottom(6).build();
        icon.set_accessible_role(gtk::AccessibleRole::Presentation);
        content.append(&icon);
    }
    let label = |text: &str, classes: &[&str]| {
        gtk::Label::builder().label(text).wrap(true).justify(gtk::Justification::Center).css_classes(classes).build()
    };
    content.append(&label("TinyHand", &["title-1"]));
    content.append(&label(&format!("Version {}", env!("CARGO_PKG_VERSION")), &["dim-label"]));
    content.append(&label("Unofficial Linux port of TinyHand by Kamran Ahmed", &["dim-label"]));
    let group = adw::PreferencesGroup::new();
    group.add(&content);
    group
}

fn link_row(title: &str, shown: &str, uri: &'static str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle(shown).activatable(true).build();
    row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
    row.connect_activated(move |row| open_uri(row, uri));
    row
}

fn reset_group(store: &Rc<Store>) -> adw::PreferencesGroup {
    let button = gtk::Button::builder().label("Reset All Settings…").halign(gtk::Align::Center).build();
    button.add_css_class("pill");
    button.connect_clicked(glib::clone!(
        #[strong]
        store,
        move |button| confirm_reset(button, &store)
    ));
    let group = adw::PreferencesGroup::new();
    group.add(&button);
    group
}

fn confirm_reset(button: &gtk::Button, store: &Rc<Store>) {
    let dialog = adw::MessageDialog::builder()
        .heading("Reset all settings?")
        .body("Size, shadow, drawing, sound and the toolbar go back to how they were at first. Your own hands stay.")
        .modal(true)
        .build();
    dialog.set_transient_for(button.root().and_downcast_ref::<gtk::Window>());
    dialog.add_responses(&[("cancel", "_Cancel"), ("reset", "_Reset")]);
    dialog.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    dialog.connect_response(
        Some("reset"),
        glib::clone!(
            #[strong]
            store,
            #[weak]
            button,
            move |_, _| {
                store.update(|c| *c = Config::default());
                toast(&button, "Settings reset");
            }
        ),
    );
    dialog.present();
}
