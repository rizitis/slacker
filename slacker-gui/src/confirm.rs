//! Root actions. slacker runs without `--yes`, so it asks its own questions —
//! the plan and "Proceed? [y/N]", the package picker, a conflict choice —
//! and they are put to the user in the window that shows its output, where
//! the plan can be read while answering. Only for the two commands that
//! change something without asking (`unfrozen`, `unpin`) does the GUI ask
//! first, showing the exact command line.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::commands::Spec;
use crate::ctx::Ctx;
use crate::output::{append, terminal_view, Chunk, Feed};
use crate::parse::prompt::{self, Question};
use crate::runner::{describe, Input, Status};

/// Finished lines kept for recognising a question (the options and the
/// numbered items sit just above it).
const CONTEXT_LINES: usize = 400;

/// How long output must stop on an unrecognised unfinished line before the
/// window offers to stop answering.
///
/// Two waits, because going quiet means two very different things. A line that
/// READS like a question and then stops is worth mentioning quickly. Anything
/// else — a counter, the row of dots dracut prints while it builds an initrd,
/// a hook that thinks for a while — is just work in progress, and minutes of
/// silence there are normal. One short wait for everything was the reason this
/// notice kept appearing during kernel upgrades, when nothing was wrong.
const QUIET_QUESTION: std::time::Duration = std::time::Duration::from_secs(6);
const QUIET_OTHER: std::time::Duration = std::time::Duration::from_secs(120);

/// What the command output asks for, and the least it will come down to.
///
/// These are not a split of the dialog: the output asks for OUTPUT_WANTED and
/// keeps every pixel left over after the question, so a tall window simply
/// gives it more. It comes down towards OUTPUT_LEAST only on a window too
/// short for both — and the question is the one that must stay whole there,
/// because it carries the answer being asked for, while the output still
/// scrolls.
const OUTPUT_WANTED: i32 = 260;
const OUTPUT_LEAST: i32 = 120;

/// Options of a choice per row. Two keeps each card wide enough for its
/// explanation to stay on one line at the sizes a dialog is normally opened at.
const OPTION_COLUMNS: usize = 2;

/// The room the "nothing has been printed for a while" paragraph is given, and
/// where it starts to scroll instead of growing.
const NOTICE_TEXT_MIN: i32 = 100;
const NOTICE_TEXT_MAX: i32 = 200;

/// Does this unfinished line read like something waiting for an answer?
///
/// A prompt ends where the cursor should be: after a question mark, a colon,
/// an arrow, or an option list such as `[y/N]`. Progress output ends in dots,
/// a percentage, a slash. `Building module(s)` deliberately does NOT count:
/// only a bracket holding y/n-style options does, not any bracket.
fn looks_like_question(line: &str) -> bool {
    let t = line.trim_end();
    let Some(last) = t.chars().last() else {
        return false;
    };
    match last {
        '?' | ':' | '>' => true,
        ']' | ')' => {
            let open = if last == ']' { '[' } else { '(' };
            let Some(i) = t.rfind(open) else { return false };
            let inner = &t[i + open.len_utf8()..t.len() - last.len_utf8()];
            let yes_no_ish = |c: char| matches!(c.to_ascii_lowercase(), 'y' | 'n' | 'a' | 'q');
            inner.contains('/') && inner.chars().all(|c| c.is_ascii_alphabetic() || c == '/')
                || (inner.chars().count() == 1 && inner.chars().all(yes_no_ish))
        }
        _ => false,
    }
}

/// Shows `dialog` and runs `then` if the user picked `response`.
///
/// When an alert dialog closes while one of its own buttons has keyboard
/// focus, the window keeps pointing at that (now hidden) button and no key
/// reaches anything until the user clicks. So after any response the stale
/// focus is cleared, and `then` runs from idle, once the dialog is gone.
pub fn after_choice(
    dialog: &adw::AlertDialog,
    response: &'static str,
    parent: &impl IsA<gtk::Widget>,
    then: impl FnOnce() + 'static,
) {
    let window = parent.as_ref().root().and_downcast::<gtk::Window>();
    let then = Rc::new(RefCell::new(Some(then)));
    dialog.connect_response(None, move |_, r| {
        let run = if r == response { then.borrow_mut().take() } else { None };
        let window = window.clone();
        glib::idle_add_local_once(move || {
            if let Some(w) = &window {
                if GtkWindowExt::focus(w).is_some_and(|f| !f.is_mapped()) {
                    GtkWindowExt::set_focus(w, None::<&gtk::Widget>);
                }
            }
            if let Some(f) = run {
                f();
            }
        });
    });
    dialog.present(Some(parent));
}

