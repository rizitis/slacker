//! Cleaning of raw command output, and the monospace text views that show it.

use std::cell::Cell;

use gtk::prelude::*;

/// How a piece of output relates to what came before it.
#[derive(Clone, Copy, PartialEq)]
pub enum Chunk {
    /// One or more finished lines.
    Line,
    /// A line still being redrawn in place: slacker's download counter
    /// prints with a carriage return and no newline, so without this the
    /// window would show nothing at all for the length of a transfer.
    Progress,
}

/// Where a chunk of output goes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Write {
    /// Start a new line.
    New,
    /// Overwrite the line the command is redrawing.
    OverLast,
}

/// The whole rule, kept pure so it can be tested without a display: output
/// goes on a new line unless the last line is one the command was still
/// redrawing — a further redraw replaces it, and so does the finished line,
/// which otherwise would appear twice.
pub fn write_for(redrawing: bool, _kind: Chunk) -> Write {
    if redrawing {
        Write::OverLast
    } else {
        Write::New
    }
}

/// How far back a finished download looks for its `↓` line. slacker prints
/// that line shortly before the download ends, so it is always near the end;
/// searching only here keeps a long upgrade log cheap.
const STARTED_WINDOW: i32 = 200;

/// The package a finished-download line names: slacker's `  ✓ NAME …` or
/// `  ✗ NAME …` (slacker-cli main.rs, the parallel download loop).
fn finished_download(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = t.strip_prefix("\u{2713} ").or_else(|| t.strip_prefix("\u{2717} "))?;
    rest.split_whitespace().next()
}

/// Whether `line` is the `  ↓ NAME (size)` (or `  ↓ NAME`) line slacker
/// prints when NAME's download has run for a moment and the output is not a
/// terminal. Only the exact name: `↓ rust-bindgen` is not `rust`'s line.
fn started_download(line: &str, name: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("\u{2193} ") else { return false };
    rest == name || rest.strip_prefix(name).is_some_and(|r| r.starts_with(" ("))
}

/// Of `lines`, the one `finished` makes stale: the last `↓ NAME` line for the
/// package a `✓`/`✗` line names. None for any other line, or if there is none.
pub fn stale_started(lines: &[&str], finished: &str) -> Option<usize> {
    let name = finished_download(finished)?;
    lines.iter().rposition(|l| started_download(l, name))
}

/// How many lines to drop from the front to keep at most `max_lines`.
fn overflow(line_count: i32, max_lines: i32) -> i32 {
    (line_count - max_lines).max(0)
}

/// Writes command output into a buffer the way a terminal would: each
/// redraw replaces the previous one, and the finished line replaces the
/// last redraw rather than appearing under it.
pub struct Feed {
    buffer: gtk::TextBuffer,
    /// Whether the last line written is one the command is still redrawing.
    redrawing: Cell<bool>,
    max_lines: Option<i32>,
    /// Drop a download's `↓` line once its `✓`/`✗` line arrives (the
    /// transaction window). The command log keeps every line as printed.
    tidy_downloads: bool,
}

impl Feed {
    pub fn new(buffer: &gtk::TextBuffer) -> Self {
        Self { buffer: buffer.clone(), redrawing: Cell::new(false), max_lines: None, tidy_downloads: false }
    }

    /// As `new`, and a finished download removes its `↓ NAME (size)` line,
    /// as a terminal does when slacker redraws its live line. Display only.
    pub fn tidying_downloads(buffer: &gtk::TextBuffer) -> Self {
        Self { tidy_downloads: true, ..Self::new(buffer) }
    }

    /// As `new`, but keeping only the last `max_lines` lines.
    pub fn capped(buffer: &gtk::TextBuffer, max_lines: i32) -> Self {
        Self { max_lines: Some(max_lines), ..Self::new(buffer) }
    }

    pub fn buffer(&self) -> &gtk::TextBuffer {
        &self.buffer
    }

    pub fn push(&self, text: &str, kind: Chunk) {
        if self.tidy_downloads && kind == Chunk::Line {
            for line in text.split('\n') {
                self.drop_started(line);
            }
        }
        let redrawing = self.redrawing.replace(kind == Chunk::Progress);
        match write_for(redrawing, kind) {
            Write::OverLast => replace_last_line(&self.buffer, text),
            Write::New => append(&self.buffer, text),
        }
        if kind == Chunk::Line {
            self.cap();
        }
    }

