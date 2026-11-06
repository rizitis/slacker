//! The `requires:` lines of `slacker info NAME` (cmd_info): what each
//! candidate build pulls in, read by slacker exactly as its resolver reads it
//! (the build's `.dep` file, else `PACKAGE REQUIRED`), and which of those it
//! leaves out.
//!
//! ```text
//!   [lngn] ghostty-1.4.0-x86_64-5_lngn
//!         series: x   compressed: ?   uncompressed: ?   md5 ok
//!         requires: blueprint-compiler, cargo-auditable (not pulled in: !ghostty:cargo-auditable), bash (not pulled in: frozen)
//! ```

/// Why slacker leaves a dependency out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// A `!package:dependency` rule; the package as written, `*` for any.
    Rule(String),
    Frozen,
    /// A reason this version of the GUI does not know, verbatim.
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    pub held: Option<Held>,
    /// The candidate builds (by repo) that list it.
    pub repos: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Requires {
    /// Every candidate's repo, highest priority first, as `info` lists them.
    pub repos: Vec<String>,
    /// The union over all candidates, in order of first appearance.
    pub deps: Vec<Dep>,
    /// RESOLVE_DEPS=no: slacker pulls in no dependency at all.
    pub resolve_off: bool,
}

const OFF: &str = "(RESOLVE_DEPS=no: none are pulled in)";
const HELD: &str = " (not pulled in: ";

pub fn parse(text: &str) -> Requires {
    let mut out = Requires::default();
    let mut in_list = false;
    let mut repo: Option<String> = None;
    let mut after_series = false;
    for line in text.lines() {
        let t = line.trim();
        if t == "Available candidates (highest priority first):" {
            in_list = true;
            continue;
        }
        if !in_list {
            continue;
        }
        // A candidate: two spaces, then `[repo] name-version-arch-build`.
        if let Some(r) = line
            .strip_prefix("  [")
            .and_then(|l| l.split_once(']'))
            .map(|(r, _)| r)
            .filter(|r| !r.is_empty() && !r.contains(char::is_whitespace))
        {
            out.repos.push(r.to_string());
            repo = Some(r.to_string());
            after_series = false;
            continue;
        }
        if repo.is_some() && t.starts_with("series: ") {
            after_series = true;
            continue;
        }
        // Only the line right under a candidate's series line, so a
        // description line that happens to start with "requires:" is not read.
        let take = after_series;
        after_series = false;
        let (Some(r), true) = (&repo, take) else { continue };
        let Some(list) = t.strip_prefix("requires: ") else { continue };
        let list = match list.strip_suffix(OFF) {
            Some(l) => {
                out.resolve_off = true;
                l.trim_end()
            }
            None => list,
        };
        for item in list.split(", ") {
            let (name, held) = match item.split_once(HELD) {
                Some((n, why)) => {
                    let why = why.strip_suffix(')').unwrap_or(why);
                    let held = if why == "frozen" {
                        Held::Frozen
                    } else {
                        match why.strip_prefix('!').and_then(|w| w.split_once(':')) {
                            Some((p, d)) if d == n && !p.is_empty() => Held::Rule(p.to_string()),
                            _ => Held::Other(why.to_string()),
                        }
                    };
                    (n, Some(held))
                }
                None => (item, None),
            };
            let name = name.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                continue;
            }
            match out.deps.iter_mut().find(|d| d.name == name) {
                Some(d) => {
                    if !d.repos.contains(r) {
                        d.repos.push(r.clone());
                    }
                    // A rule is the same for every build; frozen can differ
                    // per repo. The rule is what the dialog can change.
                    if held.is_some() && (matches!(held, Some(Held::Rule(_))) || d.held.is_none()) {
                        d.held = held;
                    }
                }
                None => out.deps.push(Dep { name: name.to_string(), held, repos: vec![r.clone()] }),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::fixture;

    fn names(r: &Requires) -> Vec<&str> {
        r.deps.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn one_candidate_with_each_kind() {
        let r = parse(&fixture("info-deps.txt"));
        assert_eq!(r.repos, vec!["lngn"]);
        assert!(!r.resolve_off);
        assert_eq!(names(&r), vec!["blueprint-compiler", "gtk4-layer-shell", "cargo-auditable", "bash"]);
        let held: Vec<Option<Held>> = r.deps.iter().map(|d| d.held.clone()).collect();
        assert_eq!(
            held,
            vec![None, None, Some(Held::Rule("ghostty".into())), Some(Held::Frozen)]
        );
        assert!(r.deps.iter().all(|d| d.repos == vec!["lngn"]));
    }

    #[test]
    fn two_candidates_are_merged_and_any_package_rules_read() {
        let r = parse(&fixture("info-deps-two.txt"));
        assert_eq!(r.repos, vec!["conraid", "lngn"]);
        assert_eq!(names(&r), vec!["gtk4", "xvfb-run"]);
        assert_eq!(r.deps[0].repos, vec!["conraid"]);
        assert_eq!(r.deps[1].repos, vec!["lngn"]);
        assert_eq!(r.deps[1].held, Some(Held::Rule("*".into())));
    }

    #[test]
    fn resolve_deps_off() {
        let r = parse(&fixture("info-deps-off.txt"));
        assert!(r.resolve_off);
        assert_eq!(names(&r), vec!["blueprint-compiler", "gtk4-layer-shell", "cargo-auditable", "bash"]);
        assert!(r.deps.iter().all(|d| d.held.is_none()));
    }

    #[test]
    fn no_requirements() {
        let r = parse(&fixture("info-nodeps.txt"));
        assert_eq!(r.repos, vec!["slackware"]);
        assert!(r.deps.is_empty());
    }

    #[test]
    fn nothing_offers_it() {
        let r = parse("No package named 'zz' in any repo.\n");
        assert_eq!(r, Requires::default());
    }

    #[test]
    fn a_description_line_is_not_a_requires_line() {
        let text = "Available candidates (highest priority first):\n  [r] a-1-x86_64-1\n        series: x   compressed: ?   uncompressed: ?   md5 ok\n        a (test)\n        requires: b\n";
        assert!(parse(text).deps.is_empty());
    }

    #[test]
    fn unknown_reason_is_kept() {
        let text = "Available candidates (highest priority first):\n  [r] a-1-x86_64-1\n        series: x   compressed: ?   uncompressed: ?   md5 ok\n        requires: b (not pulled in: something new)\n";
        assert_eq!(parse(text).deps[0].held, Some(Held::Other("something new".into())));
    }
}
