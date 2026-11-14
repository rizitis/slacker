//! Main window: sidebar with the pages, a Refresh button, a details window
//! per application or runtime, and the actions that start a transaction
//! (update, uninstall, remove unused runtimes) in txn.rs.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::catalog::{self, Catalog, Entry};
use crate::checks::{self, Helper};
use crate::commands::{self, Inst};
use crate::parse::{self, appstream::GROUPS, list::Item, remotes::Remote, search::Hit, updates::Update};
use crate::runner;
use crate::txn;

/// Who draws the window frame. Same rule as slacker-gui, without its
/// environment variable: GNOME gets the app's own frame (its compositor draws
/// none on Wayland), every other desktop the window manager's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Frame {
    System,
    App,
}

fn frame_mode(desktop: Option<&str>) -> Frame {
    let gnome = desktop.is_some_and(|d| d.split(':').any(|p| p.trim().eq_ignore_ascii_case("gnome")));
    if gnome {
        Frame::App
    } else {
        Frame::System
    }
}

/// Everything one refresh learned.
#[derive(Default)]
struct Data {
    flatpak: Option<PathBuf>,
    version: Option<String>,
    installations: Vec<String>,
    apps: Vec<Item>,
    runtimes: Vec<Item>,
    remotes: Vec<Remote>,
    /// (command, what it printed on stderr or why it failed)
    notes: Vec<(String, String)>,
    /// Every command run, as typed.
    ran: Vec<String>,
    /// `flatpak --default-arch`, e.g. x86_64.
    arch: String,
}

struct Ui {
    window: gtk::ApplicationWindow,
    frame: Frame,
    overview: gtk::Box,
    apps: gtk::Box,
    runtimes: gtk::Box,
    remotes: gtk::Box,
    updates: gtk::Box,
    search: gtk::Box,
    search_entry: gtk::SearchEntry,
    browse: gtk::Box,
    /// The Browse catalog; None while it is being read.
    catalog: RefCell<Option<Catalog>>,
    browse_group: RefCell<String>,
    browse_limit: Cell<usize>,
    found: RefCell<Option<Found>>,
    spinner: gtk::Spinner,
    refresh: gtk::Button,
    busy: Cell<bool>,
    data: RefCell<Data>,
    /// The last check for updates, if any since the last change.
    checked: RefCell<Option<Checked>>,
    checking: Cell<bool>,
}

/// What the last search found.
struct Found {
    text: String,
    hits: Vec<Hit>,
    note: Option<String>,
    ran: String,
}

/// What one check for updates found.
#[derive(Default)]
struct Checked {
    found: Vec<(Inst, Update)>,
    notes: Vec<(String, String)>,
    ran: Vec<String>,
}

/// Browse comes first: the window opens on the applications, with their
/// icons. Overview (checks and messages) is last.
const PAGES: &[(&str, &str, &str)] = &[
    ("browse", "Browse", "view-grid-symbolic"),
    ("search", "Search", "system-search-symbolic"),
    ("apps", "Installed", "view-app-grid-symbolic"),
    ("updates", "Updates", "software-update-available-symbolic"),
    ("runtimes", "Runtimes", "application-x-addon-symbolic"),
    ("remotes", "Remotes", "network-server-symbolic"),
    ("overview", "Overview", "dialog-information-symbolic"),
];

/// Cards shown at first, and added by each Show More.
const BROWSE_STEP: usize = 60;

pub fn build(app: &adw::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Slacker Flatpak")
        .default_width(1100)
        .default_height(740)
        .build();
    let frame = frame_mode(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref());
    if frame == Frame::App {
        window.set_titlebar(Some(&gtk::Box::builder().visible(false).build()));
    }

    let page_box = || gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();
    let ui = Rc::new(Ui {
        window: window.clone(),
        frame,
        overview: page_box(),
        apps: page_box(),
        runtimes: page_box(),
        remotes: page_box(),
        updates: page_box(),
        search: page_box(),
        search_entry: gtk::SearchEntry::builder()
            .placeholder_text("Search the applications your remotes offer")
            .hexpand(true)
            .build(),
        found: RefCell::default(),
        browse: page_box(),
        catalog: RefCell::default(),
        browse_group: RefCell::new("All".to_string()),
        browse_limit: Cell::new(BROWSE_STEP),
        spinner: gtk::Spinner::builder().visible(false).tooltip_text("flatpak is running").build(),
        refresh: gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Refresh").build(),
        busy: Cell::new(false),
        data: RefCell::default(),
        checked: RefCell::default(),
        checking: Cell::new(false),
    });

    let stack = gtk::Stack::builder()
        .hhomogeneous(false)
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(150)
        .build();
    for ((name, _, _), content) in PAGES.iter().zip([&ui.browse, &ui.search, &ui.apps, &ui.updates, &ui.runtimes, &ui.remotes, &ui.overview]) {
        let clamp = adw::Clamp::builder().maximum_size(980).child(content).build();
        clamp.set_margin_top(18);
        clamp.set_margin_bottom(18);
        clamp.set_margin_start(12);
        clamp.set_margin_end(12);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&clamp)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        stack.add_named(&scroller, Some(name));
    }

    let content_title = adw::WindowTitle::new("Browse", "");
    let content_header = adw::HeaderBar::builder().title_widget(&content_title).build();
    content_header.set_show_start_title_buttons(frame == Frame::App);
    content_header.set_show_end_title_buttons(frame == Frame::App);
    content_header.pack_start(&ui.refresh);
    content_header.pack_end(&ui.spinner);
    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&content_header);
    content_view.set_content(Some(&stack));
    let content_page = adw::NavigationPage::builder().title("Browse").tag("content").child(&content_view).build();

    let nav = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
    nav.add_css_class("navigation-sidebar");
    for (name, title, icon) in PAGES {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.append(&gtk::Image::from_icon_name(icon));
        b.append(&gtk::Label::builder().label(*title).xalign(0.0).hexpand(true).build());
        let row = gtk::ListBoxRow::builder().child(&b).build();
        row.set_widget_name(name);
        nav.append(&row);
    }
    let sidebar_header = adw::HeaderBar::builder().title_widget(&gtk::Label::new(Some("slacker-flatpak"))).build();
    sidebar_header.set_show_start_title_buttons(frame == Frame::App);
    sidebar_header.set_show_end_title_buttons(frame == Frame::App);
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(&nav));
    let sidebar_page =
        adw::NavigationPage::builder().title("Slacker Flatpak").tag("sidebar").child(&sidebar_view).build();

    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content_page)
        .min_sidebar_width(200.0)
        .max_sidebar_width(240.0)
        .build();
    // Same sizes as slacker-gui: a narrow window shows the sidebar and the
    // page one at a time.
    let bin = adw::BreakpointBin::builder().child(&split).width_request(540).height_request(420).build();
    let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 760sp").expect("valid breakpoint"));
    bp.add_setter(&split, "collapsed", Some(&true.to_value()));
    bin.add_breakpoint(bp);
    window.set_child(Some(&bin));

    {
        let (stack, content_title, content_page, split) =
            (stack.clone(), content_title.clone(), content_page.clone(), split.clone());
        nav.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let name = row.widget_name();
            if let Some((_, title, _)) = PAGES.iter().find(|(n, _, _)| *n == name.as_str()) {
                stack.set_visible_child_name(&name);
                content_title.set_title(title);
                content_page.set_title(title);
                split.set_show_content(true);
            }
        });
    }
    nav.select_row(nav.row_at_index(0).as_ref());

    {
        let ui = ui.clone();
        ui.refresh.clone().connect_clicked(move |_| refresh(&ui));
    }
    {
        let ui2 = ui.clone();
        ui.search_entry.connect_activate(move |e| run_search(&ui2, e.text().trim()));
    }

    window.present();
    refresh(&ui);
}

