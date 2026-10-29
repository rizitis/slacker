//! `frozen`, `unfrozen`, `pin`, `unpin`: editing blacklist rules and pins.

use crate::cli::Cli;
use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::{closest, confirm, Outcome};
use crate::{config, ui};
use std::collections::HashSet;

/// Add one or more package names to the blacklist ("freeze" them so update,
/// upgrade-all, reinstall, and clean-system leave them alone).
/// A soft warning for a `frozen` rule that parses but is almost certainly a
/// mistake: an `@repo` naming no active repo, or a plain regex whose pattern
/// contains whitespace — a package id (`name-version-arch-build`) never does,
/// so it usually means a forgotten `@` or a quoting slip. None if it looks ok.
fn frozen_warn(raw: &str, rule: &config::BlacklistRule, active: &[&str]) -> Option<String> {
    let q = format!("\"{raw}\"");
    if let Some(r) = rule.repo() {
        if !active.contains(&r) {
            return Some(format!("{q:<22} no active repo '{r}'"));
        }
    }
    if let Some(pat) = rule.pattern() {
        if pat.contains(char::is_whitespace) {
            let first = pat.split_whitespace().next().unwrap_or("");
            if active.contains(&first) {
                return Some(format!(
                    "{q:<22} looks like repo '{first}' without '@' — did you mean \"@{raw}\"?"
                ));
            }
            return Some(format!("{q:<22} pattern has a space; package names never do"));
        }
    }
    None
}

