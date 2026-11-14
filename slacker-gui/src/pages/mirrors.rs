//! Mirrors: `slacker find-mirror`, shown read-only. slacker never switches
//! the mirror itself, and neither does the GUI: the user edits the `mirrors`
//! file from the command line. Only mirrors in sync with upstream are
//! suggested, even though slacker's own list may start with stale ones.

use std::rc::Rc;

use adw::prelude::*;

use super::Page;
use crate::commands::{self, Privilege};
use crate::ctx::Ctx;
use crate::parse::mirrors::{self, FindMirror, Freshness, Suggestions};
use crate::runner::describe;
use crate::widgets::{self, Tone};

const MIRRORS_FILE: &str = "/etc/slacker/mirrors";

pub fn page(ctx: &Ctx) -> Page {
    let (scroller, body) = widgets::page_body();
    let again = widgets::flat_icon_button("view-refresh-symbolic", "Probe again");
    again.set_visible(false);
    ctx.runner.watch(&again);

    let run: Rc<dyn Fn()> = {
        let (ctx, body, again) = (ctx.clone(), body.clone(), again.clone());
        Rc::new(move || {
            widgets::clear(&body);
            body.append(&widgets::loading("Probing the official mirrors\u{2026} this takes a few seconds"));
            let (ctx2, body2, again2) = (ctx.clone(), body.clone(), again.clone());
            ctx.runner.capture(commands::find_mirror(), move |status, text| {
                widgets::clear(&body2);
                again2.set_visible(true);
                let parsed = mirrors::parse(&text);
                if parsed.ranked.is_empty() {
                    body2.append(&widgets::status_page(
                        "network-error-symbolic",
                        "No mirror could be ranked",
                        &describe(&status, Privilege::User),
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
        let r = run.clone();
        again.connect_clicked(move |_| r());
    }

    // Probing is network work; it starts only when the user asks.
    let start = widgets::status_page(
        "find-location-symbolic",
        "Find the fastest mirror",
        "slacker probes the official Slackware mirrors, checks that each is in sync with upstream, and ranks them by speed. It only suggests: your configuration is not changed.",
    );
    let go = gtk::Button::with_label("Find Mirrors");
    go.add_css_class("pill");
    go.add_css_class("suggested-action");
    go.set_halign(gtk::Align::Center);
    ctx.runner.watch(&go);
    {
        let r = run.clone();
        go.connect_clicked(move |_| r());
    }
    start.set_child(Some(&go));
    body.append(&start);

    Page {
        name: "mirrors",
        title: "Mirrors",
        subtitle: "slacker find-mirror",
        icon: "find-location-symbolic",
        widget: scroller.upcast(),
        header: Some(again.upcast()),
        load: None,
        reload_on_change: false,
    }
}

fn render(ctx: &Ctx, body: &gtk::Box, f: &FindMirror) {
    // Summary strip.
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let stat = |value: &str, label: &str| {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.add_css_class("stat");
        b.set_hexpand(true);
        let v = gtk::Label::builder().label(value).xalign(0.0).build();
        v.add_css_class("stat-value");
        let l = gtk::Label::builder().label(label).xalign(0.0).wrap(true).build();
        l.add_css_class("stat-label");
        b.append(&v);
        b.append(&l);
        strip.append(&b);
    };
    if let Some((ok, all)) = f.reachable {
        stat(&format!("{ok}/{all}"), "mirrors reachable");
    }
    if let Some(best) = f.ranked.first() {
        stat(&format!("{} ms", best.latency_ms), "fastest response");
    }
    let in_sync = f.ranked.iter().filter(|m| m.freshness == Freshness::InSync).count();
    stat(&format!("{in_sync}/{}", f.ranked.len()), "shown in sync");
    body.append(&strip);

    let desc = match (&f.validated, f.upstream_unreachable) {
        (Some(date), _) => format!("Freshness checked against upstream ({date})"),
        (None, true) => "Upstream was unreachable: ranked by speed only, freshness unknown".to_string(),
        (None, false) => String::new(),
    };
    let g = widgets::group("Fastest mirrors", &desc);
    for m in &f.ranked {
        let host = m
            .url
            .split("://")
            .nth(1)
            .and_then(|r| r.split('/').next())
            .unwrap_or(&m.url);
        let row = widgets::row(host, &m.url);
        row.set_subtitle_selectable(true);
        row.add_prefix(&widgets::priority_badge(m.rank as i32, m.rank == 1));
        let meta = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        meta.set_valign(gtk::Align::Center);
        if m.yours {
            meta.append(&widgets::pill("yours", Tone::Accent));
        }
        match &m.freshness {
            Freshness::InSync => meta.append(&widgets::pill("in sync", Tone::Success)),
            Freshness::Behind(lag) => meta.append(&widgets::pill(&format!("{lag} behind"), Tone::Warning)),
            Freshness::Unknown => meta.append(&widgets::pill("freshness unknown", Tone::Neutral)),
        }
        if !m.country.is_empty() {
            meta.append(&widgets::pill(&m.country.to_uppercase(), Tone::Neutral));
        }
        let lat = widgets::dim_label(&format!("{} ms", m.latency_ms));
        lat.set_width_chars(7);
        lat.set_xalign(1.0);
        meta.append(&lat);
        row.add_suffix(&meta);
        g.add(&row);
    }
    body.append(&g);

    match mirrors::suggestions(f, 3) {
        Suggestions::InSync(lines) => {
            body.append(&suggested_group(
                ctx,
                &lines,
                "In sync with upstream, fastest first. Use one of these as the single active line of the mirrors file. Mirrors that are behind are never suggested.",
                None,
            ));
        }
        Suggestions::Unchecked(lines) if !lines.is_empty() => {
            body.append(&suggested_group(
                ctx,
                &lines,
                "Upstream could not be reached, so these are ranked by speed only and may be out of date. Probe again later before switching.",
                Some("dialog-warning-symbolic"),
            ));
        }
        Suggestions::Unchecked(_) => {}
        Suggestions::NoneInSync => {
            let g = widgets::group("Suggested lines", "");
            let row = widgets::row(
                "No mirror is in sync right now",
                "Every fast mirror found is behind upstream. Keep your current mirror and probe again later.",
            );
            row.add_prefix(&widgets::tile("dialog-warning-symbolic", Some("warning")));
            g.add(&row);
            body.append(&g);
        }
    }

    let g = widgets::group(
        "Changing the mirror",
        "slacker does not change the mirror for you, and neither does this window. Do it from a terminal, as root:",
    );
    let steps = [
        (
            "1",
            format!("Edit {MIRRORS_FILE}"),
            "Comment out the line that is active now.".to_string(),
        ),
        (
            "2",
            "Paste one suggested line".to_string(),
            "It must be the only uncommented line in the file.".to_string(),
        ),
        (
            "3",
            "Refresh the package lists".to_string(),
            "slacker update".to_string(),
        ),
    ];
    for (n, title, sub) in steps {
        let row = widgets::row(&title, &sub);
        let badge = gtk::Label::new(Some(n));
        badge.add_css_class("pri-badge");
        badge.set_valign(gtk::Align::Center);
        row.add_prefix(&badge);
        if n == "1" {
            row.add_suffix(&copy_button(ctx, MIRRORS_FILE));
        }
        if n == "3" {
            row.add_suffix(&copy_button(ctx, "slacker update"));
        }
        g.add(&row);
    }
    body.append(&g);

    if !f.unrecognized.is_empty() {
        let g = widgets::group("Other output", "Lines this version of the GUI does not recognise.");
        g.add(&widgets::raw_text(&f.unrecognized.join("\n")));
        body.append(&g);
    }
}

fn suggested_group(ctx: &Ctx, lines: &[String], desc: &str, warn_icon: Option<&str>) -> adw::PreferencesGroup {
    let g = widgets::group("Suggested lines", desc);
    for line in lines {
        let row = widgets::row(line, "");
        row.add_css_class("mono-title");
        row.add_prefix(&match warn_icon {
            Some(i) => widgets::tile(i, Some("warning")),
            None => widgets::tile("object-select-symbolic", Some("success")),
        });
        row.add_suffix(&copy_button(ctx, line));
        g.add(&row);
    }
    g
}

fn copy_button(ctx: &Ctx, text: &str) -> gtk::Button {
    let b = widgets::flat_icon_button("edit-copy-symbolic", "Copy");
    b.add_css_class("flat");
    b.set_valign(gtk::Align::Center);
    let (ctx, text) = (ctx.clone(), text.to_string());
    b.connect_clicked(move |btn| {
        btn.clipboard().set_text(&text);
        ctx.toast_short("Copied to the clipboard");
    });
    b
}
