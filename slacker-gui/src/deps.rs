//! A package's dependencies, each one on or off: `slacker info NAME` lists
//! them (its `requires:` line, with the ones slacker leaves out marked), and
//! turning one off writes `!NAME:DEPENDENCY` to the blacklist through
//! `slacker nodep`; turning it back on removes that line through `slacker
//! undep`. Nothing changes until Apply.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::commands;
use crate::confirm::{self, Action};
use crate::ctx::Ctx;
use crate::parse::deps::{self, Held};
use crate::widgets::{self, Tone};

/// One switch: the dependency, whether it is pulled in now, and the switch.
struct Choice {
    name: String,
    was_on: bool,
    switch: adw::SwitchRow,
}

/// The one Dependencies window: a dependency's own list opens as a page in it
/// (with a back arrow), never as a window of its own, however deep it goes.
#[derive(Clone)]
struct Window {
    nav: adw::NavigationView,
    dialog: adw::Dialog,
}

pub fn dialog(ctx: &Ctx, package: &str) {
    let nav = adw::NavigationView::new();
    let dialog = adw::Dialog::builder().title("Dependencies").child(&nav).build();
    let win = Window { nav, dialog: dialog.clone() };
    win.nav.push(&page(ctx, package, &win));
    crate::widgets::present_fitted(&dialog, &ctx.window, (640, 560));
}

/// The dependencies of `package`, as one page of the window. Cancel and Apply
/// act on this page only: on a deeper page they go back to the one before,
/// on the first they close the window.
fn page(ctx: &Ctx, package: &str, win: &Window) -> adw::NavigationPage {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.append(&widgets::loading("Reading the dependencies\u{2026}"));
    let scroller = gtk::ScrolledWindow::builder()
        .child(&content)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .build();
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.add_css_class("dialog-body");
    inner.append(&scroller);

    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("pill");
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("pill");
    apply.add_css_class("suggested-action");
    apply.set_sensitive(false);
    let bottom = gtk::Box::builder().halign(gtk::Align::End).spacing(12).build();
    bottom.add_css_class("tx-bottom");
    bottom.append(&cancel);
    bottom.append(&apply);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&inner));
    view.add_bottom_bar(&bottom);
    let page = adw::NavigationPage::builder()
        .title(format!("Dependencies of {package}"))
        .child(&view)
        .build();

    // Leave this page: back to the one before, or close the window.
    let leave = {
        let win = win.clone();
        move || {
            if win.nav.navigation_stack().n_items() > 1 {
                win.nav.pop();
            } else {
                win.dialog.close();
            }
        }
    };
    {
        let leave = leave.clone();
        cancel.connect_clicked(move |_| leave());
    }

    let choices: Rc<RefCell<Vec<Choice>>> = Rc::default();
    let pending = {
        let choices = choices.clone();
        move || -> (Vec<String>, Vec<String>) {
            let (mut off, mut on) = (Vec::new(), Vec::new());
            for c in choices.borrow().iter() {
                match (c.was_on, c.switch.is_active()) {
                    (true, false) => off.push(c.name.clone()),
                    (false, true) => on.push(c.name.clone()),
                    _ => {}
                }
            }
            (off, on)
        }
    };

    {
        let (ctx2, package2, apply2, choices2, pending2, win2) =
            (ctx.clone(), package.to_string(), apply.clone(), choices.clone(), pending.clone(), win.clone());
        ctx.runner.capture(commands::info(package), move |status, text| {
            widgets::clear(&content);
            let req = deps::parse(&text);
            if req.repos.is_empty() {
                let msg = if status.answered() {
                    format!("No configured repository offers {package2}, so slacker has no dependency list for it.")
                } else {
                    crate::runner::describe(&status, commands::Privilege::User)
                };
                content.append(&widgets::status_page("package-x-generic-symbolic", "No dependency list", &msg));
                return;
            }
            if req.deps.is_empty() {
                content.append(&widgets::status_page(
                    "object-select-symbolic",
                    "No dependencies",
                    &format!("No build of {package2} in the configured repositories lists a dependency."),
                ));
                return;
            }
            render(&ctx2, &content, &package2, &req, &choices2, &win2);
            let update = {
                let (apply, pending) = (apply2.clone(), pending2.clone());
                move || {
                    let (off, on) = pending();
                    apply.set_sensitive(!off.is_empty() || !on.is_empty());
                }
            };
            for c in choices2.borrow().iter() {
                let u = update.clone();
                c.switch.connect_active_notify(move |_| u());
            }
        });
    }

    let (ctx2, package2) = (ctx.clone(), package.to_string());
    apply.connect_clicked(move |_| {
        let (off, on) = pending();
        if off.is_empty() && on.is_empty() {
            return;
        }
        leave();
        let (ctx, package) = (ctx2.clone(), package2.clone());
        // After the page is gone, so the next window opens over the main one.
        glib::idle_add_local_once(move || run(&ctx, &package, off, on));
    });
    page
}

