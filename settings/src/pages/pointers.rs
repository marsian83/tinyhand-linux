//! Pointers: the toolbar's pins, your own hands, and every pointer to choose from.
//!
//! A widget subclass, so its signal handlers can reach the page's state through weak
//! references to the page itself (no reference cycles through `Rc`).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib};
use tinyhand::config::{self, Config};
use tinyhand::handmaker::{self, CustomHand};

use crate::handmaker_flow::MakeHandPage;
use crate::pins;
use crate::store::Store;
use crate::thumbs::Thumbs;
use crate::widgets::{pointer_icon, toast};

/// A pointer in the grid.
struct Tile {
    id: String,
    child: gtk::FlowBoxChild,
    pin: gtk::ToggleButton,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PointersPage {
        pub(super) store: RefCell<Rc<Store>>,
        pub(super) thumbs: RefCell<Rc<Thumbs>>,
        pub(super) strip: gtk::Box,
        pub(super) strip_note: gtk::Label,
        pub(super) reset: gtk::Button,
        pub(super) yours: adw::PreferencesGroup,
        pub(super) your_rows: RefCell<Vec<gtk::Widget>>,
        pub(super) grid: gtk::FlowBox,
        pub(super) tiles: RefCell<Vec<Tile>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PointersPage {
        const NAME: &'static str = "TinyHandPointersPage";
        type Type = super::PointersPage;
        type ParentType = adw::PreferencesPage;
    }

    impl ObjectImpl for PointersPage {}
    impl WidgetImpl for PointersPage {}
    impl PreferencesPageImpl for PointersPage {}
}

glib::wrapper! {
    pub struct PointersPage(ObjectSubclass<imp::PointersPage>)
        @extends adw::PreferencesPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PointersPage {
    pub fn new(store: &Rc<Store>, thumbs: &Rc<Thumbs>) -> PointersPage {
        let page: PointersPage = glib::Object::builder()
            .property("name", "pointers")
            .property("title", "Pointers")
            .property("icon-name", "input-mouse-symbolic")
            .build();
        let imp = page.imp();
        imp.store.replace(Rc::clone(store));
        imp.thumbs.replace(Rc::clone(thumbs));
        page.add(&page.toolbar_group());
        page.add(&page.yours_group());
        page.add(&page.grid_group());
        page.hands_changed();
        store.connect_changed(glib::clone!(
            #[weak]
            page,
            move |c| page.sync(c)
        ));
        page
    }

    fn store(&self) -> Rc<Store> {
        self.imp().store.borrow().clone()
    }

    fn thumbs(&self) -> Rc<Thumbs> {
        self.imp().thumbs.borrow().clone()
    }

    // ---------------------------------------------------------------- building

    fn toolbar_group(&self) -> adw::PreferencesGroup {
        let imp = self.imp();
        let reset = &imp.reset;
        reset.set_label("Reset");
        reset.set_valign(gtk::Align::Center);
        reset.set_tooltip_text(Some("Pin the pointers TinyHand starts with"));
        reset.add_css_class("flat");
        reset.connect_clicked(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.store().update(|c| c.toolbar = Config::default().toolbar)
        ));
        imp.strip.set_spacing(6);
        imp.strip.set_homogeneous(true);
        imp.strip.add_css_class("card");
        imp.strip.add_css_class("pin-strip");
        imp.strip_note.set_xalign(0.0);
        imp.strip_note.set_wrap(true);
        imp.strip_note.set_margin_top(8);
        imp.strip_note.add_css_class("dim-label");
        imp.strip_note.add_css_class("caption");
        let group = adw::PreferencesGroup::builder()
            .title("In your toolbar")
            .description("Switch pointers from the toolbar while TinyHand is on.")
            .header_suffix(reset)
            .build();
        group.add(&imp.strip);
        group.add(&imp.strip_note);
        group
    }

    fn yours_group(&self) -> adw::PreferencesGroup {
        let group = &self.imp().yours;
        group.set_title("Your hands");
        group.clone()
    }

    fn grid_group(&self) -> adw::PreferencesGroup {
        let grid = &self.imp().grid;
        grid.set_selection_mode(gtk::SelectionMode::Single);
        grid.set_activate_on_single_click(true);
        grid.set_homogeneous(true);
        grid.set_min_children_per_line(3);
        grid.set_max_children_per_line(12);
        grid.set_column_spacing(4);
        grid.set_row_spacing(4);
        grid.add_css_class("card");
        grid.add_css_class("hand-grid");
        grid.connect_selected_children_changed(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.selection_changed()
        ));
        let group = adw::PreferencesGroup::builder()
            .title("All pointers")
            .description("Click a pointer to use it. Pin it to add it to the toolbar.")
            .build();
        group.add(grid);
        group
    }