pub struct Action {
    pub spec: Spec,
    /// e.g. "Install vim"
    pub title: String,
    /// The button that starts it when the GUI asks first, e.g. "Unpin".
    pub verb: String,
    pub destructive: bool,
}

/// Whether a finished root action may have changed installed packages:
/// anything except a dismissed or refused authorization, or a failure to
/// start at all.
pub fn may_have_changed(status: &Status) -> bool {
    !matches!(status, Status::Exited(126 | 127) | Status::SpawnFailed(_))
}

pub fn run_as_root(ctx: &Ctx, action: Action, on_finish: impl FnOnce(&Status) + 'static) {
    // slacker shows its plan and asks before it writes: its question is the
    // confirmation, asked about what it will really do.
    if action.spec.confirms {
        transaction(ctx, action, on_finish);
        return;
    }

    let command = ctx.runner.argv(&action.spec).join(" ");
    let dialog = adw::AlertDialog::builder()
        .heading(format!("{}?", action.title))
        .body("slacker makes this change as soon as it runs. You may be asked for your password.")
        .close_response("cancel")
        .default_response(if action.destructive { "cancel" } else { "run" })
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("run", &action.verb)]);
    dialog.set_response_appearance(
        "run",
        if action.destructive {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        },
    );
    let cmd = gtk::Label::builder()
        .label(&command)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .xalign(0.0)
        .build();
    cmd.add_css_class("command-line");
    dialog.set_extra_child(Some(&cmd));

    let ctx2 = ctx.clone();
    after_choice(&dialog, "run", &ctx.window, move || transaction(&ctx2, action, on_finish));
}

/// The part of the window where slacker's questions are answered.
#[derive(Clone)]
struct Asker {
    input: Input,
    area: gtk::Box,
    state: gtk::Label,
    destructive: bool,
    /// The question on screen, so a repeated redraw does not rebuild it.
    showing: Rc<RefCell<Option<Question>>>,
}

impl Asker {
    fn answer(&self, answer: &str) {
        self.clear();
        self.state.set_text("Running\u{2026}");
        self.input.send(answer);
    }

    fn clear(&self) {
        *self.showing.borrow_mut() = None;
        while let Some(c) = self.area.first_child() {
            self.area.remove(&c);
        }
        // Back to the accent panel a real question wears.
        self.area.remove_css_class("notice");
        self.area.set_visible(false);
    }

    fn ask(&self, q: Question) {
        if self.showing.borrow().as_ref() == Some(&q) {
            return;
        }
        self.clear();
        self.state.set_text("slacker is asking");
        match &q {
            Question::YesNo { text, default_yes } => self.yes_no(text, *default_yes),
            Question::Choice { heading, options } => self.choice(heading.as_deref(), options),
            Question::Pick { .. } => self.pick(&q),
        }
        self.area.set_visible(true);
        *self.showing.borrow_mut() = Some(q);
    }

    fn heading(&self, text: &str) {
        let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
        l.add_css_class("heading");
        self.area.append(&l);
    }

    fn yes_no(&self, text: &str, default_yes: bool) {
        self.heading(text);
        let row = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
        let no = gtk::Button::with_label("No");
        let yes = gtk::Button::with_label("Yes");
        yes.add_css_class(if self.destructive { "destructive-action" } else { "suggested-action" });
        for b in [&no, &yes] {
            b.add_css_class("pill");
            row.append(b);
        }
        self.area.append(&row);
        {
            let a = self.clone();
            no.connect_clicked(move |_| a.answer("n"));
        }
        {
            let a = self.clone();
            yes.connect_clicked(move |_| a.answer("y"));
        }
        // Enter takes slacker's own default, as it would on a terminal.
        let default = if default_yes { yes } else { no };
        glib::idle_add_local_once(move || {
            default.grab_focus();
        });
    }

