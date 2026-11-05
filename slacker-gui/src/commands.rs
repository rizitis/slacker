//! The only place that decides which slacker command lines the GUI runs
//! and whether each one runs as the desktop user or as root.
//!
//! The split follows slacker's own `requires_privilege`: search, info,
//! list-repos, status, check-updates and history are free for anyone;
//! everything that writes needs root.
//!
//! No command carries `--yes`. A command that changes the system asks its
//! own questions — the plan and "Proceed? [y/N]", a package picker, a
//! conflict choice — and the GUI puts them to the user as they come. Where
//! nobody answers, slacker reads end of input and takes its own default,
//! which for every change is No.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Privilege {
    /// Runs as the desktop user; never changes the system.
    User,
    /// Runs as root through pkexec.
    Root,
}

#[derive(Clone, Debug)]
pub struct Spec {
    pub args: Vec<String>,
    pub privilege: Privilege,
    /// slacker itself shows what it is about to do and asks before it
    /// writes anything. When false the change happens as soon as the
    /// command runs, so the GUI asks first.
    pub confirms: bool,
}

fn user(args: &[&str]) -> Spec {
    Spec {
        args: args.iter().map(|a| a.to_string()).collect(),
        privilege: Privilege::User,
        confirms: false,
    }
}

fn root(args: &[&str], rest: &[String], confirms: bool) -> Spec {
    let mut v: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    v.extend(rest.iter().cloned());
    Spec { args: v, privilege: Privilege::Root, confirms }
}

// ---- read-only, desktop user --------------------------------------------

pub fn version() -> Spec {
    user(&["--version"])
}

pub fn status() -> Spec {
    user(&["status"])
}

/// slacker's search takes exactly one exact name (case-insensitive).
pub fn search(name: &str) -> Spec {
    user(&["search", name])
}

/// info takes exactly one name.
pub fn info(name: &str) -> Spec {
    user(&["info", name])
}

pub fn list_repos() -> Spec {
    user(&["list-repos"])
}

/// Probes the official mirrors; prints suggestions, never edits `mirrors`.
pub fn find_mirror() -> Spec {
    user(&["find-mirror"])
}

/// `show-changelog [REPO]`: the official (tracked) repo when no name is
/// given. Always fetched fresh, so it needs the network.
pub fn show_changelog(repo: Option<&str>) -> Spec {
    match repo {
        Some(r) => user(&["show-changelog", r]),
        None => user(&["show-changelog"]),
    }
}

pub fn check_updates() -> Spec {
    user(&["check-updates"])
}

pub fn history_recent(last: usize) -> Spec {
    let n = last.to_string();
    user(&["history", "--last", &n])
}

pub fn history_installed() -> Spec {
    user(&["history", "--installed"])
}

// ---- listing that slacker allows only as root -----------------------------
// (not in its `requires_privilege` read-only list; with no argument they
// only print)

pub fn frozen_list() -> Spec {
    root(&["frozen"], &[], false)
}

pub fn pin_list() -> Spec {
    root(&["pin"], &[], false)
}

pub fn nodep_list() -> Spec {
    root(&["nodep"], &[], false)
}

// ---- changes that slacker confirms itself ---------------------------------

/// Prints the rule it will add, then "Add these to the blacklist? [y/N]"
/// (first "declare them anyway? [y/N]" when the rule looks like a mistake).
pub fn freeze(rule: &str) -> Spec {
    root(&["frozen"], &[rule.to_string()], true)
}

/// "write it to the blacklist? [y/N]"
pub fn pin(repo: &str, package: &str) -> Spec {
    root(&["pin"], &[format!("{repo}:{package}")], true)
}

/// "Write it to the repos file? [y/N]". Priorities are whole numbers; the
/// GUI offers only non-negative ones, so the value never reads as an option.
pub fn pri_repo(priority: u32, name: &str) -> Spec {
    root(&["pri-repo"], &[priority.to_string(), name.to_string()], true)
}

/// Lists the repositories with news and asks which to fetch; fetching
/// changes no package.
pub fn update() -> Spec {
    root(&["update"], &[], true)
}

/// A picker when several packages match, the plan, then "Install new
/// packages? [y/N]" (a conflict choice instead when the plan conflicts).
pub fn install_new() -> Spec {
    root(&["install-new"], &[], true)
}

/// Picker, plan, "Proceed with upgrade-all? [y/N]".
pub fn upgrade_all() -> Spec {
    root(&["upgrade-all"], &[], true)
}