fn refresh(ui: &Rc<Ui>) {
    if ui.busy.replace(true) {
        return;
    }
    ui.refresh.set_sensitive(false);
    ui.spinner.set_visible(true);
    ui.spinner.start();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let data = load().await;
        *ui.data.borrow_mut() = data;
        *ui.catalog.borrow_mut() = None;
        render(&ui);
        // The catalog is read from disk off the main loop; Browse shows
        // "Reading…" until then.
        let (b, r, a) = {
            let d = ui.data.borrow();
            let remotes: Vec<String> = d.remotes.iter().filter(|r| !r.disabled()).map(|r| r.name.clone()).collect();
            (bases(&d), remotes, d.arch.clone())
        };
        let cat = gio::spawn_blocking(move || catalog::load(&b, &r, &a)).await.unwrap_or_default();
        *ui.catalog.borrow_mut() = Some(cat);
        render_browse(&ui);
        ui.spinner.stop();
        ui.spinner.set_visible(false);
        ui.refresh.set_sensitive(true);
        ui.busy.set(false);
    });
}

async fn load() -> Data {
    let mut d = Data { flatpak: checks::flatpak_path(), ..Default::default() };
    let Some(fp) = d.flatpak.clone() else { return d };

    if let Some(out) = step(&fp, commands::version(), &mut d).await {
        d.version = parse::version(&out);
    }
    if let Some(out) = step(&fp, commands::installations(), &mut d).await {
        d.installations = parse::installations(&out);
    }
    if let Some(out) = step(&fp, commands::list_apps(), &mut d).await {
        d.apps = parse::list::parse(&out);
    }
    if let Some(out) = step(&fp, commands::list_runtimes(), &mut d).await {
        d.runtimes = parse::list::parse(&out);
    }
    if let Some(out) = step(&fp, commands::remotes(), &mut d).await {
        d.remotes = parse::remotes::parse(&out);
    }
    if let Some(out) = step(&fp, commands::default_arch(), &mut d).await {
        d.arch = out.trim().to_string();
    }
    for v in [&mut d.apps, &mut d.runtimes] {
        v.sort_by(|a, b| {
            a.title().to_lowercase().cmp(&b.title().to_lowercase()).then(a.installation.cmp(&b.installation))
        });
    }
    d
}

/// Runs one command. Anything on stderr is kept as a note even when the
/// command succeeds: flatpak can print "error:" and still exit 0.
async fn step(fp: &std::path::Path, argv: Vec<String>, d: &mut Data) -> Option<String> {
    let shown = commands::display(&argv);
    d.ran.push(shown.clone());
    let out = runner::run(fp, &argv).await;
    let err = out.stderr.trim();
    if !err.is_empty() {
        d.notes.push((shown.clone(), err.to_string()));
    }
    if out.ok {
        Some(out.stdout)
    } else {
        if err.is_empty() {
            d.notes.push((shown, "failed without a message".to_string()));
        }
        None
    }
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn render(ui: &Rc<Ui>) {
    let d = ui.data.borrow();
    for b in [&ui.overview, &ui.apps, &ui.runtimes, &ui.remotes] {
        clear(b);
    }
    render_overview(&ui.overview, &d);
    render_items(ui, &ui.apps, &d.apps, false);
    render_items(ui, &ui.runtimes, &d.runtimes, true);
    render_remotes(&ui.remotes, &d);
    drop(d);
    render_updates(ui);
    render_search(ui);
    render_browse(ui);
}

fn group(title: &str, description: Option<&str>) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::builder().title(glib::markup_escape_text(title)).build();
    if let Some(desc) = description {
        g.set_description(Some(&glib::markup_escape_text(desc)));
    }
    g
}

fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(glib::markup_escape_text(title))
        .subtitle(glib::markup_escape_text(subtitle))
        .subtitle_selectable(true)
        .build()
}

fn tag(text: &str, warn: bool) -> gtk::Label {
    let l = gtk::Label::builder().label(text).valign(gtk::Align::Center).build();
    l.add_css_class("caption");
    l.add_css_class(if warn { "warning" } else { "dim-label" });
    l
}

/// The commands a page shows, as typed, under the page.
fn commands_footer(lines: &[String]) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(lines.join("\n"))
        .xalign(0.0)
        .wrap(true)
        // A command line has few spaces; let it break anywhere rather than
        // make the page wider than a narrow window.
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .build();
    l.add_css_class("monospace");
    l.add_css_class("dim-label");
    l.add_css_class("caption");
    l
}

fn render_overview(page: &gtk::Box, d: &Data) {
    let Some(fp) = &d.flatpak else {
        page.append(
            &adw::StatusPage::builder()
                .icon_name("dialog-warning-symbolic")
                .title("flatpak is not installed")
                .description("flatpak is not part of Slackware. It is available from third-party repositories; \
                              slacker search flatpak shows which of yours offer it.")
                .build(),
        );
        return;
    };

    let g = group("Flatpak", None);
    g.add(&row("Version", d.version.as_deref().unwrap_or("unknown")));
    g.add(&row("Program", &fp.to_string_lossy()));
    let (helper_text, helper_warn) = match checks::helper() {
        Helper::Present(p) => (format!("present: {}", p.display()), false),
        Helper::NoService => (
            format!("not found ({} is missing). Changes to system-wide Flatpak need root.", checks::HELPER_SERVICE),
            true,
        ),
        Helper::Missing(p) => (format!("the service names {} but it is not there", p.display()), true),
    };
    let helper_row = row("System helper", &helper_text);
    if helper_warn {
        helper_row.add_suffix(&tag("warning", true));
    }
    g.add(&helper_row);
    let mut inst = vec![format!("your user: {}", checks::user_data_home().join("flatpak").display())];
    inst.extend(d.installations.iter().map(|i| format!("system-wide: {i}")));
    g.add(&row("Installations", &inst.join("\n")));
    page.append(&g);

    let count = |v: &[Item], i: Inst| v.iter().filter(|x| x.inst() == Some(i)).count();
    let g = group("Installed", None);
    for (label, v) in [("Applications", &d.apps), ("Runtimes", &d.runtimes)] {
        g.add(&row(label, &format!("{} for your user, {} system-wide", count(v, Inst::User), count(v, Inst::System))));
    }
    g.add(&row("Remotes", &d.remotes.len().to_string()));
    page.append(&g);

    // Things worth a look.
    let mut warnings: Vec<(String, String)> = Vec::new();
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let missing = checks::missing_exports(&dirs, &checks::user_data_home(), &d.installations);
    if !missing.is_empty() {
        let list: Vec<String> = missing.iter().map(|p| p.display().to_string()).collect();
        warnings.push((
            "Not on XDG_DATA_DIRS".into(),
            format!(
                "{}\nFlatpak applications from here may not show in the desktop menus. \
                 /etc/profile.d/flatpak.sh adds them, but only for login shells.",
                list.join("\n")
            ),
        ));
    }
    for g in parse::list::duplicates(&d.apps) {
        let parts: Vec<String> =
            g.iter()
                .map(|i| {
                    let place = i.inst().map(Inst::words).unwrap_or(i.installation.as_str());
                    format!("{place} {}", if i.version.is_empty() { &i.active } else { &i.version })
                })
                .collect();
        let same = if parse::list::differ(&g) { "different versions" } else { "same commit" };
        warnings.push((
            format!("{} is installed twice", g[0].title()),
            format!("{} ({}): {}", g[0].id, same, parts.join(", ")),
        ));
    }
    for r in &d.remotes {
        if r.no_gpg() {
            let place = Inst::from_label(&r.installation).map(Inst::words).unwrap_or(r.installation.as_str());
            warnings.push((format!("Remote {} ({place})", r.name), "GPG verification is off".into()));
        }
    }
    if !warnings.is_empty() {
        let g = group("Worth a look", None);
        for (t, s) in &warnings {
            let r = row(t, s);
            r.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
            g.add(&r);
        }
        page.append(&g);
    }

    if !d.notes.is_empty() {
        let g = group("Messages from flatpak", Some("As printed, with the command that printed them."));
        for (cmd, text) in &d.notes {
            g.add(&row(cmd, text));
        }
        page.append(&g);
    }

    page.append(&commands_footer(&d.ran));
}

