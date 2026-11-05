//! Installed packages: `slacker history --installed`, filterable.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;

use super::Page;
use crate::commands;
use crate::confirm::{self, Action};
use crate::ctx::Ctx;
use crate::parse::history::{self, Event};
use crate::parse::repos;
use crate::widgets::{self, Tone};

/// Rows shown at once; the filter narrows the rest down.
const SHOWN: usize = 200;

pub fn page(ctx: &Ctx) -> Page {
    let (scroller, body) = widgets::page_body();

    let top = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let filter = gtk::SearchEntry::builder()
        .placeholder_text("Filter installed packages")
        .hexpand(true)
        .build();
    filter.add_css_class("search-hero");
    let count = gtk::Label::builder().xalign(0.0).build();
    count.add_css_class("dim-label");
    count.add_css_class("caption");
    top.append(&filter);
    top.append(&count);
    body.append(&top);

    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.append(&list_box);

    let data: Rc<RefCell<Vec<Event>>> = Rc::default();
    // Names of the configured repositories, from `slacker list-repos`. None
    // until read (or if it could not be read).
    let repo_names: Rc<RefCell<Option<HashSet<String>>>> = Rc::default();

    let render: Rc<dyn Fn()> = {
        let (ctx, data, list_box, filter, count, repo_names) = (
            ctx.clone(),
            data.clone(),
            list_box.clone(),
            filter.clone(),
            count.clone(),
            repo_names.clone(),
        );
        Rc::new(move || {
            widgets::clear(&list_box);
            let names = repo_names.borrow();
            let needle = filter.text().to_lowercase();
            let all = data.borrow();
            let matches: Vec<&Event> = all
                .iter()
                .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
                .collect();
            count.set_text(&if needle.is_empty() {
                format!("{} packages installed", all.len())
            } else {
                format!("{} of {} packages match", matches.len(), all.len())
            });
            if matches.is_empty() {
                if !all.is_empty() {
                    list_box.append(&widgets::status_page("edit-find-symbolic", "No match", "No installed package name contains that text."));
                }
                return;
            }
            let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
            list.add_css_class("boxed-list");
            for e in matches.iter().take(SHOWN) {
                list.append(&package_row(&ctx, e, names.as_ref()));
            }
            list_box.append(&list);
            if matches.len() > SHOWN {
                let more = gtk::Label::new(Some(&format!(
                    "Showing the first {SHOWN}. Type in the filter to find the rest."
                )));
                more.add_css_class("dim-label");
                list_box.append(&more);
            }
        })
    };
    {
        let r = render.clone();
        filter.connect_search_changed(move |_| r());
    }
    {
        // Pins become known once the Frozen & pins page has been read.
        let (r, data) = (render.clone(), data.clone());
        ctx.on_pins_changed(move || {
            if !data.borrow().is_empty() {
                r();
            }
        });
    }

    let refresh = widgets::flat_icon_button("view-refresh-symbolic", "Reload");
    let load: Rc<dyn Fn()> = {
        let (ctx, data, list_box, render, count, repo_names) = (
            ctx.clone(),
            data.clone(),
            list_box.clone(),
            render.clone(),
            count.clone(),
            repo_names.clone(),
        );
        Rc::new(move || {
            widgets::clear(&list_box);
            count.set_text("");
            list_box.append(&widgets::loading("Reading the package database\u{2026}"));
            // The repository names decide which packages can be reinstalled:
            // `history` labels each package with the repo that serves its
            // build, or with its build tag (e.g. SBo) when no repo does.
            let names = repo_names.clone();
            ctx.runner.capture(commands::list_repos(), move |status, text| {
                let parsed = repos::parse(&text);
                *names.borrow_mut() = (status.answered() && !parsed.repos.is_empty())
                    .then(|| parsed.repos.into_iter().map(|r| r.name).collect());
            });
            let (data2, render2, list_box2) = (data.clone(), render.clone(), list_box.clone());
            ctx.runner.capture(commands::history_installed(), move |status, text| {
                let mut parsed = history::parse(&text);
                parsed.events.sort_by_key(|e| e.name.to_lowercase());
                *data2.borrow_mut() = parsed.events;
                render2();
                if data2.borrow().is_empty() {
                    widgets::clear(&list_box2);
                    let msg = if parsed.messages.is_empty() {
                        crate::runner::describe(&status, commands::Privilege::User)
                    } else {
                        parsed.messages.join("\n")
                    };
                    list_box2.append(&widgets::status_page("package-x-generic-symbolic", "No installed packages listed", &msg));
                } else if !parsed.messages.is_empty() {
                    list_box2.append(&widgets::raw_text(&parsed.messages.join("\n")));
                }
            });
        })
    };
    {
        let l = load.clone();
        refresh.connect_clicked(move |_| l());
    }

    Page {
        name: "packages",
        title: "Installed",
        subtitle: "slacker history --installed",
        icon: "package-x-generic-symbolic",
        widget: scroller.upcast(),
        header: Some(refresh.upcast()),
        load: Some(load),
        reload_on_change: true,
    }
}

