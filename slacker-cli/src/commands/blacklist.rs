//! `frozen`, `unfrozen`, `pin`, `unpin`, `nodep`, `undep`: editing blacklist
//! rules, pins and dependency exclusions.

use crate::cli::Cli;
use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::{closest, confirm, Outcome};
use crate::{config, repo, ui};
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
        // No argument: show the current freeze rules (pins are listed by `pin`,
        // dependency exclusions by `nodep`).
        let text =
            std::fs::read_to_string(cfg.config_dir.join("blacklist")).unwrap_or_default();
        let rules: Vec<&str> = text
            .lines()
            .map(config::strip_comment)
            .filter(|r| !r.is_empty())
            .filter(|r| parse_pin_line(r).is_none())
            .filter(|r| !r.starts_with('!'))
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

/// Parse a `nodep`/`undep` argument `package:dependency` (a leading `!`, as the
/// rule is written in the file, is accepted too) into its canonical rule text
/// `!package:dependency` and the (package, dependency) pair. The checks are the
/// blacklist parser's own, so the commands and the file never disagree.
fn nodep_spec(spec: &str) -> Result<(String, String, String), String> {
    let body = spec.trim();
    let body = body.strip_prefix('!').unwrap_or(body);
    let rule = config::parse_blacklist_rule(&format!("!{body}"))?;
    let (p, d) = rule.nodep().ok_or_else(|| format!("'{spec}' is not a dependency rule"))?;
    Ok((format!("!{p}:{d}"), p.to_string(), d.to_string()))
}

/// Parse every `nodep`/`undep` argument, collecting ALL problems so they are
/// reported together and nothing is changed unless every argument is valid.
/// Duplicates are dropped, first occurrence kept.
fn nodep_specs(specs: &[String]) -> Result<Vec<(String, String, String)>, String> {
    let mut rules: Vec<(String, String, String)> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    for s in specs {
        match nodep_spec(s) {
            Ok(r) => {
                if !rules.iter().any(|(l, _, _)| *l == r.0) {
                    rules.push(r);
                }
            }
            Err(e) => errs.push(e),
        }
    }
    if errs.is_empty() {
        Ok(rules)
    } else {
        Err(format!("{} problem(s), nothing changed:\n  {}", errs.len(), errs.join("\n  ")))
    }
}