fn render_items(ui: &Rc<Ui>, page: &gtk::Box, items: &[Item], runtimes: bool) {
    let (empty, argv) = if runtimes {
        ("No runtimes installed.", commands::list_runtimes())
    } else {
        ("No applications installed.", commands::list_apps())
    };
    if items.is_empty() {
        page.append(&adw::StatusPage::builder().icon_name("folder-symbolic").title(empty).build());
    }
    let dups = parse::list::duplicates(items);
    for inst in [Inst::User, Inst::System] {
        let mine: Vec<&Item> = items.iter().filter(|i| i.inst() == Some(inst)).collect();
        if mine.is_empty() {
            continue;
        }
        let g = group(&heading(&ui.data.borrow().installations, inst), Some(&format!("{} installed", mine.len())));
        if runtimes {
            let b = gtk::Button::builder()
                .label("Remove Unused\u{2026}")
                .tooltip_text(commands::display(&commands::uninstall_unused(inst)))
                .valign(gtk::Align::Center)
                .build();
            let ui2 = ui.clone();
            b.connect_clicked(move |_| {
                start(
                    &ui2,
                    txn::Spec {
                        title: format!("Remove unused runtimes, {}", inst.words()),
                        inst,
                        argv: commands::uninstall_unused(inst),
                    },
                )
            });
            g.set_header_suffix(Some(&b));
        }
        for item in mine {
            let mut sub = item.id.clone();
            for part in [&item.version, &item.branch, &item.origin] {
                if !part.is_empty() {
                    sub.push_str("  ·  ");
                    sub.push_str(part);
                }
            }
            let r = row(item.title(), &sub);
            if !runtimes {
                r.add_prefix(&installed_image(&ui.data.borrow(), item, inst));
            }
            // A selectable subtitle would take the click meant for the row.
            r.set_subtitle_selectable(false);
            r.set_activatable(true);
            if dups.iter().any(|g| g.iter().any(|x| std::ptr::eq(*x, item))) {
                let other = if inst == Inst::User { Inst::System } else { Inst::User };
                r.add_suffix(&tag(&format!("also in {}", other.words()), true));
            }
            r.add_suffix(&tag(&item.size, false));
            r.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let (ui2, item2) = (ui.clone(), item.clone());
            r.connect_activated(move |_| details(&ui2, &item2, inst));
            g.add(&r);
        }
        page.append(&g);
    }
    // Rows from another system installation (not "system" or "user").
    let other: Vec<&Item> = items.iter().filter(|i| i.inst().is_none()).collect();
    if !other.is_empty() {
        let g = group("Other installations", Some("Details are not shown for these yet."));
        for item in other {
            g.add(&row(item.title(), &format!("{}  ·  {}", item.id, item.installation)));
        }
        page.append(&g);
    }
    page.append(&commands_footer(&[commands::display(&argv)]));
}

fn render_remotes(page: &gtk::Box, d: &Data) {
    if d.remotes.is_empty() {
        page.append(
            &adw::StatusPage::builder()
                .icon_name("network-server-symbolic")
                .title("No remotes")
                .description("To add Flathub for your user:\nflatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo")
                .build(),
        );
    }
    for inst in [Inst::User, Inst::System] {
        let mine: Vec<&Remote> = d.remotes.iter().filter(|r| r.installation == inst.label()).collect();
        if mine.is_empty() {
            continue;
        }
        let g = group(&heading(&d.installations, inst), None);
        for r in mine {
            let title = if r.title.is_empty() { r.name.clone() } else { format!("{} ({})", r.name, r.title) };
            let ar = row(&title, &r.url);
            if r.no_gpg() {
                ar.add_suffix(&tag("no GPG verification", true));
            }
            if r.disabled() {
                ar.add_suffix(&tag("disabled", false));
            }
            ar.add_suffix(&tag(&format!("priority {}", r.priority), false));
            g.add(&ar);
        }
        page.append(&g);
    }
    page.append(&commands_footer(&[commands::display(&commands::remotes())]));
}

/// A group title that says which Flatpak installation and where it is.
fn heading(system_installs: &[String], inst: Inst) -> String {
    match inst {
        Inst::User => {
            let path = checks::user_data_home().join("flatpak");
            let home = std::env::var("HOME").unwrap_or_default();
            let shown = match path.to_string_lossy().strip_prefix(home.as_str()) {
                Some(rest) if !home.is_empty() => format!("~{rest}"),
                _ => path.display().to_string(),
            };
            format!("Flatpak, your user ({shown})")
        }
        Inst::System => {
            let path = system_installs.first().map(String::as_str).unwrap_or("/var/lib/flatpak");
            format!("Flatpak, system-wide ({path})")
        }
    }
}

/// A separate, resizable window with `flatpak info` and its permissions.
fn details(ui: &Rc<Ui>, item: &Item, inst: Inst) {
    let (info, perms) = match (
        commands::info(inst, &item.id, &item.branch),
        commands::permissions(inst, &item.id, &item.branch),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("slacker-flatpak: {e}");
            return;
        }
    };
    let Some(fp) = ui.data.borrow().flatpak.clone() else { return };
    let uninstall = match commands::uninstall(inst, &item.id, &item.branch) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("slacker-flatpak: {e}");
            return;
        }
    };

    let buffer = gtk::TextBuffer::new(None);
    let view = gtk::TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
    let tv = adw::ToolbarView::new();
    // With the window manager's frame the title and buttons are there already.
    if ui.frame == Frame::App {
        tv.add_top_bar(&adw::HeaderBar::new());
    }
    tv.set_content(Some(&scroller));

    // Uninstall: flatpak lists what it will remove and asks before doing it.
    // The application's data in ~/.var/app stays.
    let remove = gtk::Button::builder()
        .label("Uninstall\u{2026}")
        .tooltip_text(commands::display(&uninstall))
        .build();
    remove.add_css_class("destructive-action");
    let note = gtk::Label::builder()
        .label("flatpak shows what it will remove and asks first. Application data in ~/.var/app is kept.")
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .build();
    note.add_css_class("dim-label");
    note.add_css_class("caption");
    let bar = gtk::Box::builder().spacing(12).margin_top(8).margin_bottom(8).margin_start(12).margin_end(12).build();
    bar.append(&note);
    bar.append(&remove);
    tv.add_bottom_bar(&bar);
    let win = gtk::Window::builder()
        .title(format!("{}, {}", item.title(), inst.words()))
        .transient_for(&ui.window)
        .modal(false)
        .resizable(true)
        .default_width(760)
        .default_height(560)
        .child(&tv)
        .build();
    if ui.frame == Frame::App {
        win.set_titlebar(Some(&gtk::Box::builder().visible(false).build()));
    }
    buffer.set_text("Loading…");
    win.present();
    {
        let (ui, win, title) = (ui.clone(), win.clone(), format!("Uninstall {}, {}", item.title(), inst.words()));
        remove.connect_clicked(move |_| {
            win.close();
            start(&ui, txn::Spec { title: title.clone(), inst, argv: uninstall.clone() });
        });
    }

    glib::spawn_future_local(async move {
        let mut text = String::new();
        for (heading, argv) in [("Information", info), ("Permissions", perms)] {
            let shown = commands::display(&argv);
            let out = runner::run(&fp, &argv).await;
            text.push_str(&format!("== {heading}\n$ {shown}\n\n{}", out.stdout.trim_end()));
            if !out.stderr.trim().is_empty() {
                text.push_str(&format!("\n\n{}", out.stderr.trim_end()));
            }
            if !out.ok {
                text.push_str("\n\n(the command failed)");
            }
            text.push_str("\n\n");
        }
        buffer.set_text(text.trim_end());
    });
}


