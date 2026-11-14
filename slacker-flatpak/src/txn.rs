//! The transaction window: runs one flatpak command that changes something
//! (update, uninstall) on a pty, shows its output as it comes, and puts each
//! question flatpak asks in front of the user.
//!
//! Stop is always there, and the window's X and Escape lead to it:
//! - before the user has said yes to anything, Stop answers "n" if flatpak is
//!   asking, or ends it if it is still looking; nothing has changed yet;
//! - after a yes, Stop asks once more, then ends flatpak's process group.
//!   For the system installation the root helper is not stopped (that would
//!   need root); the window says so, and points to `flatpak repair`.

use std::cell::{Cell, RefCell};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::commands::{self, Inst};
use crate::parse::transaction::{self, Prompt};
use crate::pty;

pub struct Spec {
    pub title: String,
    pub inst: Inst,
    pub argv: Vec<String>,
}

struct T {
    win: gtk::Window,
    buffer: gtk::TextBuffer,
    view: gtk::TextView,
    perms: gtk::Label,
    perms_box: gtk::Box,
    question: gtk::Label,
    question_box: gtk::Box,
    yes: gtk::Button,
    no: gtk::Button,
    status: gtk::Label,
    stop: gtk::Button,
    child: RefCell<Option<pty::PtyChild>>,
    pending: RefCell<Vec<u8>>,
    text: RefCell<String>,
    waiting: RefCell<Option<Prompt>>,
    said_yes: Cell<bool>,
    said_no: Cell<bool>,
    stopped: Cell<bool>,
    finished: Cell<bool>,
    inst: Inst,
    on_done: Rc<dyn Fn()>,
}

pub fn run(parent: &gtk::ApplicationWindow, app_frame: bool, flatpak: PathBuf, spec: Spec, on_done: Rc<dyn Fn()>) {
    let shown = commands::display(&spec.argv);

    let command = gtk::Label::builder()
        .label(format!("$ {shown}"))
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .build();
    command.add_css_class("monospace");
    command.add_css_class("dim-label");

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

    let perms = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).build();
    let perms_title = gtk::Label::builder()
        .label("Permissions, as flatpak lists them:")
        .xalign(0.0)
        .build();
    perms_title.add_css_class("heading");
    perms_title.add_css_class("warning");
    let perms_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).visible(false).build();
    perms_box.append(&perms_title);
    perms_box.append(&perms);

    let question = gtk::Label::builder().xalign(0.0).wrap(true).hexpand(true).build();
    question.add_css_class("heading");
    let yes = gtk::Button::with_label("Yes");
    yes.add_css_class("suggested-action");
    let no = gtk::Button::with_label("No");
    let question_box = gtk::Box::builder().spacing(8).visible(false).build();
    question_box.append(&question);
    question_box.append(&no);
    question_box.append(&yes);

    let status = gtk::Label::builder()
        .label("Running\u{2026}")
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .selectable(true)
        .build();
    let stop = gtk::Button::builder().label("Stop").valign(gtk::Align::End).build();
    let bottom = gtk::Box::builder().spacing(8).build();
    bottom.append(&status);
    bottom.append(&stop);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&command);
    content.append(&scroller);
    content.append(&perms_box);
    content.append(&question_box);
    content.append(&bottom);

    let tv = adw::ToolbarView::new();
    if app_frame {
        tv.add_top_bar(&adw::HeaderBar::new());
    }
    tv.set_content(Some(&content));
    let win = gtk::Window::builder()
        .title(&spec.title)
        .transient_for(parent)
        .modal(true)
        .resizable(true)
        .default_width(840)
        .default_height(620)
        .child(&tv)
        .build();
    if app_frame {
        win.set_titlebar(Some(&gtk::Box::builder().visible(false).build()));
    }

    let t = Rc::new(T {
        win: win.clone(),
        buffer,
        view,
        perms,
        perms_box,
        question,
        question_box,
        yes: yes.clone(),
        no: no.clone(),
        status,
        stop: stop.clone(),
        child: RefCell::new(None),
        pending: RefCell::default(),
        text: RefCell::default(),
        waiting: RefCell::new(None),
        said_yes: Cell::new(false),
        said_no: Cell::new(false),
        stopped: Cell::new(false),
        finished: Cell::new(false),
        inst: spec.inst,
        on_done,
    });

    {
        let t = t.clone();
        yes.connect_clicked(move |_| answer(&t, true));
    }
    {
        let t = t.clone();
        no.connect_clicked(move |_| answer(&t, false));
    }
    {
        let t = t.clone();
        stop.connect_clicked(move |_| stop_or_close(&t));
    }
    {
        let t = t.clone();
        win.connect_close_request(move |_| {
            if t.finished.get() {
                glib::Propagation::Proceed
            } else {
                stop_or_close(&t);
                glib::Propagation::Stop
            }
        });
    }
    let esc = gtk::EventControllerKey::new();
    {
        let t = t.clone();
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                stop_or_close(&t);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    win.add_controller(esc);
    win.present();
    // Keep the selectable command line from taking focus and showing selected.
    t.view.grab_focus();

    match pty::spawn(&flatpak, &spec.argv) {
        Err(e) => {
            t.text.borrow_mut().push_str(&format!("could not start flatpak: {e}\n"));
            show_text(&t);
            finish(&t, None);
        }
        Ok(child) => {
            let fd = child.master.as_raw_fd();
            *t.child.borrow_mut() = Some(child);
            // The pty is read without blocking, a few times a second, on the
            // main loop (glib 0.22 keeps its fd watches in a separate crate).
            let t2 = t.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(40), move || read_some(&t2, fd));
        }
    }
}

