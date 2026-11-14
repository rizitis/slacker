//! Main window. One window with three views:
//! - Explore: a search bar, the categories, and every application the remotes
//!   offer, as tiles with their icons;
//! - Installed: the installed applications (runtimes behind a switch);
//! - Updates.
//!
//! A tile or a row opens the application's page (icon, information, Open,
//! Install, Uninstall). Remotes and Overview (the checks) are in the menu; a
//! banner says when the checks found something.
//!
//! Changes (install, update, uninstall) run in txn.rs.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};

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
    nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    banner: adw::Banner,
    updates_page: adw::ViewStackPage,
    // Explore
    search: gtk::SearchEntry,
    /// The categories: a list at the side, or a drop-down in a narrow window.
    cats: gtk::ListBox,
    cat_drop: gtk::DropDown,
    cat_names: RefCell<Vec<String>>,
    /// Set while the two are being filled, so that is not taken as a choice.
    cat_filling: Cell<bool>,
    catalog_notes: gtk::Label,
    store: gio::ListStore,
    explore_stack: gtk::Stack,
    explore_status: adw::StatusPage,
    /// The Explore catalog; None while it is being read.
    catalog: RefCell<Option<Catalog>>,
    group: RefCell<String>,
    // Installed and Updates
    installed: gtk::Box,
    show_runtimes: Cell<bool>,
    updates: gtk::Box,
    spinner: gtk::Spinner,
    refresh: gtk::Button,
    busy: Cell<bool>,
    data: RefCell<Data>,
    /// The last check for updates, if any since the last change.
    checked: RefCell<Option<Checked>>,
    checking: Cell<bool>,
    /// Icons already scaled to the size they are shown at.
    icons: RefCell<HashMap<(PathBuf, i32), gdk::Texture>>,
}

/// What one check for updates found.
#[derive(Default)]
struct Checked {
    found: Vec<(Inst, Update)>,
    notes: Vec<(String, String)>,
    ran: Vec<String>,
}

/// An application or runtime as its page shows it.
#[derive(Clone)]
struct Target {
    id: String,
    branch: String,
    name: String,
    summary: String,
    /// The remote it comes from, if known.
    remote: Option<String>,
    icon: Option<PathBuf>,
    runtime: bool,
}

/// Icon sizes, in logical pixels.
const TILE_ICON: i32 = 64;
const ROW_ICON: i32 = 40;
const PAGE_ICON: i32 = 128;

/// Scaled icons kept in memory. Past this the cache starts over.
const ICON_CACHE_MAX: usize = 800;

/// Same sizes as slacker-gui: below this width the view switcher moves from
/// the header to the bottom of the window.
const NARROW: &str = "max-width: 760sp";

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

    // Explore: search and categories on top, the tiles below.
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search applications by name, description or ID")
        .hexpand(true)
        .build();
    let cats = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
    cats.add_css_class("navigation-sidebar");
    // The categories, and the slacker mark under them (as in slacker-gui).
    let side_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side_box.append(&cats);
    if let Some(logo) = side_logo() {
        side_box.append(&logo);
    }
    let cat_side = gtk::ScrolledWindow::builder()
        .child(&side_box)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .width_request(210)
        .build();
    let cat_drop = gtk::DropDown::builder().visible(false).valign(gtk::Align::Center).build();
    let catalog_notes = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
    catalog_notes.add_css_class("caption");
    catalog_notes.add_css_class("dim-label");
    let search_row = gtk::Box::builder().spacing(8).build();
    search_row.append(&search);
    search_row.append(&cat_drop);
    let top = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
    top.append(&search_row);
    top.append(&catalog_notes);
    let top_clamp = adw::Clamp::builder().maximum_size(1200).child(&top).build();
    top_clamp.set_margin_top(18);
    top_clamp.set_margin_bottom(6);
    top_clamp.set_margin_start(12);
    top_clamp.set_margin_end(12);
    let explore_stack = gtk::Stack::builder().vexpand(true).build();
    let explore_status = adw::StatusPage::new();
    explore_stack.add_named(&explore_status, Some("status"));
    let right = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
    right.append(&top_clamp);
    right.append(&explore_stack);
    let explore = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    explore.append(&cat_side);
    explore.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    explore.append(&right);

    let column = || gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();
    let installed = column();
    let updates = column();
    let stack = adw::ViewStack::new();
    stack.add_titled_with_icon(&explore, Some("explore"), "Explore", "view-grid-symbolic");
    stack.add_titled_with_icon(&scrolled(&installed), Some("installed"), "Installed", "view-app-grid-symbolic");
    let updates_page =
        stack.add_titled_with_icon(&scrolled(&updates), Some("updates"), "Updates", "software-update-available-symbolic");

    let ui = Rc::new(Ui {
        window: window.clone(),
        frame,
        nav: adw::NavigationView::new(),
        toasts: adw::ToastOverlay::new(),
        banner: adw::Banner::builder().button_label("Show").revealed(false).build(),
        updates_page,
        search,
        cats,
        cat_drop,
        cat_names: RefCell::default(),
        cat_filling: Cell::new(false),
        catalog_notes,
        store: gio::ListStore::new::<glib::BoxedAnyObject>(),
        explore_stack,
        explore_status,
        catalog: RefCell::default(),
        group: RefCell::new("All".to_string()),
        installed,
        show_runtimes: Cell::new(false),
        updates,
        spinner: gtk::Spinner::builder().visible(false).tooltip_text("flatpak is running").build(),
        refresh: gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Refresh").build(),
        busy: Cell::new(false),
        data: RefCell::default(),
        checked: RefCell::default(),
        checking: Cell::new(false),
        icons: RefCell::default(),
    });
    let grid = tile_grid(&ui);
    // No panel behind the tiles: they sit on the window like the rest.
    grid.remove_css_class("view");
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_bottom(12);
    ui.explore_stack.add_named(
        &gtk::ScrolledWindow::builder().child(&grid).hscrollbar_policy(gtk::PolicyType::Never).build(),
        Some("grid"),
    );
    stack.set_visible_child_name("explore");

    // Header: Refresh, the view switcher, the spinner and the menu.
    let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
    let header = adw::HeaderBar::builder().title_widget(&switcher).build();
    header.set_show_start_title_buttons(frame == Frame::App);
    header.set_show_end_title_buttons(frame == Frame::App);
    header.pack_start(&ui.refresh);
    let menu = gio::Menu::new();
    menu.append(Some("Remotes"), Some("win.remotes"));
    menu.append(Some("Overview and Checks"), Some("win.overview"));
    header.pack_end(&gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).tooltip_text("Menu").build());
    header.pack_end(&ui.spinner);
    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();

    let root_view = adw::ToolbarView::new();
    root_view.add_top_bar(&header);
    root_view.add_top_bar(&ui.banner);
    root_view.set_content(Some(&stack));
    root_view.add_bottom_bar(&switcher_bar);
    let root = adw::NavigationPage::builder().title("Slacker Flatpak").tag("root").child(&root_view).build();
    ui.nav.add(&root);
    ui.toasts.set_child(Some(&ui.nav));

    let bin = adw::BreakpointBin::builder().child(&ui.toasts).width_request(540).height_request(420).build();
    let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse(NARROW).expect("valid breakpoint"));
    bp.add_setter(&switcher, "visible", Some(&false.to_value()));
    bp.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));
    bp.add_setter(&cat_side, "visible", Some(&false.to_value()));
    bp.add_setter(&explore.last_child().and_then(|w| w.prev_sibling()).expect("separator"), "visible", Some(&false.to_value()));
    bp.add_setter(&ui.cat_drop, "visible", Some(&true.to_value()));
    bin.add_breakpoint(bp);
    window.set_child(Some(&bin));

    for (name, f) in [("remotes", show_remotes as fn(&Rc<Ui>)), ("overview", show_overview)] {
        let action = gio::SimpleAction::new(name, None);
        let ui2 = ui.clone();
        action.connect_activate(move |_, _| f(&ui2));
        window.add_action(&action);
    }
    {
        let ui2 = ui.clone();
        ui.banner.connect_button_clicked(move |_| show_overview(&ui2));
    }
    {
        let ui2 = ui.clone();
        ui.refresh.connect_clicked(move |_| refresh(&ui2));
    }
    {
        let ui2 = ui.clone();
        ui.search.connect_search_changed(move |_| refilter(&ui2));
    }
    {
        let ui2 = ui.clone();
        ui.cats.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                choose_group(&ui2, r.index());
            }
        });
    }
    {
        let ui2 = ui.clone();
        ui.cat_drop.connect_selected_notify(move |d| {
            if d.selected() != gtk::INVALID_LIST_POSITION {
                choose_group(&ui2, d.selected() as i32);
            }
        });
    }

    window.present();
    refresh(&ui);
}