/// Opens the transaction window; when it ends, the pages are read again and
/// the last update check is dropped (it may no longer be true).
fn start(ui: &Rc<Ui>, spec: txn::Spec) {
    let Some(fp) = ui.data.borrow().flatpak.clone() else { return };
    let ui2 = ui.clone();
    let on_done: Rc<dyn Fn()> = Rc::new(move || {
        *ui2.checked.borrow_mut() = None;
        refresh(&ui2);
    });
    txn::run(&ui.window, ui.frame == Frame::App, fp, spec, on_done);
}

fn render_updates(ui: &Rc<Ui>) {
    let page = &ui.updates;
    clear(page);
    let d = ui.data.borrow();
    if d.flatpak.is_none() {
        return;
    }

    let intro = group(
        "Updates",
        Some("Checking asks every remote, over the network. Each update runs flatpak, which lists \
              what it will change and asks before doing it."),
    );
    let check = gtk::Button::builder()
        .label(if ui.checking.get() { "Checking\u{2026}" } else { "Check for Updates" })
        .sensitive(!ui.checking.get())
        .valign(gtk::Align::Center)
        .build();
    check.add_css_class("suggested-action");
    {
        let ui2 = ui.clone();
        check.connect_clicked(move |_| check_updates(&ui2));
    }
    intro.set_header_suffix(Some(&check));
    for inst in [Inst::User, Inst::System] {
        let r = row(&format!("Update {}", inst.words()), &commands::display(&commands::update(inst)));
        let b = gtk::Button::builder().label("Update\u{2026}").valign(gtk::Align::Center).build();
        let ui2 = ui.clone();
        b.connect_clicked(move |_| {
            start(
                &ui2,
                txn::Spec {
                    title: format!("Update, {}", inst.words()),
                    inst,
                    argv: commands::update(inst),
                },
            )
        });
        r.add_suffix(&b);
        intro.add(&r);
    }
    page.append(&intro);

    let checked = ui.checked.borrow();
    let Some(c) = checked.as_ref() else {
        page.append(&commands_footer(&["Not checked yet.".to_string()]));
        return;
    };
    for inst in [Inst::User, Inst::System] {
        let mine: Vec<&Update> = c.found.iter().filter(|(i, _)| *i == inst).map(|(_, u)| u).collect();
        let g = group(&heading(&d.installations, inst), None);
        if mine.is_empty() {
            // remote-ls --updates lists updates only, not removals.
            g.add(&row(
                "No updates found",
                "flatpak update may still remove end-of-life runtimes; it lists them and asks first.",
            ));
        }
        for u in mine {
            let mut sub = u.id.clone();
            for part in [&u.version, &u.branch, &u.origin] {
                if !part.is_empty() {
                    sub.push_str("  ·  ");
                    sub.push_str(part);
                }
            }
            let r = row(u.title(), &sub);
            if !u.download.is_empty() {
                r.add_suffix(&tag(&u.download, false));
            }
            g.add(&r);
        }
        page.append(&g);
    }
    if !c.notes.is_empty() {
        let g = group(
            "Messages from flatpak",
            Some("A remote that could not be read hides only its own updates."),
        );
        for (cmd, text) in &c.notes {
            g.add(&row(cmd, text));
        }
        page.append(&g);
    }
    page.append(&commands_footer(&c.ran));
}

/// Asks each enabled remote of each installation for updates, one at a time.
fn check_updates(ui: &Rc<Ui>) {
    if ui.checking.replace(true) {
        return;
    }
    render_updates(ui);
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let (fp, remotes) = {
            let d = ui.data.borrow();
            (d.flatpak.clone(), d.remotes.clone())
        };
        let mut c = Checked::default();
        if let Some(fp) = fp {
            for r in remotes.iter().filter(|r| !r.disabled()) {
                let Some(inst) = Inst::from_label(&r.installation) else { continue };
                let argv = match commands::check_updates(inst, &r.name) {
                    Ok(a) => a,
                    Err(e) => {
                        c.notes.push((r.name.clone(), e));
                        continue;
                    }
                };
                let shown = commands::display(&argv);
                c.ran.push(shown.clone());
                let out = runner::run(&fp, &argv).await;
                if !out.stderr.trim().is_empty() {
                    c.notes.push((shown.clone(), out.stderr.trim().to_string()));
                } else if !out.ok {
                    c.notes.push((shown.clone(), "failed without a message".to_string()));
                }
                if out.ok {
                    c.found.extend(parse::updates::parse(&out.stdout).into_iter().map(|u| (inst, u)));
                }
            }
        }
        *ui.checked.borrow_mut() = Some(c);
        ui.checking.set(false);
        render_updates(&ui);
    });
}


