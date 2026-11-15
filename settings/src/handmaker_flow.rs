//! "Make your own hand", in three steps like the macOS app: describe it, get the image
//! from ChatGPT (or any image tool) with our prompt, then drop the PNG in. Checking and
//! composing the image runs on a worker thread; the result shows as a preview to name
//! and add, or as a friendly sentence saying what's wrong with the image.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib};
use tinyhand::handmaker::{self, CustomHand, Made};

use crate::thumbs::{self, Rgba};
use crate::widgets::{copy, icon_button, open_uri, toast};

/// The part of a composed hand the preview shows (the hand and the top of its stick),
/// in the 1024×1536 canvas `handmaker` composes onto. Same crop as the macOS app.
const PREVIEW_AREA: (f64, f64, f64, f64) = (112.0, 90.0, 800.0, 1000.0);
/// Preview size in device pixels (shown at half that, so it's sharp at 2×).
const PREVIEW_PX: (i32, i32) = (288, 360);

const DROP_TEXT: &str = "Drop the PNG here, or click to choose it";

type OnAdded = Box<dyn Fn(CustomHand)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MakeHandPage {
        pub(super) idea: adw::EntryRow,
        pub(super) prompt: gtk::Label,
        pub(super) stack: gtk::Stack,
        pub(super) drop_zone: gtk::Button,
        pub(super) drop_icon: gtk::Image,
        pub(super) drop_text: gtk::Label,
        pub(super) drop_hint: gtk::Label,
        pub(super) preview: gtk::Image,
        pub(super) name: adw::EntryRow,
        pub(super) add: gtk::Button,
        /// The checked, composed hand waiting to be added.
        pub(super) draft: RefCell<Option<Made>>,
        pub(super) busy: Cell<bool>,
        pub(super) on_added: OnceCell<OnAdded>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MakeHandPage {
        const NAME: &'static str = "TinyHandMakeHandPage";
        type Type = super::MakeHandPage;
        type ParentType = adw::NavigationPage;
    }

    impl ObjectImpl for MakeHandPage {}
    impl WidgetImpl for MakeHandPage {}
    impl NavigationPageImpl for MakeHandPage {}
}

