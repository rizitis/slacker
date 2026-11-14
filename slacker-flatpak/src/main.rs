//! slacker-flatpak: a GTK4/libadwaita front-end for the Flatpak installations
//! on a Slackware system.
//!
//! It runs `flatpak` as the user, never as root, and shows what it prints:
//! installed applications and runtimes, remotes, updates, and a few checks
//! (flatpak, its system helper, XDG_DATA_DIRS). Changes (update, uninstall)
//! run flatpak on a pty so its own questions reach the user; privileges for
//! the system installation come from flatpak's system helper and polkit.
//! Every command it runs is the same one you can run in a terminal.

mod catalog;
mod checks;
mod commands;
mod parse;
mod pty;
mod runner;
mod txn;
mod window;

use adw::prelude::*;
use gtk::glib;

const APP_ID: &str = "nl.slackware.forge.rizitis.SlackerFlatpak";

/// Shown when started as root (e.g. `sudo slacker-flatpak`).
const ROOT_REFUSAL: &str = "slacker-flatpak: do not run this as root.\n\
Start it as your normal user. flatpak itself asks the system helper (polkit)\n\
when a change to the system installation needs privileges.";

/// The effective user id, read from the owner of `/proc/self`.
fn effective_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

fn main() -> glib::ExitCode {
    // As root, "user" would mean root's own installation in /root and the
    // whole GTK stack would run privileged. Refuse before touching the display.
    if effective_uid() == Some(0) {
        eprintln!("{ROOT_REFUSAL}");
        return glib::ExitCode::FAILURE;
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);
    });
    app.connect_activate(window::build);
    app.run()
}