/// The installations' base directories, user first: where appstream icons are.
fn bases(d: &Data) -> Vec<PathBuf> {
    let mut v = vec![checks::user_data_home().join("flatpak")];
    v.extend(d.installations.iter().map(PathBuf::from));
    v
}

fn icon_for(d: &Data, remote: &str, id: &str) -> gtk::Image {
    let img = match checks::appstream_icon(&bases(d), remote, &d.arch, id) {
        Some(p) => gtk::Image::from_file(p),
        None => gtk::Image::from_icon_name("application-x-executable-symbolic"),
    };
    img.set_pixel_size(48);
    img
}

fn render_search(ui: &Rc<Ui>) {
    let page = &ui.search;
    clear(page);
    page.append(&ui.search_entry);
    let d = ui.data.borrow();
    let found = ui.found.borrow();
    let Some(f) = found.as_ref() else {
        page.append(&commands_footer(&[
            "Looks in the appstream data flatpak keeps on disk (it is refreshed by every flatpak update); \
             no network. Press Enter to search."
                .to_string(),
        ]));
        return;
    };
    if let Some(n) = &f.note {
        let g = group("Messages from flatpak", None);
        g.add(&row(&f.ran, n));
        page.append(&g);
    }
    let g = group(&format!("Results for \u{201c}{}\u{201d}", f.text), Some(&format!("{} found", f.hits.len())));
    if f.hits.is_empty() {
        g.add(&row("Nothing found", ""));
    }
    for hit in &f.hits {
        for remote in &hit.remotes {
            let mut sub = hit.id.clone();
            for part in [&hit.version, &hit.branch, remote] {
                if !part.is_empty() {
                    sub.push_str("  ·  ");
                    sub.push_str(part);
                }
            }
            if !hit.description.is_empty() {
                sub.push('\n');
                sub.push_str(&hit.description);
            }
            let r = row(hit.title(), &sub);
            r.set_subtitle_selectable(false);
            r.add_prefix(&icon_for(&d, remote, &hit.id));
            for inst in [Inst::User, Inst::System] {
                if d.apps.iter().any(|a| a.id == hit.id && a.branch == hit.branch && a.inst() == Some(inst)) {
                    r.add_suffix(&tag(&format!("installed in {}", inst.words()), false));
                }
            }
            let b = gtk::Button::builder().label("Install\u{2026}").valign(gtk::Align::Center).build();
            let (ui2, hit2, remote2) = (ui.clone(), hit.clone(), remote.clone());
            b.connect_clicked(move |_| install_dialog(&ui2, &hit2, &remote2));
            r.add_suffix(&b);
            g.add(&r);
        }
    }
    page.append(&g);
    page.append(&commands_footer(std::slice::from_ref(&f.ran)));
}

fn run_search(ui: &Rc<Ui>, text: &str) {
    let argv = match commands::search(text) {
        Ok(a) => a,
        Err(e) => {
            *ui.found.borrow_mut() =
                Some(Found { text: text.to_string(), hits: Vec::new(), note: Some(e), ran: String::new() });
            render_search(ui);
            return;
        }
    };
    let Some(fp) = ui.data.borrow().flatpak.clone() else { return };
    let (ui, text) = (ui.clone(), text.to_string());
    glib::spawn_future_local(async move {
        let ran = commands::display(&argv);
        let out = runner::run(&fp, &argv).await;
        let note = if !out.stderr.trim().is_empty() {
            Some(out.stderr.trim().to_string())
        } else if !out.ok {
            Some("failed without a message".to_string())
        } else {
            None
        };
        let hits = if out.ok { parse::search::parse(&out.stdout) } else { Vec::new() };
        *ui.found.borrow_mut() = Some(Found { text, hits, note, ran });
        render_search(&ui);
    });
}