pub fn install(names: &[String]) -> Spec {
    root(&["install"], names, true)
}

pub fn reinstall(names: &[String]) -> Spec {
    root(&["reinstall"], names, true)
}

pub fn remove(names: &[String]) -> Spec {
    root(&["remove"], names, true)
}

/// Prints the exclusions it will add (with a note when the package does not
/// itself require the dependency), then "write them to the blacklist? [y/N]".
/// Each rule is `package:dependency`, as made by `dep_rule`.
pub fn nodep(rules: &[String]) -> Spec {
    root(&["nodep"], rules, true)
}

// ---- changes that slacker makes at once -----------------------------------

/// Removes the rule whose text matches exactly; asks nothing.
pub fn unfreeze(rule: &str) -> Spec {
    root(&["unfrozen"], &[rule.to_string()], false)
}

/// Removes the package's pin; asks nothing.
pub fn unpin(package: &str) -> Spec {
    root(&["unpin"], &[package.to_string()], false)
}

/// Removes the given dependency exclusions (`package:dependency`); asks
/// nothing.
pub fn undep(rules: &[String]) -> Spec {
    root(&["undep"], rules, false)
}

/// Builds the `package:dependency` argument of `nodep`/`undep` from two exact
/// package names, checked the way slacker checks a `!package:dependency` line:
/// letters, digits and `._+-` only, so no space, comma or colon. A blank
/// package, or `*`, means any package. A leading `-` is refused, so the text
/// can never become an option.
pub fn dep_rule(package: &str, dependency: &str) -> Result<String, String> {
    fn name(what: &str, t: &str) -> Result<(), String> {
        if t.is_empty() {
            return Err(format!("Type the {what}."));
        }
        if t.starts_with('-') {
            return Err(format!("The {what} cannot start with \u{201c}-\u{201d}."));
        }
        if !t.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c)) {
            return Err(format!(
                "The {what} is one exact package name: letters, digits and \u{201c}._+-\u{201d}, no spaces."
            ));
        }
        Ok(())
    }
    let p = package.trim();
    let p = if p.is_empty() { "*" } else { p };
    let d = dependency.trim();
    if p != "*" {
        name("package", p)?;
    }
    name("dependency", d)?;
    Ok(format!("{p}:{d}"))
}

/// Checks a typed freeze rule. Spaces are allowed (`@repo PATTERN`); a
/// leading `-` is not, so the text can never become an option.
pub fn rule_text(input: &str) -> Result<String, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("Type a rule.".to_string());
    }
    if t.starts_with('-') {
        return Err("A rule cannot start with \u{201c}-\u{201d}.".to_string());
    }
    if t.chars().any(char::is_control) {
        return Err("The rule contains control characters.".to_string());
    }
    Ok(t.to_string())
}