glib::wrapper! {
    pub struct MakeHandPage(ObjectSubclass<imp::MakeHandPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MakeHandPage {
    /// `on_added` runs with the new hand once it's saved; the page then closes itself.
    pub fn new(on_added: impl Fn(CustomHand) + 'static) -> MakeHandPage {
        let page: MakeHandPage = glib::Object::builder().property("title", "Make your own hand").build();
        let _ = page.imp().on_added.set(Box::new(on_added));
        let content = adw::PreferencesPage::new();
        content.add(&page.describe_step());
        content.add(&page.image_step());
        content.add(&page.prompt_group());
        content.add(&page.add_step());
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&content));
        page.set_child(Some(&view));
        page.update_prompt();
        page
    }

    // ---------------------------------------------------------------- the steps

    fn describe_step(&self) -> adw::PreferencesGroup {
        let idea = &self.imp().idea;
        idea.set_title("e.g. knitted wool in mustard yellow");
        idea.connect_changed(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.update_prompt()
        ));
        let group = adw::PreferencesGroup::builder()
            .title("1. Describe it")
            .description("What should your hand look like?")
            .build();
        group.add(idea);
        group
    }

    fn image_step(&self) -> adw::PreferencesGroup {
        let open = icon_button("adw-external-link-symbolic", "Open _ChatGPT");
        open.add_css_class("suggested-action");
        open.add_css_class("pill");
        open.connect_clicked(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |b| open_uri(b, &chatgpt_uri(&page.prompt_text()))
        ));
        let copy_button = icon_button("edit-copy-symbolic", "Copy _Prompt");
        copy_button.add_css_class("pill");
        copy_button.connect_clicked(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |b| copy(b, &page.prompt_text(), "Prompt")
        ));
        let buttons = gtk::Box::builder().spacing(12).build();
        buttons.append(&open);
        buttons.append(&copy_button);
        let group = adw::PreferencesGroup::builder()
            .title("2. Make the image")
            .description(
                "Open it in ChatGPT, or copy the prompt for another image tool. \
                 Download the result as a PNG with a transparent background.",
            )
            .build();
        group.add(&buttons);
        group
    }

    /// The prompt itself, folded away, for people who want to read or tweak it.
    fn prompt_group(&self) -> adw::PreferencesGroup {
        let prompt = &self.imp().prompt;
        prompt.set_wrap(true);
        prompt.set_selectable(true);
        prompt.set_xalign(0.0);
        prompt.set_margin_top(12);
        prompt.set_margin_bottom(12);
        prompt.set_margin_start(12);
        prompt.set_margin_end(12);
        prompt.add_css_class("caption");
        let expander = adw::ExpanderRow::builder().title("See the prompt").build();
        expander.add_row(prompt);
        let group = adw::PreferencesGroup::new();
        group.add(&expander);
        group
    }

    fn add_step(&self) -> adw::PreferencesGroup {
        let stack = &self.imp().stack;
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_vhomogeneous(false);
        stack.add_named(&self.drop_zone(), Some("drop"));
        stack.add_named(&busy_view(), Some("busy"));
        stack.add_named(&self.preview_view(), Some("preview"));
        stack.set_visible_child_name("drop");
        let group = adw::PreferencesGroup::builder().title("3. Add it").build();
        group.add(stack);
        group
    }

    fn drop_zone(&self) -> gtk::Button {
        let imp = self.imp();
        imp.drop_icon.set_icon_name(Some("document-open-symbolic"));
        imp.drop_icon.set_pixel_size(24);
        imp.drop_text.set_label(DROP_TEXT);
        imp.drop_text.set_wrap(true);
        imp.drop_text.set_justify(gtk::Justification::Center);
        imp.drop_hint.add_css_class("dim-label");
        imp.drop_hint.add_css_class("caption");
        imp.drop_hint.set_visible(false);
        let content =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).valign(gtk::Align::Center).build();
        content.append(&imp.drop_icon);
        content.append(&imp.drop_text);
        content.append(&imp.drop_hint);
        let zone = &imp.drop_zone;
        zone.set_child(Some(&content));
        zone.set_height_request(140);
        zone.add_css_class("flat");
        zone.add_css_class("drop-zone");
        zone.connect_clicked(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.choose_file()
        ));
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop.connect_drop(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                let path = value.get::<gdk::FileList>().ok().and_then(|l| l.files().first()?.path());
                match path {
                    Some(path) => page.check(path),
                    None => page.show_problem("TinyHand can only use files on this computer."),
                }
                true
            }
        ));
        zone.add_controller(drop);
        zone.clone()
    }

    fn preview_view(&self) -> gtk::Box {
        let imp = self.imp();
        imp.preview.set_pixel_size(PREVIEW_PX.1 / 2);
        imp.preview.update_property(&[gtk::accessible::Property::Label("Your hand")]);
        let frame = gtk::Box::builder().valign(gtk::Align::Start).css_classes(["hand-preview"]).build();
        frame.append(&imp.preview);

        imp.name.set_title("Name");
        if let Some(text) = imp.name.delegate().and_downcast::<gtk::Text>() {
            text.set_max_length(16);
        }
        let name_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        name_list.add_css_class("boxed-list");
        name_list.append(&imp.name);

        let hint = gtk::Label::builder()
            .label(
                "It should show the back of the hand, like the ones in TinyHand. \
                 If you see the palm, try another image.",
            )
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();
        imp.add.set_label("_Add Hand");
        imp.add.set_use_underline(true);
        imp.add.add_css_class("suggested-action");
        imp.add.add_css_class("pill");
        imp.add.connect_clicked(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.add_hand()
        ));
        let other = gtk::Button::builder().label("Use Another Image").css_classes(["flat"]).build();
        other.connect_clicked(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.choose_file()
        ));
        let buttons = gtk::Box::builder().spacing(12).build();
        buttons.append(&imp.add);
        buttons.append(&other);

        let side = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        side.append(&name_list);
        side.append(&hint);
        side.append(&buttons);
        let view = gtk::Box::builder().spacing(22).build();
        view.append(&frame);
        view.append(&side);
        view
    }

    // ---------------------------------------------------------------- behaviour

    fn prompt_text(&self) -> String {
        handmaker::prompt(&self.imp().idea.text())
    }

    fn update_prompt(&self) {
        self.imp().prompt.set_label(&self.prompt_text());
    }

    fn choose_file(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("PNG images"));
        filter.add_mime_type("image/png");
        filter.add_suffix("png");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Choose Your Hand")
            .modal(true)
            .filters(&filters)
            .default_filter(&filter)
            .build();
        let window = self.root().and_downcast::<gtk::Window>();
        dialog.open(
            window.as_ref(),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = page)]
                self,
                move |result| match result.map(|f| f.path()) {
                    Ok(Some(path)) => page.check(path),
                    Ok(None) => page.show_problem("TinyHand can only use files on this computer."),
                    Err(_) => {} // cancelled
                }
            ),
        );
    }

    /// Check and compose the image on a worker thread, then show the preview or the problem.
    pub fn check(&self, path: PathBuf) {
        if self.imp().busy.replace(true) {
            return;
        }
        self.imp().stack.set_visible_child_name("busy");
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let file = path.clone();
                let result = gio::spawn_blocking(move || make_draft(&file)).await;
                page.imp().busy.set(false);
                match result {
                    Ok(Ok((made, preview))) => page.show_draft(made, &preview, &path),
                    Ok(Err(problem)) => page.show_problem(&problem),
                    Err(_) => page.show_problem("TinyHand couldn't read that file."),
                }
            }
        ));
    }

    fn show_draft(&self, made: Made, preview: &Rgba, path: &Path) {
        let imp = self.imp();
        imp.preview.set_paintable(Some(&preview.texture()));
        imp.name.set_text(&handmaker::suggested_name(path));
        imp.draft.replace(Some(made));
        imp.add.set_sensitive(true);
        imp.stack.set_visible_child_name("preview");
    }

    fn show_problem(&self, problem: &str) {
        let imp = self.imp();
        imp.draft.replace(None);
        imp.drop_icon.set_icon_name(Some("dialog-warning-symbolic"));
        imp.drop_text.set_label(problem);
        imp.drop_hint.set_label("Try again with another image");
        imp.drop_hint.set_visible(true);
        imp.drop_zone.add_css_class("problem");
        imp.stack.set_visible_child_name("drop");
    }

    /// Save the hand (encoding the PNG off the main thread), then hand it to `on_added`.
    fn add_hand(&self) {
        let imp = self.imp();
        let Some(made) = imp.draft.take() else { return };
        let name = imp.name.text().to_string();
        imp.add.set_sensitive(false);
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let saved = gio::spawn_blocking(move || {
                    let result = handmaker::add(&made, &name);
                    (made, result)
                })
                .await;
                match saved {
                    Ok((_, Ok(hand))) => page.added(hand),
                    Ok((made, Err(e))) => {
                        page.imp().draft.replace(Some(made));
                        page.imp().add.set_sensitive(true);
                        toast(&page, &e);
                    }
                    Err(_) => toast(&page, "TinyHand couldn't save the hand."),
                }
            }
        ));
    }

    fn added(&self, hand: CustomHand) {
        let window = self.root().and_downcast::<adw::PreferencesWindow>();
        if let Some(on_added) = self.imp().on_added.get() {
            on_added(hand);
        }
        if let Some(window) = window {
            window.pop_subpage();
        }
    }
}

/// Runs on a worker thread: check and compose the hand, and scale its preview.
fn make_draft(path: &Path) -> Result<(Made, Rgba), String> {
    let made = handmaker::make(path)?;
    let preview =
        thumbs::scaled(made.rgba.clone(), made.width as usize, made.height as usize, PREVIEW_AREA, PREVIEW_PX)
            .ok_or("TinyHand couldn't show that image.")?;
    Ok((made, preview))
}

fn busy_view() -> gtk::Box {
    let view = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .height_request(140)
        .build();
    let spinner = gtk::Spinner::new();
    spinner.start();
    view.append(&spinner);
    view.append(&gtk::Label::new(Some("Checking the image…")));
    view
}

/// ChatGPT with the prompt already typed in.
fn chatgpt_uri(prompt: &str) -> String {
    format!("https://chatgpt.com/?q={}", glib::Uri::escape_string(prompt, None, false))
}

#[cfg(test)]
mod tests {
    #[test]
    fn chatgpt_link_carries_the_prompt() {
        let uri = super::chatgpt_uri("a hand & more\nlines");
        assert!(uri.starts_with("https://chatgpt.com/?q=a%20hand%20%26%20more%0Alines"), "{uri}");
    }
}
