//! Main window: sidebar navigation, per-page header actions, toasts, and the
//! command output dialog.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::ctx::Ctx;
use crate::output::terminal_view;
use crate::pages::{self, Page};
use crate::resolve;
use crate::runner::Runner;
use crate::zoom;

/// `SLACKER_GUI_FRAME=system` or `=app` chooses who draws the window frame.
const FRAME_ENV: &str = "SLACKER_GUI_FRAME";

/// Who draws the window frame: the title bar and the minimise, maximise and
/// close buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Frame {
    /// The window manager, as for every other window (KWin, Xfwm4, ...).
    System,
    /// slacker-gui itself, in the libadwaita style.
    App,
}

/// SLACKER_GUI_FRAME decides when set. Otherwise GNOME gets the app's own
/// frame, as every GNOME application has one and its compositor draws none on
/// Wayland; every other desktop gets the window manager's.
fn frame_mode(env: Option<&str>, desktop: Option<&str>) -> Frame {
    match env.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("system") => return Frame::System,
        Some("app") => return Frame::App,
        _ => {}
    }
    let gnome = desktop.is_some_and(|d| d.split(':').any(|p| p.trim().eq_ignore_ascii_case("gnome")));
    if gnome {
        Frame::App
    } else {
        Frame::System
    }
}