fn package_row(ctx: &Ctx, e: &Event, repo_names: Option<&HashSet<String>>) -> adw::ActionRow {
    // A package no configured repository serves (an SBo or local build) has
    // nothing to be reinstalled from. If the list is unknown, keep the
    // action and let slacker answer.
    let from_repo = repo_names.is_none_or(|n| n.contains(&e.source));
    let row = widgets::row(&e.name, &format!("Installed {}", e.when));
    row.set_activatable(true);
    row.add_prefix(&widgets::tile("package-x-generic-symbolic", None));
    let meta = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    meta.set_valign(gtk::Align::Center);
    meta.append(&widgets::dim_label(&e.detail));
    meta.append(&widgets::pill(&e.source, Tone::Neutral));
    if let Some(repo) = ctx.pinned_repo(&e.name) {
        meta.append(&widgets::pill(&format!("pinned to {repo}"), Tone::Accent));
    }
    row.add_suffix(&meta);

    // Actions live in a small menu so the list stays calm.
    let pop_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let popover = gtk::Popover::builder().child(&pop_box).build();
    popover.add_css_class("menu");
    let item = |label: &str| {
        let b = gtk::Button::with_label(label);
        b.add_css_class("flat");
        if let Some(l) = b.child().and_downcast::<gtk::Label>() {
            l.set_xalign(0.0);
        }
        pop_box.append(&b);
        b
    };
    let details = item("Details");
    let freeze = item("Freeze\u{2026}");
    let pinned = ctx.pinned_repo(&e.name);
    let pin = item(if pinned.is_some() { "Pin to Another Repository\u{2026}" } else { "Pin to a Repository\u{2026}" });
    let unpin = pinned.as_ref().map(|r| item(&format!("Remove Pin ({r})")));
    let deps = item("Dependencies\u{2026}");
    pop_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let reinstall = from_repo.then(|| item("Reinstall\u{2026}"));
    if !from_repo {
        let note = gtk::Label::builder()
            .label(format!("No configured repository offers {}, so it cannot be reinstalled from here.", e.name))
            .wrap(true)
            .max_width_chars(34)
            .xalign(0.0)
            .build();
        note.add_css_class("dim-label");
        note.add_css_class("caption");
        note.add_css_class("menu-note");
        pop_box.append(&note);
    }
    let remove = item("Remove\u{2026}");
    remove.add_css_class("error");
    if let Some(b) = &reinstall {
        ctx.runner.watch(b);
    }
    ctx.runner.watch(&remove);

    let menu = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .popover(&popover)
        .valign(gtk::Align::Center)
        .tooltip_text("Actions")
        .build();
    menu.add_css_class("flat");
    row.add_suffix(&menu);

    let name = e.name.clone();
    {
        let (ctx, name, p) = (ctx.clone(), name.clone(), popover.clone());
        details.connect_clicked(move |_| {
            p.popdown();
            crate::pages::search::show_info(&ctx, &name);
        });
    }
    {
        let (ctx, name, p) = (ctx.clone(), name.clone(), popover.clone());
        freeze.connect_clicked(move |_| {
            p.popdown();
            crate::edit::freeze_dialog(&ctx, Some(&name));
        });
    }
    {
        let (ctx, name, p) = (ctx.clone(), name.clone(), popover.clone());
        pin.connect_clicked(move |_| {
            p.popdown();
            crate::edit::pin_dialog(&ctx, Some(&name));
        });
    }
    if let Some(b) = &unpin {
        let (ctx, name, p) = (ctx.clone(), name.clone(), popover.clone());
        b.connect_clicked(move |_| {
            p.popdown();
            crate::edit::unpin(&ctx, &name);
        });
    }
    {
        let (ctx, name, p) = (ctx.clone(), name.clone(), popover.clone());
        deps.connect_clicked(move |_| {
            p.popdown();
            crate::deps::dialog(&ctx, &name);
        });
    }
    for b in [&freeze, &pin].into_iter().chain(unpin.iter()) {
        ctx.runner.watch(b);
    }
    let root_action = |button: &gtk::Button, verb: &'static str, destructive: bool, make: fn(&[String]) -> commands::Spec| {
        let (ctx, name, p) = (ctx.clone(), name.clone(), popover.clone());
        button.connect_clicked(move |_| {
            p.popdown();
            let ctx2 = ctx.clone();
            confirm::run_as_root(
                &ctx,
                Action {
                    spec: make(std::slice::from_ref(&name)),
                    title: format!("{verb} {name}"),
                    verb: verb.to_string(),
                    destructive,
                },
                move |st| {
                    if confirm::may_have_changed(st) {
                        ctx2.system_changed();
                    }
                },
            );
        });
    };
    if let Some(b) = &reinstall {
        root_action(b, "Reinstall", false, commands::reinstall);
    }
    root_action(&remove, "Remove", true, commands::remove);

    {
        let (ctx, name) = (ctx.clone(), name.clone());
        row.connect_activated(move |_| crate::pages::search::show_info(&ctx, &name));
    }
    row
}
