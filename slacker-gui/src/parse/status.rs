//! `slacker status` (cmd_status + status_full).
//!
//! Rows are `"  {mark} {label:<10} {detail}"`. Labels longer than ten
//! characters are followed by a single space, so the end of a label cannot be
//! found from the text alone; the fixed set below is copied from slacker's
//! source and matched longest first. A continuation line (13 spaces) belongs
//! to the row above it. After the last section comes the verdict: one line
//! without indent, then `  → step`, `  ! note` and `  · hint` lines.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    Ok,
    Bad,
    Warn,
    Info,
}

impl Mark {
    fn from_char(c: char) -> Option<Mark> {
        match c {
            '\u{2713}' => Some(Mark::Ok),
            '\u{2717}' => Some(Mark::Bad),
            '!' => Some(Mark::Warn),
            '\u{00B7}' => Some(Mark::Info),
            _ => None,
        }
    }
}

/// Every label `status` prints (srow calls in main.rs), plus the empty one.
pub const LABELS: [&str; 38] = [
    "Pkgtools", "Tools", "Locate DB", "Config dir", "Config", "Mirror", "Proxy", "Freshness",
    "Repos", "Cache", "Tag rules", "Verify", "Transport", "Credentials", "Repo release",
    "Repo arch", "Repo trust", "GPG keys", "Metadata", "Blacklist", "Pins", "Tag prio",
    "Configs", "Integrity", "Symlinks", "Writable", "Ownership", "Admin dir", "Packages",
    "Leftovers", "Incomplete", "CheckRestart", "By source", "Connection", "Reachable", "Updates", "All repos", "Stock-db",
];

/// The row slacker prints when upgradepkg left `<pkg>-upgraded-<time>`
/// records behind; the record names follow it as unlabelled rows.
pub const LEFTOVERS: &str = "Leftovers";

/// The row slacker prints for what an install cut off leaves (a record with
/// no file list, an install script that did not run, installpkg's own
/// directory); each follows it as an unlabelled row.
pub const INCOMPLETE: &str = "Incomplete";

/// The row about running programs that still use replaced files. The GUI
/// never runs as root, so it shows "not checked (needs root …)" there.
pub const CHECK_RESTART: &str = "CheckRestart";

pub const SECTIONS: [&str; 4] = ["Environment", "Setup", "Installed", "Online"];

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub mark: Mark,
    /// Empty for the unlabelled follow-up row.
    pub label: String,
    pub detail: String,
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ItemKind {
    Step,
    Note,
    Hint,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub headline: String,
    /// Printed as `✓ slacker is set up correctly.`
    pub all_good: bool,
    pub items: Vec<(ItemKind, String)>,
}

#[derive(Debug, Default)]
pub struct Status {
    pub sections: Vec<Section>,
    pub verdict: Option<Verdict>,
    /// Lines outside sections that slacker prints on early exits.
    pub messages: Vec<String>,
    pub unrecognized: Vec<String>,
}

fn match_label(rest: &str) -> Option<(&'static str, &str)> {
    let mut labels = LABELS;
    labels.sort_by_key(|l| std::cmp::Reverse(l.len()));
    for l in labels {
        if let Some(after) = rest.strip_prefix(l) {
            if after.is_empty() {
                return Some((l, ""));
            }
            if after.starts_with(' ') {
                return Some((l, after.trim_start()));
            }
        }
    }
    // The unlabelled row: ten spaces of padding, then one separator.
    rest.strip_prefix("           ").map(|d| ("", d.trim_start()))
}