/// Checks one typed package name. Anything starting with `-` is refused so
/// typed text can never turn into a slacker option.
pub fn single_name(input: &str) -> Result<String, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("Type a package name.".to_string());
    }
    if t.chars().any(char::is_whitespace) {
        return Err("Search takes one exact package name.".to_string());
    }
    if t.starts_with('-') {
        return Err(format!("\u{201c}{t}\u{201d} starts with \u{201c}-\u{201d} and would be read as an option."));
    }
    if t.chars().any(char::is_control) {
        return Err("The name contains control characters.".to_string());
    }
    Ok(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_spec() -> Vec<Spec> {
        let n = vec!["vim".to_string()];
        vec![
            version(),
            status(),
            search("vim"),
            info("vim"),
            list_repos(),
            find_mirror(),
            show_changelog(None),
            show_changelog(Some("conraid")),
            check_updates(),
            history_recent(10),
            history_installed(),
            frozen_list(),
            pin_list(),
            freeze("kde/"),
            pin("alienbob", "vlc"),
            pri_repo(61, "alienbob"),
            update(),
            install_new(),
            upgrade_all(),
            install(&n),
            reinstall(&n),
            remove(&n),
            unfreeze("kde/"),
            unpin("vlc"),
            nodep_list(),
            nodep(&["ghostty:xvfb-run".to_string()]),
            undep(&["ghostty:xvfb-run".to_string()]),
        ]
    }

    #[test]
    fn no_command_answers_for_the_user() {
        for s in every_spec() {
            assert!(
                !s.args.iter().any(|a| a == "--yes" || a == "-y" || a == "--dry-run"),
                "{:?}",
                s.args
            );
        }
    }

    #[test]
    fn changes_run_as_root_and_reads_as_the_user() {
        let n = vec!["vim".to_string()];
        let r = vec!["a:b".to_string()];
        for s in [freeze("x"), pin("a", "b"), pri_repo(1, "a"), update(), install_new(), upgrade_all(),
                  install(&n), reinstall(&n), remove(&n), unfreeze("x"), unpin("b"), frozen_list(), pin_list(),
                  nodep_list(), nodep(&r), undep(&r)] {
            assert_eq!(s.privilege, Privilege::Root, "{:?}", s.args);
        }
        for s in [version(), status(), search("vim"), info("vim"), list_repos(), find_mirror(),
                  show_changelog(None), check_updates(), history_recent(10), history_installed()] {
            assert_eq!(s.privilege, Privilege::User, "{:?}", s.args);
        }
    }

    #[test]
    fn only_unfreeze_unpin_and_undep_change_without_asking() {
        // Every root change except these three prints its plan and asks first
        // (checked against slacker's cmd_* functions), so the GUI must ask
        // for these three itself.
        let n = vec!["vim".to_string()];
        let r = vec!["a:b".to_string()];
        for s in [freeze("x"), pin("a", "b"), pri_repo(1, "a"), update(), install_new(), upgrade_all(),
                  install(&n), reinstall(&n), remove(&n), nodep(&r)] {
            assert!(s.confirms, "{:?}", s.args);
        }
        assert!(!unfreeze("x").confirms && !unpin("b").confirms && !undep(&r).confirms);
    }

    #[test]
    fn arguments_are_exactly_what_slacker_expects() {
        assert_eq!(search("vim").args, vec!["search", "vim"]);
        assert_eq!(info("vim").args, vec!["info", "vim"]);
        assert_eq!(history_recent(300).args, vec!["history", "--last", "300"]);
        assert_eq!(show_changelog(Some("conraid")).args, vec!["show-changelog", "conraid"]);
        assert_eq!(frozen_list().args, vec!["frozen"]);
        assert_eq!(pin_list().args, vec!["pin"]);
        assert_eq!(freeze("@testing kernel-generic").args, vec!["frozen", "@testing kernel-generic"]);
        assert_eq!(pin("alienbob", "vlc").args, vec!["pin", "alienbob:vlc"]);
        assert_eq!(pri_repo(61, "alienbob").args, vec!["pri-repo", "61", "alienbob"]);
        assert_eq!(unfreeze("fcitx5*").args, vec!["unfrozen", "fcitx5*"]);
        assert_eq!(unpin("vlc").args, vec!["unpin", "vlc"]);
        assert_eq!(upgrade_all().args, vec!["upgrade-all"]);
        assert_eq!(nodep_list().args, vec!["nodep"]);
        let r = vec!["ghostty:xvfb-run".to_string(), "*:cargo-auditable".to_string()];
        assert_eq!(nodep(&r).args, vec!["nodep", "ghostty:xvfb-run", "*:cargo-auditable"]);
        assert_eq!(undep(&r).args, vec!["undep", "ghostty:xvfb-run", "*:cargo-auditable"]);
    }

    #[test]
    fn typed_dependency_rules_are_checked() {
        assert_eq!(dep_rule(" ghostty ", " xvfb-run ").unwrap(), "ghostty:xvfb-run");
        assert_eq!(dep_rule("", "cargo-auditable").unwrap(), "*:cargo-auditable");
        assert_eq!(dep_rule("*", "cargo-auditable").unwrap(), "*:cargo-auditable");
        assert_eq!(dep_rule("slackpkg+", "gtk+3").unwrap(), "slackpkg+:gtk+3");
        for (p, d) in [("a", ""), ("a", "*"), ("a b", "c"), ("a", "b,c"), ("a", "b c"), ("a:b", "c"),
                       ("a", "b:c"), ("--yes", "b"), ("a", "-b"), ("py*", "b"), ("a", "b\u{7}")] {
            assert!(dep_rule(p, d).is_err(), "{p:?} {d:?}");
        }
    }

    #[test]
    fn typed_rules_are_checked() {
        assert_eq!(rule_text(" @alienbob vlc ").unwrap(), "@alienbob vlc");
        assert!(rule_text("").is_err());
        assert!(rule_text("--yes").is_err());
    }

    #[test]
    fn typed_names_are_checked() {
        assert_eq!(single_name("  vim ").unwrap(), "vim");
        assert!(single_name("  ").is_err());
        assert!(single_name("vim emacs").is_err());
        assert!(single_name("--yes").is_err());
        assert!(single_name("a\u{7}b").is_err());
    }
}