/// The slacker mark under the categories. Same as slacker-gui: the SVG is
/// compiled in and rasterised once through gdk-pixbuf (librsvg), at twice the
/// shown size so it stays sharp on HiDPI screens. Without an SVG loader there
/// is simply no logo; nothing else changes.
fn side_logo() -> Option<gtk::Image> {
    const SVG: &[u8] = include_bytes!("slacker-mark.svg");
    const SIZE: i32 = 140;
    let loader = gdk_pixbuf::PixbufLoader::with_type("svg").ok()?;
    loader.set_size(SIZE * 2, SIZE * 2);
    loader.write(SVG).ok()?;
    loader.close().ok()?;
    let pixbuf = loader.pixbuf()?;
    let texture = gdk::Texture::for_pixbuf(&pixbuf);
    let image = gtk::Image::from_paintable(Some(&texture));
    image.set_pixel_size(SIZE);
    image.set_vexpand(true);
    image.set_valign(gtk::Align::Center);
    image.set_halign(gtk::Align::Center);
    image.set_margin_top(24);
    image.set_margin_bottom(24);
    Some(image)
}

/// A view's content, centred and scrollable.
fn scrolled(content: &gtk::Box) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder().maximum_size(980).child(content).build();
    clamp.set_margin_top(18);
    clamp.set_margin_bottom(18);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);
    gtk::ScrolledWindow::builder()
        .child(&clamp)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build()
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
        // The catalog is read from disk off the main loop; Explore shows
        // "Reading…" until then.
        let (b, r, a) = {
            let d = ui.data.borrow();
            let remotes: Vec<String> = d.remotes.iter().filter(|r| !r.disabled()).map(|r| r.name.clone()).collect();
            (bases(&d), remotes, d.arch.clone())
        };
        let cat = gio::spawn_blocking(move || catalog::load(&b, &r, &a)).await.unwrap_or_default();
        *ui.catalog.borrow_mut() = Some(cat);
        render_explore(&ui);
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
async fn step(fp: &Path, argv: Vec<String>, d: &mut Data) -> Option<String> {
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
    render_installed(ui);
    render_updates(ui);
    render_explore(ui);
    render_banner(ui);
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

// ---------------------------------------------------------------- icons ---

/// An icon file scaled once to the size it is shown at (times the display's
/// scale), instead of the whole 128×128 file; kept for the next time.
fn texture(ui: &Ui, path: &Path, size: i32) -> Option<gdk::Texture> {
    let px = size * ui.window.scale_factor().max(1);
    let key = (path.to_path_buf(), px);
    if let Some(t) = ui.icons.borrow().get(&key) {
        return Some(t.clone());
    }
    let pb = gdk_pixbuf::Pixbuf::from_file_at_scale(path, px, px, true).ok()?;
    let t = gdk::Texture::for_pixbuf(&pb);
    let mut cache = ui.icons.borrow_mut();
    if cache.len() >= ICON_CACHE_MAX {
        cache.clear();
    }
    cache.insert(key, t.clone());
    Some(t)
}

fn set_icon(ui: &Ui, img: &gtk::Image, path: Option<&Path>, size: i32) {
    match path.and_then(|p| texture(ui, p, size)) {
        Some(t) => img.set_paintable(Some(&t)),
        None => img.set_icon_name(Some("application-x-executable-symbolic")),
    }
    img.set_pixel_size(size);
}

fn icon(ui: &Ui, path: Option<&Path>, size: i32) -> gtk::Image {
    let img = gtk::Image::new();
    set_icon(ui, &img, path, size);
    img
}

/// The installations' base directories, user first: where appstream icons are.
fn bases(d: &Data) -> Vec<PathBuf> {
    let mut v = vec![checks::user_data_home().join("flatpak")];
    v.extend(d.installations.iter().map(PathBuf::from));
    v
}

fn base_of(d: &Data, inst: Inst) -> PathBuf {
    match inst {
        Inst::User => checks::user_data_home().join("flatpak"),
        Inst::System => d.installations.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/var/lib/flatpak")),
    }
}

/// An installed application's icon file: the one it exports, else the one in
/// its remote's appstream data.
fn installed_icon_path(d: &Data, item: &Item, inst: Inst) -> Option<PathBuf> {
    checks::installed_icon(&base_of(d, inst), &item.id)
        .or_else(|| checks::appstream_icon(&bases(d), &item.origin, &d.arch, &item.id))
}

// -------------------------------------------------------------- explore ---

fn installed_in(d: &Data, id: &str, branch: &str) -> Vec<Inst> {
    [Inst::User, Inst::System]
        .into_iter()
        .filter(|i| d.apps.iter().any(|a| a.id == id && a.branch == branch && a.inst() == Some(*i)))
        .collect()
}

/// The tiles: a GridView, so only the tiles on screen exist (and only their
/// icons are loaded), however many applications there are.
fn tile_grid(ui: &Rc<Ui>) -> gtk::GridView {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, li| {
        let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
        let img = gtk::Image::new();
        let name = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(18)
            .justify(gtk::Justification::Center)
            .build();
        name.add_css_class("heading");
        let summary = gtk::Label::builder()
            .wrap(true)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(22)
            .justify(gtk::Justification::Center)
            .valign(gtk::Align::Start)
            .vexpand(true)
            .build();
        summary.add_css_class("dim-label");
        summary.add_css_class("caption");
        let badge = gtk::Label::builder().label("Installed").visible(false).build();
        badge.add_css_class("caption");
        badge.add_css_class("accent");
        let b = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .width_request(170)
            .height_request(160)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(4)
            .margin_end(4)
            .build();
        let inner = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(14)
            .margin_bottom(10)
            .margin_start(10)
            .margin_end(10)
            .vexpand(true)
            .build();
        inner.append(&img);
        inner.append(&name);
        inner.append(&summary);
        inner.append(&badge);
        b.append(&inner);
        b.add_css_class("card");
        li.set_child(Some(&b));
    });
    {
        let ui = ui.clone();
        factory.connect_bind(move |_, li| {
            let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
            let (Some(obj), Some(b)) = (li.item().and_downcast::<glib::BoxedAnyObject>(), li.child().and_downcast::<gtk::Box>())
            else {
                return;
            };
            let e = obj.borrow::<Entry>();
            let Some(inner) = b.first_child() else { return };
            let mut kids = Vec::new();
            let mut c = inner.first_child();
            while let Some(w) = c {
                c = w.next_sibling();
                kids.push(w);
            }
            let [img, name, summary, badge] = kids.as_slice() else { return };
            if let Some(img) = img.downcast_ref::<gtk::Image>() {
                set_icon(&ui, img, e.icon.as_deref(), TILE_ICON);
            }
            if let Some(l) = name.downcast_ref::<gtk::Label>() {
                l.set_label(if e.app.name.is_empty() { &e.app.id } else { &e.app.name });
            }
            if let Some(l) = summary.downcast_ref::<gtk::Label>() {
                l.set_label(&e.app.summary);
            }
            let installed = !installed_in(&ui.data.borrow(), &e.app.id, &e.app.branch).is_empty();
            badge.set_visible(installed);
            b.set_tooltip_text(Some(&format!("{}  ({})", e.app.id, e.remote)));
        });
    }
    let grid = gtk::GridView::builder()
        .model(&gtk::NoSelection::new(Some(ui.store.clone())))
        .factory(&factory)
        .min_columns(1)
        .max_columns(10)
        .single_click_activate(true)
        .build();
    {
        let ui = ui.clone();
        grid.connect_activate(move |_, pos| {
            let Some(obj) = ui.store.item(pos).and_downcast::<glib::BoxedAnyObject>() else { return };
            let t = {
                let e = obj.borrow::<Entry>();
                Target {
                    id: e.app.id.clone(),
                    branch: e.app.branch.clone(),
                    name: e.app.name.clone(),
                    summary: e.app.summary.clone(),
                    remote: Some(e.remote.clone()),
                    icon: e.icon.clone(),
                    runtime: false,
                }
            };
            show_app(&ui, t);
        });
    }
    grid
}