    /// The options are laid out as cards, two to a row, instead of one tall
    /// column.
    ///
    /// A conflict question has four options; stacked, they push the command
    /// output — the part being read in order to decide — down to a slit. Two
    /// to a row they take half the height, and that half goes back to the
    /// output. Each card carries its own explanation, which wraps to as many
    /// lines as the width it is given needs.
    fn choice(&self, heading: Option<&str>, options: &[prompt::Opt]) {
        if let Some(h) = heading {
            self.heading(h);
        }
        let columns = options.len().min(OPTION_COLUMNS).max(1);
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let mut row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let mut first_default = None;
        for (i, o) in options.iter().enumerate() {
            if i % columns == 0 {
                row = gtk::Box::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .spacing(8)
                    .homogeneous(true)
                    .build();
                rows.append(&row);
            }
            let title = gtk::Label::builder().label(capitalise(&o.label)).xalign(0.0).build();
            title.add_css_class("heading");
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            top.append(&title);
            if o.default {
                let tag = gtk::Label::new(Some("default"));
                tag.add_css_class("tag");
                tag.set_valign(gtk::Align::Center);
                top.append(&tag);
            }
            let detail = gtk::Label::builder()
                .label(&o.detail)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build();
            detail.add_css_class("dim-label");
            let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
            inner.append(&top);
            inner.append(&detail);
            let card = gtk::Button::builder().child(&inner).build();
            card.add_css_class("card");
            card.add_css_class("option-card");
            if o.default {
                first_default.get_or_insert(card.clone());
            }
            let (a, key) = (self.clone(), o.key.clone());
            card.connect_clicked(move |_| a.answer(&key));
            row.append(&card);
        }
        // A last row that is not full is padded, so its cards keep the width
        // the rows above gave theirs instead of stretching to fill it.
        for _ in options.len()..options.len().next_multiple_of(columns) {
            row.append(&gtk::Box::new(gtk::Orientation::Horizontal, 0));
        }
        self.area.append(&rows);
        if let Some(r) = first_default {
            glib::idle_add_local_once(move || {
                r.grab_focus();
            });
        }
    }

    fn pick(&self, q: &Question) {
        let Question::Pick { heading, items, none, .. } = q else { return };
        if !heading.is_empty() {
            self.heading(heading);
        }

        let checks: Rc<Vec<(u32, gtk::CheckButton)>> = Rc::new(
            items
                .iter()
                .map(|i| (i.number, gtk::CheckButton::builder().active(true).valign(gtk::Align::Center).build()))
                .collect(),
        );
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        list.add_css_class("boxed-list");
        for (item, (_, check)) in items.iter().zip(checks.iter()) {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&item.text)
                .activatable_widget(check)
                .build();
            row.add_prefix(check);
            list.append(&row);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(260)
            .build();
        self.area.append(&scroller);

        let bar = gtk::Box::builder().spacing(8).build();
        let all = gtk::Button::with_label("Select All");
        let clear = gtk::Button::with_label("Select None");
        for b in [&all, &clear] {
            b.add_css_class("flat");
            bar.append(b);
        }
        let spacer = gtk::Box::builder().hexpand(true).build();
        bar.append(&spacer);
        let cancel = gtk::Button::with_label("Cancel");
        let go = gtk::Button::new();
        go.add_css_class("suggested-action");
        for b in [&cancel, &go] {
            b.add_css_class("pill");
            bar.append(b);
        }
        self.area.append(&bar);

        let total = items.len();
        let refresh = {
            let (checks, go) = (checks.clone(), go.clone());
            Rc::new(move || {
                let n = checks.iter().filter(|(_, c)| c.is_active()).count();
                go.set_label(&if n == total { "Continue with All".to_string() } else { format!("Continue with {n} of {total}") });
                go.set_sensitive(n > 0);
            })
        };
        refresh();
        for (_, c) in checks.iter() {
            let r = refresh.clone();
            c.connect_toggled(move |_| r());
        }
        for (button, state) in [(&all, true), (&clear, false)] {
            let checks = checks.clone();
            button.connect_clicked(move |_| {
                for (_, c) in checks.iter() {
                    c.set_active(state);
                }
            });
        }
        {
            let (a, none) = (self.clone(), none.to_string());
            cancel.connect_clicked(move |_| a.answer(&none));
        }
        {
            let (a, q, checks) = (self.clone(), q.clone(), checks.clone());
            go.connect_clicked(move |_| {
                let chosen: Vec<u32> = checks.iter().filter(|(_, c)| c.is_active()).map(|(n, _)| *n).collect();
                if let Some(ans) = q.pick_answer(&chosen) {
                    a.answer(&ans);
                }
            });
        }
        glib::idle_add_local_once(move || {
            go.grab_focus();
        });
    }