/// Drop every dependency-exclusion line whose (package, dependency) is in
/// `wanted`, keeping all other lines verbatim (comments, blanks, freezes, pins,
/// other exclusions). Returns the new text and the removed rules, canonical and
/// without repeats.
fn blacklist_remove_nodeps(text: &str, wanted: &[(String, String)]) -> (String, Vec<String>) {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for line in text.lines() {
        let rule = config::strip_comment(line);
        if rule.starts_with('!') {
            if let Ok((canon, p, d)) = nodep_spec(rule) {
                if wanted.iter().any(|(wp, wd)| *wp == p && *wd == d) {
                    if !removed.contains(&canon) {
                        removed.push(canon);
                    }
                    continue;
                }
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

/// "'dep'" of "'package'" / "any package", for messages.
fn of_whom(parent: &str) -> String {
    if parent == "*" {
        "any package".to_string()
    } else {
        format!("'{parent}'")
    }
}

/// Print the current dependency exclusions, with `hint` underneath.
fn list_nodeps(cfg: &Config, hint: &str) {
    let rules = cfg.nodeps();
    if rules.is_empty() {
        println!("No dependency exclusions set.");
    } else {
        println!("{}", ui::blue("Current dependency exclusions:"));
        for (p, d) in rules {
            println!(
                "  {}  {}",
                ui::white(&format!("!{p}:{d}")),
                ui::dim(&format!("('{d}' is not pulled in for {})", of_whom(p)))
            );
        }
    }
    println!("  {}", ui::dim(hint));
}

const NODEP_HINT: &str = "to add: `nodep package:dependency...`, e.g. \
    `nodep python-sqlalchemy:python-psycopg2`, or `nodep '*:cargo-auditable'` for any package";

/// Never pull a dependency in for one package (`nodep package:dependency`), or
/// for any package (`nodep '*:dependency'`); several may be given at once.
/// Writes one `!package:dependency` line per rule into the `blacklist` file;
/// the counterpart is `undep`. Other packages that require the dependency still
/// pull it in, which is what makes this safer than freezing the dependency.
pub(crate) fn cmd_nodep(cli: &Cli, cfg: &Config, specs: &[String]) -> Result<Outcome, String> {
    if specs.is_empty() {
        list_nodeps(cfg, NODEP_HINT);
        return Ok(Outcome::Ok);
    }
    let rules = nodep_specs(specs)?;

    // Tolerant load: a rule may be set before `update`. A name no repo offers
    // is recorded anyway (repos change), with a typo hint.
    let (db, _missing) = PkgDb::load_available(cfg);
    let mut seen: HashSet<&str> = HashSet::new();
    for (_, p, d) in &rules {
        for n in [p.as_str(), d.as_str()] {
            if n != "*" && seen.insert(n) && db.candidates(n).is_empty() {
                let mut msg =
                    format!("note: no repo currently provides '{n}' — rule recorded anyway");
                if let Some(s) = closest(n, db.available_names()) {
                    msg.push_str(&format!("; did you mean '{s}'?"));
                }
                println!("{}", ui::purple(&msg));
            }
        }
    }

    // A rule only acts when the package itself requires the dependency. Say so
    // when it does not (a common slip: naming the top package of a chain), and
    // name the package one level down that does, but still record the rule:
    // a repo may add the dependency later.
    for (_, p, d) in &rules {
        if p != "*" {
            if let Some(msg) = nodep_reach_note(cfg, &db, p, d) {
                println!("{}", ui::purple(&msg));
            }
        }
    }

    let present = cfg.nodeps();
    let (already, to_add): (Vec<_>, Vec<_>) = rules
        .into_iter()
        .partition(|(_, p, d)| present.iter().any(|(pp, pd)| pp == p && pd == d));
    if !already.is_empty() {
        let names: Vec<&str> = already.iter().map(|(l, _, _)| l.as_str()).collect();
        println!("{}", ui::dim(&format!("already set, skipping: {}", names.join(", "))));
    }
    if to_add.is_empty() {
        println!("Nothing new to add — every given rule is already set.");
        return Ok(Outcome::Ok);
    }

    println!("About to add {} dependency exclusion(s):", to_add.len());
    for (i, (line, p, d)) in to_add.iter().enumerate() {
        println!(
            "  {}. {}  {}  never pulls '{d}' in as a dependency of {}",
            i + 1,
            ui::white(line),
            ui::dim("→"),
            of_whom(p)
        );
    }
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("write them to the blacklist?", cli.yes) {
        println!("{}", ui::blue("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }
    let path = cfg.config_dir.join("blacklist");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for (line, _, _) in &to_add {
        text.push_str(line);
        text.push('\n');
    }
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    let added: Vec<&str> = to_add.iter().map(|(l, _, _)| l.as_str()).collect();
    println!("{}", ui::green(&format!("Added: {}", added.join(", "))));
    Ok(Outcome::Ok)
}

/// The dependencies `pkg` declares, read as the resolver reads them (its
/// `.dep`, else `PACKAGE REQUIRED`).
fn declared_deps(cfg: &Config, pkg: &crate::repo::AvailPkg) -> Vec<String> {
    cfg.repo_by_name(&pkg.repo).map(|r| repo::fetch_dep(r, pkg)).unwrap_or_default()
}

/// None when some build of `parent` requires `dep` itself (or no repo offers
/// `parent`, which `cmd_nodep` reports on its own). Otherwise a note that the
/// rule does nothing, naming the dependencies of `parent` that require `dep`
/// (one level down, each taken from the parent's repo, as the resolver takes
/// it).
fn nodep_reach_note(cfg: &Config, db: &PkgDb, parent: &str, dep: &str) -> Option<String> {
    let builds = db.candidates(parent);
    if builds.is_empty() {
        return None;
    }
    let mut via: Vec<String> = Vec::new();
    for b in &builds {
        let deps = declared_deps(cfg, b);
        if deps.iter().any(|d| d == dep) {
            return None;
        }
        for x in deps {
            if via.contains(&x) {
                continue;
            }
            if let Some(o) = db.resolve(&format!("{}:{x}", b.repo)) {
                if declared_deps(cfg, o).iter().any(|d| d == dep) {
                    via.push(x);
                }
            }
        }
    }
    if via.is_empty() {
        return Some(format!(
            "note: '{parent}' does not require '{dep}' — this rule does nothing as it stands; \
             `slacker --dry-run install {parent}` shows which package brings it in (\"for ...\")"
        ));
    }
    let through: Vec<String> = via.iter().map(|v| format!("'{v}'")).collect();
    let fixes: Vec<String> = via.iter().map(|v| format!("`nodep {v}:{dep}`")).collect();
    Some(format!(
        "note: '{parent}' does not require '{dep}' itself; it comes in through {} \
         — this rule does nothing; use {} (`slacker --dry-run install {parent}` shows \
         every package that brings it in)",
        through.join(" or "),
        fixes.join(" or ")
    ))
}

/// Remove dependency exclusions (`undep package:dependency...`). The
/// counterpart to `nodep`. With no argument, lists the current ones.
pub(crate) fn cmd_undep(cli: &Cli, cfg: &Config, names: &[String]) -> Result<Outcome, String> {
    if names.is_empty() {
        list_nodeps(cfg, "to remove: `undep package:dependency...`, written as listed above");
        return Ok(Outcome::Ok);
    }
    let rules = nodep_specs(names)?;
    let wanted: Vec<(String, String)> = rules.iter().map(|(_, p, d)| (p.clone(), d.clone())).collect();
    let path = cfg.config_dir.join("blacklist");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let (new_text, removed) = blacklist_remove_nodeps(&text, &wanted);
    let not_found: Vec<&str> = rules
        .iter()
        .map(|(l, _, _)| l.as_str())
        .filter(|l| !removed.iter().any(|r| r == l))
        .collect();
    if removed.is_empty() {
        return Err(format!(
            "no dependency exclusion found for: {}\n  (run `slacker undep` with no argument to list them)",
            not_found.join(", ")
        ));
    }
    let report_missing = || {
        if !not_found.is_empty() {
            println!("  {}", ui::dim(&format!("not found (unchanged): {}", not_found.join(", "))));
        }
    };
    if cli.dry_run {
        println!("Would remove: {}", removed.join(", "));
        report_missing();
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    std::fs::write(&path, new_text).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{}", ui::green(&format!("Removed: {}", removed.join(", "))));
    report_missing();
    Ok(Outcome::Ok)
}

#[cfg(test)]
mod tests {
    use super::blacklist_remove;
    use super::{blacklist_remove_nodeps, nodep_spec, nodep_specs};
    use super::{blacklist_remove_pins, parse_pin_line};

    #[test]
    fn nodep_spec_canonicalises_and_validates() {
        let s = |x: &str| nodep_spec(x).map(|(l, _, _)| l);
        assert_eq!(s("python-sqlalchemy:python-psycopg2").unwrap(), "!python-sqlalchemy:python-psycopg2");
        assert_eq!(s("!a:b").unwrap(), "!a:b"); // as written in the file
        assert_eq!(s(" a:b ").unwrap(), "!a:b"); // outer blanks of the argument
        assert_eq!(s("*:cargo-auditable").unwrap(), "!*:cargo-auditable");
        assert_eq!(s("slackpkg+:gtk+3").unwrap(), "!slackpkg+:gtk+3");
        for bad in ["a", "a:", ":b", "a:*", "py*:b", "a:b:c", "a b:c", "a : b", "a:b,c", "a:b c", "!!a:b"] {
            assert!(s(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn nodep_specs_reports_every_problem_and_drops_repeats() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let ok = nodep_specs(&v(&["a:b", "!a:b", "a:c"])).unwrap();
        let lines: Vec<&str> = ok.iter().map(|(l, _, _)| l.as_str()).collect();
        assert_eq!(lines, vec!["!a:b", "!a:c"]);
        let err = nodep_specs(&v(&["a:b", "x", "y:z,w"])).unwrap_err();
        assert!(err.starts_with("2 problem(s), nothing changed"), "{err}");
    }

    #[test]
    fn remove_nodeps_drops_only_matching_rules() {
        let text = "# head\nfirefox\n!a:b  # note\n!*:c\n@alienbob 100% vlc\n!a:x\n!a:b\n";
        let (out, removed) = blacklist_remove_nodeps(text, &[("a".into(), "b".into())]);
        assert_eq!(removed, vec!["!a:b".to_string()]); // both copies go, reported once
        assert_eq!(out, "# head\nfirefox\n!*:c\n@alienbob 100% vlc\n!a:x\n");
        let (out2, removed2) = blacklist_remove_nodeps(&out, &[("x".into(), "c".into())]);
        assert!(removed2.is_empty()); // `!*:c` is not `!x:c`
        assert_eq!(out2, out);
    }

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