/// The categories (with how many each has) and the tiles.
fn render_explore(ui: &Rc<Ui>) {
    let have_flatpak = ui.data.borrow().flatpak.is_some();
    let cat = ui.catalog.borrow();
    let entries: Vec<&Entry> = cat.as_ref().map(|c| c.entries.iter().filter(|e| !e.app.branch.is_empty()).collect()).unwrap_or_default();
    let mut current = ui.group.borrow().clone();
    let mut shown: Vec<(String, usize)> = Vec::new();
    for name in std::iter::once("All").chain(GROUPS.iter().map(|(g, _)| *g)) {
        let n = if name == "All" {
            entries.len()
        } else {
            entries.iter().filter(|e| parse::appstream::in_group(&e.app, name)).count()
        };
        // Empty categories are left out; All always stays.
        if n > 0 || name == "All" {
            shown.push((name.to_string(), n));
        }
    }
    if !shown.iter().any(|(n, _)| *n == current) {
        current = "All".to_string();
        *ui.group.borrow_mut() = current.clone();
    }
    let at = shown.iter().position(|(n, _)| *n == current).unwrap_or(0);
    ui.cat_filling.set(true);
    while let Some(c) = ui.cats.first_child() {
        ui.cats.remove(&c);
    }
    let strings = gtk::StringList::new(&[]);
    for (name, n) in &shown {
        let b = gtk::Box::builder().spacing(12).build();
        b.append(&gtk::Label::builder().label(name.as_str()).xalign(0.0).hexpand(true).build());
        if cat.is_some() {
            let c = gtk::Label::new(Some(&n.to_string()));
            c.add_css_class("dim-label");
            c.add_css_class("caption");
            b.append(&c);
        }
        ui.cats.append(&gtk::ListBoxRow::builder().child(&b).build());
        strings.append(&if cat.is_some() { format!("{name}  ({n})") } else { name.clone() });
    }
    ui.cat_drop.set_model(Some(&strings));
    ui.cats.select_row(ui.cats.row_at_index(at as i32).as_ref());
    ui.cat_drop.set_selected(at as u32);
    *ui.cat_names.borrow_mut() = shown.into_iter().map(|(n, _)| n).collect();
    ui.cat_filling.set(false);
    match cat.as_ref() {
        Some(c) if !c.notes.is_empty() => {
            ui.catalog_notes.set_label(&format!("Not shown: {}", c.notes.join("; ")));
            ui.catalog_notes.set_visible(true);
        }
        _ => ui.catalog_notes.set_visible(false),
    }
    ui.search.set_sensitive(cat.is_some());
    drop(cat);
    if !have_flatpak {
        ui.explore_status.set_icon_name(Some("dialog-warning-symbolic"));
        ui.explore_status.set_title("flatpak is not installed");
        ui.explore_status.set_description(Some(
            "flatpak is not part of Slackware. It is available from third-party repositories; \
             slacker search flatpak shows which of yours offer it.",
        ));
        ui.explore_stack.set_visible_child_name("status");
        return;
    }
    refilter(ui);
}

