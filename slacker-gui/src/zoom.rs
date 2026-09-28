//! Window zoom: Ctrl and the wheel, or Ctrl with `+`, `-` and `0`.
//!
//! The desktop's font size is the right default and the styles follow it (they
//! are all ratios of it, see style.css). This is the second lever, for the
//! person whose screen needs this one window larger than the rest: it scales
//! the whole window by setting `font-size` on the window node, which every
//! ratio below it then follows, so headings, captions and the monospace views
//! keep their proportions instead of drifting apart.
//!
//! The chosen size is remembered in `~/.config/slacker-gui/zoom` (a plain
//! number of percent, nothing else), so it survives a restart without needing
//! a settings schema installed. Losing that file just means the next start is
//! back at the desktop's own size, which is a safe place to be.

use std::cell::RefCell;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Percent of the desktop font size. 100 means "exactly what the desktop says".
const DEFAULT: i32 = 100;
const MIN: i32 = 70;
const MAX: i32 = 250;
const STEP: i32 = 10;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

struct State {
    percent: i32,
    provider: gtk::CssProvider,
}

/// Start the zoom: read the remembered size and put its provider on the
/// display, above the application stylesheet so it wins for the window node.
pub fn init(display: &gdk::Display) {
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
    let percent = clamp(read_saved().unwrap_or(DEFAULT));
    STATE.with(|s| *s.borrow_mut() = Some(State { percent, provider }));
    apply();
}

/// Current size, in percent of the desktop font size.
pub fn percent() -> i32 {
    STATE.with(|s| s.borrow().as_ref().map(|st| st.percent).unwrap_or(DEFAULT))
}

/// Change the size by `delta` percent; `set(DEFAULT)` is the reset.
pub fn step(delta: i32) -> i32 {
    set(percent() + delta)
}

pub fn reset() -> i32 {
    set(DEFAULT)
}

fn set(target: i32) -> i32 {
    let want = clamp(target);
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        match s.as_mut() {
            Some(st) if st.percent != want => {
                st.percent = want;
                true
            }
            _ => false,
        }
    });
    if changed {
        apply();
        save(want);
    }
    want
}

fn clamp(p: i32) -> i32 {
    p.clamp(MIN, MAX)
}

/// 100% loads an empty sheet rather than `font-size: 100%`: at the default the
/// window should be styled exactly as if this module were not here.
fn apply() {
    STATE.with(|s| {
        if let Some(st) = s.borrow().as_ref() {
            let css = if st.percent == DEFAULT {
                String::new()
            } else {
                format!("window {{ font-size: {}%; }}\n", st.percent)
            };
            st.provider.load_from_data(&css);
        }
    });
}

fn saved_path() -> std::path::PathBuf {
    glib::user_config_dir().join("slacker-gui").join("zoom")
}

fn read_saved() -> Option<i32> {
    std::fs::read_to_string(saved_path()).ok()?.trim().parse().ok()
}

/// Best-effort: a zoom level is not worth an error dialog, and a read-only or
/// full home directory must not stop the window from working.
fn save(percent: i32) {
    let path = saved_path();
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    let _ = std::fs::write(&path, format!("{percent}\n"));
}

/// Ctrl with `+`, `-` and `0`, as every other zoomable window has them.
/// `equal` is listed next to `plus` because that is the same key unshifted.
pub fn install_actions(app: &adw::Application, notify: impl Fn(i32) + Clone + 'static) {
    for (name, accels, delta) in [
        ("zoom-in", vec!["<Control>plus", "<Control>equal", "<Control>KP_Add"], STEP),
        ("zoom-out", vec!["<Control>minus", "<Control>KP_Subtract"], -STEP),
        ("zoom-reset", vec!["<Control>0", "<Control>KP_0"], 0),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let notify = notify.clone();
        action.connect_activate(move |_, _| {
            let now = if delta == 0 { reset() } else { step(delta) };
            notify(now);
        });
        app.add_action(&action);
        app.set_accels_for_action(&format!("app.{name}"), &accels);
    }
}

/// Ctrl and the wheel. The controller runs in the CAPTURE phase so the scrolled
/// areas inside the window do not eat the event first; without Ctrl it passes
/// straight through and scrolling works as usual.
pub fn attach(widget: &impl IsA<gtk::Widget>, notify: impl Fn(i32) + 'static) {
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    scroll.connect_scroll(move |c, _dx, dy| {
        if !c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) || dy == 0.0 {
            return glib::Propagation::Proceed;
        }
        // Wheel up is a negative delta, and up means larger.
        notify(step(if dy < 0.0 { STEP } else { -STEP }));
        glib::Propagation::Stop
    });
    widget.as_ref().add_controller(scroll);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_stays_inside_readable_limits() {
        assert_eq!(clamp(DEFAULT), DEFAULT);
        assert_eq!(clamp(MIN - 50), MIN);
        assert_eq!(clamp(MAX + 500), MAX);
        // The steps land on round numbers from the default.
        assert_eq!(clamp(DEFAULT + STEP), 110);
        assert_eq!(clamp(DEFAULT - STEP), 90);
    }

    #[test]
    fn the_remembered_size_is_a_plain_number() {
        // Whatever else ends up in that file, it never makes the window
        // unreadable: garbage reads as None and falls back to the desktop size.
        let parse = |s: &str| s.trim().parse::<i32>().ok().map(clamp);
        assert_eq!(parse("130\n"), Some(130));
        assert_eq!(parse("  90  "), Some(90));
        assert_eq!(parse("9000"), Some(MAX));
        assert_eq!(parse(""), None);
        assert_eq!(parse("huge"), None);
    }
}