    /// A line of the GUI's own (the command line, the exit status). It ends
    /// whatever was being redrawn.
    pub fn line(&self, text: &str) {
        self.redrawing.set(false);
        append(&self.buffer, text);
        self.cap();
    }

    /// Removes the `↓` line `finished` makes stale, if it is among the last
    /// STARTED_WINDOW lines. The last line is never touched while it is being
    /// redrawn.
    fn drop_started(&self, finished: &str) {
        if finished_download(finished).is_none() {
            return;
        }
        let b = &self.buffer;
        let count = b.line_count();
        let first = (count - STARTED_WINDOW).max(0);
        let Some(from) = b.iter_at_line(first) else { return };
        let text = b.text(&from, &b.end_iter(), false);
        let mut lines: Vec<&str> = text.split('\n').collect();
        if self.redrawing.get() {
            lines.pop();
        }
        if let Some(i) = stale_started(&lines, finished) {
            delete_line(b, first + i as i32);
        }
    }

    fn cap(&self) {
        if let Some(max) = self.max_lines {
            trim(&self.buffer, max);
        }
    }
}

/// Turns raw bytes into display text: lossy UTF-8, ANSI escape sequences
/// removed, and carriage-return redraws (progress lines) reduced to the
/// last state, the way a terminal would show them. No trailing newline.
pub fn clean(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let plain = strip_ansi(&text);
    let body = plain.strip_suffix('\n').unwrap_or(&plain);
    body.split('\n')
        .map(|line| {
            let line = line.trim_end_matches('\r');
            line.rsplit('\r').next().unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ params... final byte in 0x40..=0x7e
            Some('[') => {
                chars.next();
                for f in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&f) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... terminated by BEL or ESC \
            Some(']') => {
                chars.next();
                while let Some(f) = chars.next() {
                    if f == '\u{7}' {
                        break;
                    }
                    if f == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Appends one block of text as its own line(s).
pub fn append(buffer: &gtk::TextBuffer, text: &str) {
    let mut end = buffer.end_iter();
    if end.offset() > 0 {
        buffer.insert(&mut end, "\n");
    }
    buffer.insert(&mut end, text);
}

/// Replaces the last line, for output that redraws itself in place
/// (slacker's download counter prints with a carriage return).
pub fn replace_last_line(buffer: &gtk::TextBuffer, text: &str) {
    let mut start = buffer.end_iter();
    start.set_line_offset(0);
    let mut end = buffer.end_iter();
    buffer.delete(&mut start, &mut end);
    let mut at = buffer.end_iter();
    buffer.insert(&mut at, text);
}

/// Deletes line `n` and its line break; the last line takes the break
/// before it instead, so no empty line is left behind.
pub fn delete_line(buffer: &gtk::TextBuffer, n: i32) {
    let count = buffer.line_count();
    if n < 0 || n >= count {
        return;
    }
    let (start, end) = if n + 1 < count {
        (buffer.iter_at_line(n), buffer.iter_at_line(n + 1))
    } else if n > 0 {
        let mut s = buffer.iter_at_line(n - 1);
        if let Some(s) = s.as_mut() {
            s.forward_to_line_end();
        }
        (s, Some(buffer.end_iter()))
    } else {
        (Some(buffer.start_iter()), Some(buffer.end_iter()))
    };
    if let (Some(mut s), Some(mut e)) = (start, end) {
        buffer.delete(&mut s, &mut e);
    }
}

/// Drops the oldest lines once the buffer is longer than `max_lines`.
/// Without this the command log grows for as long as the window is open —
/// one `show-changelog` alone is tens of thousands of lines.
pub fn trim(buffer: &gtk::TextBuffer, max_lines: i32) {
    let extra = overflow(buffer.line_count(), max_lines);
    if extra == 0 {
        return;
    }
    let Some(mut cut) = buffer.iter_at_line(extra) else { return };
    let mut start = buffer.start_iter();
    buffer.delete(&mut start, &mut cut);
}

/// A read-only monospace view on `buffer` that stays at the top, for text
/// read from its first line on, like a ChangeLog (newest entry first). Text
/// arriving later is added below without moving the view.
pub fn document_view(buffer: &gtk::TextBuffer) -> gtk::ScrolledWindow {
    text_card(buffer).0
}

fn text_card(buffer: &gtk::TextBuffer) -> (gtk::ScrolledWindow, gtk::TextView) {
    let view = gtk::TextView::builder()
        .buffer(buffer)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(16)
        .right_margin(16)
        .top_margin(14)
        .bottom_margin(14)
        .build();
    view.add_css_class("terminal");
    let scroller = gtk::ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .vexpand(true)
        .build();
    scroller.add_css_class("terminal-card");
    (scroller, view)
}

/// A read-only monospace view on `buffer` that follows new output.
///
/// The view owns what it adds to the buffer: the runner's log outlives every
/// dialog that shows it, so the scroll handler and its mark are removed when
/// the view goes away. Otherwise each reopening would leave another dead view
/// attached to the buffer, scrolled on every line of output for ever after.
pub fn terminal_view(buffer: &gtk::TextBuffer) -> gtk::ScrolledWindow {
    let (scroller, view) = text_card(buffer);

    // Follow new output, and start at the end when opened on an old log.
    let mark = buffer.create_mark(None, &buffer.end_iter(), false);
    let follow = view.downgrade();
    let m = mark.clone();
    let handler = buffer.connect_changed(move |_| {
        if let Some(v) = follow.upgrade() {
            v.scroll_mark_onscreen(&m);
        }
    });
    {
        let (b, m) = (buffer.clone(), mark.clone());
        let handler = std::cell::Cell::new(Some(handler));
        view.connect_destroy(move |_| {
            if let Some(h) = handler.take() {
                b.disconnect(h);
                b.delete_mark(&m);
            }
        });
    }
    let start = view.clone();
    gtk::glib::idle_add_local_once(move || start.scroll_mark_onscreen(&mark));
    scroller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same rule `Feed` applies, over a list of lines instead of a
    /// GTK buffer (which can only be touched on the main thread).
    fn simulate(steps: &[(&str, Chunk)]) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut redrawing = false;
        for (text, kind) in steps {
            match write_for(redrawing, *kind) {
                Write::OverLast => {
                    lines.pop();
                    lines.extend(text.split('\n').map(str::to_string));
                }
                Write::New => lines.extend(text.split('\n').map(str::to_string)),
            }
            redrawing = *kind == Chunk::Progress;
        }
        lines.join("\n")
    }

    #[test]
    fn a_redrawn_line_is_replaced_never_repeated() {
        // What a download looks like: several redraws of one line, then the
        // finished line, then ordinary output.
        let out = simulate(&[
            ("Upgrade (1):", Chunk::Line),
            ("  glibc: 20 MB", Chunk::Progress),
            ("  glibc: 60 MB", Chunk::Progress),
            ("  glibc: 100 MB", Chunk::Progress),
            ("  glibc: 100 MB", Chunk::Line),
            ("Done.", Chunk::Line),
        ]);
        assert_eq!(out, "Upgrade (1):\n  glibc: 100 MB\nDone.");
    }

    #[test]
    fn the_chunk_that_finishes_a_redraw_may_carry_more_lines() {
        let out = simulate(&[
            ("  glibc: 20 MB", Chunk::Progress),
            ("  glibc: 100 MB\nUpgrading glibc...\nDone.", Chunk::Line),
        ]);
        assert_eq!(out, "  glibc: 100 MB\nUpgrading glibc...\nDone.");
    }

    #[test]
    fn plain_output_is_never_overwritten() {
        let out = simulate(&[("one", Chunk::Line), ("two", Chunk::Line)]);
        assert_eq!(out, "one\ntwo");
    }

    #[test]
    fn the_log_drops_only_what_is_over_the_cap() {
        assert_eq!(overflow(10, 4_000), 0);
        assert_eq!(overflow(4_000, 4_000), 0);
        assert_eq!(overflow(4_007, 4_000), 7);
    }

    /// slacker's parallel download lines when the output is not a terminal,
    /// as in gegechris99's screenshot (issue #25).
    #[test]
    fn a_finished_download_finds_its_started_line() {
        let lines = [
            "  \u{2713} openexr [4/8]",
            "  \u{2193} cmake (13 M)",
            "  \u{2193} nodejs (25 M)",
            "  \u{2193} rust (102 M)",
            "  \u{2193} vim (9.2 M)",
        ];
        assert_eq!(stale_started(&lines, "  \u{2713} vim [5/8]"), Some(4));
        assert_eq!(stale_started(&lines, "  \u{2713} cmake [7/8]"), Some(1));
        // A failed download ends its line too; the ✗ line stays and says why.
        assert_eq!(stale_started(&lines, "  \u{2717} rust [8/8]  connection reset"), Some(3));
        // With slacker's verify suffix between the name and the counter.
        assert_eq!(stale_started(&lines, "  \u{2713} nodejs gpg+md5 [6/8]"), Some(2));
        // Only the exact name, and only for ✓/✗ lines.
        assert_eq!(stale_started(&["  \u{2193} rust-bindgen (3 M)"], "  \u{2713} rust [1/1]"), None);
        assert_eq!(stale_started(&["  \u{2193} rust"], "  \u{2713} rust [1/1]"), Some(0));
        assert_eq!(stale_started(&lines, "Upgrading: cmake-4.5.0"), None);
        assert_eq!(stale_started(&lines, "  \u{2713} flex [1/8]"), None);
    }

    /// Fifteen parallel downloads finishing in a scrambled order: every ↓
    /// line goes, every ✓ stays, nothing else is touched.
    #[test]
    fn fifteen_parallel_downloads_leave_only_their_ticks() {
        let mut lines: Vec<String> = vec!["Downloading 15 package(s) (15 parallel)...".into()];
        for i in 0..15 {
            lines.push(format!("  \u{2193} pkg{i} ({i} M)"));
        }
        let order = [7, 0, 14, 3, 11, 1, 9, 5, 13, 2, 8, 12, 4, 10, 6];
        for (done, i) in order.iter().enumerate() {
            let tick = format!("  \u{2713} pkg{i} [{}/15]", done + 1);
            let view: Vec<&str> = lines.iter().map(String::as_str).collect();
            let at = stale_started(&view, &tick).expect("its ↓ line");
            assert_eq!(lines[at], format!("  \u{2193} pkg{i} ({i} M)"));
            lines.remove(at);
            lines.push(tick);
        }
        assert_eq!(lines.len(), 16);
        assert!(lines.iter().all(|l| !l.contains('\u{2193}')));
        assert_eq!(lines.iter().filter(|l| l.contains('\u{2713}')).count(), 15);
    }

    /// The real GTK buffer path, when a display is there (skipped otherwise).
    #[test]
    fn the_transaction_window_drops_started_lines_the_log_keeps_them() {
        if gtk::init().is_err() {
            return;
        }
        let tidy = gtk::TextBuffer::new(None);
        let raw = gtk::TextBuffer::new(None);
        let (t, r) = (Feed::tidying_downloads(&tidy), Feed::new(&raw));
        for f in [&t, &r] {
            f.push("Downloading 3 package(s) (3 parallel)...", Chunk::Line);
            f.push("  \u{2193} cmake (13 M)\n  \u{2193} rust (102 M)", Chunk::Line);
            f.push("  \u{2713} rust [1/3]", Chunk::Line);
            f.push("  \u{2193} vim (9.2 M)", Chunk::Line);
            f.push("  \u{2713} vim [2/3]\n  \u{2717} cmake [3/3]  timed out", Chunk::Line);
        }
        let text = |b: &gtk::TextBuffer| b.text(&b.start_iter(), &b.end_iter(), false).to_string();
        assert_eq!(
            text(&tidy),
            "Downloading 3 package(s) (3 parallel)...\n  \u{2713} rust [1/3]\n  \u{2713} vim [2/3]\n  \u{2717} cmake [3/3]  timed out"
        );
        assert_eq!(text(&raw).matches('\u{2193}').count(), 3);
        // The last line alone, and the first line alone.
        let one = gtk::TextBuffer::new(None);
        let f = Feed::tidying_downloads(&one);
        f.push("  \u{2193} x (1 M)", Chunk::Line);
        f.push("  \u{2713} x [1/1]", Chunk::Line);
        assert_eq!(text(&one), "  \u{2713} x [1/1]");
    }

    #[test]
    fn strips_colour_codes() {
        assert_eq!(clean(b"\x1b[1;31mFROZEN\x1b[0m vim\n"), "FROZEN vim");
    }

    #[test]
    fn keeps_last_progress_state() {
        assert_eq!(clean(b"get  10%\rget  50%\rget 100%\n"), "get 100%");
        assert_eq!(clean(b"line one\r\nline two\r\n"), "line one\nline two");
    }

    #[test]
    fn strips_osc_sequences() {
        assert_eq!(clean(b"\x1b]0;title\x07ok\n"), "ok");
    }

    #[test]
    fn survives_invalid_utf8() {
        assert_eq!(clean(b"a\xffb\n"), "a\u{fffd}b");
    }
}
