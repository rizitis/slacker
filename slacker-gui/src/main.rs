//! slacker-gui: a GTK4/libadwaita front-end for the slacker package manager.
//!
//! The GUI holds no package, resolver, GPG or history logic of its own.
//! Every piece of information comes from running the slacker binary and
//! every change goes through it: read-only commands run as the desktop user,
//! commands that modify the system run as root through pkexec.

mod commands;
mod confirm;
mod ctx;
mod edit;
mod output;
mod pages;
mod parse;
mod resolve;
mod runner;
mod widgets;
mod window;
mod zoom;

#[cfg(test)]
mod install_tests;


use adw::prelude::*;
use gtk::{gdk, glib};

const APP_ID: &str = "nl.slackware.forge.rizitis.SlackerGui";

/// Shown when the GUI is started as root (e.g. `sudo slacker-gui`).
const ROOT_REFUSAL: &str = "slacker-gui: do not run this as root.\n\
Start Slacker GUI as your normal user. Read-only views run as you, and only\n\
the actions that change the system run as root, through pkexec, after asking\n\
for the password. For root work in a terminal, use slacker itself.";

/// The effective user id, read from the owner of `/proc/self` (no extra
/// dependency). None if `/proc` is not available.
fn effective_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

fn main() -> glib::ExitCode {
    // As root the whole GTK stack would run privileged, pkexec would stop
    // asking for a password, and GTK would write root-owned files into the
    // session. Refuse before touching the display.
    if effective_uid() == Some(0) {
        eprintln!("{ROOT_REFUSAL}");
        return glib::ExitCode::FAILURE;
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        // Installed as hicolor/*/apps/<APP_ID>; used by X11 window managers.
        gtk::Window::set_default_icon_name(APP_ID);
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);
        load_style();
        if let Some(display) = gdk::Display::default() {
            // After load_style: the zoom sheet sits above the application one.
            zoom::init(&display);
        }
    });
    // GApplication already makes the app unique per session: a second
    // `slacker-gui` (desktop entry, terminal, another workspace) finds the
    // running one on the session bus, asks it to activate, and exits. So
    // activation only builds the window the first time; after that it
    // brings the existing one forward (the window manager switches to its
    // workspace, or moves it here, as it is set up to do).
    app.connect_activate(|app| match app.active_window() {
        Some(w) => w.present(),
        None => window::build(app),
    });
    app.run()
}

fn load_style() {
    let Some(display) = gdk::Display::default() else { return };
    let mut sheets = vec![include_str!("style.css")];
    // Older libadwaita does not understand `:root` variables; only newer
    // releases take the accent from them.
    if (adw::major_version(), adw::minor_version()) >= (1, 6) {
        sheets.push(include_str!("style-vars.css"));
    }
    for css in sheets {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(css);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    /// `/proc/self` is owned by the effective uid: a file this process
    /// creates gets the same owner.
    #[test]
    fn proc_self_owner_is_the_effective_uid() {
        let dir = std::env::temp_dir().join(format!("slacker-gui-uid-{}", std::process::id()));
        std::fs::write(&dir, b"x").unwrap();
        let file_uid = std::fs::metadata(&dir).unwrap().uid();
        let _ = std::fs::remove_file(&dir);
        assert_eq!(super::effective_uid(), Some(file_uid));
    }
}
