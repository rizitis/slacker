//! Updates: `check-updates` per repo, plus update / install-new / upgrade-all.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;

use super::Page;
use crate::commands::{self, Spec};
use crate::confirm::{self, Action};
use crate::ctx::Ctx;
use crate::parse::updates::{self, State};
use crate::runner::Status;
use crate::widgets::{self, Tone};

pub fn page(ctx: &Ctx) -> Page {
    let (scroller, body) = widgets::page_body();

    let summary = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&summary);

    // What to do, in the order slacker recommends. Kept separate from the
    // report above: fetching the lists installs nothing by itself.
    let actions = widgets::group(
        "What to do",
        "In this order. Updating the lists only fetches what the repositories published; the packages on this system change in steps 2 and 3.",
    );
    body.append(&actions);

    let repos = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&repos);

    let check = widgets::flat_icon_button("view-refresh-symbolic", "Check for updates");
    ctx.runner.watch(&check);

    // A check already queued or running is not started again: opening the
    // page twice, or coming back to it, must not stack check-updates runs.
    let checking = Rc::new(Cell::new(false));
    let load: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let (summary, repos) = (summary.clone(), repos.clone());
        let checking = checking.clone();
        Rc::new(move || {
            if checking.replace(true) {
                return;
            }
            widgets::clear(&summary);
            widgets::clear(&repos);
            summary.append(&widgets::loading("Checking every repository\u{2026}"));
            let (summary2, repos2) = (summary.clone(), repos.clone());
            let checking2 = checking.clone();
            ctx.runner.capture(commands::check_updates(), move |status, text| {
                checking2.set(false);
                widgets::clear(&summary2);
                let parsed = updates::parse(&text);
                summary2.append(&summary_card(&status, &parsed));
                if !parsed.repos.is_empty() {
                    repos2.append(&repo_group(&parsed));
                }
                if !parsed.notes.is_empty() {
                    let g = widgets::group("Messages from slacker", "");
                    for n in &parsed.notes {
                        let warn = n.starts_with("WARNING");
                        let r = widgets::row(n, "");
                        r.add_prefix(&if warn {
                            widgets::tile("dialog-warning-symbolic", Some("warning"))
                        } else {
                            widgets::tile("dialog-information-symbolic", None)
                        });
                        g.add(&r);
                    }
                    repos2.append(&g);
                }
            });
        })
    };
    {
        let l = load.clone();
        check.connect_clicked(move |_| l());
    }

    // check-updates compares the repositories' lists with the cached ones
    // and never looks at installed packages, so install-new and upgrade-all
    // cannot change its answer: it is not run again after them. After an
    // update the answer above is out of date, and the page says so instead
    // of checking every repository again. Coming back to the page checks
    // again (see window.rs).
    let after = {
        let ctx = ctx.clone();
        let (summary, repos) = (summary.clone(), repos.clone());
        Rc::new(move |st: &Status, label: &str, lists_changed: bool| {
            if st.code() == Some(50) {
                ctx.toast(&format!("slacker upgraded itself. Run \u{201c}{label}\u{201d} again."));
            }
            if confirm::may_have_changed(st) {
                ctx.system_changed();
                if lists_changed {
                    widgets::clear(&summary);
                    widgets::clear(&repos);
                    summary.append(&stale_card());
                }
            }
        })
    };

    let add_action = |step: i32,
                      title: &str,
                      subtitle: &str,
                      verb: &str,
                      spec: fn() -> Spec,
                      primary: bool,
                      lists_changed: bool| {
        let row = widgets::row(title, subtitle);
        row.add_prefix(&widgets::priority_badge(step, primary));
        let b = widgets::row_button(verb, if primary { Some("suggested-action") } else { None });
        ctx.runner.watch(&b);
        let (ctx2, after2, title2, verb2) = (ctx.clone(), after.clone(), title.to_string(), verb.to_string());
        b.connect_clicked(move |_| {
            let after3 = after2.clone();
            let label = title2.clone();
            let action = Action {
                spec: spec(),
                title: title2.clone(),
                verb: verb2.clone(),
                destructive: false,
            };
            // slacker lists its plan and asks before anything is written;
            // the questions come up in the transaction window.
            confirm::run_as_root(&ctx2, action, move |st| after3(st, &label, lists_changed));
        });
        row.add_suffix(&b);
        row.set_activatable_widget(Some(&b));
        actions.add(&row);
    };
    add_action(
        1,
        "Update the package lists",
        "Fetch what the repositories have published. Nothing is installed, removed or upgraded by this step.",
        "Update",
        commands::update,
        false,
        true,
    );
    add_action(
        2,
        "Install new packages",
        "Packages the official repositories added and this system does not have. slacker lists them before installing.",
        "Install new",
        commands::install_new,
        false,
        false,
    );
    add_action(
        3,
        "Upgrade all",
        "Installed packages that have a newer build. slacker lists them before upgrading.",
        "Upgrade all",
        commands::upgrade_all,
        true,
        false,
    );

    Page {
        name: "updates",
        title: "Updates",
        subtitle: "slacker check-updates",
        icon: "software-update-available-symbolic",
        widget: scroller.upcast(),
        header: Some(check.upcast()),
        load: Some(load),
        reload_on_change: false,
    }
}