/// Fills the tiles with what the category and the search text let through.
/// The search looks at the name, the summary and the ID, ignoring case.
/// A category was picked in the side list or the drop-down; the other one
/// follows.
fn choose_group(ui: &Rc<Ui>, index: i32) {
    if ui.cat_filling.get() {
        return;
    }
    let Some(name) = ui.cat_names.borrow().get(index.max(0) as usize).cloned() else { return };
    if *ui.group.borrow() == name {
        return;
    }
    *ui.group.borrow_mut() = name;
    ui.cat_filling.set(true);
    ui.cats.select_row(ui.cats.row_at_index(index).as_ref());
    ui.cat_drop.set_selected(index as u32);
    ui.cat_filling.set(false);
    refilter(ui);
}

fn refilter(ui: &Rc<Ui>) {
    let cat = ui.catalog.borrow();
    let Some(c) = cat.as_ref() else {
        ui.store.remove_all();
        ui.explore_status.set_icon_name(None);
        ui.explore_status.set_paintable(None::<&gdk::Paintable>);
        ui.explore_status.set_title("Reading the applications your remotes offer\u{2026}");
        ui.explore_status.set_description(None);
        ui.explore_stack.set_visible_child_name("status");
        return;
    };
    let group = ui.group.borrow().clone();
    let query = ui.search.text().trim().to_lowercase();
    let objs: Vec<glib::BoxedAnyObject> = c
        .entries
        .iter()
        .filter(|e| !e.app.branch.is_empty())
        .filter(|e| group == "All" || parse::appstream::in_group(&e.app, &group))
        .filter(|e| {
            query.is_empty()
                || e.app.name.to_lowercase().contains(&query)
                || e.app.summary.to_lowercase().contains(&query)
                || e.app.id.to_lowercase().contains(&query)
        })
        .map(|e| glib::BoxedAnyObject::new(e.clone()))
        .collect();
    let empty_catalog = c.entries.is_empty();
    drop(cat);
    ui.store.remove_all();
    ui.store.extend_from_slice(&objs);
    if !objs.is_empty() {
        ui.explore_stack.set_visible_child_name("grid");
        return;
    }
    if empty_catalog {
        ui.explore_status.set_icon_name(Some("view-grid-symbolic"));
        ui.explore_status.set_title("No applications to show");
        ui.explore_status.set_description(Some(
            "flatpak has no appstream data on disk for your remotes yet. It fetches it on every update.",
        ));
    } else {
        ui.explore_status.set_icon_name(Some("system-search-symbolic"));
        ui.explore_status.set_title("Nothing found");
        ui.explore_status.set_description(Some(if group == "All" {
            "Try other words."
        } else {
            "Try other words, or the All category."
        }));
    }
    ui.explore_stack.set_visible_child_name("status");
}

// ------------------------------------------------------------ installed ---

fn render_installed(ui: &Rc<Ui>) {
    let page = &ui.installed;
    clear(page);
    let d = ui.data.borrow();
    if d.flatpak.is_none() {
        return;
    }
    let sw = adw::SwitchRow::builder()
        .title("Show runtimes")
        .subtitle("The platforms and extensions applications run on")
        .active(ui.show_runtimes.get())
        .build();
    {
        let ui2 = ui.clone();
        sw.connect_active_notify(move |s| {
            ui2.show_runtimes.set(s.is_active());
            let ui3 = ui2.clone();
            glib::idle_add_local_once(move || render_installed(&ui3));
        });
    }
    let g = adw::PreferencesGroup::new();
    g.add(&sw);
    page.append(&g);

    items(ui, page, &d, &d.apps, false);
    let mut ran = vec![commands::display(&commands::list_apps())];
    if ui.show_runtimes.get() {
        items(ui, page, &d, &d.runtimes, true);
        ran.push(commands::display(&commands::list_runtimes()));
    }
    page.append(&commands_footer(&ran));
}

fn items(ui: &Rc<Ui>, page: &gtk::Box, d: &Data, items: &[Item], runtimes: bool) {
    if items.is_empty() {
        let empty = if runtimes { "No runtimes installed" } else { "No applications installed" };
        page.append(&adw::StatusPage::builder().icon_name("folder-symbolic").title(empty).build());
        return;
    }
    let dups = parse::list::duplicates(items);
    for inst in [Inst::User, Inst::System] {
        let mine: Vec<&Item> = items.iter().filter(|i| i.inst() == Some(inst)).collect();
        if mine.is_empty() {
            continue;
        }
        let what = if runtimes { "Runtimes" } else { "Applications" };
        let g = group(&format!("{what}, {}", heading(&d.installations, inst)), Some(&format!("{} installed", mine.len())));
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
                    sub.push_str("  \u{b7}  ");
                    sub.push_str(part);
                }
            }
            let r = row(item.title(), &sub);
            let icon_path = if runtimes { None } else { installed_icon_path(d, item, inst) };
            if !runtimes {
                r.add_prefix(&icon(ui, icon_path.as_deref(), ROW_ICON));
            }
            // A selectable subtitle would take the click meant for the row.
            r.set_subtitle_selectable(false);
            r.set_activatable(true);
            if dups.iter().any(|g| g.iter().any(|x| std::ptr::eq(*x, item))) {
                let other = if inst == Inst::User { Inst::System } else { Inst::User };
                r.add_suffix(&tag(&format!("also in {}", other.words()), true));
            }
            r.add_suffix(&tag(&item.size, false));
            if !runtimes {
                let open = gtk::Button::builder().label("Open").valign(gtk::Align::Center).build();
                let (ui2, item2) = (ui.clone(), item.clone());
                open.connect_clicked(move |_| launch(&ui2, inst, &item2.id, &item2.branch, item2.title()));
                r.add_suffix(&open);
            }
            r.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let summary = summary_of(ui, &item.id);
            let t = Target {
                id: item.id.clone(),
                branch: item.branch.clone(),
                name: item.title().to_string(),
                summary,
                remote: (!item.origin.is_empty()).then(|| item.origin.clone()),
                icon: icon_path,
                runtime: runtimes,
            };
            let ui2 = ui.clone();
            r.connect_activated(move |_| show_app(&ui2, t.clone()));
            g.add(&r);
        }
        page.append(&g);
    }
    // Rows from another system installation (not "system" or "user").
    let other: Vec<&Item> = items.iter().filter(|i| i.inst().is_none()).collect();
    if !other.is_empty() {
        let g = group("Other installations", Some("Details are not shown for these yet."));
        for item in other {
            g.add(&row(item.title(), &format!("{}  \u{b7}  {}", item.id, item.installation)));
        }
        page.append(&g);
    }
}

