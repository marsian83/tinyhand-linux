//! Sound: the tap sound, picked from tiles that play when clicked (like the macOS app).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tinyhand::config::Config;
use tinyhand::sound::{Choice, TapSound};

use crate::store::Store;
use crate::widgets::{sentence, switch_row};

pub fn page(store: &Rc<Store>) -> adw::PreferencesPage {
    let page =
        adw::PreferencesPage::builder().name("sound").title("Sound").icon_name("audio-volume-high-symbolic").build();
    let taps = adw::PreferencesGroup::builder()
        .title("Tap sound")
        .description("Click a sound to hear it. It plays each time the hand taps.")
        .build();
    taps.add(&tiles(store));
    page.add(&taps);

    let laser = adw::PreferencesGroup::new();
    let row = switch_row(
        store,
        "Laser dot taps too",
        "Off by default, for quiet pointing.",
        |c| c.laser_sound,
        |c, on| c.laser_sound = on,
    );
    row.set_sensitive(store.get().sound);
    store.connect_changed(glib::clone!(
        #[weak]
        row,
        move |c| row.set_sensitive(c.sound)
    ));
    laser.add(&row);
    page.add(&laser);
    page
}

/// Which tile is lit: None for "Off".
fn lit(c: &Config) -> Option<&str> {
    c.sound.then_some(c.tap_sound.as_str())
}

/// "Off" and one tile per sound. Picking a sound turns the tap sound on and plays it.
fn tiles(store: &Rc<Store>) -> gtk::FlowBox {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(4)
        .max_children_per_line(7)
        .column_spacing(8)
        .row_spacing(8)
        .build();
    // Plays previews; dropped (closing the audio device) with the window.
    let preview = Rc::new(RefCell::new(TapSound::new(Choice::Thock)));
    let mut group: Option<gtk::ToggleButton> = None;
    let choices = std::iter::once(None).chain(Choice::ALL.into_iter().map(Some));
    for choice in choices {
        let tile = tile(choice);
        tile.set_group(group.as_ref());
        group.get_or_insert_with(|| tile.clone());
        let name = choice.map(Choice::name);
        tile.set_active(lit(&store.get()) == name);
        tile.connect_clicked(glib::clone!(
            #[strong]
            store,
            #[strong]
            preview,
            move |_| {
                if let Some(choice) = choice {
                    let mut p = preview.borrow_mut();
                    p.set_choice(choice);
                    p.play();
                }
                store.update(|c| match choice {
                    Some(choice) => {
                        c.sound = true;
                        c.tap_sound = choice.name().into();
                    }
                    None => c.sound = false,
                });
            }
        ));
        store.connect_changed(glib::clone!(
            #[weak]
            tile,
            move |c| tile.set_active(lit(c) == name)
        ));
        flow.append(&tile);
        if let Some(child) = flow.last_child() {
            child.set_focusable(false);
        }
    }
    flow
}

fn tile(choice: Option<Choice>) -> gtk::ToggleButton {
    let (icon, title) = match choice {
        None => ("audio-volume-muted-symbolic", "Off".to_string()),
        Some(c) => ("audio-volume-high-symbolic", sentence(c.name())),
    };
    let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
    content.append(&gtk::Image::builder().icon_name(icon).pixel_size(20).build());
    content.append(&gtk::Label::new(Some(&title)));
    gtk::ToggleButton::builder().child(&content).css_classes(["sound-tile"]).tooltip_text(&title).build()
}