/// Before installing: which installation, whether it has the remote, and what
/// the application asks for. Then the transaction window does the rest.
fn install_dialog(ui: &Rc<Ui>, hit: &Hit, remote: &str) {
    let (fp, remotes) = {
        let d = ui.data.borrow();
        let Some(fp) = d.flatpak.clone() else { return };
        (fp, d.remotes.clone())
    };
    let present = |inst: Inst| remotes.iter().any(|r| r.name == remote && r.installation == inst.label() && !r.disabled());
    let (in_user, in_system) = (present(Inst::User), present(Inst::System));
    let has = move |inst: Inst| if inst == Inst::User { in_user } else { in_system };

    let title = gtk::Label::builder()
        .label(format!("{}  ({}//{}, from {})", hit.title(), hit.id, hit.branch, remote))
        .xalign(0.0)
        .wrap(true)
        .build();
    title.add_css_class("heading");
    let user = gtk::CheckButton::with_label(&format!("Install into {}", Inst::User.words()));
    let system = gtk::CheckButton::with_label(&format!("Install into {}", Inst::System.words()));
    system.set_group(Some(&user));
    user.set_active(true);
    let avail = gtk::Label::builder().xalign(0.0).wrap(true).build();

    let buffer = gtk::TextBuffer::new(None);
    let view = gtk::TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
    scroller.add_css_class("card");
    let perms_title = gtk::Label::builder()
        .label("What it asks for (from flatpak, for information):")
        .xalign(0.0)
        .build();
    perms_title.add_css_class("heading");

    let cancel = gtk::Button::with_label("Cancel");
    let go = gtk::Button::with_label("Install\u{2026}");
    go.add_css_class("suggested-action");
    let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
    buttons.append(&cancel);
    buttons.append(&go);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    for w in [title.upcast_ref::<gtk::Widget>(), user.upcast_ref(), system.upcast_ref(), avail.upcast_ref(), perms_title.upcast_ref(), scroller.upcast_ref(), buttons.upcast_ref()] {
        content.append(w);
    }
    let tv = adw::ToolbarView::new();
    if ui.frame == Frame::App {
        tv.add_top_bar(&adw::HeaderBar::new());
    }
    tv.set_content(Some(&content));
    let win = gtk::Window::builder()
        .title(format!("Install {}", hit.title()))
        .transient_for(&ui.window)
        .modal(true)
        .resizable(true)
        .default_width(760)
        .default_height(600)
        .child(&tv)
        .build();
    if ui.frame == Frame::App {
        win.set_titlebar(Some(&gtk::Box::builder().visible(false).build()));
    }

    // Shows, for the chosen installation, whether the remote is there and
    // what the application asks for.
    let generation = Rc::new(Cell::new(0u32));
    let show: Rc<dyn Fn(Inst)> = {
        let (avail, buffer, go, generation) = (avail.clone(), buffer.clone(), go.clone(), generation.clone());
        let (fp, hit, remote) = (fp.clone(), hit.clone(), remote.to_string());
        Rc::new(move |inst: Inst| {
            let n = generation.get() + 1;
            generation.set(n);
            if !has(inst) {
                avail.set_label(&format!(
                    "The remote \u{201c}{remote}\u{201d} is not set up in {}, so flatpak cannot install from it there.",
                    inst.words()
                ));
                avail.add_css_class("warning");
                go.set_sensitive(false);
                buffer.set_text("");
                return;
            }
            avail.remove_css_class("warning");
            avail.set_label(&format!(
                "flatpak will list everything it installs (runtimes too) and ask before doing it. Command: {}",
                commands::install(inst, &remote, &hit.id, &hit.branch).map(|a| commands::display(&a)).unwrap_or_default()
            ));
            go.set_sensitive(true);
            let argv = match commands::remote_info(inst, &remote, &hit.id, &hit.branch) {
                Ok(a) => a,
                Err(e) => {
                    buffer.set_text(&e);
                    return;
                }
            };
            buffer.set_text("Asking flatpak\u{2026}");
            let (fp, buffer, generation) = (fp.clone(), buffer.clone(), generation.clone());
            glib::spawn_future_local(async move {
                let shown = commands::display(&argv);
                let out = runner::run(&fp, &argv).await;
                if generation.get() != n {
                    return; // the choice changed meanwhile
                }
                let mut text = format!("$ {shown}\n\n");
                let access = parse::metadata::access(&out.stdout);
                if access.is_empty() && out.ok {
                    text.push_str("(no [Context] or bus policy in the metadata)\n");
                }
                for (section, lines) in access {
                    text.push_str(&format!("[{section}]\n"));
                    for l in lines {
                        text.push_str(&format!("  {l}\n"));
                    }
                    text.push('\n');
                }
                if !out.stderr.trim().is_empty() {
                    text.push_str(out.stderr.trim());
                    text.push('\n');
                }
                if !out.ok {
                    text.push_str("(the command failed)\n");
                }
                buffer.set_text(text.trim_end());
            });
        })
    };
    {
        let show = show.clone();
        user.connect_toggled(move |b| {
            if b.is_active() {
                show(Inst::User)
            }
        });
    }
    {
        let show = show.clone();
        system.connect_toggled(move |b| {
            if b.is_active() {
                show(Inst::System)
            }
        });
    }
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let (ui, win, hit, remote, user) = (ui.clone(), win.clone(), hit.clone(), remote.to_string(), user.clone());
        go.connect_clicked(move |_| {
            let inst = if user.is_active() { Inst::User } else { Inst::System };
            let Ok(argv) = commands::install(inst, &remote, &hit.id, &hit.branch) else { return };
            win.close();
            start(&ui, txn::Spec { title: format!("Install {}, {}", hit.title(), inst.words()), inst, argv });
        });
    }
    win.present();
    show(Inst::User);
}


/// An installed application's icon: the one it exports, else the one in its
/// remote's appstream data, else a generic one.
fn installed_image(d: &Data, item: &Item, inst: Inst) -> gtk::Image {
    let base = match inst {
        Inst::User => checks::user_data_home().join("flatpak"),
        Inst::System => d.installations.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/var/lib/flatpak")),
    };
    let img = match checks::installed_icon(&base, &item.id)
        .or_else(|| checks::appstream_icon(&bases(d), &item.origin, &d.arch, &item.id))
    {
        Some(p) => gtk::Image::from_file(p),
        None => gtk::Image::from_icon_name("application-x-executable-symbolic"),
    };
    img.set_pixel_size(40);
    img
}