/// The catalog's one-line summary of an application, if the catalog has it.
fn summary_of(ui: &Ui, id: &str) -> String {
    ui.catalog
        .borrow()
        .as_ref()
        .and_then(|c| c.entries.iter().find(|e| e.app.id == id).map(|e| e.app.summary.clone()))
        .unwrap_or_default()
}

/// Where an installation is, with ~ for the home directory.
fn inst_path(system_installs: &[String], inst: Inst) -> String {
    match inst {
        Inst::User => {
            let path = checks::user_data_home().join("flatpak");
            let home = std::env::var("HOME").unwrap_or_default();
            match path.to_string_lossy().strip_prefix(home.as_str()) {
                Some(rest) if !home.is_empty() => format!("~{rest}"),
                _ => path.display().to_string(),
            }
        }
        Inst::System => system_installs.first().cloned().unwrap_or_else(|| "/var/lib/flatpak".to_string()),
    }
}

/// A title that says which Flatpak installation and where it is.
fn heading(system_installs: &[String], inst: Inst) -> String {
    let who = if inst == Inst::User { "your user" } else { "system-wide" };
    format!("{who} ({})", inst_path(system_installs, inst))
}

// --------------------------------------------------------- open (launch) --

/// Starts an installed application with `flatpak run`, as the user, with the
/// user's own environment (not the one slacker-flatpak gives flatpak for
/// reading its tables). It runs on its own; its output is not kept.
fn launch(ui: &Rc<Ui>, inst: Inst, id: &str, branch: &str, title: &str) {
    let Some(fp) = ui.data.borrow().flatpak.clone() else { return };
    let argv = match commands::run_app(inst, id, branch) {
        Ok(a) => a,
        Err(e) => {
            toast_error(ui, &e);
            return;
        }
    };
    let launcher =
        gio::SubprocessLauncher::new(gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_SILENCE);
    let mut full: Vec<&OsStr> = vec![fp.as_os_str()];
    full.extend(argv.iter().map(OsStr::new));
    match launcher.spawn(&full) {
        Ok(p) => {
            toast(ui, &format!("Opening {title}\u{2026}"));
            let (ui2, p2, title) = (ui.clone(), p.clone(), title.to_string());
            // Reaps the process when it ends, and says so if it ended with an
            // error (flatpak run passes on the application's exit status).
            p.wait_async(None::<&gio::Cancellable>, move |r| {
                if r.is_ok() && p2.has_exited() && p2.exit_status() != 0 {
                    toast_error(&ui2, &format!("{title} ended with exit status {}", p2.exit_status()));
                }
            });
        }
        Err(e) => toast_error(ui, &format!("could not run flatpak: {e}")),
    }
}

fn toast(ui: &Ui, text: &str) {
    ui.toasts.add_toast(adw::Toast::builder().title(glib::markup_escape_text(text)).timeout(4).build());
}

/// A failure stays until it is closed, so it is not missed.
fn toast_error(ui: &Ui, text: &str) {
    ui.toasts.add_toast(adw::Toast::builder().title(glib::markup_escape_text(text)).timeout(0).build());
}

// ------------------------------------------------------- the app's page ---

/// A page with a header (and back button) pushed over the main views.
fn push(ui: &Rc<Ui>, title: &str, content: &gtk::Widget) {
    let header = adw::HeaderBar::new();
    header.set_show_start_title_buttons(ui.frame == Frame::App);
    header.set_show_end_title_buttons(ui.frame == Frame::App);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(content));
    let page = adw::NavigationPage::builder().title(title).child(&tv).build();
    ui.nav.push(&page);
}

