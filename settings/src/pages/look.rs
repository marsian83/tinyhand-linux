//! Look: how big the hand is, its shadow, and scrolling (Appearance on macOS).

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::store::Store;
use crate::thumbs::Thumbs;
use crate::widgets::switch_row;
use tinyhand::config::{self, Config};

pub fn page(store: &Rc<Store>, thumbs: &Rc<Thumbs>) -> adw::PreferencesPage {
    let page =
        adw::PreferencesPage::builder().name("look").title("Look").icon_name("applications-graphics-symbolic").build();
    page.add(&pointer_group(store, thumbs));
    page.add(&scroll_group(store));
    page
}

fn pointer_group(store: &Rc<Store>, thumbs: &Rc<Thumbs>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Pointer").build();
    group.add(&size_row(store, thumbs));
    let shadow = switch_row(store, "Shadow under the pointer", "", |c| c.shadow, |c, on| c.shadow = on);
    group.add(&shadow);
    group
}

/// Size slider, saved once it rests and applied live to a running pointer.
fn size_row(store: &Rc<Store>, thumbs: &Rc<Thumbs>) -> adw::ActionRow {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.08, 0.20, 0.005);
    scale.set_value(store.get().size.into());
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.set_width_request(220);
    scale.set_valign(gtk::Align::Center);
    scale.update_property(&[gtk::accessible::Property::Label("Size")]);
    scale.connect_value_changed(glib::clone!(
        #[strong]
        store,
        move |s| {
            let size = s.value() as f32;
            store.update_soon(move |c| c.size = size);
        }
    ));
    store.connect_changed(glib::clone!(
        #[weak]
        scale,
        move |c| {
            if (scale.value() - f64::from(c.size)).abs() > 1e-3 {
                scale.set_value(c.size.into());
            }
        }
    ));
    // The current hand, small and big, at the ends of the slider.
    let small = gtk::Image::builder().pixel_size(18).build();
    let big = gtk::Image::builder().pixel_size(30).build();
    let show_hand = glib::clone!(
        #[weak]
        small,
        #[weak]
        big,
        #[strong]
        thumbs,
        move |c: &Config| {
            let id = icon_hand(c);
            thumbs.show(id, &small);
            thumbs.show(id, &big);
        }
    );
    show_hand(&store.get());
    store.connect_changed(show_hand);
    let row = adw::ActionRow::builder().title("Size").subtitle("Hand height, compared with the screen").build();
    row.add_suffix(&small);
    row.add_suffix(&scale);
    row.add_suffix(&big);
    row
}

/// The hand to show next to the size slider: the current one, or lavender for the laser
/// or a hand given as a path.
fn icon_hand(c: &Config) -> &str {
    if c.is_laser() || config::hand_title(&c.hand).is_none() {
        "lavender"
    } else {
        &c.hand
    }
}

fn scroll_group(store: &Rc<Store>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Scrolling").build();
    let row = switch_row(
        store,
        "Scroll the window underneath",
        "The hand hides for a moment while you scroll.",
        |c| c.scroll_through,
        |c, on| c.scroll_through = on,
    );
    group.add(&row);
    group
}
