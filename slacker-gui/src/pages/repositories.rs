//! Repositories: `slacker list-repos`.

use std::rc::Rc;

use adw::prelude::*;

use super::Page;
use crate::commands;
use crate::ctx::Ctx;
use crate::parse::repos::{self, Repo, RepoState, Repos};
use crate::widgets::{self, Tone};

pub fn page(ctx: &Ctx) -> Page {
    let (scroller, body) = widgets::page_body();
    let refresh = widgets::flat_icon_button("view-refresh-symbolic", "Reload");

    let load: Rc<dyn Fn()> = {
        let (ctx, body) = (ctx.clone(), body.clone());
        Rc::new(move || {
            widgets::clear(&body);
            body.append(&widgets::loading("Reading the repository list\u{2026}"));
            let (ctx2, body2) = (ctx.clone(), body.clone());
            ctx.runner.capture(commands::list_repos(), move |status, text| {
                widgets::clear(&body2);
                let parsed = repos::parse(&text);
                if parsed.repos.is_empty() {
                    body2.append(&widgets::status_page(
                        "network-server-symbolic",
                        "No repositories listed",
                        &crate::runner::describe(&status, commands::Privilege::User),
                    ));
                    if !text.trim().is_empty() {
                        body2.append(&widgets::raw_text(&text));
                    }
                    return;
                }
                render(&ctx2, &body2, &parsed);
            });
        })
    };
    {
        let l = load.clone();
        refresh.connect_clicked(move |_| l());
    }

    Page {
        name: "repositories",
        title: "Repositories",
        subtitle: "slacker list-repos",
        icon: "network-server-symbolic",
        widget: scroller.upcast(),
        header: Some(refresh.upcast()),
        load: Some(load),
        reload_on_change: true,
    }
}

fn render(ctx: &Ctx, body: &gtk::Box, r: &Repos) {
    // Summary strip.
    // A flow box, so the tiles wrap onto two lines in a narrow window instead
    // of setting how narrow the window may get.
    let strip = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(4)
        .column_spacing(12)
        .row_spacing(12)
        .build();
    strip.add_css_class("stat-strip");
    let stat = |value: String, label: &str| {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.add_css_class("stat");
        b.set_hexpand(true);
        let v = gtk::Label::builder().label(&value).xalign(0.0).build();
        v.add_css_class("stat-value");
        let l = gtk::Label::builder().label(label).xalign(0.0).build();
        l.add_css_class("stat-label");
        b.append(&v);
        b.append(&l);
        strip.insert(&b, -1);
    };
    stat(r.repos.len().to_string(), "repositories");
    stat(r.tags.len().to_string(), "build-tag rules");
    if let Some(t) = r.total_installed {
        stat(t.to_string(), "packages installed");
    }
    let flagged = r.repos.iter().filter(|x| x.state != RepoState::Normal).count();
    stat(flagged.to_string(), "frozen or unreachable");
    body.append(&strip);

    let g = widgets::group(
        "Repositories",
        "Highest priority first. Click a repository to change its priority.",
    );
    let top = r.repos.iter().map(|x| x.priority).max();
    let taken: Rc<Vec<(i32, String)>> =
        Rc::new(r.repos.iter().map(|x| (x.priority, x.name.clone())).collect());
    for repo in &r.repos {
        let row = repo_row(repo, Some(repo.priority) == top);
        row.set_activatable(true);
        row.set_tooltip_text(Some("Change priority"));
        let edit = widgets::flat_icon_button("document-edit-symbolic", "Change priority");
        edit.add_css_class("flat");
        edit.set_valign(gtk::Align::Center);
        ctx.runner.watch(&edit);
        row.add_suffix(&edit);
        let open = {
            let (ctx, name, pri, taken) = (ctx.clone(), repo.name.clone(), repo.priority, taken.clone());
            move || crate::edit::priority_dialog(&ctx, &name, pri, &taken)
        };
        let o = open.clone();
        edit.connect_clicked(move |_| o());
        row.connect_activated(move |_| open());
        g.add(&row);
    }
    body.append(&g);

    if !r.tags.is_empty() {
        let g = widgets::group("Build-tag priorities", "Packages recognised by their build tag");
        for t in &r.tags {
            let row = widgets::row(&t.name, &t.tag);
            row.add_prefix(&widgets::priority_badge(t.priority, false));
            if t.unused {
                row.add_suffix(&widgets::pill("no installed package", Tone::Warning));
            }
            row.add_suffix(&widgets::dim_label(&format!("{} installed", t.installed)));
            g.add(&row);
        }
        body.append(&g);
    }

    let mut extra: Vec<(String, String)> = Vec::new();
    if let Some(o) = &r.other_tags {
        extra.push(("Installed under other build tags".into(), o.clone()));
    }
    for n in &r.notes {
        extra.push((n.clone(), String::new()));
    }
    if !extra.is_empty() {
        let g = widgets::group("Notes", "");
        for (t, s) in extra {
            let row = widgets::row(&t, &s);
            row.add_prefix(&widgets::tile("dialog-information-symbolic", None));
            g.add(&row);
        }
        body.append(&g);
    }
    if !r.unrecognized.is_empty() {
        let g = widgets::group("Other output", "Lines this version of the GUI does not recognise.");
        g.add(&widgets::raw_text(&r.unrecognized.join("\n")));
        body.append(&g);
    }
}

fn repo_row(repo: &Repo, top: bool) -> adw::ActionRow {
    let row = widgets::row(&repo.name, &repo.url);
    row.add_prefix(&widgets::priority_badge(repo.priority, top));

    let pills = gtk::Box::builder()
        .spacing(6)
        .valign(gtk::Align::Center)
        .build();
    let add = |l: gtk::Label| pills.append(&l);

    match repo.state {
        RepoState::Frozen => add(widgets::pill("frozen", Tone::Error)),
        RepoState::Unreachable => add(widgets::pill("unreachable", Tone::Warning)),
        RepoState::Normal => {}
    }
    for f in &repo.flags {
        let tone = match f.as_str() {
            "official" => Tone::Accent,
            "insecure" => Tone::Warning,
            _ => Tone::Neutral,
        };
        add(widgets::pill(f, tone));
    }
    match repo.verify.as_str() {
        "all" => add(widgets::pill("verified", Tone::Success)),
        "none" => add(widgets::pill("not verified", Tone::Error)),
        v => add(widgets::pill(&format!("verify {v}"), Tone::Warning)),
    }
    row.add_suffix(&pills);

    let count = match repo.installed {
        Some(n) => format!("{n} installed"),
        None => "no metadata".to_string(),
    };
    let c = widgets::dim_label(&count);
    c.set_width_chars(12);
    c.set_xalign(1.0);
    row.add_suffix(&c);
    row
}