fn show_app(ui: &Rc<Ui>, t: Target) {
    let d = ui.data.borrow();
    let Some(fp) = d.flatpak.clone() else { return };
    let list = if t.runtime { &d.runtimes } else { &d.apps };
    let installed: Vec<Inst> = [Inst::User, Inst::System]
        .into_iter()
        .filter(|i| list.iter().any(|a| a.id == t.id && a.branch == t.branch && a.inst() == Some(*i)))
        .collect();
    let title = if t.name.is_empty() { t.id.clone() } else { t.name.clone() };
    // A page opened from Installed before the catalog was read has no summary.
    let summary = if t.summary.is_empty() { summary_of(ui, &t.id) } else { t.summary.clone() };

    let col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();

    // Icon, name, summary, ID.
    let hero = gtk::Box::builder().spacing(18).build();
    if !t.runtime {
        hero.append(&icon(ui, t.icon.as_deref(), PAGE_ICON));
    }
    let words =
        gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).valign(gtk::Align::Center).hexpand(true).build();
    let name = gtk::Label::builder().label(&title).xalign(0.0).wrap(true).build();
    name.add_css_class("title-1");
    words.append(&name);
    if !summary.is_empty() {
        let s = gtk::Label::builder().label(&summary).xalign(0.0).wrap(true).build();
        s.add_css_class("dim-label");
        words.append(&s);
    }
    let mut meta = format!("{}  \u{b7}  {}", t.id, t.branch);
    if let Some(r) = &t.remote {
        meta.push_str(&format!("  \u{b7}  from {r}"));
    }
    let m = gtk::Label::builder().label(meta).xalign(0.0).wrap(true).build();
    m.add_css_class("caption");
    m.add_css_class("dim-label");
    words.append(&m);
    hero.append(&words);
    col.append(&hero);

    // One row per installation it is in, with Open and Uninstall; or Install.
    if !installed.is_empty() {
        let g = group("Installed in", None);
        for &inst in &installed {
            let it = list.iter().find(|a| a.id == t.id && a.branch == t.branch && a.inst() == Some(inst));
            let mut sub = vec![inst_path(&d.installations, inst)];
            if let Some(it) = it {
                for part in [&it.version, &it.size] {
                    if !part.is_empty() {
                        sub.push(part.clone());
                    }
                }
            }
            let r = row(inst.words(), &sub.join("  \u{b7}  "));
            r.set_subtitle_selectable(false);
            if !t.runtime {
                if let Ok(argv) = commands::run_app(inst, &t.id, &t.branch) {
                    let b = gtk::Button::builder()
                        .label("Open")
                        .valign(gtk::Align::Center)
                        .tooltip_text(commands::display(&argv))
                        .build();
                    b.add_css_class("suggested-action");
                    let (ui2, t2, title2) = (ui.clone(), t.clone(), title.clone());
                    b.connect_clicked(move |_| launch(&ui2, inst, &t2.id, &t2.branch, &title2));
                    r.add_suffix(&b);
                }
            }
            if let Ok(argv) = commands::uninstall(inst, &t.id, &t.branch) {
                let b = gtk::Button::builder()
                    .label("Uninstall\u{2026}")
                    .valign(gtk::Align::Center)
                    .tooltip_text(commands::display(&argv))
                    .build();
                b.add_css_class("destructive-action");
                let (ui2, spec_title) = (ui.clone(), format!("Uninstall {title}, {}", inst.words()));
                b.connect_clicked(move |_| start(&ui2, txn::Spec { title: spec_title.clone(), inst, argv: argv.clone() }));
                r.add_suffix(&b);
            }
            g.add(&r);
        }
        col.append(&g);
        col.append(&note("Uninstall: flatpak shows what it will remove and asks first. Application data in ~/.var/app is kept."));
    } else if !t.runtime {
        if let Some(remote) = &t.remote {
            let b = gtk::Button::builder().label("Install\u{2026}").halign(gtk::Align::Start).build();
            b.add_css_class("suggested-action");
            b.add_css_class("pill");
            let hit = Hit {
                name: t.name.clone(),
                description: summary.clone(),
                id: t.id.clone(),
                version: String::new(),
                branch: t.branch.clone(),
                remotes: vec![remote.clone()],
            };
            let (ui2, remote) = (ui.clone(), remote.clone());
            b.connect_clicked(move |_| install_dialog(&ui2, &hit, &remote));
            col.append(&b);
            col.append(&note("Install: you choose your user's or the system-wide Flatpak and see what the application asks for; then flatpak lists everything it installs and asks first."));
        }
    }

    // The same, from a terminal.
    let mut cmds: Vec<String> = Vec::new();
    for &inst in &installed {
        if !t.runtime {
            if let Ok(a) = commands::run_app(inst, &t.id, &t.branch) {
                cmds.push(commands::display(&a));
            }
        }
        if let Ok(a) = commands::uninstall(inst, &t.id, &t.branch) {
            cmds.push(commands::display(&a));
        }
    }
    if installed.is_empty() && !t.runtime {
        if let Some(remote) = &t.remote {
            for inst in [Inst::User, Inst::System] {
                let usable = d.remotes.iter().any(|r| &r.name == remote && r.installation == inst.label() && !r.disabled());
                if usable {
                    if let Ok(a) = commands::install(inst, remote, &t.id, &t.branch) {
                        cmds.push(commands::display(&a));
                    }
                }
            }
        }
    }
    if !cmds.is_empty() {
        let h = gtk::Label::builder().label("From a terminal").xalign(0.0).build();
        h.add_css_class("heading");
        col.append(&h);
        let l = gtk::Label::builder()
            .label(cmds.join("\n"))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .build();
        l.add_css_class("monospace");
        let f = gtk::Frame::builder().child(&l).build();
        l.set_margin_top(10);
        l.set_margin_bottom(10);
        l.set_margin_start(12);
        l.set_margin_end(12);
        col.append(&f);
        col.append(&note("Run as your user, not as root (as root, --user means root's own Flatpak). flatpak lists what it will change and asks first; for the system-wide Flatpak it asks its system helper."));
    }

    // What flatpak says about it.
    let heading = gtk::Label::builder()
        .label(if installed.is_empty() { "What it asks for" } else { "Information and permissions" })
        .xalign(0.0)
        .build();
    heading.add_css_class("heading");
    col.append(&heading);
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
    let frame = gtk::Frame::builder().child(&view).build();
    frame.add_css_class("view");
    col.append(&frame);
    buffer.set_text("Asking flatpak\u{2026}");

    let mut ask: Vec<(&str, Vec<String>)> = Vec::new();
    if let Some(&inst) = installed.first() {
        if let (Ok(a), Ok(b)) = (commands::info(inst, &t.id, &t.branch), commands::permissions(inst, &t.id, &t.branch)) {
            ask.push(("Information", a));
            ask.push(("Permissions", b));
        }
    } else if let Some(remote) = &t.remote {
        // The first installation that has this remote.
        let inst = [Inst::User, Inst::System]
            .into_iter()
            .find(|i| d.remotes.iter().any(|r| &r.name == remote && r.installation == i.label() && !r.disabled()));
        if let Some(Ok(a)) = inst.map(|i| commands::remote_info(i, remote, &t.id, &t.branch)) {
            ask.push(("Metadata", a));
        }
    }
    drop(d);

    let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    content.append(&scrolled(&col));
    push(ui, &title, content.upcast_ref());

    let installed_now = !installed.is_empty();
    glib::spawn_future_local(async move {
        if ask.is_empty() {
            buffer.set_text("Nothing to ask: the remote is not set up, or is disabled, in your user's and the system-wide Flatpak.");
            return;
        }
        let mut text = String::new();
        for (what, argv) in ask {
            let shown = commands::display(&argv);
            let out = runner::run(&fp, &argv).await;
            text.push_str(&format!("$ {shown}\n\n"));
            if installed_now {
                text.push_str(out.stdout.trim_end());
                text.push('\n');
            } else {
                // Only the parts that say what it may reach.
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
            }
            if !out.stderr.trim().is_empty() {
                text.push_str(&format!("\n{}\n", out.stderr.trim_end()));
            }
            if !out.ok {
                text.push_str(&format!("\n({what}: the command failed)\n"));
            }
            text.push('\n');
        }
        buffer.set_text(text.trim_end());
    });
}

fn note(text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
    l.add_css_class("dim-label");
    l.add_css_class("caption");
    l
}

/// Opens the transaction window; when it ends, everything is read again and
/// the last update check is dropped (it may no longer be true). Pages pushed
/// over the main views are closed first: what they show is about to change.
fn start(ui: &Rc<Ui>, spec: txn::Spec) {
    let Some(fp) = ui.data.borrow().flatpak.clone() else { return };
    ui.nav.pop_to_tag("root");
    let ui2 = ui.clone();
    let on_done: Rc<dyn Fn()> = Rc::new(move || {
        *ui2.checked.borrow_mut() = None;
        refresh(&ui2);
    });
    txn::run(&ui.window, ui.frame == Frame::App, fp, spec, on_done);
}

// -------------------------------------------------------------- updates ---

