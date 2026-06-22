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
    if enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn blue(s: &str) -> String {
    paint("34", s)
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
    fn plain_when_not_a_tty() {
        // Under `cargo test` stdout is not a TTY, so colouring is disabled and
        // every helper must return its input unchanged (no escape sequences).
        for f in [blue, green, red, purple, yellow, white] {
            let out = f("bash");
            assert_eq!(out, "bash");
            assert!(!out.contains('\x1b'));
        }
    }
}
