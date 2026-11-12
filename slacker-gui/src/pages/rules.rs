//! Frozen rules, pins and dependency exclusions: `slacker frozen`, `slacker
//! pin` and `slacker nodep` without arguments. slacker lists these only as
//! root, so the page reads them on request, through pkexec.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use super::Page;
use crate::commands::{self, Privilege};
use crate::ctx::Ctx;
use crate::edit;
use crate::parse::rules;
use crate::runner::describe;
use crate::widgets::{self, Tone};

type Slot = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

pub fn page(ctx: &Ctx) -> Page {
    let (scroller, body) = widgets::page_body();
    let refresh = widgets::flat_icon_button("view-refresh-symbolic", "Read again");
    refresh.set_visible(false);

    // `load` is referenced by the locked view it creates on failure, so it
    // lives in a slot filled right after it is built.
    let slot: Slot = Rc::default();
    // Whether the list has been read once in this session. Until it has,
    // nothing here may run as root on its own.
    let unlocked = Rc::new(std::cell::Cell::new(false));
    let load: Rc<dyn Fn()> = {
        let (ctx, body, refresh, slot, unlocked) =
            (ctx.clone(), body.clone(), refresh.clone(), slot.clone(), unlocked.clone());
        Rc::new(move || {
            widgets::clear(&body);
            body.append(&widgets::loading("Waiting for authorization\u{2026}"));
            let (ctx2, body2, refresh2, slot2, unlocked2) =
                (ctx.clone(), body.clone(), refresh.clone(), slot.clone(), unlocked.clone());
            ctx.runner.capture(commands::frozen_list(), move |fst, ftext| {
                if !fst.answered() {
                    widgets::clear(&body2);
                    body2.append(&locked(&ctx2, &slot2, Some(&describe(&fst, Privilege::Root))));
                    return;
                }
                let (ctx3, body3, refresh3) = (ctx2.clone(), body2.clone(), refresh2.clone());
                ctx2.runner.capture(commands::pin_list(), move |pst, ptext| {
                    let (ctx4, body4, refresh4) = (ctx3.clone(), body3.clone(), refresh3.clone());
                    ctx3.runner.capture(commands::nodep_list(), move |nst, ntext| {
                        widgets::clear(&body4);
                        refresh4.set_visible(true);
                        unlocked2.set(true);
                        let frozen = rules::parse_frozen(&ftext);
                        let pins = rules::parse_pins(&ptext);
                        let nodeps = rules::parse_nodeps(&ntext);
                        let pin_error = if pst.answered() {
                            ctx4.set_pins(&pins.pins);
                            None
                        } else {
                            Some(describe(&pst, Privilege::Root))
                        };
                        let nodep_error = (!nst.answered()).then(|| describe(&nst, Privilege::Root));
                        render(&ctx4, &body4, &frozen, &pins, pin_error, &nodeps, nodep_error);
                    });
                });
            });
        })
    };
    *slot.borrow_mut() = Some(load.clone());
    {
        // A rule or pin changed, here or from another page. Read the list
        // again only if it was already on screen: re-reading needs root, and
        // a page the user never opened must not raise a password prompt of
        // its own after an unrelated change elsewhere.
        let (l, unlocked) = (load.clone(), unlocked.clone());
        ctx.on_rules_changed(move || {
            if unlocked.get() {
                l();
            }
        });
    }
    {
        let l = load.clone();
        refresh.connect_clicked(move |_| l());
    }

    // Nothing runs as root until the user asks for it.
    body.append(&locked(ctx, &slot, None));

    Page {
        name: "rules",
        title: "Frozen & Pins",
        subtitle: "slacker frozen \u{00b7} pin \u{00b7} nodep",
        icon: "changes-prevent-symbolic",
        widget: scroller.upcast(),
        header: Some(refresh.upcast()),
        load: None,
        reload_on_change: false,
    }
}

fn locked(ctx: &Ctx, slot: &Slot, error: Option<&str>) -> gtk::Widget {
    let desc = match error {
        Some(e) => format!("{e}."),
        None => "slacker shows frozen rules, pins and dependency exclusions only to root, so reading them asks for your password.".to_string(),
    };
    let sp = widgets::status_page("changes-prevent-symbolic", "Blacklist rules", &desc);
    let b = gtk::Button::with_label("Show the rules");
    b.add_css_class("pill");
    b.add_css_class("suggested-action");
    b.set_halign(gtk::Align::Center);
    ctx.runner.watch(&b);
    let slot = slot.clone();
    b.connect_clicked(move |_| {
        let f = slot.borrow().clone();
        if let Some(f) = f {
            f();
        }
    });
    sp.set_child(Some(&b));
    sp.upcast()
}

