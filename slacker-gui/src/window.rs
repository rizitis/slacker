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

pub fn build(app: &adw::Application) {
    let resolved = resolve::slacker_binary();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Slacker")
        .default_width(1180)
        .default_height(780)
        .width_request(360)
        .height_request(420)
        .build();

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

    let footer = gtk::Label::builder()
        .label(resolved.path.to_string_lossy().as_ref())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .tooltip_text("The slacker binary this window runs")
        .build();
    footer.add_css_class("sidebar-footer");

    let sidebar_scroller = gtk::ScrolledWindow::builder()
        .child(&nav)
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
    window.set_content(Some(&split));

    let bp = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 680sp").expect("valid breakpoint"),
    );
    bp.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(bp);

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

fn show_log(window: &adw::ApplicationWindow, runner: &Runner) {
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
        .content_width(860)
        .content_height(600)
        .child(&view)
        .build();
    dialog.present(Some(window));
}

fn show_about(window: &adw::ApplicationWindow) {
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