pub(crate) fn cmd_frozen(cli: &Cli, cfg: &Config, names: &[String]) -> Result<Outcome, String> {
    if names.is_empty() {
        // No argument: show the current freeze rules (pins are listed by `pin`).
        let text =
            std::fs::read_to_string(cfg.config_dir.join("blacklist")).unwrap_or_default();
        let rules: Vec<&str> = text
            .lines()
            .map(config::strip_comment)
            .filter(|r| !r.is_empty())
            .filter(|r| parse_pin_line(r).is_none())
            .collect();
        if rules.is_empty() {
            println!("No frozen rules set.");
        } else {
            println!("{}", ui::blue("Current frozen rules:"));
            for r in &rules {
                println!("  {}", ui::white(r));
            }
        }
        println!(
            "  {}",
            ui::dim(
                "to add a rule: `frozen <rule>`, e.g. `frozen vlc`, `frozen kde/`, `frozen \"@alienbob vlc\"`"
            )
        );
        return Ok(Outcome::Ok);
    }
    let active: Vec<&str> = cfg.repos.iter().map(|r| r.name.as_str()).collect();

    // Single pre-flight pass: parse every argument and collect *all* problems
    // (syntax errors and unknown-@repo typos) so they can be reported together,
    // before anything is written.
    let mut rules: Vec<(String, config::BlacklistRule)> = Vec::new();
    let mut syntax_errs: Vec<String> = Vec::new();
    let mut all_warns: Vec<String> = Vec::new();
    let mut repo_issue = false; // a message referred to an unknown/inactive repo
    for (idx, n) in names.iter().enumerate() {
        let raw = n.trim().to_string();
        match config::parse_blacklist_rule(n) {
            Ok(rule) => {
                if let Some(w) = frozen_warn(&raw, &rule, &active) {
                    all_warns.push(w);
                }
                rules.push((raw, rule));
            }
            Err(e) => {
                // Drop the leading "'raw': " the parser prepends so the batched
                // list stays aligned.
                let pfx = format!("'{raw}': ");
                let detail = e.strip_prefix(pfx.as_str()).unwrap_or(e.as_str()).to_string();
                // First check that the @repo (if any) is even an active repo: a
                // mistyped/inactive repo is the likelier mistake than a missing
                // pattern, so lead with it (with a typo hint), and remind that a
                // not-yet-enabled repo can still be declared with a full rule + --yes.
                let repo_tok = n
                    .trim()
                    .strip_prefix('@')
                    .and_then(|s| s.split(char::is_whitespace).next())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let detail = match repo_tok {
                    Some(r) if !active.contains(&r) => {
                        repo_issue = true;
                        let mut d = format!("no active repo '{r}'");
                        if let Some(s) = closest(r, active.iter().copied()) {
                            d.push_str(&format!(" — did you mean '@{s}'?"));
                        }
                        d.push_str(
                            "; a rule also needs a pattern, e.g. \"@REPO vlc\" \
                             (add --yes to declare one for a repo you will enable later)",
                        );
                        d
                    }
                    _ => detail,
                };
                let mut msg = format!("{:<22} {detail}", format!("\"{raw}\""));
                // Bare `@repo` followed by a separate argument => the user likely
                // forgot to quote them as one rule.
                if n.starts_with('@') && !n.contains(char::is_whitespace) && repo_tok.is_some_and(|r| active.contains(&r)) {
                    if let Some(next) = names.get(idx + 1) {
                        msg.push_str(&format!("  (did you mean \"{n} {next}\" ?)"));
                    }
                }
                syntax_errs.push(msg);
            }
        }
    }

    // Any syntax error is fatal: report every problem found (syntax + repo
    // typos) so the user can fix them all in one pass, and change nothing.
    if !syntax_errs.is_empty() {
        let total = syntax_errs.len() + all_warns.len();
        let mut out = format!("{total} problem(s), nothing changed:\n");
        for s in syntax_errs.iter().chain(all_warns.iter()) {
            out.push_str(&format!("  {s}\n"));
        }
        if !all_warns.is_empty() || repo_issue {
            out.push_str(&format!("  active repos: {}", active.join(", ")));
        }
        return Err(out.trim_end().to_string());
    }

    // Load the current blacklist and drop rules already present, so the
    // confirmation reflects exactly what will be added (not duplicates).
    let path = cfg.config_dir.join("blacklist");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let present: HashSet<String> = existing
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();

    let mut to_add: Vec<(String, config::BlacklistRule)> = Vec::new();
    let mut already: Vec<String> = Vec::new();
    for (raw, rule) in rules {
        if present.contains(&raw) {
            already.push(raw);
        } else {
            to_add.push((raw, rule));
        }
    }
    if !already.is_empty() {
        println!(
            "{}",
            ui::dim(&format!("already frozen, skipping: {}", already.join(", ")))
        );
    }
    if to_add.is_empty() {
        println!("Nothing new to add — every given rule is already frozen.");
        return Ok(Outcome::Ok);
    }

    // Rules that parse but look like mistakes (unknown @repo, or a regex with a
    // space that can never match) — considered only among the ones being added.
    let warns: Vec<String> = to_add
        .iter()
        .filter_map(|(raw, rule)| frozen_warn(raw, rule, &active))
        .collect();
    if !warns.is_empty() {
        println!(
            "{}",
            ui::purple(&format!("{} rule(s) look like a mistake:", warns.len()))
        );
        for s in &warns {
            println!("  {s}");
        }
        println!("  active repos: {}", active.join(", "));
        if !confirm("declare them anyway?", cli.yes || cli.dry_run) {
            println!("{}", ui::blue("aborted — nothing changed"));
            return Ok(Outcome::Ok);
        }
    }

    // Always confirm before writing: spell out exactly what will be frozen.
    println!("About to add {} blacklist rule(s):", to_add.len());
    for (i, (raw, rule)) in to_add.iter().enumerate() {
        println!(
            "  {}. {}  {}  {}",
            i + 1,
            ui::white(&format!("\"{raw}\"")),
            ui::dim("→"),
            rule.describe()
        );
    }
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("Add these to the blacklist?", cli.yes) {
        println!("{}", ui::blue("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }

    // Append the new rules.
    let mut body = existing;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    for (raw, _) in &to_add {
        body.push_str(raw);
        body.push('\n');
    }
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    let added: Vec<&str> = to_add.iter().map(|(r, _)| r.as_str()).collect();
    println!("Frozen (added to blacklist): {}", added.join(", "));
    Ok(Outcome::Ok)
}

/// Remove blacklist rules from the file text by EXACT canonical match.
///
/// Each file line is canonicalised with the same `strip_comment` the parser
/// uses, then compared to the requested rules by literal string equality — a
/// rule is NEVER interpreted as a regex, so metacharacters (`.*`, `*`, `-`,
/// `[]`, `/`) are matched verbatim and a partial name can never match a longer
/// rule. Comment and blank lines, and any rule not requested, are preserved
/// exactly. Returns (new_text, removed_rules, not_found_rules). Pure: no I/O.
fn blacklist_remove(text: &str, wanted: &[&str]) -> (String, Vec<String>, Vec<String>) {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for line in text.lines() {
        let rule = config::strip_comment(line);
        if !rule.is_empty() && wanted.contains(&rule) {
            removed.push(rule.to_string());
        } else {
            kept.push(line); // comments, blanks and untouched rules kept verbatim
        }
    }
    let mut not_found: Vec<String> = Vec::new();
    for w in wanted {
        if !removed.iter().any(|r| r.as_str() == *w) {
            not_found.push((*w).to_string());
        }
    }
    let mut body = kept.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    (body, removed, not_found)
}

/// Remove ("unfreeze") one or more blacklist rules. The counterpart to `frozen`.
/// With no argument it lists the current rules so the exact text to remove is
/// visible. Matching is by exact literal rule text (see `blacklist_remove`).
pub(crate) fn cmd_unfrozen(cli: &Cli, cfg: &Config, names: &[String]) -> Result<Outcome, String> {
    let path = cfg.config_dir.join("blacklist");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    // No argument: show the current rules (exact text) and stop.
    if names.is_empty() {
        let rules: Vec<&str> = text
            .lines()
            .map(config::strip_comment)
            .filter(|r| !r.is_empty())
            .collect();
        if rules.is_empty() {
            println!("The blacklist has no rules.");
            return Ok(Outcome::Ok);
        }
        println!("{}", ui::blue("Current blacklist rules:"));
        for r in &rules {
            println!("  {}", ui::white(r));
        }
        return Err(
            "unfrozen: give one or more rules to remove, EXACTLY as shown above \
             (quote rules with spaces or shell-special characters)"
                .into(),
        );
    }

    // Canonicalise the requested rules the same way a file line is canonicalised,
    // then remove by exact literal match.
    let wanted: Vec<&str> = names.iter().map(|s| config::strip_comment(s)).collect();
    let (new_text, removed, not_found) = blacklist_remove(&text, &wanted);

    if removed.is_empty() {
        return Err(format!(
            "no matching rule in {}: {}\n\
             (rules must match exactly — run `slacker unfrozen` with no argument to see them)",
            path.display(),
            not_found.join(", ")
        ));
    }

    if cli.dry_run {
        println!("Would unfreeze (remove from blacklist): {}", removed.join(", "));
        if !not_found.is_empty() {
            println!(
                "  {}",
                ui::dim(&format!("not found (unchanged): {}", not_found.join(", ")))
            );
        }
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }

    std::fs::write(&path, new_text).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!(
        "Unfrozen (removed from blacklist): {}",
        removed.join(", ")
    );
    if !not_found.is_empty() {
        println!(
            "  {}",
            ui::dim(&format!("not found (unchanged): {}", not_found.join(", ")))
        );
    }
    Ok(Outcome::Ok)
}

/// If `rule` is a pin line `@repo 100% pkg`, return (repo, pkg). Mirrors the
/// parser in config::parse_blacklist_rule so the two never disagree.
fn parse_pin_line(rule: &str) -> Option<(&str, &str)> {
    let after = rule.strip_prefix('@')?;
    let mut it = after.splitn(2, char::is_whitespace);
    let repo = it.next()?.trim();
    let rest = it.next()?.trim();
    let pkg = rest.strip_prefix("100%")?.trim();
    if repo.is_empty() || pkg.is_empty() || pkg.contains(char::is_whitespace) {
        return None;
    }
    Some((repo, pkg))
}

/// Drop every pin line (`@repo 100% name`) whose package is in `names`, keeping
/// all other lines (freezes, comments, blanks) verbatim. Returns the new text
/// and the removed (package, repo) pairs.
fn blacklist_remove_pins(text: &str, names: &[&str]) -> (String, Vec<(String, String)>) {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let rule = config::strip_comment(line);
        if let Some((repo, pkg)) = parse_pin_line(rule) {
            if names.contains(&pkg) {
                removed.push((pkg.to_string(), repo.to_string()));
                continue;
            }
        }
        kept.push(line);
    }
    let mut body = kept.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    (body, removed)
}

/// Pin a package to one repo regardless of priority. Writes `@repo 100% package`
/// into the `blacklist` file; the counterpart is `unpin`. A package has at most
/// one pin, so pinning to a new repo replaces any existing pin for it.
pub(crate) fn cmd_pin(cli: &Cli, cfg: &Config, spec: Option<&str>) -> Result<Outcome, String> {
    // No argument: show the current pins (like `unpin`) and how to add one.
    let spec = match spec {
        Some(s) => s,
        None => {
            let pins = cfg.pins();
            if pins.is_empty() {
                println!("No pins set.");
            } else {
                println!("{}", ui::blue("Current pins (package -> repo):"));
                for (pkg, repo) in pins {
                    println!("  {} {} {}", ui::white(pkg), ui::dim("->"), ui::cyan(repo));
                }
            }
            println!(
                "  {}",
                ui::dim("to add a pin: `pin repo:package`, e.g. `pin alienbob:vlc`")
            );
            return Ok(Outcome::Ok);
        }
    };
    let (repo, name) = spec.split_once(':').ok_or_else(|| {
        format!("pin takes repo:package, e.g. \"pin alienbob:vlc\" (got \"{spec}\")")
    })?;
    let (repo, name) = (repo.trim(), name.trim());
    if repo.is_empty() || name.is_empty() {
        return Err(format!("pin takes repo:package, e.g. \"pin alienbob:vlc\" (got \"{spec}\")"));
    }
    if name.contains(char::is_whitespace) {
        return Err(format!("pin takes a single package name, not \"{name}\""));
    }
    if config::name_has_pattern_chars(name) {
        let is_glob = name.contains(['*', '?']);
        return Err(format!(
            "pin takes an EXACT package name — no {} (got \"{name}\"). \
             For pattern freezing use `slacker frozen`, which accepts both globs \
             (`*`, `?`) and regexes; e.g. `slacker frozen \"{name}\"`.",
            if is_glob { "shell wildcards" } else { "patterns" }
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        let fixed = name.trim_matches('-');
        return Err(format!(
            "\"{name}\" is not a valid package name (a name does not start or end with '-'){}",
            if fixed.is_empty() {
                ".".to_string()
            } else {
                format!(" — did you mean \"{fixed}\"?")
            }
        ));
    }

    // The target repo must be active (with a did-you-mean for a typo).
    let active: Vec<&str> = cfg.repos.iter().map(|r| r.name.as_str()).collect();
    if !active.contains(&repo) {
        let mut msg = format!("no active repo '{repo}'");
        if let Some(s) = closest(repo, active.iter().copied()) {
            msg.push_str(&format!(" — did you mean '{s}'?"));
        }
        msg.push_str(&format!("\n  active repos: {}", active.join(", ")));
        return Err(msg);
    }

    // Tolerant load: a pin may be set before `update` (or for a repo not yet
    // fetched). Missing metadata just means we cannot confirm the package yet.
    let (db, _missing) = PkgDb::load_available(cfg);

    // Warn (but allow) if the repo does not currently provide the package — repos
    // change, and a pin may be set ahead of time. Offer a name typo hint.
    if db.resolve(&format!("{repo}:{name}")).is_none() {
        println!(
            "{}",
            ui::purple(&format!(
                "note: '{repo}' does not currently provide '{name}' — pin recorded anyway; \
                 it takes effect once the repo offers it"
            ))
        );
        if let Some(s) = closest(name, db.available_names()) {
            println!("  {}", ui::dim(&format!("did you mean '{repo}:{s}'?")));
        }
    }

    // A freeze on the same package overrides the pin (frozen is absolute) — say so.
    let frozen_here = cfg.blacklist_hit(name, db.series_of(name), Some(repo));
    if frozen_here {
        println!(
            "{}",
            ui::purple(&format!(
                "warning: '{name}' is also frozen (blacklisted) — the freeze wins, so the pin \
                 will have no effect until you `unfrozen` it"
            ))
        );
    }

    let line = format!("@{repo} 100% {name}");
    let path = cfg.config_dir.join("blacklist");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    // Already pinned exactly here? Nothing to do.
    if existing.lines().any(|l| config::strip_comment(l) == line) {
        println!("Already pinned: {name} -> {repo}");
        return Ok(Outcome::Ok);
    }

    // Pinned to a DIFFERENT repo? A package has one pin, so replace it.
    let prior = cfg.pinned_repo(name).map(str::to_string);
    let (mut text, _) = blacklist_remove_pins(&existing, &[name]);
    if let Some(p) = &prior {
        println!("{}", ui::dim(&format!("replacing existing pin: {name} -> {p}")));
    }

    println!("About to pin (only source for '{name}', ignoring priority):");
    println!("  {}", ui::white(&line));
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("write it to the blacklist?", cli.yes) {
        println!("{}", ui::blue("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }

    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{}", ui::green(&format!("Pinned: {name} -> {repo}")));
    Ok(Outcome::Ok)
}

/// Remove a package's pin, returning it to normal priority-based resolution. The
/// counterpart to `pin`. With no argument, lists the current pins.
pub(crate) fn cmd_unpin(cli: &Cli, cfg: &Config, names: &[String]) -> Result<Outcome, String> {
    let path = cfg.config_dir.join("blacklist");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    if names.is_empty() {
        let pins = cfg.pins();
        if pins.is_empty() {
            println!("No pins set.");
            println!(
                "  {}",
                ui::dim("to add a pin: `pin repo:package`, e.g. `pin alienbob:vlc`")
            );
            return Ok(Outcome::Ok);
        }
        println!("{}", ui::blue("Current pins (package -> repo):"));
        for (pkg, repo) in pins {
            println!("  {} {} {}", ui::white(pkg), ui::dim("->"), ui::cyan(repo));
        }
        println!(
            "  {}",
            ui::dim("to remove a pin: `unpin package`, e.g. `unpin vlc`")
        );
        return Ok(Outcome::Ok);
    }

    let wanted: Vec<&str> = names.iter().map(|s| s.trim()).collect();
    let (new_text, removed) = blacklist_remove_pins(&text, &wanted);
    if removed.is_empty() {
        return Err(format!(
            "no pin found for: {}\n  (run `slacker unpin` with no argument to list pins)",
            wanted.join(", ")
        ));
    }
    if cli.dry_run {
        let summary: Vec<String> =
            removed.iter().map(|(p, r)| format!("{p} (was -> {r})")).collect();
        println!("Would unpin: {}", summary.join(", "));
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    std::fs::write(&path, new_text).map_err(|e| format!("write {}: {e}", path.display()))?;
    let summary: Vec<String> =
        removed.iter().map(|(p, r)| format!("{p} (was -> {r})")).collect();
    println!("{}", ui::green(&format!("Unpinned: {}", summary.join(", "))));
    Ok(Outcome::Ok)
}

#[cfg(test)]
mod tests {
    use super::blacklist_remove;
    use super::{blacklist_remove_pins, parse_pin_line};

    #[test]
    fn parse_pin_line_recognizes_pins_only() {
        assert_eq!(parse_pin_line("@alienbob 100% vlc"), Some(("alienbob", "vlc")));
        // Not pins:
        assert_eq!(parse_pin_line("@alienbob vlc"), None); // freeze
        assert_eq!(parse_pin_line("vlc"), None); // bare freeze
        assert_eq!(parse_pin_line("@alienbob 100%"), None); // no package
        assert_eq!(parse_pin_line("@alienbob 100% a b"), None); // two names
    }

    #[test]
    fn remove_pins_drops_only_matching_pins() {
        // Freezes (plain + scoped) and comments must survive; only the named pins go.
        let text = "# head\nfirefox\n@alienbob 100% vlc\n@conraid 100% mpv\n@alienbob kde/\n";
        let (out, removed) = blacklist_remove_pins(text, &["vlc"]);
        assert_eq!(removed, vec![("vlc".to_string(), "alienbob".to_string())]);
        assert_eq!(out, "# head\nfirefox\n@conraid 100% mpv\n@alienbob kde/\n");
        // Removing a name with no pin removes nothing.
        let (out2, removed2) = blacklist_remove_pins(&out, &["firefox"]);
        assert!(removed2.is_empty());
        assert_eq!(out2, out);
    }

    #[test]
    fn removes_exact_rule_only_no_glob() {
        // 'fcitx5*' must remove only that line, never the longer 'fcitx5-qt'.
        let text = "# header\nsbopkg\nfcitx5*\nfcitx5-qt\nemacs\n";
        let (out, removed, nf) = blacklist_remove(text, &["fcitx5*"]);
        assert_eq!(removed, vec!["fcitx5*"]);
        assert!(nf.is_empty());
        assert_eq!(out, "# header\nsbopkg\nfcitx5-qt\nemacs\n");
    }

    #[test]
    fn regex_metachars_are_literal() {
        let text = "xf86-.*-202.*\nvlc\n";
        let (out, removed, _) = blacklist_remove(text, &["xf86-.*-202.*"]);
        assert_eq!(removed, vec!["xf86-.*-202.*"]);
        assert_eq!(out, "vlc\n");
    }

    #[test]
    fn partial_name_does_not_match() {
        // 'fcitx5' must NOT match the rule 'fcitx5*' — exact only.
        let text = "fcitx5*\n";
        let (out, removed, nf) = blacklist_remove(text, &["fcitx5"]);
        assert!(removed.is_empty());
        assert_eq!(nf, vec!["fcitx5"]);
        assert_eq!(out, "fcitx5*\n"); // unchanged
    }

    #[test]
    fn preserves_comments_and_reports_not_found() {
        // 'emacs # note' canonicalises to 'emacs' and matches; 'ghost' does not.
        let text = "# keep me\nemacs  # inline note\nsbopkg\n";
        let (out, removed, nf) = blacklist_remove(text, &["emacs", "ghost"]);
        assert_eq!(removed, vec!["emacs"]);
        assert_eq!(nf, vec!["ghost"]);
        assert_eq!(out, "# keep me\nsbopkg\n");
    }

    #[test]
    fn repo_scoped_rule_does_not_touch_bare_rule() {
        let text = "@alienbob vlc\nvlc\n";
        let (out, removed, _) = blacklist_remove(text, &["@alienbob vlc"]);
        assert_eq!(removed, vec!["@alienbob vlc"]);
        assert_eq!(out, "vlc\n");
    }

    #[test]
    fn removing_last_rule_leaves_header_intact() {
        let text = "# blacklist\nemacs\n";
        let (out, removed, _) = blacklist_remove(text, &["emacs"]);
        assert_eq!(removed, vec!["emacs"]);
        assert_eq!(out, "# blacklist\n");
    }
}