    /// Output stopped on a line that is not one of the known questions. This is
    /// a note, not an alarm: work that goes quiet (building an initrd, a slow
    /// hook) looks exactly like this from outside, so it says what it sees and
    /// leaves the judgement, and the action, to the person watching.
    fn unrecognised(&self, line: &str) {
        self.clear();
        self.area.add_css_class("notice");
        let l = gtk::Label::builder()
            .label(format!(
                "Nothing has been printed for a while. The last line is:\n\u{201c}{}\u{201d}\n\
                 That is usually fine, some steps are quiet for a long time while they work. \
                 If it looks stuck on a question this window cannot answer, you can stop \
                 answering: from then on every question takes slacker\u{2019}s own default, \
                 which never installs, upgrades or removes a package and never changes a setting.",
                line.trim()
            ))
            .xalign(0.0)
            .wrap(true)
            .build();
        l.add_css_class("dim-label");
        // A paragraph that wraps is the one thing here whose height is not
        // known until it is drawn, and the button under it must stay reachable
        // whatever that height turns out to be. In its own scroller the text
        // gives way instead of pushing the button out of the panel.
        let text = gtk::ScrolledWindow::builder()
            .child(&l)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .min_content_height(NOTICE_TEXT_MIN)
            .max_content_height(NOTICE_TEXT_MAX)
            .build();
        let stop = gtk::Button::with_label("Stop Answering");
        stop.add_css_class("pill");
        stop.set_halign(gtk::Align::End);
        let a = self.clone();
        stop.connect_clicked(move |_| {
            a.clear();
            a.input.close();
        });
        self.area.append(&text);
        self.area.append(&stop);
        self.area.set_visible(true);
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn transaction(ctx: &Ctx, action: Action, on_finish: impl FnOnce(&Status) + 'static) {
    let buffer = gtk::TextBuffer::new(None);

    let spinner = gtk::Spinner::builder().spinning(true).width_request(20).height_request(20).build();
    let state_icon = gtk::Image::builder().pixel_size(20).visible(false).build();
    let state = gtk::Label::builder()
        .label("Waiting for authorization\u{2026}")
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    state.add_css_class("heading");
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    status_row.add_css_class("tx-status");
    status_row.append(&spinner);
    status_row.append(&state_icon);
    status_row.append(&state);

    let area = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .visible(false)
        .build();
    area.add_css_class("question");

    let close = gtk::Button::with_label("Close");
    close.add_css_class("pill");
    close.set_sensitive(false);
    let bottom = gtk::Box::builder().halign(gtk::Align::End).build();
    bottom.add_css_class("tx-bottom");
    bottom.append(&close);

    // The output and the question share the dialog, and a question with several
    // options is tall. Left to themselves the two fight over the same pixels
    // and the output loses, which is the half being read in order to answer.
    // So the two are given different claims on the height instead of a fixed
    // split: the question asks for exactly what it needs and is never made
    // smaller, the output asks for OUTPUT_WANTED and takes everything left
    // over on top of that, and only where even that will not fit does it come
    // down towards OUTPUT_LEAST. Nothing to drag and nothing to configure: the
    // same three numbers lay the dialog out at any window size or font size.
    let output = terminal_view(&buffer);
    output.set_min_content_height(OUTPUT_LEAST);
    output.set_max_content_height(OUTPUT_WANTED);
    output.set_propagate_natural_height(true);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.add_css_class("tx-body");
    body.append(&status_row);
    body.append(&output);
    body.append(&area);

    let header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&body));
    view.add_bottom_bar(&bottom);

    let dialog = adw::Dialog::builder()
        .title(&action.title)
        .content_width(1040)
        .content_height(820)
        .can_close(false)
        .child(&view)
        .build();
    {
        let d = dialog.clone();
        close.connect_clicked(move |_| {
            d.close();
        });
    }
    dialog.present(Some(&ctx.window));

    let feed = Feed::new(&buffer);
    feed.line(&format!("$ {}", ctx.runner.argv(&action.spec).join(" ")));

    let input = Input::new();
    let asker = Asker {
        input: input.clone(),
        area: area.clone(),
        state: state.clone(),
        destructive: action.destructive,
        showing: Rc::default(),
    };
    // Finished lines, for the options and items printed above a question.
    let lines: Rc<RefCell<Vec<String>>> = Rc::default();
    // Bumped on every piece of output; a quiet-timer that finds it changed
    // knows output resumed.
    let generation = Rc::new(Cell::new(0u64));
    let started = Rc::new(Cell::new(false));