    fn tile(&self, id: &str, title: &str) -> Tile {
        let pin = gtk::ToggleButton::builder()
            .icon_name("view-pin-symbolic")
            .tooltip_text("Pin to the toolbar")
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .css_classes(["flat", "circular", "pin"])
            .build();
        pin.update_property(&[gtk::accessible::Property::Label(&format!("Pin {title} to the toolbar"))]);
        pin.connect_toggled(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[to_owned]
            id,
            move |b| page.pin_toggled(&id, b.is_active())
        ));
        let overlay =
            gtk::Overlay::builder().child(&pointer_icon(id, 56, &self.thumbs())).halign(gtk::Align::Center).build();
        overlay.add_overlay(&pin);
        let label = gtk::Label::builder()
            .label(title)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(10)
            .css_classes(["title"])
            .build();
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
        content.add_css_class("hand-tile");
        content.append(&overlay);
        content.append(&label);
        let child = gtk::FlowBoxChild::builder().child(&content).build();
        child.update_property(&[gtk::accessible::Property::Label(title)]);
        Tile { id: id.to_string(), child, pin }
    }

    /// One of the toolbar's slots: click to use it, drag onto another slot to reorder.
    fn strip_slot(&self, id: &str, index: usize, current: bool) -> gtk::Button {
        let title = config::hand_title(id).unwrap_or_else(|| id.to_string());
        let slot = gtk::Button::builder()
            .child(&pointer_icon(id, 36, &self.thumbs()))
            .tooltip_text(&title)
            .hexpand(true)
            .css_classes(["pin-slot"])
            .build();
        if current {
            slot.add_css_class("current");
        }
        slot.connect_clicked(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[to_owned]
            id,
            move |_| page.select(&id)
        ));
        let drag = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
        let value = id.to_value();
        drag.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&value)));
        drag.connect_drag_begin(glib::clone!(
            #[weak]
            slot,
            move |source, _| source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&slot))), 0, 0)
        ));
        slot.add_controller(drag);
        let drop = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        drop.connect_drop(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                let Ok(dragged) = value.get::<String>() else { return false };
                page.store().update(|c| pins::move_to(&mut c.toolbar, &dragged, index));
                true
            }
        ));
        slot.add_controller(drop);
        slot
    }

    fn empty_slot() -> gtk::Widget {
        let slot = gtk::Box::builder().hexpand(true).css_classes(["pin-slot", "empty"]).build();
        slot.upcast()
    }

    fn custom_row(&self, hand: &CustomHand) -> adw::EntryRow {
        let row = adw::EntryRow::builder().title("Name").text(&hand.name).show_apply_button(true).build();
        if let Some(text) = row.delegate().and_downcast::<gtk::Text>() {
            text.set_max_length(16);
        }
        row.add_prefix(&pointer_icon(&hand.id, 32, &self.thumbs()));
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text(format!("Delete {}", hand.name))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let id = hand.id.clone();
        remove.connect_clicked(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[strong]
            id,
            move |_| page.confirm_remove(&id)
        ));
        row.add_suffix(&remove);
        row.connect_apply(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |row| page.rename(&id, row)
        ));
        row
    }

    fn make_row(&self) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title("Make your own hand…")
            .subtitle("Turn any idea into a hand with ChatGPT or another image tool.")
            .activatable(true)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("list-add-symbolic"));
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        row.connect_activated(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| {
                let _ = page.make_hand();
            }
        ));
        row
    }

    // ---------------------------------------------------------------- refreshing

    /// The list of hands changed (one was made, renamed or removed): rebuild the grid and
    /// "Your hands", then show the settings.
    fn hands_changed(&self) {
        self.rebuild_grid();
        self.rebuild_yours();
        self.sync(&self.store().get());
    }

    fn rebuild_grid(&self) {
        let imp = self.imp();
        imp.grid.remove_all();
        let tiles: Vec<Tile> = config::all_hands().iter().map(|(id, title)| self.tile(id, title)).collect();
        for t in &tiles {
            imp.grid.append(&t.child);
        }
        imp.tiles.replace(tiles);
    }

    fn rebuild_yours(&self) {
        let imp = self.imp();
        for row in imp.your_rows.take() {
            imp.yours.remove(&row);
        }
        let mut rows: Vec<gtk::Widget> = handmaker::list().iter().map(|h| self.custom_row(h).upcast()).collect();
        rows.push(self.make_row().upcast());
        for row in &rows {
            imp.yours.add(row);
        }
        imp.your_rows.replace(rows);
    }

    /// Show the settings: the current pointer, the pins, and the toolbar strip.
    fn sync(&self, c: &Config) {
        let imp = self.imp();
        let mut current = None;
        for t in imp.tiles.borrow().iter() {
            t.pin.set_active(c.toolbar.contains(&t.id));
            t.pin.set_visible(!c.toolbar.is_empty());
            if t.id == c.hand {
                current = Some(t.child.clone());
            }
        }
        match current {
            Some(child) => imp.grid.select_child(&child),
            None => imp.grid.unselect_all(),
        }
        self.rebuild_strip(c);
    }

    fn rebuild_strip(&self, c: &Config) {
        let imp = self.imp();
        while let Some(child) = imp.strip.first_child() {
            imp.strip.remove(&child);
        }
        imp.strip.set_visible(!c.toolbar.is_empty());
        imp.reset.set_visible(!c.toolbar.is_empty());
        if c.toolbar.is_empty() {
            imp.strip_note.set_label("The toolbar is off. Turn it on under Drawing.");
            return;
        }
        for (i, id) in c.toolbar.iter().enumerate() {
            imp.strip.append(&self.strip_slot(id, i, *id == c.hand));
        }
        for _ in c.toolbar.len()..pins::MAX {
            imp.strip.append(&Self::empty_slot());
        }
        imp.strip_note.set_label(if c.toolbar.len() >= pins::MAX {
            "The toolbar is full. Unpin a pointer to add another. Drag to reorder."
        } else {
            "Pin pointers below to add them. Drag to reorder."
        });
    }

    // ---------------------------------------------------------------- actions

    fn select(&self, id: &str) {
        if self.store().get().hand != id {
            self.store().update(|c| c.hand = id.to_string());
        }
    }

    fn selection_changed(&self) {
        let Some(child) = self.imp().grid.selected_children().into_iter().next() else { return };
        let id = self.imp().tiles.borrow().iter().find(|t| t.child == child).map(|t| t.id.clone());
        if let Some(id) = id {
            self.select(&id);
        }
    }

    fn pin_toggled(&self, id: &str, pin: bool) {
        let c = self.store().get();
        if c.toolbar.iter().any(|p| p == id) == pin {
            return; // already so (this is `sync` setting the button)
        }
        let mut pinned = c.toolbar.clone();
        match pins::toggle(&mut pinned, id) {
            Ok(_) => self.store().update(|c| c.toolbar = pinned),
            Err(refused) => {
                toast(self, &refused.message(&config::hand_title(id).unwrap_or_default()));
                self.sync(&c); // puts the button back
            }
        }
    }

    /// Open "Make your own hand" on top of the settings.
    pub fn make_hand(&self) -> Option<MakeHandPage> {
        let window = self.root().and_downcast::<adw::PreferencesWindow>()?;
        let flow = MakeHandPage::new(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |hand| page.hand_added(&hand)
        ));
        window.push_subpage(&flow);
        Some(flow)
    }

    fn hand_added(&self, hand: &CustomHand) {
        let mut pinned = false;
        self.store().update(|c| {
            c.hand = hand.id.clone();
            pinned = !c.toolbar.is_empty() && pins::add_new(&mut c.toolbar, &hand.id);
        });
        self.hands_changed();
        let message =
            if pinned { format!("{} is in your toolbar.", hand.name) } else { format!("{} is ready.", hand.name) };
        toast(self, &message);
    }

    fn rename(&self, id: &str, row: &adw::EntryRow) {
        if let Err(e) = handmaker::rename(id, &row.text()) {
            toast(self, &e);
            return;
        }
        // Show the name as saved (trimmed, at most 16 characters).
        if let Some(hand) = handmaker::get(id).filter(|h| h.name != row.text()) {
            row.set_text(&hand.name);
        }
        // Only the grid shows names; rebuilding the rows would take the focus away.
        self.rebuild_grid();
        self.sync(&self.store().get());
    }

    fn confirm_remove(&self, id: &str) {
        let Some(hand) = handmaker::get(id) else { return };
        let dialog = adw::MessageDialog::builder()
            .heading(format!("Delete {}?", hand.name))
            .body("Its image is deleted too.")
            .modal(true)
            .build();
        dialog.set_transient_for(self.root().and_downcast_ref::<gtk::Window>());
        dialog.add_responses(&[("cancel", "_Cancel"), ("delete", "_Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        dialog.connect_response(
            Some("delete"),
            glib::clone!(
                #[weak(rename_to = page)]
                self,
                #[to_owned]
                id,
                move |_, _| page.remove(&id)
            ),
        );
        dialog.present();
    }

    fn remove(&self, id: &str) {
        if let Err(e) = handmaker::remove(id) {
            toast(self, &e);
            return;
        }
        let fallback = Config::default().toolbar;
        self.store().update(|c| {
            if c.hand == id {
                c.hand = Config::default().hand;
            }
            pins::forget(&mut c.toolbar, id, &fallback);
        });
        self.thumbs().forget(id);
        self.hands_changed();
    }
}
