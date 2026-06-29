//! Minimal ANSI colouring for terminal output, zypper-style.
//!
//! Colour is applied only when stdout is a real TTY and `NO_COLOR` is unset, so
//! piped or redirected output (and logs/CI) stay plain text. The palette is
//! deliberately small and maps to slacker's plan categories:
//!   - blue   : messages and prompts
//!   - green  : packages being installed / upgraded (new or changed)
//!   - red    : packages being removed
//!   - purple : frozen (blacklisted) packages left untouched
//!   - yellow : packages being reinstalled
//!   - white  : a package name, in every category
//!
//! Helpers take and return owned `String`s so callers can compose freely.

use std::io::IsTerminal;

/// Whether to emit colour escapes at all.
fn enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

fn paint(code: &str, s: &str) -> String {
    paint_when(enabled(), code, s)
}

/// A red `error:` label for the top-level diagnostic on stderr. Gated on STDERR
/// being a TTY (not stdout, since the error goes to stderr) so redirected error
/// streams and logs stay plain text.
pub fn err_label() -> String {
    let on = std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal();
    paint_when(on, "31", "error:")
}

/// The pure colour gate: wrap `s` in the escape for `code` when `on`, else
/// return it unchanged. Kept separate from [`enabled`] so the painting logic can
/// be tested deterministically, without depending on whether the test runner's
/// stdout happens to be a terminal.
fn paint_when(on: bool, code: &str, s: &str) -> String {
    if on {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn blue(s: &str) -> String {
    // Light cornflower blue (256-colour). The basic ANSI blue (34) renders as a
    // hard-to-read dark navy on most terminals; this is softer and lighter.
    paint("38;5;111", s)
}
pub fn green(s: &str) -> String {
    paint("32", s)
}
pub fn red(s: &str) -> String {
    paint("31", s)
}
pub fn purple(s: &str) -> String {
    paint("35", s)
}
pub fn yellow(s: &str) -> String {
    paint("33", s)
}
pub fn white(s: &str) -> String {
    paint("37", s)
}
pub fn cyan(s: &str) -> String {
    paint("36", s)
}
/// Dim / grey, for secondary text (versions, table rules, separators).
pub fn dim(s: &str) -> String {
    paint("90", s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_gate_is_pure() {
        // The colour gate depends only on its boolean argument, never on the
        // ambient TTY of the test runner — which differs between a piped build
        // (e.g. `... | tee build.log`) and an interactive one. Off: the input,
        // unchanged. On: the input wrapped in the escape for that code.
        for code in ["34", "32", "31", "35", "33", "37", "36", "90"] {
            let off = paint_when(false, code, "bash");
            assert_eq!(off, "bash");
            assert!(!off.contains('\x1b'));

            let on = paint_when(true, code, "bash");
            assert_eq!(on, format!("\x1b[{code}mbash\x1b[0m"));
        }
    }

    #[test]
    fn helpers_map_to_the_right_code() {
        // Each helper either returns plain text (colour disabled in this
        // environment) or wraps its input in that helper's own code — checked
        // without forcing the TTY state, so it holds either way.
        let cases: [(fn(&str) -> String, &str); 8] = [
            (blue, "38;5;111"),
            (green, "32"),
            (red, "31"),
            (purple, "35"),
            (yellow, "33"),
            (white, "37"),
            (cyan, "36"),
            (dim, "90"),
        ];
        for (f, code) in cases {
            let out = f("bash");
            let coloured = format!("\x1b[{code}mbash\x1b[0m");
            assert!(
                out == "bash" || out == coloured,
                "helper produced {out:?}, expected plain \"bash\" or {coloured:?}"
            );
        }
    }
}