    let privilege = action.spec.privilege;
    let (ctx2, title) = (ctx.clone(), action.title.clone());
    let on_output = {
        let (asker, lines, generation, started, state) =
            (asker.clone(), lines.clone(), generation.clone(), started.clone(), state.clone());
        move |text: &str, kind: Chunk| {
            if !started.replace(true) {
                state.set_text("Running\u{2026}");
            }
            feed.push(text, kind);
            let now = generation.get() + 1;
            generation.set(now);
            match kind {
                Chunk::Line => {
                    let mut l = lines.borrow_mut();
                    l.extend(text.split('\n').map(str::to_string));
                    let excess = l.len().saturating_sub(CONTEXT_LINES);
                    l.drain(..excess);
                }
                Chunk::Progress => {
                    let before = lines.borrow();
                    let before: Vec<&str> = before.iter().map(String::as_str).collect();
                    match prompt::detect(&before, text) {
                        Some(q) => asker.ask(q),
                        None if asker.input.is_open() && asker.showing.borrow().is_none() => {
                            // Not a known question — perhaps a counter, perhaps
                            // a question from a newer slacker. Offer a way out
                            // only if nothing follows it for a while.
                            let (a, g, line) = (asker.clone(), generation.clone(), text.to_string());
                            let wait = if looks_like_question(text) {
                                QUIET_QUESTION
                            } else {
                                QUIET_OTHER
                            };
                            glib::timeout_add_local_once(wait, move || {
                                if g.get() == now && a.input.is_open() && a.showing.borrow().is_none() {
                                    a.unrecognised(&line);
                                }
                            });
                        }
                        None => {}
                    }
                }
            }
        }
    };

    ctx.runner.run_answering(action.spec, &input, on_output, move |status| {
        generation.set(generation.get() + 1);
        asker.clear();
        spinner.set_visible(false);
        state_icon.set_visible(true);
        let message = describe(&status, privilege);
        let (icon_name, class) = match status.code() {
            Some(0) => ("object-select-symbolic", "success"),
            Some(20 | 50 | 100) => ("dialog-information-symbolic", "accent"),
            Some(126) => ("dialog-information-symbolic", "dim-label"),
            _ => ("dialog-error-symbolic", "error"),
        };
        state_icon.set_icon_name(Some(icon_name));
        state_icon.add_css_class(class);
        state.set_text(&message);
        if buffer.line_count() <= 1 {
            append(&buffer, "(no output)");
        }
        dialog.set_can_close(true);
        close.set_sensitive(true);
        close.add_css_class("suggested-action");
        // From idle: when the last answer ends the command at once, the
        // answered button is still being taken away and GTK would move the
        // focus again after this.
        let c = close.clone();
        glib::idle_add_local_once(move || {
            c.grab_focus();
        });
        ctx2.toast(&format!("{title}: {message}"));
        on_finish(&status);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_or_failed_start_changed_nothing() {
        assert!(!may_have_changed(&Status::Exited(126)));
        assert!(!may_have_changed(&Status::Exited(127)));
        assert!(!may_have_changed(&Status::SpawnFailed("x".into())));
        assert!(may_have_changed(&Status::Exited(0)));
        assert!(may_have_changed(&Status::Exited(1)));
    }

    #[test]
    fn option_labels_read_as_buttons() {
        assert_eq!(capitalise("skip-all"), "Skip-all");
        assert_eq!(capitalise(""), "");
    }

    #[test]
    fn a_prompt_is_told_apart_from_work_in_progress() {
        // Waiting for an answer: the short wait applies.
        for line in [
            "Proceed with the upgrade? [Y/n] ",
            "Enter a number to revert to (or [n] to cancel): ",
            "Keep, Overwrite, Skip? (k/o/s)",
            "slacker> ",
        ] {
            assert!(looks_like_question(line), "should read as a question: {line:?}");
        }

        // Work in progress: quiet here is normal, so the long wait applies.
        // The dracut line is the one that kept raising a false alarm during
        // kernel upgrades, both while the dots accumulate and before they do.
        for line in [
            "Building module(s)............",
            "Building module(s)",
            "  downloading kernel-generic-6.12.8-x86_64-1.txz  57%",
            "dracut[I]: *** Creating image file '/boot/initrd.img.tmp' ***",
            "",
        ] {
            assert!(!looks_like_question(line), "should read as progress: {line:?}");
        }
    }
}