fn render(
    ctx: &Ctx,
    body: &gtk::Box,
    frozen: &rules::Frozen,
    pins: &rules::Pins,
    pin_error: Option<String>,
    nodeps: &rules::Nodeps,
    nodep_error: Option<String>,
) {
    let g = widgets::group(
        "Frozen rules",
        "Installed packages that match are kept as they are; packages that match and are not installed are left out of install-new and upgrades.",
    );
    g.set_header_suffix(Some(&add_button(ctx, "Add Rule", |c| edit::freeze_dialog(c, None))));
    if frozen.rules.is_empty() {
        let r = widgets::row("No frozen rules", "");
        r.add_prefix(&widgets::tile("changes-allow-symbolic", None));
        g.add(&r);
    }
    for rule in &frozen.rules {
        let (title, scope) = match rule.strip_prefix('@').and_then(|r| r.split_once(char::is_whitespace)) {
            Some((repo, rest)) => (rest.trim().to_string(), Some(repo.to_string())),
            None => (rule.clone(), None),
        };
        let subtitle = if title.ends_with('/') { "Whole series" } else { "" };
        let r = widgets::row(&title, subtitle);
        r.add_css_class("rule-row");
        r.add_prefix(&widgets::tile("changes-prevent-symbolic", Some("purple")));
        if let Some(repo) = scope {
            r.add_suffix(&widgets::pill(&format!("only in {repo}"), Tone::Neutral));
        }
        r.add_suffix(&remove_button(ctx, "Unfreeze", rule.clone(), edit::unfreeze));
        g.add(&r);
    }
    body.append(&g);

    let g = widgets::group(
        "Pins",
        "Each pinned package is taken only from its repository, whatever the priorities say. A freeze on the same package wins.",
    );
    if pin_error.is_none() {
        g.set_header_suffix(Some(&add_button(ctx, "Add Pin", |c| edit::pin_dialog(c, None))));
    }
    match pin_error {
        Some(e) => {
            let r = widgets::row("Pins could not be read", &e);
            r.add_prefix(&widgets::tile("dialog-error-symbolic", Some("error")));
            g.add(&r);
        }
        None if pins.pins.is_empty() => {
            let r = widgets::row("No pins", "");
            r.add_prefix(&widgets::tile("view-pin-symbolic", None));
            g.add(&r);
        }
        None => {
            for (pkg, repo) in &pins.pins {
                let r = widgets::row(pkg, "");
                r.add_prefix(&widgets::tile("view-pin-symbolic", Some("accent")));
                r.add_suffix(&widgets::pill(&format!("from {repo}"), Tone::Accent));
                r.add_suffix(&remove_button(ctx, "Unpin", pkg.clone(), edit::unpin));
                g.add(&r);
            }
        }
    }
    body.append(&g);

    let g = widgets::group(
        "Dependency exclusions",
        "Each dependency is not pulled in for the package named, or for any package. Other packages that require it still pull it in. To choose for one package, open its Dependencies\u{2026} window from Installed or from the package details.",
    );
    if nodep_error.is_none() {
        g.set_header_suffix(Some(&add_button(ctx, "Add Exclusion", edit::nodep_dialog)));
    }
    match nodep_error {
        Some(e) => {
            let r = widgets::row("Dependency exclusions could not be read", &e);
            r.add_prefix(&widgets::tile("dialog-error-symbolic", Some("error")));
            g.add(&r);
        }
        None if nodeps.rules.is_empty() => {
            let r = widgets::row("No dependency exclusions", "");
            r.add_prefix(&widgets::tile("changes-allow-symbolic", None));
            g.add(&r);
        }
        None => {
            for (pkg, dep) in &nodeps.rules {
                let whom = if pkg == "*" { "Not pulled in for any package".to_string() } else { format!("Not pulled in for {pkg}") };
                let r = widgets::row(dep, &whom);
                r.add_css_class("rule-row");
                r.add_prefix(&widgets::tile("action-unavailable-symbolic", Some("purple")));
                r.add_suffix(&remove_button(ctx, "Remove", format!("{pkg}:{dep}"), edit::undep));
                g.add(&r);
            }
        }
    }
    body.append(&g);

    let mut other: Vec<&str> = frozen.unrecognized.iter().map(String::as_str).collect();
    other.extend(pins.unrecognized.iter().map(String::as_str));
    other.extend(nodeps.unrecognized.iter().map(String::as_str));
    if !other.is_empty() {
        let g = widgets::group("Other output", "Lines this version of the GUI does not recognise.");
        g.add(&widgets::raw_text(&other.join("\n")));
        body.append(&g);
    }
}

fn add_button(ctx: &Ctx, label: &str, open: fn(&Ctx)) -> gtk::Button {
    let content = adw::ButtonContent::builder().icon_name("list-add-symbolic").label(label).build();
    let b = gtk::Button::builder().child(&content).valign(gtk::Align::Center).build();
    b.add_css_class("flat");
    ctx.runner.watch(&b);
    let ctx = ctx.clone();
    b.connect_clicked(move |_| open(&ctx));
    b
}

fn remove_button(ctx: &Ctx, label: &str, value: String, run: fn(&Ctx, &str)) -> gtk::Button {
    let b = widgets::row_button(label, Some("flat"));
    ctx.runner.watch(&b);
    let ctx = ctx.clone();
    b.connect_clicked(move |_| run(&ctx, &value));
    b
}