fn render(
    ctx: &Ctx,
    content: &gtk::Box,
    package: &str,
    req: &deps::Requires,
    choices: &Rc<RefCell<Vec<Choice>>>,
    win: &Window,
) {
    if req.resolve_off {
        let r = widgets::row(
            "RESOLVE_DEPS=no",
            "slacker.conf turns dependency resolution off, so slacker pulls in none of these. The choices below take effect once it is turned back on.",
        );
        r.add_prefix(&widgets::tile("dialog-warning-symbolic", Some("warning")));
        let g = widgets::group("", "");
        g.add(&r);
        content.append(&g);
    }

    let g = widgets::group(
        "Pulled in when it is installed or upgraded",
        &format!(
            "Turn a dependency off and slacker no longer pulls it in for {package}: \u{201c}!{package}:dependency\u{201d} is written to the blacklist. \
             Other packages that require it still pull it in, and nothing installed is removed."
        ),
    );
    let several = req.repos.len() > 1;
    for d in &req.deps {
        let mut notes: Vec<String> = Vec::new();
        if several && d.repos.len() < req.repos.len() {
            notes.push(format!("Listed by the {} build only", d.repos.join(", ")));
        }
        let lock = match &d.held {
            Some(Held::Rule(p)) if p == package => None,
            Some(Held::Rule(p)) if p == "*" => Some((
                "for any package",
                format!(
                    "Not pulled in for any package (\u{201c}!*:{}\u{201d}). Remove that rule under Frozen & Pins to change it.",
                    d.name
                ),
            )),
            Some(Held::Rule(p)) => Some(("rule", format!("Not pulled in: !{p}:{}", d.name))),
            Some(Held::Frozen) => Some(("frozen", "Frozen, so slacker never pulls it in. Unfreeze it under Frozen & Pins to change it.".to_string())),
            Some(Held::Other(why)) => Some(("not pulled in", format!("Not pulled in: {why}"))),
            None => None,
        };
        let row: adw::PreferencesRow = match lock {
            None => {
                let on = d.held.is_none();
                let s = adw::SwitchRow::builder().use_markup(false).title(&d.name).active(on).build();
                s.set_title_lines(0);
                s.set_subtitle_lines(0);
                if !notes.is_empty() {
                    s.set_subtitle(&notes.join("\n"));
                }
                s.add_prefix(&widgets::tile("package-x-generic-symbolic", None));
                s.add_suffix(&deeper(ctx, &d.name, win));
                choices.borrow_mut().push(Choice { name: d.name.clone(), was_on: on, switch: s.clone() });
                s.upcast()
            }
            Some((tag, why)) => {
                notes.push(why);
                let r = widgets::row(&d.name, &notes.join("\n"));
                r.add_prefix(&widgets::tile("changes-prevent-symbolic", Some("purple")));
                r.add_suffix(&widgets::pill(tag, Tone::Purple));
                r.add_suffix(&deeper(ctx, &d.name, win));
                r.upcast()
            }
        };
        g.add(&row);
    }
    content.append(&g);
}

/// Opens a dependency's own list, as the next page of the same window: a rule
/// only acts on the package that itself requires the dependency, so this is
/// how to find it one level down (ghostty \u{2192} blueprint-compiler
/// \u{2192} xvfb-run).
fn deeper(ctx: &Ctx, name: &str, win: &Window) -> gtk::Button {
    let b = gtk::Button::from_icon_name("go-next-symbolic");
    b.set_tooltip_text(Some(&format!("Dependencies of {name}")));
    b.set_valign(gtk::Align::Center);
    b.add_css_class("flat");
    let (ctx, name, win) = (ctx.clone(), name.to_string(), win.clone());
    b.connect_clicked(move |_| win.nav.push(&page(&ctx, &name, &win)));
    b
}

/// `nodep` for the dependencies turned off (slacker lists them and asks),
/// then `undep` for the ones turned back on (the GUI asks first, since
/// slacker does not).
fn run(ctx: &Ctx, package: &str, off: Vec<String>, on: Vec<String>) {
    let rules = |names: &[String]| -> Vec<String> {
        names.iter().filter_map(|d| commands::dep_rule(package, d).ok()).collect()
    };
    let (off_rules, on_rules) = (rules(&off), rules(&on));
    let off_title = format!("Stop pulling in {} for {package}", list(&off));
    let package = package.to_string();
    let undep = {
        let ctx = ctx.clone();
        move || {
            if on_rules.is_empty() {
                return;
            }
            confirm::run_as_root(
                &ctx,
                Action {
                    spec: commands::undep(&on_rules),
                    title: format!("Pull {} in again for {package}", list(&on)),
                    verb: "Allow".into(),
                    destructive: false,
                },
                after_change(&ctx),
            );
        }
    };
    if off_rules.is_empty() {
        undep();
        return;
    }
    let ctx2 = ctx.clone();
    confirm::run_as_root(
        ctx,
        Action { spec: commands::nodep(&off_rules), title: off_title, verb: "Write".into(), destructive: false },
        move |st| {
            after_change(&ctx2)(st);
            if st.code() == Some(0) {
                undep();
            }
        },
    );
}

fn list(names: &[String]) -> String {
    names.iter().map(|n| format!("\u{201c}{n}\u{201d}")).collect::<Vec<_>>().join(", ")
}

fn after_change(ctx: &Ctx) -> impl FnOnce(&crate::runner::Status) + 'static {
    let ctx = ctx.clone();
    move |st| {
        if confirm::may_have_changed(st) {
            ctx.rules_changed();
        }
    }
}