pub fn build(app: &adw::Application) {
    let resolved = resolve::slacker_binary();

    // A plain GTK window, not an adw::ApplicationWindow: libadwaita always
    // draws its own frame, so the window could never wear the one the window
    // manager gives every other window (KWin, Xfwm4, ...).
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Slacker")
        .default_width(1180)
        .default_height(780)
        .build();
    let frame = frame_mode(
        std::env::var(FRAME_ENV).ok().as_deref(),
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
    );
    if frame == Frame::App {
        // An empty title bar makes GTK draw the frame itself (as libadwaita
        // does); the header bars below then carry the window buttons.
        window.set_titlebar(Some(&gtk::Box::builder().visible(false).build()));
    }

    // ---- content side ----------------------------------------------------
    let spinner = gtk::Spinner::builder().visible(false).tooltip_text("slacker is running").build();
    let runner = Runner::new(resolved.path.clone(), spinner.clone());
    let toasts = adw::ToastOverlay::new();
    let ctx = Ctx::new(runner.clone(), window.clone(), toasts.clone());

    let pages: Vec<Page> = vec![
        pages::overview::page(&ctx, &resolved),
        pages::search::page(&ctx),
        pages::updates::page(&ctx),
        pages::changelog::page(&ctx),
        pages::packages::page(&ctx),
        pages::repositories::page(&ctx),
        pages::mirrors::page(&ctx),
        pages::rules::page(&ctx),
        pages::history::page(&ctx),
    ];

    let stack = gtk::Stack::builder()
        .hhomogeneous(false)
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(150)
        .build();
    let header_actions = gtk::Stack::builder().hhomogeneous(false).build();
    for p in &pages {
        stack.add_named(&p.widget, Some(p.name));
        let slot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        if let Some(h) = &p.header {
            slot.append(h);
        }
        header_actions.add_named(&slot, Some(p.name));
    }
    toasts.set_child(Some(&stack));

    let menu = gio::Menu::new();
    menu.append(Some("Command Output"), Some("win.command-output"));
    menu.append(Some("About Slacker GUI"), Some("win.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text("Main Menu")
        .build();
    let content_title = adw::WindowTitle::new("Doctor", "slacker status");
    let content_header = adw::HeaderBar::builder().title_widget(&content_title).build();
    // With the window manager's frame its buttons are there already.
    content_header.set_show_start_title_buttons(frame == Frame::App);
    content_header.set_show_end_title_buttons(frame == Frame::App);
    content_header.pack_end(&menu_button);
    content_header.pack_end(&spinner);
    content_header.pack_start(&header_actions);

    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&content_header);
    content_view.set_content(Some(&toasts));
    let content_page = adw::NavigationPage::builder()
        .title("Doctor")
        .tag("content")
        .child(&content_view)
        .build();

    // ---- sidebar ---------------------------------------------------------
    let nav = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
    nav.add_css_class("navigation-sidebar");
    for p in &pages {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.add_css_class("nav-row");
        let i = gtk::Image::from_icon_name(p.icon);
        let l = gtk::Label::builder().label(p.title).xalign(0.0).hexpand(true).build();
        b.append(&i);
        b.append(&l);
        let row = gtk::ListBoxRow::builder().child(&b).build();
        row.set_widget_name(p.name);
        nav.append(&row);
    }

    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let mark = gtk::Label::new(Some("\u{25C6}"));
    mark.add_css_class("brand-mark");
    let name = gtk::Label::new(Some("slacker"));
    name.add_css_class("brand");
    brand.append(&mark);
    brand.append(&name);
    let sidebar_header = adw::HeaderBar::builder().title_widget(&brand).build();
    sidebar_header.set_show_start_title_buttons(frame == Frame::App);
    sidebar_header.set_show_end_title_buttons(frame == Frame::App);

    let footer = gtk::Label::builder()
        .label(resolved.path.to_string_lossy().as_ref())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .tooltip_text("The slacker binary this window runs")
        .build();
    footer.add_css_class("sidebar-footer");

    // The page list, then the slacker mark centred in the free space under it.
    let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_box.append(&nav);
    if let Some(logo) = sidebar_logo() {
        sidebar_box.append(&logo);
    }

    let sidebar_scroller = gtk::ScrolledWindow::builder()
        .child(&sidebar_box)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(&sidebar_scroller));
    sidebar_view.add_bottom_bar(&footer);
    let sidebar_page = adw::NavigationPage::builder()
        .title("Slacker")
        .tag("sidebar")
        .child(&sidebar_view)
        .build();

    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content_page)
        .min_sidebar_width(210.0)
        .max_sidebar_width(260.0)
        .build();
    // A narrow window shows the sidebar and the page one at a time. An
    // adw::BreakpointBin carries the breakpoint in a plain GTK window (an
    // adw::ApplicationWindow did it before), and lets the window be made
    // narrower than the two side by side would need.
    // The numbers come from the pages themselves: side by side the sidebar
    // and a page need about 745 px, a page alone about 535 px. Collapsing any
    // later, or letting the window get narrower, cuts the page off (and
    // libadwaita warns that the split view exceeds the bin).
    let bin = adw::BreakpointBin::builder()
        .child(&split)
        .width_request(540)
        .height_request(420)
        .build();
    let bp = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 760sp").expect("valid breakpoint"),
    );
    bp.add_setter(&split, "collapsed", Some(&true.to_value()));
    bin.add_breakpoint(bp);
    window.set_child(Some(&bin));

    // ---- navigation and lazy loading ---------------------------------------
    let loaded: Rc<RefCell<HashSet<&'static str>>> = Rc::default();
    let current: Rc<RefCell<&'static str>> = Rc::new(RefCell::new("doctor"));
    let pages = Rc::new(pages);

    let show: Rc<dyn Fn(&str)> = {
        let (pages, loaded, current) = (pages.clone(), loaded.clone(), current.clone());
        let (stack, header_actions, content_title, content_page, split) = (
            stack.clone(),
            header_actions.clone(),
            content_title.clone(),
            content_page.clone(),
            split.clone(),
        );
        Rc::new(move |name: &str| {
            let Some(p) = pages.iter().find(|p| p.name == name) else { return };
            stack.set_visible_child_name(p.name);
            header_actions.set_visible_child_name(p.name);
            content_title.set_title(p.title);
            content_title.set_subtitle(p.subtitle);
            content_page.set_title(p.title);
            split.set_show_content(true);
            *current.borrow_mut() = p.name;
            let first = loaded.borrow_mut().insert(p.name);
            if let Some(load) = &p.load {
                // Search only takes focus; data pages load once, then on demand.
                if first || p.name == "search" {
                    load();
                }
            }
        })
    };
    {
        let show = show.clone();
        nav.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                show(r.widget_name().as_str());
            }
        });
    }

    // After an install/remove/upgrade: reload the visible page if it shows
    // system state, and let the others reload when next opened.
    {
        let (pages, loaded, current) = (pages.clone(), loaded.clone(), current.clone());
        ctx.on_system_changed(move || {
            let now = *current.borrow();
            // Mark the others stale first and let go of `loaded`: reloading
            // runs page code, which is free to touch it again.
            let visible = {
                let mut l = loaded.borrow_mut();
                let mut visible = None;
                for p in pages.iter().filter(|p| p.reload_on_change) {
                    if p.name == now {
                        visible = p.load.clone();
                    } else {
                        l.remove(p.name);
                    }
                }
                visible
            };
            if let Some(load) = visible {
                load();
            }
        });
    }

    {
        let (runner, win) = (runner.clone(), window.clone());
        let action = gio::SimpleAction::new("command-output", None);
        action.connect_activate(move |_, _| show_log(&win, &runner));
        window.add_action(&action);
        app.set_accels_for_action("win.command-output", &["<Control><Shift>o"]);

        let win = window.clone();
        let about = gio::SimpleAction::new("about", None);
        about.connect_activate(move |_, _| show_about(&win));
        window.add_action(&about);
    }

    // Ctrl with the wheel, or with +, - and 0, scales this window. Both report
    // the new size in a toast, so the change is legible even on a page with
    // little text on it, and so the way back to the desktop's own size is
    // visible once someone has zoomed.
    {
        let announce = {
            let ctx = ctx.clone();
            move |percent: i32| {
                ctx.toast(&match percent {
                    100 => "Font size: the desktop's own".to_string(),
                    p => format!("Font size: {p}% of the desktop's"),
                });
            }
        };
        zoom::install_actions(app, announce.clone());
        zoom::attach(&window, announce);
    }

    // Ctrl+F: go to Search.
    {
        let nav = nav.clone();
        let keys = gtk::ShortcutController::new();
        keys.set_scope(gtk::ShortcutScope::Managed);
        let action = gtk::CallbackAction::new(move |_, _| {
            if let Some(row) = nav.row_at_index(1) {
                nav.select_row(Some(&row));
            }
            glib::Propagation::Stop
        });
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(action),
        ));
        window.add_controller(keys);
    }

    if let Some(first) = nav.row_at_index(0) {
        nav.select_row(Some(&first));
    }
    window.present();
}