fn read_some(t: &Rc<T>, fd: i32) -> glib::ControlFlow {
    let mut ended = false;
    let mut got = false;
    loop {
        match pty::read(fd) {
            pty::Read::Data(d) => {
                got = true;
                t.pending.borrow_mut().extend_from_slice(&d);
            }
            pty::Read::Again => break,
            pty::Read::End => {
                ended = true;
                break;
            }
        }
    }
    // Decode what is complete UTF-8; keep a split character for later.
    {
        let mut p = t.pending.borrow_mut();
        let good = match std::str::from_utf8(&p) {
            Ok(s) => s.len(),
            Err(e) if ended || e.error_len().is_some() => p.len(),
            Err(e) => e.valid_up_to(),
        };
        let chunk: Vec<u8> = p.drain(..good).collect();
        t.text.borrow_mut().push_str(&String::from_utf8_lossy(&chunk));
    }
    if got || ended {
        show_text(t);
    }
    if ended {
        wait_for_exit(t.clone());
        return glib::ControlFlow::Break;
    }
    glib::ControlFlow::Continue
}

fn show_text(t: &Rc<T>) {
    let clean = transaction::clean(&t.text.borrow());
    t.buffer.set_text(&clean);
    let mut end = t.buffer.end_iter();
    t.view.scroll_to_iter(&mut end, 0.0, false, 0.0, 1.0);

    let perms = transaction::new_permissions(&clean);
    if !perms.is_empty() {
        let lines: Vec<String> = perms.iter().map(|(h, body)| format!("{h}: {}", body.join(", "))).collect();
        t.perms.set_label(&lines.join("\n"));
        t.perms_box.set_visible(true);
    }

    if t.finished.get() {
        return;
    }
    let partial = clean.rsplit('\n').next().unwrap_or("");
    let p = transaction::prompt(partial);
    match &p {
        Some(Prompt::YesNo { question, default_yes }) => {
            t.question.set_label(question);
            t.yes.set_visible(true);
            t.no.set_visible(true);
            if *default_yes {
                t.win.set_default_widget(Some(&t.yes));
            }
            t.question_box.set_visible(true);
            t.status.set_label("flatpak is waiting for your answer.");
        }
        Some(Prompt::Other(q)) => {
            t.question.set_label(&format!("flatpak asks: {q}\nThis window cannot answer that question; Stop ends flatpak."));
            t.yes.set_visible(false);
            t.no.set_visible(false);
            t.question_box.set_visible(true);
            t.status.set_label("flatpak is waiting.");
        }
        None => {
            t.question_box.set_visible(false);
            if t.waiting.borrow().is_some() {
                t.status.set_label("Running\u{2026}");
            }
        }
    }
    *t.waiting.borrow_mut() = p;
}

