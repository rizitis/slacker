//! Doctor: `slacker --version` and `slacker status` (the setup doctor), laid out.

use std::rc::Rc;

use adw::prelude::*;

use super::Page;
use crate::commands;
use crate::ctx::Ctx;
use crate::parse::status::{self, ItemKind, Mark, Status as Report};
use crate::resolve::Resolved;
use crate::widgets::{self, Tone};

pub fn page(ctx: &Ctx, resolved: &Resolved) -> Page {
    let (scroller, body) = widgets::page_body();

    let refresh = widgets::flat_icon_button("view-refresh-symbolic", "Check again");
    ctx.runner.watch(&refresh);

    let found = resolved.found;
    let notes = resolved.notes.clone();
    let load: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let body = body.clone();
        Rc::new(move || {
            widgets::clear(&body);
            if !found {
                body.append(&not_found(&ctx, &notes));
                return;
            }
            body.append(&widgets::loading("Checking your setup\u{2026}"));
            let (ctx2, body2, notes2) = (ctx.clone(), body.clone(), notes.clone());
            ctx.runner.capture(commands::version(), move |_, version| {
                let version = version.lines().next().unwrap_or("").trim().to_string();
                let (ctx3, body3) = (ctx2.clone(), body2.clone());
                ctx2.runner.capture(commands::status(), move |st, text| {
                    widgets::clear(&body3);
                    let report = status::parse(&text);
                    if report.sections.is_empty() {
                        body3.append(&failed(&text, &crate::runner::describe(&st, crate::commands::Privilege::User)));
                    } else {
                        render(&ctx3, &body3, &report, &version);
                    }
                    body3.append(&program_group(&ctx3, &version, &notes2));
                });
            });
        })
    };
    {
        let l = load.clone();
        refresh.connect_clicked(move |_| l());
    }

    Page {
        name: "doctor",
        title: "Doctor",
        subtitle: "slacker status",
        icon: "emblem-favorite-symbolic",
        widget: scroller.upcast(),
        header: Some(refresh.upcast()),
        load: Some(load),
        reload_on_change: true,
    }
}

fn render(ctx: &Ctx, body: &gtk::Box, report: &Report, version: &str) {
    body.append(&hero(report, version));

    for section in &report.sections {
        let problems = section.rows.iter().filter(|r| matches!(r.mark, Mark::Bad | Mark::Warn)).count();
        let desc = match problems {
            0 => String::new(),
            1 => "1 item needs attention".to_string(),
            n => format!("{n} items need attention"),
        };
        let g = widgets::group(&section.title, &desc);
        for r in &section.rows {
            let mut subtitle = r.detail.clone();
            for e in &r.extra {
                subtitle.push('\n');
                subtitle.push_str(e);
            }
            let title = if r.label.is_empty() { "\u{2003}" } else { r.label.as_str() };
            let row = widgets::row(title, &subtitle);
            row.add_prefix(&widgets::mark_tile(r.mark));
            g.add(&row);
            if r.label == status::LEFTOVERS {
                g.add(&fix_upgrade_row(ctx));
            }
        }
        body.append(&g);
    }

    let mut other: Vec<&str> = report.messages.iter().map(String::as_str).collect();
    other.extend(report.unrecognized.iter().map(String::as_str));
    if !other.is_empty() {
        let g = widgets::group("Other output", "Lines this version of the GUI does not recognise, shown as slacker printed them.");
        g.add(&widgets::raw_text(&other.join("\n")));
        body.append(&g);
    }
}

