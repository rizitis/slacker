//! Adding and removing frozen rules, pins and dependency exclusions. Every
//! change goes through slacker (`frozen`, `unfrozen`, `pin`, `unpin`,
//! `nodep`, `undep`). Additions show slacker's
//! own description and question in the GUI and are written only when the
//! user answers yes; removals ask first in the GUI, since slacker does not.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::commands;
use crate::confirm::{self, Action};
use crate::ctx::Ctx;
use crate::parse::repos;

fn after_change(ctx: &Ctx) -> impl FnOnce(&crate::runner::Status) + 'static {
    let ctx = ctx.clone();
    move |st| {
        if confirm::may_have_changed(st) {
            ctx.rules_changed();
            ctx.system_changed();
        }
    }
}

fn hint_label(text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
    l.add_css_class("dim-label");
    l.add_css_class("caption");
    l
}

/// Asks for a freeze rule, then lets slacker show and confirm it.
pub fn freeze_dialog(ctx: &Ctx, prefill: Option<&str>) {
    let entry = gtk::Entry::builder()
        .placeholder_text("vlc   fcitx5*   kde/   @testing kernel-generic")
        .activates_default(true)
        .build();
    if let Some(p) = prefill {
        entry.set_text(p);
    }
    let error = hint_label("");
    error.add_css_class("error");
    error.set_visible(false);

    let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
    extra.append(&entry);
    extra.append(&hint_label(
        "A package name, a glob, a regular expression, a series ending in \u{201c}/\u{201d}, \
         or \u{201c}@repo PATTERN\u{201d} to freeze it in one repository only. \
         slacker checks the rule before anything is written.",
    ));
    extra.append(&error);

    let dialog = adw::AlertDialog::builder()
        .heading("Freeze packages")
        .body("Installed packages that match stay as they are. Packages that match and are not installed are left out of install-new and upgrades.")
        .extra_child(&extra)
        .close_response("cancel")
        .default_response("check")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("check", "Check")]);
    dialog.set_response_appearance("check", adw::ResponseAppearance::Suggested);

    let valid = {
        let (d, e) = (dialog.clone(), error.clone());
        move |text: &str| {
            let r = commands::rule_text(text);
            d.set_response_enabled("check", r.is_ok());
            match (&r, text.trim().is_empty()) {
                (Err(msg), false) => {
                    e.set_text(msg);
                    e.set_visible(true);
                }
                _ => e.set_visible(false),
            }
        }
    };
    valid(&entry.text());
    entry.connect_changed(move |e| valid(&e.text()));

    let ctx2 = ctx.clone();
    let focus = entry.clone();
    confirm::after_choice(&dialog, "check", &ctx.window, move || {
        let ctx = ctx2;
        let Ok(rule) = commands::rule_text(&entry.text()) else { return };
        confirm::run_as_root(
            &ctx,
            Action {
                spec: commands::freeze(&rule),
                title: format!("Freeze \u{201c}{rule}\u{201d}"),
                verb: "Freeze".into(),
                destructive: false,
            },
            after_change(&ctx),
        );
    });
    glib::idle_add_local_once(move || {
        focus.grab_focus();
    });
}