fn answer(t: &Rc<T>, yes: bool) {
    if !matches!(*t.waiting.borrow(), Some(Prompt::YesNo { .. })) {
        return;
    }
    let fd = match t.child.borrow().as_ref() {
        Some(c) => c.master.as_raw_fd(),
        None => return,
    };
    if yes {
        t.said_yes.set(true);
    } else {
        t.said_no.set(true);
    }
    if pty::write_all(fd, if yes { b"y\n" } else { b"n\n" }).is_ok() {
        *t.waiting.borrow_mut() = None;
        t.question_box.set_visible(false);
        t.status.set_label("Running\u{2026}");
    }
}

fn stop_or_close(t: &Rc<T>) {
    if t.finished.get() {
        t.win.destroy();
        return;
    }
    // Asked a yes/no question before any yes: "no" is the clean way out.
    if !t.said_yes.get() && matches!(*t.waiting.borrow(), Some(Prompt::YesNo { .. })) {
        answer(t, false);
        return;
    }
    // Nothing confirmed yet: flatpak has changed nothing, end it.
    if !t.said_yes.get() {
        kill(t);
        return;
    }
    let body = if t.inst == Inst::System {
        "flatpak is changing the system-wide Flatpak installation. Stopping it now can leave an update half done. \
         The system helper runs as root and is not stopped by this; it may finish its current step.\n\n\
         Afterwards, `flatpak repair --system` checks the installation."
    } else {
        "flatpak is changing your user's Flatpak installation. Stopping it now can leave an update half done.\n\n\
         Afterwards, `flatpak repair --user` checks the installation."
    };
    let d = adw::AlertDialog::new(Some("Stop flatpak?"), Some(body));
    d.add_response("cancel", "Keep Running");
    d.add_response("stop", "Stop");
    d.set_response_appearance("stop", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    let t2 = t.clone();
    d.connect_response(None, move |_, r| {
        if r == "stop" && !t2.finished.get() {
            kill(&t2);
        }
    });
    d.present(Some(&t.win));
}

/// SIGTERM to flatpak's process group, SIGKILL three seconds later if it is
/// still there.
fn kill(t: &Rc<T>) {
    let Some(pid) = t.child.borrow().as_ref().map(|c| c.pid()) else { return };
    t.stopped.set(true);
    t.status.set_label("Stopping\u{2026}");
    pty::signal_group(pid, libc::SIGTERM);
    let t2 = t.clone();
    glib::timeout_add_seconds_local_once(3, move || {
        if !t2.finished.get() {
            pty::signal_group(pid, libc::SIGKILL);
        }
    });
}

fn wait_for_exit(t: Rc<T>) {
    glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        let status = match t.child.borrow_mut().as_mut().map(|c| c.child.try_wait()) {
            Some(Ok(Some(s))) => Some(s),
            Some(Ok(None)) => return glib::ControlFlow::Continue,
            _ => None,
        };
        finish(&t, status);
        glib::ControlFlow::Break
    });
}

fn finish(t: &Rc<T>, status: Option<std::process::ExitStatus>) {
    t.finished.set(true);
    t.question_box.set_visible(false);
    let text = transaction::clean(&t.text.borrow());
    let errors = transaction::error_lines(&text);
    let code = status.and_then(|s| s.code());
    let mut msg = if t.stopped.get() {
        "Stopped.".to_string()
    } else if t.said_no.get() && code != Some(0) {
        // flatpak exits 1 when a question is answered no.
        format!("Cancelled: you answered no. Nothing was changed by that step (flatpak exit code {}).", code.unwrap_or(-1))
    } else {
        match code {
            Some(0) => "Finished (exit code 0).".to_string(),
            Some(c) => format!("Failed (exit code {c})."),
            None => "Ended without an exit code.".to_string(),
        }
    };
    if !errors.is_empty() {
        msg.push_str(&format!(
            "\nflatpak reported {} error line(s); they are in the output above.",
            errors.len()
        ));
    }
    if let Some(h) = transaction::hint(&text) {
        msg.push('\n');
        msg.push_str(h);
    }
    if t.stopped.get() && t.said_yes.get() {
        msg.push_str(&format!("\nTo check the installation: {}", commands::display(&commands::repair(t.inst))));
    }
    t.status.set_label(&msg);
    if (code != Some(0) && !t.said_no.get()) || !errors.is_empty() || t.stopped.get() {
        t.status.add_css_class("warning");
    }
    t.stop.set_label("Close");
    t.stop.add_css_class("suggested-action");
    *t.child.borrow_mut() = None;
    (t.on_done)();
}