pub fn parse(text: &str) -> Status {
    let mut out = Status::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }

        if !line.starts_with(' ') {
            if SECTIONS.contains(&line.trim_end()) {
                out.sections.push(Section { title: line.trim_end().to_string(), rows: Vec::new() });
            } else if !out.sections.is_empty() && out.verdict.is_none() {
                let (all_good, headline) = match line.strip_prefix("\u{2713} ") {
                    Some(h) => (true, h),
                    None => (false, line),
                };
                out.verdict = Some(Verdict {
                    headline: headline.trim_end().to_string(),
                    all_good,
                    items: Vec::new(),
                });
            } else {
                out.messages.push(line.trim_end().to_string());
            }
            continue;
        }

        if let Some(v) = out.verdict.as_mut() {
            let item = line.strip_prefix("  ").and_then(|l| {
                let mut c = l.chars();
                let kind = match c.next()? {
                    '\u{2192}' => ItemKind::Step,
                    '!' => ItemKind::Note,
                    '\u{00B7}' => ItemKind::Hint,
                    _ => return None,
                };
                Some((kind, c.as_str().strip_prefix(' ')?.to_string()))
            });
            match item {
                Some(i) => v.items.push(i),
                None => out.unrecognized.push(line.to_string()),
            }
            continue;
        }

        let Some(section) = out.sections.last_mut() else {
            out.unrecognized.push(line.to_string());
            continue;
        };

        if line.starts_with("             ") {
            match section.rows.last_mut() {
                Some(r) => r.extra.push(line.trim().to_string()),
                None => out.unrecognized.push(line.to_string()),
            }
            continue;
        }

        let row = line.strip_prefix("  ").and_then(|l| {
            let mut c = l.chars();
            let mark = Mark::from_char(c.next()?)?;
            let rest = c.as_str().strip_prefix(' ')?;
            let (label, detail) = match_label(rest)?;
            Some(Row { mark, label: label.to_string(), detail: detail.to_string(), extra: Vec::new() })
        });
        // The record names under a Leftovers or Incomplete row belong to it.
        if let (Some(r), Some(prev)) = (&row, section.rows.last_mut()) {
            if r.label.is_empty() && r.mark == Mark::Info && (prev.label == LEFTOVERS || prev.label == INCOMPLETE) {
                prev.extra.push(r.detail.clone());
                continue;
            }
        }
        match row {
            Some(r) => section.rows.push(r),
            None => out.unrecognized.push(line.to_string()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::fixture;

    #[test]
    fn captured_healthy_system() {
        let s = parse(&fixture("status-ok.txt"));
        assert!(s.unrecognized.is_empty(), "{:?}", s.unrecognized);
        assert!(s.messages.is_empty());
        let titles: Vec<&str> = s.sections.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, SECTIONS);
        let setup = &s.sections[1];
        let release = setup.rows.iter().find(|r| r.label == "Repo release").unwrap();
        assert_eq!(release.detail, "all match this system (-current)");
        let integ = setup.rows.iter().find(|r| r.label == "Integrity").unwrap();
        assert_eq!(integ.mark, Mark::Info);
        let v = s.verdict.unwrap();
        assert!(v.all_good);
        assert_eq!(v.headline, "slacker is set up correctly.");
        assert!(v.items.is_empty());
    }

    #[test]
    fn warnings_continuations_and_steps() {
        let s = parse(&fixture("status-steps.txt"));
        assert!(s.unrecognized.is_empty(), "{:?}", s.unrecognized);
        let setup = &s.sections[1];
        let trust = setup.rows.iter().find(|r| r.label == "Repo trust").unwrap();
        assert_eq!(trust.mark, Mark::Warn);
        assert_eq!(trust.extra, vec!["shady: bad signature"]);
        let prio = setup.rows.iter().find(|r| r.label == "Tag prio").unwrap();
        assert_eq!(prio.extra.len(), 1);
        let installed = &s.sections[2];
        assert_eq!(installed.rows[2].label, "");
        assert!(installed.rows[2].detail.starts_with("? = no metadata yet"));
        assert_eq!(s.sections[3].rows[0].mark, Mark::Bad);
        let v = s.verdict.unwrap();
        assert!(!v.all_good);
        let steps: Vec<&str> = v
            .items
            .iter()
            .filter(|(k, _)| *k == ItemKind::Step)
            .map(|(_, t)| t.as_str())
            .collect();
        assert_eq!(steps, ["slacker update", "slacker install-new", "slacker upgrade-all"]);
        assert_eq!(v.items.last().unwrap().0, ItemKind::Note);
    }

    #[test]
    fn leftover_records_belong_to_their_row() {
        let s = parse(&fixture("status-leftovers.txt"));
        assert!(s.unrecognized.is_empty(), "{:?}", s.unrecognized);
        let installed = s.sections.iter().find(|x| x.title == "Installed").unwrap();
        let labels: Vec<&str> = installed.rows.iter().map(|r| r.label.as_str()).collect();
        // The `? = no metadata` row under By source stays a row of its own.
        assert_eq!(labels, ["Packages", "Leftovers", "CheckRestart", "By source", ""]);
        let left = &installed.rows[1];
        assert_eq!(left.mark, Mark::Warn);
        assert!(left.detail.contains("slacker fix-upgrade"));
        assert_eq!(left.extra, vec!["openssl-3.3.2-x86_64-1-upgraded-2026-11-20,14:35:12"]);
        let cr = &installed.rows[2];
        assert_eq!(cr.mark, Mark::Info);
        assert!(cr.detail.starts_with("not checked (needs root"));
        let v = s.verdict.unwrap();
        assert!(v.items.contains(&(ItemKind::Step, "slacker fix-upgrade".to_string())));
    }

    #[test]
    fn incomplete_records_belong_to_their_row() {
        let s = parse(&fixture("status-incomplete.txt"));
        assert!(s.unrecognized.is_empty(), "{:?}", s.unrecognized);
        let installed = s.sections.iter().find(|x| x.title == "Installed").unwrap();
        let labels: Vec<&str> = installed.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Packages", "Incomplete", "CheckRestart", "By source"]);
        let inc = &installed.rows[1];
        assert_eq!(inc.mark, Mark::Warn);
        assert!(inc.detail.contains("an install that did not finish"));
        assert_eq!(inc.extra.len(), 2);
        assert_eq!(inc.extra[0], "foo-2-x86_64-1: install script did not run");
        assert!(inc.extra[1].ends_with(": left by installpkg"));
        let v = s.verdict.unwrap();
        assert!(v.items.contains(&(ItemKind::Step, "slacker fix-upgrade".to_string())));
    }

    #[test]
    fn prefix_labels_do_not_collide() {
        assert_eq!(match_label("Config dir /etc/slacker does not exist"), Some(("Config dir", "/etc/slacker does not exist")));
        assert_eq!(match_label("Configs    3 pending"), Some(("Configs", "3 pending")));
        assert_eq!(match_label("Config     repos and mirrors parse"), Some(("Config", "repos and mirrors parse")));
        assert_eq!(match_label("Repos      8 configured"), Some(("Repos", "8 configured")));
        assert_eq!(match_label("Proxy      http://p:3128"), Some(("Proxy", "http://p:3128")));
        assert_eq!(
            match_label("CheckRestart not checked (needs root)"),
            Some(("CheckRestart", "not checked (needs root)"))
        );
        assert_eq!(match_label("Nonsense   x"), None);
    }

    #[test]
    fn early_exit_is_kept_as_messages() {
        let s = parse("Environment\n  \u{2717} Config dir /etc/slacker does not exist\n\n! create it with a `repos` file\nslacker: error: configuration directory is missing\n");
        assert!(s.unrecognized.is_empty());
        assert_eq!(s.sections[0].rows[0].mark, Mark::Bad);
        // Unindented lines after the first section read as the verdict.
        let v = s.verdict.unwrap();
        assert!(!v.all_good);
        assert_eq!(s.messages, vec!["slacker: error: configuration directory is missing"]);
    }
}
