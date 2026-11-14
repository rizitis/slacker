//! What every page needs: the runner, the window, toasts, notifications
//! about system changes, and the pin list once it has been read.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib;

use crate::runner::Runner;

type Handlers = Rc<RefCell<Vec<Rc<dyn Fn()>>>>;

fn emit(handlers: &Handlers) {
    // Clone first: a handler may register or trigger further work.
    let hs: Vec<Rc<dyn Fn()>> = handlers.borrow().clone();
    for h in hs {
        h();
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub runner: Runner,
    pub window: gtk::ApplicationWindow,
    toasts: adw::ToastOverlay,
    /// The short message on show, so the next one replaces it.
    last_short: Rc<RefCell<Option<adw::Toast>>>,
    changed: Handlers,
    pins_changed: Handlers,
    rules_changed: Handlers,
    /// package -> repo, as `slacker pin` listed it. Empty until the Frozen
    /// & pins page has been opened (listing needs root).
    pins: Rc<RefCell<HashMap<String, String>>>,
}

impl Ctx {
    pub fn new(runner: Runner, window: gtk::ApplicationWindow, toasts: adw::ToastOverlay) -> Self {
        Self {
            runner,
            window,
            toasts,
            last_short: Rc::default(),
            changed: Rc::default(),
            pins_changed: Rc::default(),
            rules_changed: Rc::default(),
            pins: Rc::default(),
        }
    }

    pub fn toast(&self, message: &str) {
        let toast = adw::Toast::new(&glib::markup_escape_text(message));
        toast.set_timeout(4);
        self.toasts.add_toast(toast);
    }

    /// A short message (text size, copied) for 2 seconds. It replaces the
    /// previous short one at once instead of waiting in line behind it.
    pub fn toast_short(&self, message: &str) {
        let toast = adw::Toast::new(&glib::markup_escape_text(message));
        toast.set_timeout(2);
        if let Some(old) = self.last_short.replace(Some(toast.clone())) {
            old.dismiss();
        }
        self.toasts.add_toast(toast);
    }

    pub fn on_system_changed(&self, f: impl Fn() + 'static) {
        self.changed.borrow_mut().push(Rc::new(f));
    }

    pub fn system_changed(&self) {
        emit(&self.changed);
    }

    /// A frozen rule or pin was added or removed.
    pub fn on_rules_changed(&self, f: impl Fn() + 'static) {
        self.rules_changed.borrow_mut().push(Rc::new(f));
    }

    pub fn rules_changed(&self) {
        emit(&self.rules_changed);
    }

    pub fn on_pins_changed(&self, f: impl Fn() + 'static) {
        self.pins_changed.borrow_mut().push(Rc::new(f));
    }

    pub fn set_pins(&self, pins: &[(String, String)]) {
        *self.pins.borrow_mut() = pins.iter().cloned().collect();
        emit(&self.pins_changed);
    }

    /// The repo a package is pinned to, if the pin list is known.
    pub fn pinned_repo(&self, package: &str) -> Option<String> {
        self.pins.borrow().get(package).cloned()
    }
}