fn render_updates(ui: &Rc<Ui>) {
    let page = &ui.updates;
    clear(page);
    let d = ui.data.borrow();
    let count = ui.checked.borrow().as_ref().map(|c| c.found.len()).unwrap_or(0);
    ui.updates_page.set_badge_number(count as u32);
    ui.updates_page.set_needs_attention(count > 0);
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
        let g = group(&format!("Flatpak, {}", heading(&d.installations, inst)), None);
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
                    sub.push_str("  \u{b7}  ");
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

// --------------------------------------------- overview, checks, remotes ---

/// What the checks found that is worth a look: (title, text).
fn warnings(d: &Data) -> Vec<(String, String)> {
    let mut w: Vec<(String, String)> = Vec::new();
    if d.flatpak.is_none() {
        return w;
    }
    match checks::helper() {
        Helper::Present(_) => {}
        Helper::NoService => w.push((
            "No system helper".into(),
            format!("{} is missing. Changes to system-wide Flatpak need root.", checks::HELPER_SERVICE),
        )),
        Helper::Missing(p) => {
            w.push(("System helper missing".into(), format!("the service names {} but it is not there", p.display())))
        }
    }
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let missing = checks::missing_exports(&dirs, &checks::user_data_home(), &d.installations);
    if !missing.is_empty() {
        let list: Vec<String> = missing.iter().map(|p| p.display().to_string()).collect();
        w.push((
            "Not on XDG_DATA_DIRS".into(),
            format!(
                "{}\nFlatpak applications from here may not show in the desktop menus. \
                 /etc/profile.d/flatpak.sh adds them, but only for login shells.",
                list.join("\n")
            ),
        ));
    }
    for g in parse::list::duplicates(&d.apps) {
        let parts: Vec<String> = g
            .iter()
            .map(|i| {
                let place = i.inst().map(Inst::words).unwrap_or(i.installation.as_str());
                format!("{place} {}", if i.version.is_empty() { &i.active } else { &i.version })
            })
            .collect();
        let same = if parse::list::differ(&g) { "different versions" } else { "same commit" };
        w.push((format!("{} is installed twice", g[0].title()), format!("{} ({}): {}", g[0].id, same, parts.join(", "))));
    }
    for r in &d.remotes {
        if r.gpg_warning() {
            let place = Inst::from_label(&r.installation).map(Inst::words).unwrap_or(r.installation.as_str());
            w.push((format!("Remote {} ({place})", r.name), "GPG verification is off".into()));
        }
    }
    w
}

fn render_banner(ui: &Rc<Ui>) {
    let d = ui.data.borrow();
    let n = warnings(&d).len() + d.notes.len();
    ui.banner.set_title(&match n {
        0 => String::new(),
        1 => "1 thing worth a look".to_string(),
        n => format!("{n} things worth a look"),
    });
    ui.banner.set_revealed(n > 0);
}

fn show_overview(ui: &Rc<Ui>) {
    let page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();
    let d = ui.data.borrow();
    let Some(fp) = &d.flatpak else {
        page.append(
            &adw::StatusPage::builder()
                .icon_name("dialog-warning-symbolic")
                .title("flatpak is not installed")
                .description("flatpak is not part of Slackware. It is available from third-party repositories; \
                              slacker search flatpak shows which of yours offer it.")
                .build(),
        );
        drop(d);
        push(ui, "Overview and Checks", scrolled(&page).upcast_ref());
        return;
    };

    let g = group("Flatpak", None);
    g.add(&row("Version", d.version.as_deref().unwrap_or("unknown")));
    g.add(&row("Program", &fp.to_string_lossy()));
    let helper = match checks::helper() {
        Helper::Present(p) => format!("present: {}", p.display()),
        Helper::NoService => format!("not found ({} is missing)", checks::HELPER_SERVICE),
        Helper::Missing(p) => format!("the service names {} but it is not there", p.display()),
    };
    g.add(&row("System helper", &helper));
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

    let w = warnings(&d);
    if !w.is_empty() {
        let g = group("Worth a look", None);
        for (t, s) in &w {
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
    let mut ran = d.ran.clone();
    ran.push("Explore reads the appstream data flatpak keeps on disk (refreshed by every flatpak update); nothing is downloaded.".to_string());
    page.append(&commands_footer(&ran));
    drop(d);
    push(ui, "Overview and Checks", scrolled(&page).upcast_ref());
}

fn show_remotes(ui: &Rc<Ui>) {
    let page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();
    fill_remotes(ui, &page);
    push(ui, "Remotes", scrolled(&page).upcast_ref());
}

/// The Remotes page: every remote of both installations, the disabled ones
/// too, each with a switch that runs `flatpak remote-modify --enable` or
/// `--disable`. Adding or removing remotes is not done here.
fn fill_remotes(ui: &Rc<Ui>, page: &gtk::Box) {
    clear(page);
    let d = ui.data.borrow();
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
        let g = group(&format!("Flatpak, {}", heading(&d.installations, inst)), None);
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
            // A .flatpak file's own remote has no URL: a switch would do nothing.
            if r.is_bundle_origin() {
                let t = tag("from a .flatpak file", false);
                t.set_tooltip_text(Some(
                    "flatpak made this remote when an application was installed from a .flatpak file. \
                     It has no URL; that application is updated by installing a newer file.",
                ));
                ar.add_suffix(&t);
                g.add(&ar);
                continue;
            }
            let sw = gtk::Switch::builder().active(!r.disabled()).valign(gtk::Align::Center).build();
            if let (Ok(on), Ok(off)) = (commands::remote_enable(inst, &r.name, true), commands::remote_enable(inst, &r.name, false)) {
                sw.set_tooltip_text(Some(&commands::display(if r.disabled() { &on } else { &off })));
            }
            let (ui2, page2, name) = (ui.clone(), page.clone(), r.name.clone());
            // The switch moves only when flatpak has done it.
            sw.connect_state_set(move |sw, want| {
                // Moving back to where it is (after Cancel) runs nothing.
                if want != sw.state() {
                    set_remote(&ui2, &page2, sw, inst, &name, want);
                }
                glib::Propagation::Stop
            });
            ar.add_suffix(&sw);
            g.add(&ar);
        }
        page.append(&g);
    }
    page.append(&suggested_remotes(&d.remotes));
    if !d.remotes.is_empty() {
        page.append(&note(
            "A disabled remote stays set up, with its key, but flatpak does not use it: nothing is installed, \
             updated or found from it until it is enabled again. Adding and removing remotes is done from a terminal.",
        ));
    }
    page.append(&commands_footer(&[commands::display(&commands::remotes())]));
}

/// Remotes the user may want to add, as commands to copy. slacker-flatpak
/// does not run them. (name, title, what it is, command, page with the
/// project's own instructions)
const SUGGESTED_REMOTES: &[(&str, &str, &str, &str, &str)] = &[
    (
        "flathub-beta",
        "Flathub Beta",
        "Beta versions of Flathub applications, for testing.",
        "flatpak remote-add --user --if-not-exists flathub-beta https://flathub.org/beta-repo/flathub-beta.flatpakrepo",
        "https://discourse.flathub.org/t/how-to-use-flathub-beta/2111",
    ),
    (
        "fedora",
        "Fedora Flatpaks",
        "Applications built by Fedora from its own packages, with Fedora's runtime (an OCI registry).",
        "flatpak remote-add --user --if-not-exists fedora oci+https://registry.fedoraproject.org",
        "https://fedoramagazine.org/an-introduction-to-fedora-flatpaks/",
    ),
];

fn suggested_remotes(have: &[Remote]) -> adw::PreferencesGroup {
    let g = group(
        "Other remotes you can add",
        Some("Third-party remotes, not checked by slacker-flatpak. Run the command in a terminal as your user, \
              then press Refresh. Check the project's page for its current instructions."),
    );
    for (name, title, what, cmd, url) in SUGGESTED_REMOTES {
        let r = row(title, what);
        r.set_subtitle_selectable(false);
        let added = have.iter().any(|x| x.name == *name && x.installation == Inst::User.label());
        if added {
            r.add_suffix(&tag("already added for your user", false));
        }
        r.add_suffix(&gtk::LinkButton::builder().label("Website").uri(*url).valign(gtk::Align::Center).build());
        g.add(&r);
        if !added {
            let l = gtk::Label::builder()
                .label(*cmd)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .margin_top(10)
                .margin_bottom(10)
                .margin_start(12)
                .margin_end(12)
                .build();
            l.add_css_class("monospace");
            l.add_css_class("caption");
            g.add(&adw::PreferencesRow::builder().child(&l).activatable(false).build());
        }
    }
    g
}

/// Enables at once; asks before disabling.
fn set_remote(ui: &Rc<Ui>, page: &gtk::Box, sw: &gtk::Switch, inst: Inst, name: &str, enable: bool) {
    let argv = match commands::remote_enable(inst, name, enable) {
        Ok(a) => a,
        Err(e) => {
            toast_error(ui, &e);
            return;
        }
    };
    if enable {
        run_remote_modify(ui, page, sw, argv);
        return;
    }
    let dialog = adw::AlertDialog::new(
        Some(&format!("Disable {name}?")),
        Some(&format!(
            "In {}. Applications installed from it stay, but get no updates from it while it is disabled, \
             and nothing can be installed from it.\n\n{}",
            inst.words(),
            commands::display(&argv)
        )),
    );
    dialog.add_responses(&[("cancel", "Cancel"), ("disable", "Disable")]);
    dialog.set_response_appearance("disable", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let (ui2, page2, sw2) = (ui.clone(), page.clone(), sw.clone());
    dialog.connect_response(None, move |_, resp| {
        if resp == "disable" {
            run_remote_modify(&ui2, &page2, &sw2, argv.clone());
        } else {
            sw2.set_active(true);
        }
    });
    dialog.present(Some(&ui.window));
}

/// Runs the remote-modify command. For the system installation flatpak asks
/// its system helper, and polkit decides (it may ask for a password). Then
/// the remotes are read again for this page, and everything is refreshed.
fn run_remote_modify(ui: &Rc<Ui>, page: &gtk::Box, sw: &gtk::Switch, argv: Vec<String>) {
    let Some(fp) = ui.data.borrow().flatpak.clone() else { return };
    sw.set_sensitive(false);
    let (ui, page) = (ui.clone(), page.clone());
    glib::spawn_future_local(async move {
        let shown = commands::display(&argv);
        let out = runner::run(&fp, &argv).await;
        if !out.ok {
            let why = if out.stderr.trim().is_empty() { "failed without a message" } else { out.stderr.trim() };
            toast_error(&ui, &format!("{shown}: {why}"));
        }
        let list = runner::run(&fp, &commands::remotes()).await;
        if list.ok {
            ui.data.borrow_mut().remotes = parse::remotes::parse(&list.stdout);
        }
        fill_remotes(&ui, &page);
        refresh(&ui);
    });
}

// ------------------------------------------------------------- install ---

/// Before installing: which installation, whether it has the remote, and what
/// the application asks for. Then the transaction window does the rest.
fn install_dialog(ui: &Rc<Ui>, hit: &Hit, remote: &str) {
    let (fp, remotes) = {
        let d = ui.data.borrow();
        let Some(fp) = d.flatpak.clone() else { return };
        (fp, d.remotes.clone())
    };
    // Some(true): set up and enabled; Some(false): set up but disabled.
    let state = |inst: Inst| {
        remotes.iter().find(|r| r.name == remote && r.installation == inst.label()).map(|r| !r.disabled())
    };
    let (in_user, in_system) = (state(Inst::User), state(Inst::System));
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
            if has(inst) != Some(true) {
                avail.set_label(&if has(inst).is_some() {
                    format!(
                        "The remote \u{201c}{remote}\u{201d} is disabled in {}. Enable it in Remotes (menu) to install from it there.",
                        inst.words()
                    )
                } else {
                    format!(
                        "The remote \u{201c}{remote}\u{201d} is not set up in {}, so flatpak cannot install from it there.",
                        inst.words()
                    )
                });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_remotes_are_user_commands_with_their_own_checks() {
        for (name, _, _, cmd, url) in SUGGESTED_REMOTES {
            assert!(commands::valid_remote(name).is_ok());
            assert!(cmd.starts_with(&format!("flatpak remote-add --user --if-not-exists {name} ")), "{cmd}");
            // Never a remote without GPG, and nothing else slipped in.
            assert!(!cmd.contains("--no-gpg-verify") && !cmd.contains(';') && !cmd.contains('&'), "{cmd}");
            assert!(url.starts_with("https://"));
        }
    }

    #[test]
    fn gnome_keeps_the_app_frame_everything_else_gets_the_system_one() {
        assert_eq!(frame_mode(Some("GNOME")), Frame::App);
        assert_eq!(frame_mode(Some("ubuntu:GNOME")), Frame::App);
        assert_eq!(frame_mode(Some("KDE")), Frame::System);
        assert_eq!(frame_mode(None), Frame::System);
    }
}
