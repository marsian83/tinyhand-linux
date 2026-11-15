//! The settings window: one Adwaita preferences window with a page per section, in the
//! same order as the macOS app's sidebar.

use adw::prelude::*;
use gtk::glib;

use crate::pages;
use crate::store::Store;
use crate::thumbs::Thumbs;

/// Show the settings window, creating it the first time.
pub fn present(app: &adw::Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    let store = Store::load();
    let thumbs = Thumbs::new();
    let window = adw::PreferencesWindow::builder()
        .application(app)
        .title("TinyHand Settings")
        .default_width(860)
        .default_height(680)
        .search_enabled(true)
        .build();

    let pointers = pages::pointers::PointersPage::new(&store, &thumbs);
    window.add(&pointers);
    window.add(&pages::look::page(&store, &thumbs));
    window.add(&pages::drawing::page(&store));
    window.add(&pages::sound::page(&store));
    window.add(&pages::shortcuts::page());
    window.add(&pages::about::page(&store));

    store.set_error_handler(glib::clone!(
        #[weak]
        window,
        move |message| crate::widgets::toast(&window, message)
    ));
    // A slider still waiting to save gets saved before the window goes.
    window.connect_close_request(glib::clone!(
        #[strong]
        store,
        move |_| {
            store.flush();
            glib::Propagation::Proceed
        }
    ));
    window.present();
    open_from_env(&window, &pointers);
}

/// For screenshots and testing: `TINYHAND_SETTINGS_PAGE` names the page to show
/// (pointers, look, drawing, sound, shortcuts, about, or make-hand), and with make-hand,
/// `TINYHAND_SETTINGS_DRAFT` is an image to load into it.
fn open_from_env(window: &adw::PreferencesWindow, pointers: &pages::pointers::PointersPage) {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    match var("TINYHAND_SETTINGS_PAGE").as_deref() {
        Some("make-hand") => {
            if let (Some(flow), Some(draft)) = (pointers.make_hand(), var("TINYHAND_SETTINGS_DRAFT")) {
                flow.check(draft.into());
            }
        }
        Some(name) => window.set_visible_page_name(name),
        None => {}
    }
}