/// Asks for a package and a repository, then lets slacker show and confirm the pin.
pub fn pin_dialog(ctx: &Ctx, prefill: Option<&str>) {
    let entry = gtk::Entry::builder()
        .placeholder_text("Package name, e.g. vlc")
        .activates_default(true)
        .build();
    if let Some(p) = prefill {
        entry.set_text(p);
    }
    let names = gtk::StringList::new(&["Loading repositories\u{2026}"]);
    let repo = gtk::DropDown::builder().model(&names).sensitive(false).build();
    let error = hint_label("");
    error.add_css_class("error");
    error.set_visible(false);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let from = gtk::Label::new(Some("from"));
    from.add_css_class("dim-label");
    entry.set_hexpand(true);
    row.append(&entry);
    row.append(&from);
    row.append(&repo);

    let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
    extra.append(&row);
    extra.append(&hint_label("An exact package name, without version. A freeze on the same package still wins."));
    extra.append(&error);

    let dialog = adw::AlertDialog::builder()
        .heading("Pin a package")
        .body("The package will be taken only from the chosen repository, whatever the priorities say.")
        .extra_child(&extra)
        .close_response("cancel")
        .default_response("check")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("check", "Check")]);
    dialog.set_response_appearance("check", adw::ResponseAppearance::Suggested);

    let loaded = Rc::new(std::cell::Cell::new(false));
    let validate: Rc<dyn Fn()> = {
        let (d, e, entry, loaded) = (dialog.clone(), error.clone(), entry.clone(), loaded.clone());
        Rc::new(move || {
            let text = entry.text();
            let r = commands::single_name(&text);
            d.set_response_enabled("check", r.is_ok() && loaded.get());
            match (&r, text.trim().is_empty()) {
                (Err(msg), false) => {
                    e.set_text(msg);
                    e.set_visible(true);
                }
                _ => e.set_visible(false),
            }
        })
    };
    validate();
    {
        let v = validate.clone();
        entry.connect_changed(move |_| v());
    }

    // The repository list comes from slacker, as the user.
    {
        let (repo, names, loaded, v, err) =
            (repo.clone(), names.clone(), loaded.clone(), validate.clone(), error.clone());
        ctx.runner.capture(commands::list_repos(), move |_, text| {
            let parsed = repos::parse(&text);
            let list: Vec<&str> = parsed.repos.iter().map(|r| r.name.as_str()).collect();
            names.splice(0, names.n_items(), &list);
            if list.is_empty() {
                err.set_text("slacker listed no repositories.");
                err.set_visible(true);
                return;
            }
            repo.set_selected(0);
            repo.set_sensitive(true);
            loaded.set(true);
            v();
        });
    }

    let ctx2 = ctx.clone();
    let focus = entry.clone();
    confirm::after_choice(&dialog, "check", &ctx.window, move || {
        let ctx = ctx2;
        if !loaded.get() {
            return;
        }
        let Ok(package) = commands::single_name(&entry.text()) else { return };
        let Some(repo_name) = repo
            .selected_item()
            .and_downcast::<gtk::StringObject>()
            .map(|s| s.string().to_string())
        else {
            return;
        };
        confirm::run_as_root(
            &ctx,
            Action {
                spec: commands::pin(&repo_name, &package),
                title: format!("Pin {package} to {repo_name}"),
                verb: "Pin".into(),
                destructive: false,
            },
            after_change(&ctx),
        );
    });
    glib::idle_add_local_once(move || {
        focus.grab_focus();
    });
}

pub fn unfreeze(ctx: &Ctx, rule: &str) {
    confirm::run_as_root(
        ctx,
        Action {
            spec: commands::unfreeze(rule),
            title: format!("Unfreeze \u{201c}{rule}\u{201d}"),
            verb: "Unfreeze".into(),
            destructive: false,
        },
        after_change(ctx),
    );
}

pub fn unpin(ctx: &Ctx, package: &str) {
    confirm::run_as_root(
        ctx,
        Action {
            spec: commands::unpin(package),
            title: format!("Remove the pin on {package}"),
            verb: "Unpin".into(),
            destructive: false,
        },
        after_change(ctx),
    );
}

/// Asks for a dependency, and optionally the one package it is left out for
/// (blank: any package), then lets slacker show and confirm the exclusion.
pub fn nodep_dialog(ctx: &Ctx) {
    let package = gtk::Entry::builder()
        .placeholder_text("Any package")
        .activates_default(true)
        .hexpand(true)
        .build();
    let dependency = gtk::Entry::builder()
        .placeholder_text("Dependency, e.g. cargo-auditable")
        .activates_default(true)
        .hexpand(true)
        .build();
    let error = hint_label("");
    error.add_css_class("error");
    error.set_visible(false);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&dependency);
    let for_ = gtk::Label::new(Some("for"));
    for_.add_css_class("dim-label");
    row.append(&for_);
    row.append(&package);

    let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
    extra.append(&row);
    extra.append(&hint_label(
        "Exact package names, without version. Leave the package blank to leave the dependency out for every package. \
         A rule for one package acts only if that package itself requires the dependency; \
         its Dependencies\u{2026} window, under Installed or in the package details, shows which one does.",
    ));
    extra.append(&error);

    let dialog = adw::AlertDialog::builder()
        .heading("Leave out a dependency")
        .body("slacker will not pull the dependency in for that package. Other packages that require it still pull it in, and nothing installed is removed.")
        .extra_child(&extra)
        .close_response("cancel")
        .default_response("check")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("check", "Check")]);
    dialog.set_response_appearance("check", adw::ResponseAppearance::Suggested);

    let validate: Rc<dyn Fn()> = {
        let (d, e, p, dep) = (dialog.clone(), error.clone(), package.clone(), dependency.clone());
        Rc::new(move || {
            let r = commands::dep_rule(&p.text(), &dep.text());
            d.set_response_enabled("check", r.is_ok());
            match (&r, dep.text().trim().is_empty() && p.text().trim().is_empty()) {
                (Err(msg), false) => {
                    e.set_text(msg);
                    e.set_visible(true);
                }
                _ => e.set_visible(false),
            }
        })
    };
    validate();
    for en in [&package, &dependency] {
        let v = validate.clone();
        en.connect_changed(move |_| v());
    }

    let ctx2 = ctx.clone();
    let focus = dependency.clone();
    confirm::after_choice(&dialog, "check", &ctx.window, move || {
        let ctx = ctx2;
        let Ok(rule) = commands::dep_rule(&package.text(), &dependency.text()) else { return };
        let (p, d) = rule.split_once(':').unwrap_or(("*", &rule));
        let whom = if p == "*" { "any package".to_string() } else { p.to_string() };
        confirm::run_as_root(
            &ctx,
            Action {
                spec: commands::nodep(std::slice::from_ref(&rule)),
                title: format!("Stop pulling in \u{201c}{d}\u{201d} for {whom}"),
                verb: "Write".into(),
                destructive: false,
            },
            after_change(&ctx),
        );
    });
    glib::idle_add_local_once(move || {
        focus.grab_focus();
    });
}

