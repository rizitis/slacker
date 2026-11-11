//! Installed packages: `slacker history --installed`, filterable by name and
//! by source (the repository or build tag `history` attributes each one to).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::rc::Rc;

use adw::prelude::*;

use super::Page;
use crate::commands;
use crate::confirm::{self, Action};
use crate::ctx::Ctx;
use crate::parse::history::{self, Event};
use crate::parse::repos;
use crate::widgets::{self, Tone};

/// Called with a source name: shows only the packages from that source.
type OnSource = Rc<dyn Fn(&str)>;

/// Rows shown at first, and added by each "Show more": a row is several
/// widgets and its own menu, so thousands are not built unless asked for.
const SHOWN: usize = 200;

/// The line under a list that is not shown whole.
fn more_text(shown: usize, total: usize) -> String {
    format!("Showing the first {shown} of {total}.")
}

/// The label of the button that adds the next rows: how many it adds.
fn more_label(shown: usize, total: usize) -> String {
    format!("Show {} More", (total - shown).min(SHOWN))
}

/// Appends `rows[from..upto]` to `list` and rebuilds `footer`: when rows are
/// left, what is shown, a button for the next `SHOWN` and one for all the rest.
/// The buttons append to the same list, so the place in the page is kept.
#[allow(clippy::too_many_arguments)]
fn show_rows(
    ctx: &Ctx,
    list: &gtk::ListBox,
    footer: &gtk::Box,
    rows: Rc<Vec<Event>>,
    names: Rc<Option<HashSet<String>>>,
    on_source: Option<OnSource>,
    from: usize,
    upto: usize,
) {
    let total = rows.len();
    let upto = upto.min(total);
    for e in &rows[from.min(upto)..upto] {
        list.append(&package_row(ctx, e, names.as_ref().as_ref(), on_source.as_ref()));
    }
    widgets::clear(footer);
    if upto >= total {
        return;
    }
    let text = gtk::Label::new(Some(&more_text(upto, total)));
    text.add_css_class("dim-label");
    footer.append(&text);
    let button = |label: &str, next: usize| {
        let b = widgets::row_button(label, None);
        let (ctx, list, foot, rows, names, on_source) =
            (ctx.clone(), list.clone(), footer.clone(), rows.clone(), names.clone(), on_source.clone());
        b.connect_clicked(move |_| {
            // The clicked button goes away and focus moves to the new one at
            // the end, which would scroll the page there: put it back, so
            // reading goes on from the first new row.
            let adj = list
                .ancestor(gtk::ScrolledWindow::static_type())
                .and_downcast::<gtk::ScrolledWindow>()
                .map(|s| s.vadjustment());
            let at = adj.as_ref().map(|a| a.value());
            show_rows(&ctx, &list, &foot, rows.clone(), names.clone(), on_source.clone(), upto, next);
            if let (Some(adj), Some(at)) = (adj, at) {
                gtk::glib::idle_add_local_once(move || adj.set_value(at));
            }
        });
        footer.append(&b);
    };
    button(&more_label(upto, total), upto + SHOWN);
    if total - upto > SHOWN {
        button(&format!("Show All {total}"), total);
    }
}

