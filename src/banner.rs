//! The SLACKWARE block-art banner (an `.nfo`), shown at distribution-scale
//! moments. It is embedded into the binary at compile time and printed only on
//! an interactive terminal, so the block-art bytes never land in a pipe or log.

use std::io::IsTerminal;

/// The banner art, embedded at compile time from the `.nfo` beside this file.
/// Editing `slackware.nfo` and rebuilding is all it takes to change the art.
pub const ART: &str = include_str!("slackware.nfo");

/// A 256-colour code per shade glyph, brightest (solid) to deepest (faint), so
/// the block art reads as a blue 3-D gradient. Non-shade bytes (spaces) keep the
/// terminal's default colour, returning `None`.
fn shade_color(ch: char) -> Option<u8> {
    match ch {
        '█' | '▄' | '▀' => Some(51), // solid block  -> bright cyan-blue
        '▓' => Some(45),             // dark shade   -> bright blue
        '▒' => Some(33),             // medium shade -> blue
        '░' => Some(27),             // light shade  -> deep blue
        _ => None,                   // spaces etc.  -> default
    }
}

/// Print the banner to stdout, but only on a real terminal — never when output
/// is piped or redirected (keeps the multi-byte block-art out of files/logs).
/// Colour is a per-shade blue gradient; with NO_COLOR the plain art is printed.
pub fn show() {
    if !std::io::stdout().is_terminal() {
        return;
    }
    let art = ART.trim_matches('\n');
    if std::env::var_os("NO_COLOR").is_some() {
        println!("{art}");
        return;
    }
    let mut out = String::with_capacity(art.len() * 2);
    let mut cur: Option<u8> = None; // colour currently set, to avoid code spam
    for ch in art.chars() {
        if ch == '\n' {
            if cur.take().is_some() {
                out.push_str("\x1b[0m");
            }
            out.push('\n');
            continue;
        }
        let code = shade_color(ch);
        if code != cur {
            if cur.is_some() {
                out.push_str("\x1b[0m");
            }
            if let Some(c) = code {
                out.push_str(&format!("\x1b[38;5;{c}m"));
            }
            cur = code;
        }
        out.push(ch);
    }
    if cur.is_some() {
        out.push_str("\x1b[0m");
    }
    println!("{out}");
}
