//! Small building blocks shared by the pages.

use adw::prelude::*;

use crate::parse::status::Mark;

#[derive(Clone, Copy)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Error,
    Purple,
}

pub fn pill(text: &str, tone: Tone) -> gtk::Label {
    let l = gtk::Label::builder().label(text).valign(gtk::Align::Center).build();
    l.add_css_class("tag");
    match tone {
        Tone::Neutral => {}
        Tone::Accent => l.add_css_class("accent"),
        Tone::Success => l.add_css_class("success"),
        Tone::Warning => l.add_css_class("warning"),
        Tone::Error => l.add_css_class("error"),
        Tone::Purple => l.add_css_class("purple"),
    }
    l
}

pub fn priority_badge(priority: i32, top: bool) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(priority.to_string())
        .valign(gtk::Align::Center)
        .build();
    l.add_css_class("pri-badge");
    if top {
        l.add_css_class("top");
    }
    l
}

pub fn dim_label(text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).valign(gtk::Align::Center).build();
    l.add_css_class("dim-label");
    l.add_css_class("numeric");
    l
}

/// An ActionRow that shows text literally (slacker output is not markup).
pub fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    let r = adw::ActionRow::builder()
        .use_markup(false)
        .title(title)
        .subtitle(subtitle)
        .build();
    r.set_title_lines(0);
    r.set_subtitle_lines(0);
    r
}

pub fn icon(name: &str, size: i32) -> gtk::Image {
    let i = gtk::Image::from_icon_name(name);
    i.set_pixel_size(size);
    i.set_valign(gtk::Align::Center);
    i
}

/// A symbolic icon on a soft rounded tile, used as a row prefix.
pub fn tile(name: &str, tone: Option<&str>) -> gtk::Box {
    let b = gtk::Box::builder().valign(gtk::Align::Center).build();
    b.add_css_class("icon-tile");
    if let Some(t) = tone {
        b.add_css_class(t);
    }
    b.append(&icon(name, 18));
    b
}

pub fn mark_tile(mark: Mark) -> gtk::Box {
    match mark {
        Mark::Ok => tile("object-select-symbolic", Some("success")),
        Mark::Bad => tile("dialog-error-symbolic", Some("error")),
        Mark::Warn => tile("dialog-warning-symbolic", Some("warning")),
        Mark::Info => tile("dialog-information-symbolic", None),
    }
}

pub fn group(title: &str, description: &str) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::new();
    if !title.is_empty() {
        g.set_title(&gtk::glib::markup_escape_text(title));
    }
    if !description.is_empty() {
        g.set_description(Some(&gtk::glib::markup_escape_text(description)));
    }
    g
}

/// The standard page body: scrollable, width-clamped, generous spacing.
pub fn page_body() -> (gtk::ScrolledWindow, gtk::Box) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    content.add_css_class("page-body");
    // Rows here carry a URL and a row of tags, not prose: 880 (the usual
    // width for a page of text) wrapped the repository and mirror rows.
    let clamp = adw::Clamp::builder()
        .maximum_size(1040)
        .tightening_threshold(700)
        .child(&content)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .vexpand(true)
        .build();
    (scroller, content)
}

pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

pub fn loading(text: &str) -> gtk::Box {
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .vexpand(true)
        .margin_top(48)
        .margin_bottom(48)
        .build();
    let s = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build();
    let l = gtk::Label::new(Some(text));
    l.add_css_class("dim-label");
    b.append(&s);
    b.append(&l);
    b
}

pub fn status_page(icon_name: &str, title: &str, description: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name(icon_name)
        .title(gtk::glib::markup_escape_text(title))
        .description(gtk::glib::markup_escape_text(description))
        .vexpand(true)
        .build()
}

/// A block of slacker's own output, shown as it was printed.
///
/// A label rather than a TextView: a TextView is scrollable, so its minimum
/// height is zero, and inside a preferences group (which gives a child its
/// minimum) it collapsed to a sliver that clipped the text as soon as the
/// desktop font grew. A label measures the text it holds, so the card is
/// always as tall as what is in it, at any font size. The long command log
/// keeps its TextView (see output.rs): that one is inside a scroller, which is
/// what a TextView is for.
pub fn raw_text(text: &str) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(text.trim_end())
        .xalign(0.0)
        .yalign(0.0)
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build();
    label.add_css_class("terminal-text");
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("terminal-card");
    frame.append(&label);
    frame.upcast()
}

/// How much of its window a dialog takes, in percent, and how wide it gets.
const DIALOG_SHARE: i32 = 85;
const DIALOG_WIDEST: i32 = 1500;

/// The size of a dialog over a window of `window` pixels: most of the window,
/// never less than `least` (libadwaita itself keeps it inside a smaller window)
/// and never wider than text stays readable.
fn dialog_size(window: (i32, i32), least: (i32, i32)) -> (i32, i32) {
    let width = (window.0 * DIALOG_SHARE / 100).clamp(least.0, DIALOG_WIDEST.max(least.0));
    let height = (window.1 * DIALOG_SHARE / 100).max(least.1);
    (width, height)
}

/// Open `dialog` over `window`, sized to it and following it when the window
/// is resized, so that a large window gives long lines the room they need.
pub fn present_fitted(dialog: &adw::Dialog, window: &impl IsA<gtk::Window>, least: (i32, i32)) {
    let window = window.as_ref().clone();
    let fit = {
        let dialog = dialog.downgrade();
        move |width: i32, height: i32| {
            let Some(dialog) = dialog.upgrade() else { return };
            let (width, height) = dialog_size((width, height), least);
            dialog.set_content_width(width);
            dialog.set_content_height(height);
        }
    };
    fit(window.width(), window.height());
    if let Some(surface) = window.surface() {
        // The surface has its new size before the window's widgets are laid out
        // again, so the size is read from it.
        let (on_width, on_height) = (fit.clone(), fit);
        let handlers = [
            surface.connect_width_notify(move |s| on_width(s.width(), s.height())),
            surface.connect_height_notify(move |s| on_height(s.width(), s.height())),
        ];
        let handlers = std::cell::RefCell::new(Some(handlers));
        dialog.connect_closed(move |_| {
            for handler in handlers.borrow_mut().take().into_iter().flatten() {
                surface.disconnect(handler);
            }
        });
    }
    dialog.present(Some(&window));
}

pub fn flat_icon_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon_name);
    b.set_tooltip_text(Some(tooltip));
    b
}

pub fn row_button(label: &str, class: Option<&str>) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.set_valign(gtk::Align::Center);
    b.add_css_class("pill-button");
    if let Some(c) = class {
        b.add_css_class(c);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::dialog_size;

    #[test]
    fn a_dialog_takes_most_of_its_window_within_limits() {
        // The default window: wider than the old fixed 720.
        assert_eq!(dialog_size((1180, 780), (720, 560)), (1003, 663));
        // A large window: wide, but capped so lines stay readable.
        assert_eq!(dialog_size((1876, 833), (720, 560)), (1500, 708));
        // A small window: never asks for less than before.
        assert_eq!(dialog_size((600, 400), (720, 560)), (720, 560));
        // A dialog that was already wider than the cap keeps its size.
        assert_eq!(dialog_size((1000, 700), (1600, 820)), (1600, 820));
    }
}