/// Under the Leftovers row: what to run, and how, to finish the upgrades.
fn fix_upgrade_row(ctx: &Ctx) -> adw::ActionRow {
    let cmd = "slacker fix-upgrade";
    let row = widgets::row(
        &format!("<tt>{cmd}</tt>"),
        "From a terminal, as root. It finishes what upgradepkg did not, with the pkgtools \
         (reinstall, removepkg of the leftover record, reinstall), using the package from your repos. \
         It shows each step and asks before it changes anything.",
    );
    // Markup only for the monospace title; the subtitle has no markup characters.
    row.set_use_markup(true);
    row.add_prefix(&widgets::tile("utilities-terminal-symbolic", None));
    let b = widgets::flat_icon_button("edit-copy-symbolic", "Copy");
    b.add_css_class("flat");
    b.set_valign(gtk::Align::Center);
    let ctx = ctx.clone();
    b.connect_clicked(move |btn| {
        btn.clipboard().set_text(cmd);
        ctx.toast_short("Copied to the clipboard");
    });
    row.add_suffix(&b);
    row
}

fn hero(report: &Report, version: &str) -> gtk::Box {
    let (good, headline, items) = match &report.verdict {
        Some(v) => (v.all_good, v.headline.clone(), v.items.clone()),
        None => (false, "slacker did not finish its report".to_string(), Vec::new()),
    };
    let problems: usize = report
        .sections
        .iter()
        .flat_map(|s| &s.rows)
        .filter(|r| matches!(r.mark, Mark::Bad))
        .count();

    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    hero.add_css_class("hero");
    hero.add_css_class(if good { "good" } else if problems > 0 { "bad" } else { "attention" });

    let big = widgets::icon(
        if good {
            "object-select-symbolic"
        } else if problems > 0 {
            "dialog-error-symbolic"
        } else {
            "dialog-warning-symbolic"
        },
        48,
    );
    big.set_valign(gtk::Align::Start);
    big.add_css_class("hero-icon");
    hero.append(&big);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
    text.set_hexpand(true);
    let caption = gtk::Label::builder().label(version.to_uppercase()).xalign(0.0).build();
    caption.add_css_class("hero-caption");
    let title = gtk::Label::builder().label(&headline).xalign(0.0).wrap(true).build();
    title.add_css_class("hero-title");
    text.append(&caption);
    text.append(&title);

    let steps: Vec<&String> = items.iter().filter(|(k, _)| *k == ItemKind::Step).map(|(_, t)| t).collect();
    if !steps.is_empty() {
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(6)
            .column_spacing(8)
            .row_spacing(8)
            .margin_top(8)
            .build();
        for (i, s) in steps.iter().enumerate() {
            let chip = gtk::Label::new(Some(&format!("{}  {}", i + 1, s)));
            chip.add_css_class("step-chip");
            flow.insert(&chip, -1);
        }
        text.append(&flow);
    }
    for (kind, t) in &items {
        if *kind == ItemKind::Step {
            continue;
        }
        let l = gtk::Label::builder().label(t).xalign(0.0).wrap(true).build();
        l.add_css_class(if *kind == ItemKind::Note { "hero-note" } else { "dim-label" });
        text.append(&l);
    }
    hero.append(&text);
    hero
}

fn program_group(ctx: &Ctx, version: &str, notes: &[String]) -> adw::PreferencesGroup {
    let g = widgets::group("Program", "");
    let path = widgets::row("slacker binary", &ctx.runner.slacker().to_string_lossy());
    path.set_subtitle_selectable(true);
    path.add_prefix(&widgets::tile("application-x-executable-symbolic", None));
    if !version.is_empty() {
        path.add_suffix(&widgets::pill(version, Tone::Neutral));
    }
    g.add(&path);
    for n in notes {
        let r = widgets::row("Note", n);
        r.add_prefix(&widgets::tile("dialog-warning-symbolic", Some("warning")));
        g.add(&r);
    }
    g
}

fn not_found(_ctx: &Ctx, notes: &[String]) -> gtk::Widget {
    widgets::status_page(
        "dialog-error-symbolic",
        "slacker was not found",
        &notes.join("\n"),
    )
    .upcast()
}

fn failed(text: &str, status: &str) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let sp = widgets::status_page("dialog-error-symbolic", "slacker status did not run", status);
    sp.set_vexpand(false);
    b.append(&sp);
    if !text.trim().is_empty() {
        b.append(&widgets::raw_text(text));
    }
    b.upcast()
}