/// Shown after an update instead of a new check: the result that was here is
/// out of date, and checking again is one click (or coming back to the page).
fn stale_card() -> gtk::Box {
    hero(
        "calm",
        "view-refresh-symbolic",
        "The package lists may have changed",
        "An update was run, so the check shown before may be out of date. Press the refresh button, or open this page again, to check the repositories again.",
    )
}

fn summary_card(status: &Status, parsed: &updates::Updates) -> gtk::Box {
    let pending = parsed.repos.iter().filter(|(_, s)| *s == State::Pending).count();
    let broken = parsed
        .repos
        .iter()
        .filter(|(_, s)| matches!(s, State::Unknown | State::Unreachable))
        .count();

    let (class, icon, title, sub) = if !status.answered() {
        (
            "bad",
            "dialog-error-symbolic",
            "Could not check for updates".to_string(),
            crate::runner::describe(status, commands::Privilege::User),
        )
    } else if pending > 0 {
        (
            "attention",
            "folder-download-symbolic",
            if pending == 1 {
                "1 repository has published new data".to_string()
            } else {
                format!("{pending} repositories have published new data")
            },
            "Update the package lists (step 1), then steps 2 and 3.".to_string(),
        )
    } else if broken > 0 {
        (
            "attention",
            "dialog-warning-symbolic",
            "Some repositories could not be checked".to_string(),
            format!("{broken} of {} need attention.", parsed.repos.len()),
        )
    } else {
        (
            "good",
            "object-select-symbolic",
            "The package lists are current".to_string(),
            format!(
                "{} repositories checked. This says nothing about installed packages: install-new (2) and upgrade-all (3) show whether anything is still pending.",
                parsed.repos.len()
            ),
        )
    };

    hero(class, icon, &title, &sub)
}

fn hero(class: &str, icon: &str, title: &str, sub: &str) -> gtk::Box {
    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    hero.add_css_class("hero");
    hero.add_css_class(class);
    let i = widgets::icon(icon, 48);
    i.add_css_class("hero-icon");
    hero.append(&i);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
    text.set_valign(gtk::Align::Center);
    let caption = gtk::Label::builder().label("CHECK-UPDATES").xalign(0.0).build();
    caption.add_css_class("hero-caption");
    let t = gtk::Label::builder().label(title).xalign(0.0).wrap(true).build();
    t.add_css_class("hero-title");
    let s = gtk::Label::builder().label(sub).xalign(0.0).wrap(true).build();
    s.add_css_class("dim-label");
    text.append(&caption);
    text.append(&t);
    text.append(&s);
    hero.append(&text);
    hero
}

fn repo_group(parsed: &updates::Updates) -> adw::PreferencesGroup {
    let g = widgets::group(
        "Repository data",
        "Whether each repository has published anything since the last update of the lists",
    );
    for (name, state) in &parsed.repos {
        let r = widgets::row(name, "");
        let (icon, tone_class, text, tone) = match state {
            State::UpToDate => ("object-select-symbolic", Some("success"), "list is current", Tone::Success),
            State::Pending => ("folder-download-symbolic", Some("accent"), "new data to fetch", Tone::Accent),
            State::Unknown => ("dialog-question-symbolic", None, "unknown, run update first", Tone::Neutral),
            State::Unreachable => ("network-error-symbolic", Some("error"), "unreachable, check its URL", Tone::Error),
        };
        r.add_prefix(&widgets::tile(icon, tone_class));
        r.add_suffix(&widgets::pill(text, tone));
        g.add(&r);
    }
    g
}
