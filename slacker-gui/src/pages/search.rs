//! Search: `slacker search NAME` (one exact name), with install/remove.

use std::rc::Rc;

use adw::prelude::*;

use super::Page;
use crate::commands;
use crate::confirm::{self, Action};
use crate::ctx::Ctx;
use crate::parse::search::{self, Hit};
use crate::widgets::{self, Tone};

pub fn page(ctx: &Ctx) -> Page {
    let (scroller, body) = widgets::page_body();

    let intro = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Package name, e.g. vlc")
        .hexpand(true)
        .build();
    entry.add_css_class("search-hero");
    let hint = gtk::Label::builder()
        .label("Exact name, not case-sensitive. Press Enter to search.")
        .xalign(0.0)
        .build();
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    intro.append(&entry);
    intro.append(&hint);
    body.append(&intro);

    let results = gtk::Box::new(gtk::Orientation::Vertical, 12);
    results.append(&empty_state());
    body.append(&results);

    let last = Rc::new(std::cell::RefCell::new(None::<String>));

    let search: Rc<dyn Fn(String)> = {
        let ctx = ctx.clone();
        let results = results.clone();
        let last = last.clone();
        Rc::new(move |name: String| {
                *last.borrow_mut() = Some(name.clone());
                widgets::clear(&results);
                results.append(&widgets::loading(&format!("Searching for \u{201c}{name}\u{201d}\u{2026}")));
                let (ctx2, results2) = (ctx.clone(), results.clone());
                ctx.runner.capture(commands::search(&name), move |status, text| {
                    widgets::clear(&results2);
                    let parsed = search::parse(&text);
                    if parsed.hits.is_empty() {
                        let icon = if status.answered() { "system-search-symbolic" } else { "dialog-error-symbolic" };
                        let msg = if parsed.messages.is_empty() {
                            crate::runner::describe(&status, commands::Privilege::User)
                        } else {
                            parsed.messages.join("\n")
                        };
                        results2.append(&widgets::status_page(icon, "No results", &msg));
                        return;
                    }
                    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
                    list.add_css_class("boxed-list");
                    for hit in &parsed.hits {
                        list.append(&hit_row(&ctx2, hit));
                    }
                    results2.append(&list);
                    if !parsed.messages.is_empty() {
                        results2.append(&widgets::raw_text(&parsed.messages.join("\n")));
                    }
                });
        })
    };

    {
        let (s, hint) = (search.clone(), hint.clone());
        entry.connect_activate(move |e| match commands::single_name(&e.text()) {
            Ok(name) => {
                hint.remove_css_class("error");
                hint.set_text("Exact name, not case-sensitive. Press Enter to search.");
                s(name);
            }
            Err(msg) => {
                hint.add_css_class("error");
                hint.set_text(&msg);
            }
        });
    }
    {
        let (s, last) = (search.clone(), last.clone());
        ctx.on_system_changed(move || {
            // Take the name first: the search itself writes `last`.
            let name = last.borrow().clone();
            if let Some(name) = name {
                s(name);
            }
        });
    }

    let focus: Rc<dyn Fn()> = {
        let e = entry.clone();
        Rc::new(move || {
            // After the sidebar click has settled, or it takes focus back.
            let e = e.clone();
            gtk::glib::idle_add_local_once(move || {
                e.grab_focus();
            });
        })
    };

    Page {
        name: "search",
        title: "Search",
        subtitle: "slacker search",
        icon: "system-search-symbolic",
        widget: scroller.upcast(),
        header: None,
        load: Some(focus),
        reload_on_change: false,
    }
}

fn empty_state() -> gtk::Widget {
    widgets::status_page(
        "system-search-symbolic",
        "Search the repositories",
        "Every configured repository is searched. The result shows which one wins by priority, and which others also ship the package.",
    )
    .upcast()
}

fn hit_row(ctx: &Ctx, hit: &Hit) -> adw::ActionRow {
    let mut subtitle = search::short_summary(&hit.name, &hit.summary).to_string();
    if !hit.also.is_empty() {
        if !subtitle.is_empty() {
            subtitle.push('\n');
        }
        subtitle.push_str(&format!("Also in: {}", hit.also.join(", ")));
    }
    let row = widgets::row(&hit.name, &subtitle);
    row.set_activatable(true);
    row.add_prefix(&widgets::tile("package-x-generic-symbolic", None));

    let meta = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    meta.set_valign(gtk::Align::Center);
    meta.append(&widgets::dim_label(&hit.version));
    meta.append(&widgets::pill(&hit.source, Tone::Neutral));
    if hit.blacklisted {
        meta.append(&widgets::pill("frozen", Tone::Purple));
    }
    if let Some(repo) = ctx.pinned_repo(&hit.name) {
        meta.append(&widgets::pill(&format!("pinned to {repo}"), Tone::Accent));
    }
    if hit.installed {
        meta.append(&widgets::pill("installed", Tone::Success));
    }
    row.add_suffix(&meta);

    let names = vec![hit.name.clone()];
    // After a change the search page (and every other page) reloads through
    // the system-changed notification.
    let rerun = {
        let ctx = ctx.clone();
        move |st: &crate::runner::Status| {
            if confirm::may_have_changed(st) {
                ctx.system_changed();
            }
        }
    };
    if hit.installed {
        let b = widgets::row_button("Remove", Some("destructive-action"));
        ctx.runner.watch(&b);
        let (ctx2, names2, name) = (ctx.clone(), names.clone(), hit.name.clone());
        let rerun = rerun.clone();
        b.connect_clicked(move |_| {
            confirm::run_as_root(
                &ctx2,
                Action {
                    spec: commands::remove(&names2),
                    title: format!("Remove {name}"),
                    verb: "Remove".into(),
                    destructive: true,
                },
                rerun.clone(),
            );
        });
        row.add_suffix(&b);
    } else if !hit.blacklisted {
        let b = widgets::row_button("Install", Some("suggested-action"));
        ctx.runner.watch(&b);
        let (ctx2, names2, name) = (ctx.clone(), names.clone(), hit.name.clone());
        let rerun = rerun.clone();
        b.connect_clicked(move |_| {
            confirm::run_as_root(
                &ctx2,
                Action {
                    spec: commands::install(&names2),
                    title: format!("Install {name}"),
                    verb: "Install".into(),
                    destructive: false,
                },
                rerun.clone(),
            );
        });
        row.add_suffix(&b);
    }

    let (ctx2, name) = (ctx.clone(), hit.name.clone());
    row.connect_activated(move |_| show_info(&ctx2, &name));
    row
}

/// `slacker info NAME` in a dialog.
pub fn show_info(ctx: &Ctx, name: &str) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&widgets::loading("Reading package details\u{2026}"));
    let scroller = gtk::ScrolledWindow::builder()
        .child(&content)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.add_css_class("dialog-body");
    inner.append(&scroller);

    // What this package pulls in, each dependency on or off.
    let deps = gtk::Button::with_label("Dependencies\u{2026}");
    {
        let (ctx, name) = (ctx.clone(), name.to_string());
        deps.connect_clicked(move |_| crate::deps::dialog(&ctx, &name));
    }
    let header = adw::HeaderBar::new();
    header.pack_start(&deps);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&inner));
    let dialog = adw::Dialog::builder()
        .title(name)
        .child(&view)
        .build();
    widgets::present_fitted(&dialog, &ctx.window, (720, 560));

    ctx.runner.capture(commands::info(name), move |_, text| {
        widgets::clear(&content);
        content.append(&widgets::raw_text(&text));
    });
}
