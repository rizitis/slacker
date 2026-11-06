//! Parsers for slacker's human-readable output.
//!
//! Each parser follows the exact `println!` formats in slacker's source and is
//! tested against captured output under `fixtures/`. A line a parser does not
//! recognise is never guessed at: it is kept verbatim in `unrecognized`, and
//! the page shows it as-is next to whatever was understood.

pub mod deps;
pub mod history;
pub mod mirrors;
pub mod prompt;
pub mod repos;
pub mod rules;
pub mod search;
pub mod status;
pub mod updates;

/// Splits on runs of two or more spaces, the column separator slacker uses
/// between padded fields.
pub fn split_columns(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        match rest.find("  ") {
            Some(i) => {
                out.push(&rest[..i]);
                rest = rest[i..].trim_start();
            }
            None => {
                out.push(rest);
                break;
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn columns() {
        assert_eq!(
            super::split_columns("  a b   c  d "),
            vec!["a b", "c", "d"]
        );
        assert!(super::split_columns("   ").is_empty());
    }
}
// dropped into src/parse/mod.rs temporarily
#[cfg(test)]
mod adversarial {
    /// Nasty inputs every parser must survive without panicking.
    fn inputs() -> Vec<String> {
        let mut v: Vec<String> = vec![
            String::new(),
            "\n\n\n".into(),
            "   ".into(),
            "\u{fffd}".into(),
            "Ο πακέτο δεν βρέθηκε".into(),
            "ΑΒΓΔ-01-01 10:00  + installed    x  1-1  [r]".into(),
            "  1. ΑΒ   205ms in sync       https://x/".into(),
            "[ΑΒ] installed   πακέτο-1.0  περιγραφή".into(),
            "  ✓ Ελληνικά  detail".into(),
            "   200 | ΑΒΓ | 0 | all | file:///x  (immutable)".into(),
        ];
        // truncations of realistic lines, at every byte offset
        for line in [
            "[slackware] installed   emacs-31.1  emacs (GNU Emacs)  (also: extras emacs-31.1)",
            "2026-09-14 22:10  \u{2191} upgraded     glibc    2.44-4 \u{2192} 2.44-5  [slackware]",
            "   1. bg    205ms 1d 23h behind https://mirrors.netix.net/slackware/",
            "   200 | patches    |    0 | all    | file:////x/TREE/patches  (immutable)  (subtree)",
            "  \u{2713} Repo release all match this system (-current)",
            "  slackware  updates pending",
            "  vlc -> alienbob",
            "        requires: gtk4, cargo-auditable (not pulled in: !ghostty:cargo-auditable)   (RESOLVE_DEPS=no: none are pulled in)",
            "  !ghostty:cargo-auditable  ('cargo-auditable' is not pulled in for 'ghostty')",
        ] {
            for i in 0..=line.len() {
                if line.is_char_boundary(i) {
                    v.push(line[..i].to_string());
                }
            }
        }
        v.push("x".repeat(100_000));
        v
    }

    #[test]
    fn no_parser_panics_on_junk() {
        for s in inputs() {
            let _ = super::search::parse(&s);
            let _ = super::status::parse(&s);
            let _ = super::repos::parse(&s);
            let _ = super::updates::parse(&s);
            let _ = super::history::parse(&s);
            let lines: Vec<&str> = s.lines().collect();
            let (before, last) = lines.split_at(lines.len().saturating_sub(1));
            let _ = super::prompt::detect(before, last.first().copied().unwrap_or(""));
            let _ = super::rules::parse_frozen(&s);
            let _ = super::rules::parse_pins(&s);
            let _ = super::rules::parse_nodeps(&s);
            let _ = super::deps::parse(&s);
            let f = super::mirrors::parse(&s);
            let _ = super::mirrors::suggestions(&f, 3);
            let _ = super::split_columns(&s);
        }
    }
}