fn show_log(window: &gtk::ApplicationWindow, runner: &Runner) {
    let clear = gtk::Button::from_icon_name("edit-clear-all-symbolic");
    clear.set_tooltip_text(Some("Clear"));
    {
        let buffer = runner.log().clone();
        clear.connect_clicked(move |_| buffer.set_text(""));
    }
    let header = adw::HeaderBar::new();
    header.pack_start(&clear);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("dialog-body");
    if runner.log().char_count() == 0 {
        body.append(&crate::widgets::status_page(
            "utilities-terminal-symbolic",
            "No commands yet",
            "Every slacker command the window runs appears here with its full output.",
        ));
    } else {
        body.append(&terminal_view(runner.log()));
    }

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&body));
    let dialog = adw::Dialog::builder()
        .title("Command output")
        .child(&view)
        .build();
    crate::widgets::present_fitted(&dialog, window, (860, 600));
}

fn show_about(window: &gtk::ApplicationWindow) {
    let about = adw::AboutDialog::builder()
        .application_name("Slacker GUI")
        .application_icon(crate::APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("rizitis")
        .comments("A graphical front-end for the slacker package manager. Everything it shows comes from slacker itself.")
        .website("https://forge.slackware.nl/rizitis/slacker")
        .license_type(gtk::License::Apache20)
        .build();
    about.add_credit_section(Some("Maintainer"), &["rizitis"]);
    about.present(Some(window));
}

/// The slacker mark shown under the sidebar's page list. The SVG is compiled
/// in and rasterised once through gdk-pixbuf (librsvg), at twice the shown
/// size so it stays sharp on HiDPI screens. Without an SVG loader there is
/// simply no logo; nothing else changes.
fn sidebar_logo() -> Option<gtk::Image> {
    const SVG: &[u8] = include_bytes!("slacker-mark.svg");
    const SIZE: i32 = 140;
    let loader = gtk::gdk_pixbuf::PixbufLoader::with_type("svg").ok()?;
    loader.set_size(SIZE * 2, SIZE * 2);
    loader.write(SVG).ok()?;
    loader.close().ok()?;
    let pixbuf = loader.pixbuf()?;
    let texture = gtk::gdk::Texture::for_pixbuf(&pixbuf);
    let image = gtk::Image::from_paintable(Some(&texture));
    image.set_pixel_size(SIZE);
    image.set_vexpand(true);
    image.set_valign(gtk::Align::Center);
    image.set_halign(gtk::Align::Center);
    image.add_css_class("sidebar-logo");
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::{frame_mode, Frame};

    #[test]
    fn the_window_frame_follows_the_desktop() {
        assert_eq!(frame_mode(None, Some("KDE")), Frame::System);
        assert_eq!(frame_mode(None, Some("XFCE")), Frame::System);
        assert_eq!(frame_mode(None, None), Frame::System);
        assert_eq!(frame_mode(None, Some("GNOME")), Frame::App);
        assert_eq!(frame_mode(None, Some("ubuntu:GNOME")), Frame::App);
        // The variable wins, whatever the desktop.
        assert_eq!(frame_mode(Some("app"), Some("KDE")), Frame::App);
        assert_eq!(frame_mode(Some(" System "), Some("GNOME")), Frame::System);
        assert_eq!(frame_mode(Some("typo"), Some("KDE")), Frame::System);
    }
}
