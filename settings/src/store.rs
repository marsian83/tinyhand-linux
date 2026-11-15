//! The settings the window edits. Every change is written to the config file right away
//! and a running pointer is told to re-read it.
//!
//! Each change re-reads the file first, so edits made elsewhere (by hand, or by another
//! settings window) are kept rather than overwritten with what this window loaded.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use tinyhand::{config::Config, ipc};

/// How long a slider has to rest before its value is saved.
const DEBOUNCE: Duration = Duration::from_millis(150);

type Change = Box<dyn FnOnce(&mut Config)>;
type Listener = Box<dyn Fn(&Config)>;
type ErrorHandler = Box<dyn Fn(&str)>;

pub struct Store {
    config: RefCell<Config>,
    listeners: RefCell<Vec<Listener>>,
    /// A change waiting for the slider to rest, and the timer that will apply it.
    deferred: RefCell<Option<(glib::SourceId, Change)>>,
    on_error: RefCell<Option<ErrorHandler>>,
    /// Pins to bring back when the toolbar is turned on again.
    last_pins: RefCell<Vec<String>>,
}

/// The default settings, not yet read from the file (a placeholder until `load`).
impl Default for Store {
    fn default() -> Self {
        Store::new(Config::default())
    }
}

impl Store {
    /// The settings from the config file.
    pub fn load() -> Rc<Store> {
        Rc::new(Store::new(Config::load()))
    }

    fn new(config: Config) -> Store {
        let pins = if config.toolbar.is_empty() { Config::default().toolbar } else { config.toolbar.clone() };
        Store {
            config: RefCell::new(config),
            listeners: RefCell::default(),
            deferred: RefCell::default(),
            on_error: RefCell::default(),
            last_pins: RefCell::new(pins),
        }
    }

    /// A copy of the current settings.
    pub fn get(&self) -> Config {
        self.config.borrow().clone()
    }

    /// Change, save and apply the settings, then tell every listener.
    pub fn update(&self, change: impl FnOnce(&mut Config)) {
        let mut c = Config::load();
        let before = c.entries();
        change(&mut c);
        if c.entries() != before {
            match c.save() {
                // Err means no pointer is running, which is fine: it reads the file on start.
                Ok(()) => drop(ipc::send("reload")),
                Err(e) => self.report(&format!("Couldn't save the settings: {e}")),
            }
        }
        if !c.toolbar.is_empty() {
            self.last_pins.replace(c.toolbar.clone());
        }
        self.config.replace(c);
        let now = self.get();
        for listener in self.listeners.borrow().iter() {
            listener(&now);
        }
    }

    /// Like `update`, but waits until no other deferred change came in for a moment (for
    /// sliders). A newer deferred change replaces a waiting one.
    pub fn update_soon(self: &Rc<Self>, change: impl FnOnce(&mut Config) + 'static) {
        if let Some((timer, _)) = self.deferred.take() {
            timer.remove();
        }
        let timer = glib::timeout_add_local_once(
            DEBOUNCE,
            glib::clone!(
                #[weak(rename_to = store)]
                self,
                move || {
                    // The timer has fired, so its id must not be removed again.
                    if let Some((_, change)) = store.deferred.take() {
                        store.update(change);
                    }
                }
            ),
        );
        self.deferred.replace(Some((timer, Box::new(change))));
    }

    /// Apply a waiting deferred change now (when the window closes).
    pub fn flush(&self) {
        if let Some((timer, change)) = self.deferred.take() {
            timer.remove();
            self.update(change);
        }
    }

    /// Call `f` after every change. `f` should hold only weak references to widgets.
    pub fn connect_changed(&self, f: impl Fn(&Config) + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    /// Where to show errors (the window shows them as toasts).
    pub fn set_error_handler(&self, f: impl Fn(&str) + 'static) {
        self.on_error.replace(Some(Box::new(f)));
    }

    pub fn report(&self, message: &str) {
        match &*self.on_error.borrow() {
            Some(f) => f(message),
            None => eprintln!("tinyhand-settings: {message}"),
        }
    }

    /// The toolbar's pins to restore when it's turned back on.
    pub fn last_pins(&self) -> Vec<String> {
        self.last_pins.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn update_saves_keeps_other_edits_and_notifies() {
        let dir = std::env::temp_dir().join(format!("tinyhand-settings-test-{}", std::process::id()));
        let file = dir.join("tinyhand/config");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        // The only test that touches the environment: a private config and no pointer to reach.
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        std::fs::write(&file, "# mine\nshadow = false\n").unwrap();

        let store = Store::load();
        assert!(!store.get().shadow);
        let heard = Rc::new(Cell::new(0));
        store.connect_changed({
            let heard = Rc::clone(&heard);
            move |c| {
                assert_eq!(c.tap_sound, "pop");
                heard.set(heard.get() + 1);
            }
        });
        // Edited elsewhere after the window loaded: kept.
        std::fs::write(&file, "# mine\nshadow = false\nsize = 0.1\n").unwrap();
        store.update(|c| c.tap_sound = "pop".into());
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("# mine\n"), "{text}");
        assert!(text.contains("tap_sound = pop") && text.contains("size = 0.100"), "{text}");
        assert_eq!(heard.get(), 1);

        // Turning the toolbar off remembers its pins.
        store.update(|c| c.toolbar.clear());
        assert_eq!(store.last_pins(), Config::default().toolbar);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