/// `rule` is `package:dependency` as listed (package `*` for any).
pub fn undep(ctx: &Ctx, rule: &str) {
    let (p, d) = rule.split_once(':').unwrap_or(("*", rule));
    let whom = if p == "*" { "any package".to_string() } else { p.to_string() };
    confirm::run_as_root(
        ctx,
        Action {
            spec: commands::undep(&[rule.to_string()]),
            title: format!("Pull \u{201c}{d}\u{201d} in again for {whom}"),
            verb: "Remove Rule".into(),
            destructive: false,
        },
        after_change(ctx),
    );
}

/// Asks for a new priority for an active repository, then runs `pri-repo`,
/// which shows the change and asks before writing it. slacker checks that the number is free; its message
/// is shown when it is not.
pub fn priority_dialog(ctx: &Ctx, name: &str, current: i32, taken: &[(i32, String)]) {
    let spin = gtk::SpinButton::with_range(0.0, 10_000.0, 1.0);
    spin.set_value(current.max(0) as f64);
    spin.set_numeric(true);
    spin.set_halign(gtk::Align::Start);

    let others: Vec<String> = taken
        .iter()
        .filter(|(_, n)| n != name)
        .map(|(p, n)| format!("{p} {n}"))
        .collect();
    let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
    extra.append(&spin);
    extra.append(&hint_label(
        "A higher number wins when several repositories ship the same package. Every repository needs its own number.",
    ));
    if !others.is_empty() {
        extra.append(&hint_label(&format!("In use: {}", others.join(", "))));
    }

    let dialog = adw::AlertDialog::builder()
        .heading(format!("Priority of {name}"))
        .body(format!("Currently {current}."))
        .extra_child(&extra)
        .close_response("cancel")
        .default_response("check")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("check", "Check")]);
    dialog.set_response_appearance("check", adw::ResponseAppearance::Suggested);
    let changed = {
        let d = dialog.clone();
        move |sp: &gtk::SpinButton| d.set_response_enabled("check", sp.value_as_int() != current)
    };
    changed(&spin);
    spin.connect_value_changed(changed);
    {
        // Enter in the number field acts as "Check" when the value changed.
        let (d, sp) = (dialog.clone(), spin.clone());
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != gtk::gdk::Key::Return && key != gtk::gdk::Key::KP_Enter {
                return glib::Propagation::Proceed;
            }
            sp.update();
            if sp.value_as_int() != current {
                d.emit_by_name::<()>("response", &[&"check"]);
                d.close();
            }
            glib::Propagation::Stop
        });
        spin.add_controller(keys);
    }

    let (ctx2, name) = (ctx.clone(), name.to_string());
    let focus = spin.clone();
    confirm::after_choice(&dialog, "check", &ctx.window, move || {
        let ctx = ctx2;
        // Read the typed text too, in case Enter came before the update.
        spin.update();
        let Ok(priority) = u32::try_from(spin.value_as_int()) else { return };
        let ctx2 = ctx.clone();
        confirm::run_as_root(
            &ctx,
            Action {
                spec: commands::pri_repo(priority, &name),
                title: format!("Priority of {name}: {current} \u{2192} {priority}"),
                verb: "Change".into(),
                destructive: false,
            },
            move |st| {
                if confirm::may_have_changed(st) {
                    ctx2.system_changed();
                }
            },
        );
    });
    glib::idle_add_local_once(move || {
        focus.grab_focus();
    });
}