fn render_browse(ui: &Rc<Ui>) {
    let page = &ui.browse;
    clear(page);
    let d = ui.data.borrow();
    if d.flatpak.is_none() {
        page.append(&adw::StatusPage::builder().icon_name("dialog-warning-symbolic").title("flatpak is not installed").build());
        return;
    }
    let cat = ui.catalog.borrow();
    let Some(c) = cat.as_ref() else {
        let b = gtk::Box::builder().spacing(12).halign(gtk::Align::Center).margin_top(48).build();
        let sp = gtk::Spinner::new();
        sp.start();
        b.append(&sp);
        b.append(&gtk::Label::new(Some("Reading the applications your remotes offer\u{2026}")));
        page.append(&b);
        return;
    };
    let entries: Vec<&Entry> = c.entries.iter().filter(|e| !e.app.branch.is_empty()).collect();
    if entries.is_empty() {
        page.append(
            &adw::StatusPage::builder()
                .icon_name("view-grid-symbolic")
                .title("No applications to show")
                .description("flatpak has no appstream data on disk for your remotes yet. It fetches it on every update.")
                .build(),
        );
    } else {
        // Groups, with how many each has; empty groups are left out.
        let current = ui.browse_group.borrow().clone();
        // One row of compact buttons; it scrolls sideways in a narrow window.
        let chips = gtk::Box::builder().spacing(6).build();
        let mut first: Option<gtk::ToggleButton> = None;
        let names = std::iter::once("All").chain(GROUPS.iter().map(|(g, _)| *g));
        for name in names {
            let n = if name == "All" {
                entries.len()
            } else {
                entries.iter().filter(|e| parse::appstream::in_group(&e.app, name)).count()
            };
            if n == 0 {
                continue;
            }
            let b = gtk::ToggleButton::builder().label(format!("{name}  {n}")).active(name == current).build();
            match &first {
                Some(f) => b.set_group(Some(f)),
                None => first = Some(b.clone()),
            }
            let (ui2, name) = (ui.clone(), name.to_string());
            b.connect_clicked(move |b| {
                if b.is_active() && *ui2.browse_group.borrow() != name {
                    *ui2.browse_group.borrow_mut() = name.clone();
                    ui2.browse_limit.set(BROWSE_STEP);
                    // Rebuild after this click has been handled.
                    let ui3 = ui2.clone();
                    glib::idle_add_local_once(move || render_browse(&ui3));
                }
            });
            chips.append(&b);
        }
        let chips_scroll = gtk::ScrolledWindow::builder()
            .child(&chips)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .build();
        page.append(&chips_scroll);

        let shown: Rc<Vec<Entry>> = Rc::new(
            entries
                .iter()
                .filter(|e| current == "All" || parse::appstream::in_group(&e.app, &current))
                .map(|e| (*e).clone())
                .collect(),
        );
        let limit = ui.browse_limit.get().min(shown.len());
        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(3)
            .row_spacing(12)
            .column_spacing(12)
            .build();
        for e in &shown[..limit] {
            grid.insert(&card(ui, &d, e), -1);
        }
        page.append(&grid);
        if limit < shown.len() {
            let more = gtk::Button::builder()
                .label(format!("Show More ({} more)", shown.len() - limit))
                .halign(gtk::Align::Center)
                .build();
            // Adds cards to the grid in place, so the page keeps its scroll
            // position.
            let (ui2, grid2) = (ui.clone(), grid.clone());
            more.connect_clicked(move |b| {
                let from = ui2.browse_limit.get().min(shown.len());
                let to = (from + BROWSE_STEP).min(shown.len());
                {
                    let d = ui2.data.borrow();
                    for e in &shown[from..to] {
                        grid2.insert(&card(&ui2, &d, e), -1);
                    }
                }
                ui2.browse_limit.set(to);
                if to < shown.len() {
                    b.set_label(&format!("Show More ({} more)", shown.len() - to));
                } else {
                    b.set_visible(false);
                }
            });
            page.append(&more);
        }
    }
    if !c.notes.is_empty() {
        let g = group("Not shown", None);
        for n in &c.notes {
            g.add(&row(n, ""));
        }
        page.append(&g);
    }
    page.append(&commands_footer(&[
        "Read from the appstream data flatpak keeps on disk (refreshed by every flatpak update); nothing is downloaded."
            .to_string(),
    ]));
}

/// One application on the Browse page; a click opens the install window.
fn card(ui: &Rc<Ui>, d: &Data, e: &Entry) -> gtk::Button {
    let img = match &e.icon {
        Some(p) => gtk::Image::from_file(p),
        None => gtk::Image::from_icon_name("application-x-executable-symbolic"),
    };
    img.set_pixel_size(64);
    img.set_valign(gtk::Align::Start);
    let name = gtk::Label::builder()
        .label(if e.app.name.is_empty() { &e.app.id } else { &e.app.name })
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name.add_css_class("heading");
    let summary = gtk::Label::builder()
        .label(&e.app.summary)
        .xalign(0.0)
        .wrap(true)
        .lines(2)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    summary.add_css_class("dim-label");
    summary.add_css_class("caption");
    let mut foot = e.remote.clone();
    for inst in [Inst::User, Inst::System] {
        if d.apps.iter().any(|a| a.id == e.app.id && a.branch == e.app.branch && a.inst() == Some(inst)) {
            foot.push_str(&format!("  \u{b7}  installed in {}", inst.words()));
        }
    }
    let foot = gtk::Label::builder().label(foot).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
    foot.add_css_class("caption");
    foot.add_css_class("dim-label");
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).hexpand(true).build();
    text.append(&name);
    text.append(&summary);
    text.append(&foot);
    let inner = gtk::Box::builder()
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    inner.append(&img);
    inner.append(&text);
    let b = gtk::Button::builder().child(&inner).width_request(260).tooltip_text(&e.app.id).build();
    b.add_css_class("card");
    let (ui2, hit, remote) = (
        ui.clone(),
        Hit {
            name: e.app.name.clone(),
            description: e.app.summary.clone(),
            id: e.app.id.clone(),
            version: String::new(),
            branch: e.app.branch.clone(),
            remotes: vec![e.remote.clone()],
        },
        e.remote.clone(),
    );
    b.connect_clicked(move |_| install_dialog(&ui2, &hit, &remote));
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_keeps_the_app_frame_everything_else_gets_the_system_one() {
        assert_eq!(frame_mode(Some("GNOME")), Frame::App);
        assert_eq!(frame_mode(Some("ubuntu:GNOME")), Frame::App);
        assert_eq!(frame_mode(Some("KDE")), Frame::System);
        assert_eq!(frame_mode(None), Frame::System);
    }
}