/// The sources of the installed packages with how many each has, the largest
/// first (then by name), for the Sources menu.
fn source_counts(events: &[Event]) -> Vec<(String, usize)> {
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for e in events {
        *by.entry(e.source.as_str()).or_default() += 1;
    }
    let mut v: Vec<(String, usize)> = by.into_iter().map(|(s, n)| (s.to_string(), n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    v
}

/// Whether a package passes both filters: its name contains `needle` (already
/// lowercase; empty matches all) and its source is one of `sources` (none
/// selected matches all).
fn passes(e: &Event, needle: &str, sources: &BTreeSet<String>) -> bool {
    (needle.is_empty() || e.name.to_lowercase().contains(needle))
        && (sources.is_empty() || sources.contains(&e.source))
}

/// The line under the filter: what is shown, out of how many, and from where.
fn count_text(shown: usize, total: usize, filtered: bool, sources: &BTreeSet<String>) -> String {
    let mut text = if filtered {
        format!("{shown} of {total} packages match")
    } else {
        format!("{total} packages installed")
    };
    if !sources.is_empty() {
        let names: Vec<&str> = sources.iter().map(String::as_str).collect();
        text.push_str(&format!(" \u{00b7} {}", names.join(", ")));
    }
    text
}

/// The label of the Sources button: how many are selected, when any is.
fn sources_label(selected: usize) -> String {
    if selected == 0 {
        "Sources".to_string()
    } else {
        format!("Sources \u{00b7} {selected}")
    }
}

pub fn page(ctx: &Ctx) -> Page {
    let (scroller, body) = widgets::page_body();

    let top = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let filter = gtk::SearchEntry::builder()
        .placeholder_text("Filter installed packages")
        .hexpand(true)
        .build();
    filter.add_css_class("search-hero");
    // Next to the name filter: a menu of the sources (repositories and build
    // tags) the installed packages come from, to show only some of them.
    let sources_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let sources_pop = gtk::Popover::builder().child(&sources_box).build();
    sources_pop.add_css_class("menu");
    let sources_btn = gtk::MenuButton::builder()
        .label(sources_label(0))
        .popover(&sources_pop)
        .valign(gtk::Align::Center)
        .tooltip_text("Show only packages from the sources you choose")
        .build();
    let filter_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    filter_row.append(&filter);
    filter_row.append(&sources_btn);
    let count = gtk::Label::builder().xalign(0.0).build();
    count.add_css_class("dim-label");
    count.add_css_class("caption");
    top.append(&filter_row);
    top.append(&count);
    body.append(&top);

    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.append(&list_box);

    let data: Rc<RefCell<Vec<Event>>> = Rc::default();
    // The sources chosen in the Sources menu; none means every source.
    let selected: Rc<RefCell<BTreeSet<String>>> = Rc::default();
    // Set once `render` and `rebuild_sources` exist: a click on a row's source
    // tag shows only that source.
    let only_source: Rc<RefCell<Option<OnSource>>> = Rc::default();
    // Names of the configured repositories, from `slacker list-repos`. None
    // until read (or if it could not be read).
    let repo_names: Rc<RefCell<Option<HashSet<String>>>> = Rc::default();

    let render: Rc<dyn Fn()> = {
        let (ctx, data, list_box, filter, count, repo_names, selected, only_source, sources_btn) = (
            ctx.clone(),
            data.clone(),
            list_box.clone(),
            filter.clone(),
            count.clone(),
            repo_names.clone(),
            selected.clone(),
            only_source.clone(),
            sources_btn.clone(),
        );
        Rc::new(move || {
            widgets::clear(&list_box);
            let names = repo_names.borrow();
            let needle = filter.text().to_lowercase();
            let all = data.borrow();
            let sources = selected.borrow();
            let matches: Vec<&Event> = all.iter().filter(|e| passes(e, &needle, &sources)).collect();
            let filtered = !needle.is_empty() || !sources.is_empty();
            count.set_text(&count_text(matches.len(), all.len(), filtered, &sources));
            sources_btn.set_label(&sources_label(sources.len()));
            if matches.is_empty() {
                if !all.is_empty() {
                    let why = match (needle.is_empty(), sources.is_empty()) {
                        (false, true) => "No installed package name contains that text.",
                        (true, false) => "No installed package comes from the chosen sources.",
                        _ => "No installed package from the chosen sources has that text in its name.",
                    };
                    list_box.append(&widgets::status_page("edit-find-symbolic", "No match", why));
                }
                return;
            }
            let on_source = only_source.borrow().clone();
            let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
            list.add_css_class("boxed-list");
            list_box.append(&list);
            let footer = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(12)
                .halign(gtk::Align::Center)
                .build();
            list_box.append(&footer);
            let rows: Rc<Vec<Event>> = Rc::new(matches.into_iter().cloned().collect());
            show_rows(&ctx, &list, &footer, rows, Rc::new(names.clone()), on_source, 0, SHOWN);
        })
    };
    {
        let r = render.clone();
        filter.connect_search_changed(move |_| r());
    }

    // Fills the Sources menu from the loaded packages: one check per source
    // with its package count, and Clear. Called after every load and whenever
    // the selection changes from outside the menu.
    let rebuild_sources: Rc<dyn Fn()> = {
        let (data, selected, sources_box, sources_btn, render) =
            (data.clone(), selected.clone(), sources_box.clone(), sources_btn.clone(), render.clone());
        Rc::new(move || {
            widgets::clear(&sources_box);
            let counts = source_counts(&data.borrow());
            // A source that no longer has packages cannot stay selected: it
            // would hide everything with no way to untick it.
            selected.borrow_mut().retain(|s| counts.iter().any(|(n, _)| n == s));
            sources_btn.set_sensitive(!counts.is_empty());
            for (name, n) in &counts {
                let check = gtk::CheckButton::builder()
                    .label(name.as_str())
                    .active(selected.borrow().contains(name))
                    .hexpand(true)
                    .build();
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
                line.add_css_class("source-line");
                line.append(&check);
                line.append(&widgets::dim_label(&n.to_string()));
                let (selected, render, name, menu) =
                    (selected.clone(), render.clone(), name.clone(), sources_box.clone());
                check.connect_toggled(move |c| {
                    if c.is_active() {
                        selected.borrow_mut().insert(name.clone());
                    } else {
                        selected.borrow_mut().remove(&name);
                    }
                    // Clear (the last item) is usable only while something is ticked.
                    if let Some(b) = menu.last_child().and_downcast::<gtk::Button>() {
                        b.set_sensitive(!selected.borrow().is_empty());
                    }
                    render();
                });
                sources_box.append(&line);
            }
            if !counts.is_empty() {
                sources_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                let clear = gtk::Button::with_label("Clear");
                clear.add_css_class("flat");
                clear.set_sensitive(!selected.borrow().is_empty());
                sources_box.append(&clear);
                // Unticking the checks runs their handlers, which update the
                // selection and the list.
                let (sources_box, selected, render) = (sources_box.clone(), selected.clone(), render.clone());
                clear.connect_clicked(move |_| {
                    let mut child = sources_box.first_child();
                    while let Some(w) = child {
                        if let Some(c) = w.first_child().and_downcast::<gtk::CheckButton>() {
                            c.set_active(false);
                        }
                        child = w.next_sibling();
                    }
                    selected.borrow_mut().clear();
                    render();
                });
            }
        })
    };
    {
        let (selected, rebuild, render) = (selected.clone(), rebuild_sources.clone(), render.clone());
        *only_source.borrow_mut() = Some(Rc::new(move |source: &str| {
            {
                let mut s = selected.borrow_mut();
                s.clear();
                s.insert(source.to_string());
            }
            rebuild();
            render();
        }));
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
        let (ctx, data, list_box, render, count, repo_names, rebuild_sources) = (
            ctx.clone(),
            data.clone(),
            list_box.clone(),
            render.clone(),
            count.clone(),
            repo_names.clone(),
            rebuild_sources.clone(),
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
            let (data2, render2, list_box2, rebuild2) =
                (data.clone(), render.clone(), list_box.clone(), rebuild_sources.clone());
            ctx.runner.capture(commands::history_installed(), move |status, text| {
                let mut parsed = history::parse(&text);
                parsed.events.sort_by_key(|e| e.name.to_lowercase());
                *data2.borrow_mut() = parsed.events;
                rebuild2();
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

fn package_row(
    ctx: &Ctx,
    e: &Event,
    repo_names: Option<&HashSet<String>>,
    on_source: Option<&OnSource>,
) -> adw::ActionRow {
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
    // The source tag is a button: it shows only the packages from that source.
    let source = gtk::Button::builder()
        .child(&widgets::pill(&e.source, Tone::Neutral))
        .valign(gtk::Align::Center)
        .tooltip_text(format!("Show only packages from {}", e.source))
        .build();
    source.add_css_class("flat");
    source.add_css_class("tag-button");
    if let Some(f) = on_source {
        let (f, name) = (f.clone(), e.source.clone());
        source.connect_clicked(move |_| f(&name));
    }
    meta.append(&source);
    if let Some(repo) = ctx.pinned_repo(&e.name) {
        meta.append(&widgets::pill(&format!("pinned to {repo}"), Tone::Accent));
    }
    row.add_suffix(&meta);

    // Actions live in a small menu so the list stays calm. The menu is built
    // when it is opened, not with the row: it is most of a row's widgets, and
    // a long list has thousands of rows whose menu is never opened.
    let menu = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text("Actions")
        .build();
    menu.add_css_class("flat");
    row.add_suffix(&menu);
    let name = e.name.clone();
    {
        let (ctx, name) = (ctx.clone(), name.clone());
        menu.set_create_popup_func(move |menu| {
            let ctx = &ctx;
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
            let pinned = ctx.pinned_repo(&name);
            let pin = item(if pinned.is_some() { "Pin to Another Repository\u{2026}" } else { "Pin to a Repository\u{2026}" });
            let unpin = pinned.as_ref().map(|r| item(&format!("Remove Pin ({r})")));
            let deps = item("Dependencies\u{2026}");
            pop_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            let reinstall = from_repo.then(|| item("Reinstall\u{2026}"));
            if !from_repo {
                let note = gtk::Label::builder()
                    .label(format!("No configured repository offers {}, so it cannot be reinstalled from here.", name))
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
            menu.set_popover(Some(&popover));
        });
    }

    {
        let (ctx, name) = (ctx.clone(), name.clone());
        row.connect_activated(move |_| crate::pages::search::show_info(&ctx, &name));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::history::Kind;

    fn ev(name: &str, source: &str) -> Event {
        Event { when: String::new(), kind: Kind::Installed, name: name.into(), detail: String::new(), source: source.into() }
    }

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sources_are_counted_largest_first() {
        let all = [ev("glibc", "slackware"), ev("libfoo", "conraid"), ev("bash", "slackware"), ev("sbopkg", "SBo"), ev("zed", "conraid"), ev("tar", "slackware")];
        assert_eq!(
            source_counts(&all),
            vec![("slackware".to_string(), 3), ("conraid".to_string(), 2), ("SBo".to_string(), 1)]
        );
        // the same count: by name, without case
        let tie = [ev("a", "conraid"), ev("b", "SBo"), ev("c", "alienbob")];
        let names: Vec<String> = source_counts(&tie).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["alienbob", "conraid", "SBo"]);
        assert!(source_counts(&[]).is_empty());
    }

    #[test]
    fn name_and_source_filters_combine() {
        let e = ev("libFoo", "conraid");
        assert!(passes(&e, "", &set(&[]))); // no filter: everything
        assert!(passes(&e, "lib", &set(&[])));
        assert!(passes(&e, "foo", &set(&[]))); // the needle is lowercase, the name is not
        assert!(!passes(&e, "bar", &set(&[])));
        assert!(passes(&e, "", &set(&["conraid"])));
        assert!(passes(&e, "", &set(&["SBo", "conraid"]))); // any of the chosen sources
        assert!(!passes(&e, "", &set(&["SBo"])));
        assert!(passes(&e, "lib", &set(&["conraid"]))); // "repo:conraid lib"
        assert!(!passes(&e, "lib", &set(&["SBo"])));
        assert!(!passes(&e, "bar", &set(&["conraid"])));
        // the source is matched whole, not as text
        assert!(!passes(&ev("x", "local1"), "", &set(&["local"])));
    }

    #[test]
    fn the_count_line_says_what_is_shown() {
        assert_eq!(count_text(2279, 2279, false, &set(&[])), "2279 packages installed");
        assert_eq!(count_text(12, 2279, true, &set(&[])), "12 of 2279 packages match");
        assert_eq!(count_text(57, 2279, true, &set(&["conraid"])), "57 of 2279 packages match \u{00b7} conraid");
        assert_eq!(
            count_text(350, 2279, true, &set(&["conraid", "SBo"])),
            "350 of 2279 packages match \u{00b7} SBo, conraid"
        );
        assert_eq!(more_text(200, 2279), "Showing the first 200 of 2279.");
        assert_eq!(more_label(200, 2279), "Show 200 More");
        assert_eq!(more_label(2200, 2279), "Show 79 More"); // the last, shorter step
        assert_eq!(sources_label(0), "Sources");
        assert_eq!(sources_label(2), "Sources \u{00b7} 2");
    }
}
