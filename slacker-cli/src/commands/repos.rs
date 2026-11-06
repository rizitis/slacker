//! `add-repo`, `del-repo`, `pri-repo`, `add-tag`, `del-tag`, `vet-repo`,
//! `trust-repo`, `distrust-repo`: editing the `repos` file and repo trust state.

use crate::cli::Cli;
use crate::config::Config;
use crate::{apply_vet, at_prefix_hint, closest, confirm, reload_repo, Outcome};
use crate::{config, repo, ui};

/// What a `repos` line declares, for matching during removal. Mirrors the
/// classification in config::parse_repos: a third field that is a URL or the
/// `mirror` keyword makes it a binary repo, otherwise a build-tag priority.
enum RepoLineKind {
    Repo(String), // binary repo, by name
    Tag(String),  // tag-priority line, by tag
    Other,        // comment / blank / unparseable
}

fn classify_repos_line(raw: &str) -> RepoLineKind {
    let line = match raw.find('#') {
        Some(i) => &raw[..i],
        None => raw,
    }
    .trim();
    if line.is_empty() {
        return RepoLineKind::Other;
    }
    let mut f = line.split_whitespace();
    let (Some(_prio), Some(name), Some(third)) = (f.next(), f.next(), f.next()) else {
        return RepoLineKind::Other;
    };
    if third == "mirror" || third.contains("://") {
        RepoLineKind::Repo(name.to_string())
    } else {
        RepoLineKind::Tag(third.to_string())
    }
}

/// Append a line to the current `repos` body, ensuring a trailing newline.
fn repos_text_with(current: &str, line: &str) -> String {
    let mut out = current.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    out
}

/// Usage reminders shown on any add-* mistake (the "suggestion" half of the
/// validation). Each field is a separate word — unlike `frozen`, no quoting is
/// needed (quote only a URL that itself contains shell-special characters).
const ADD_REPO_USAGE: &str = "usage: slacker add-repo PRIORITY NAME URL [official] [immutable] [subtree] [verify=gpg,md5] [credentials=NAME] [insecure]\n  \
     e.g.  slacker add-repo 60 alienbob https://slackware.nl/people/alien/sbrepos/current/x86_64\n  \
     e.g.  slacker add-repo 70 extras https://slackware.uk/slackware/slackware64-current/extra subtree\n  \
     e.g.  slacker add-repo 80 myforge https://forge.slackware.nl/rizitis/repo credentials=forge\n  \
     (pass each field as a separate word — no quotes)";
const ADD_TAG_USAGE: &str = "usage: slacker add-tag PRIORITY NAME TAG\n  \
     e.g.  slacker add-tag 100 SBo _SBo\n  \
     (pass each field as a separate word — no quotes)";
const PRI_REPO_USAGE: &str = "usage: slacker pri-repo PRIORITY NAME\n  \
     e.g.  slacker pri-repo 90 alienbob\n  \
     (the priority comes first, then the repo name — no quotes)";

/// Parse the PRIORITY argument with friendly, suggestion-bearing errors —
/// including the common case where someone quoted the whole command (a habit
/// from `frozen`) so the first argument arrived with spaces in it.
fn parse_priority(s: &str, usage: &str) -> Result<i32, String> {
    if s.split_whitespace().count() > 1 {
        return Err(format!(
            "'{s}' contains spaces — don't quote the whole command; pass each field as a \
             separate word.\n{usage}"
        ));
    }
    s.parse::<i32>()
        .map_err(|_| format!("priority must be a whole number (e.g. 80), got '{s}'.\n{usage}"))
}

/// Parser errors are prefixed `repos:N:` (or `repos:`) where N is a line in the
/// *candidate* file we built internally — meaningless to someone adding a single
/// line (nothing was written, so there is no "line N"). Drop that prefix so the
/// add-* messages read cleanly.
fn strip_repos_prefix(e: &str) -> String {
    let rest = match e.strip_prefix("repos:") {
        Some(r) => r,
        None => return e.to_string(),
    };
    let t = rest.trim_start();
    let digits = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    if digits > 0 && t[digits..].starts_with(':') {
        t[digits + 1..].trim_start().to_string()
    } else {
        t.to_string()
    }
}

/// Rewrite the `repos` text so the active repo line named `name` carries the new
/// `pri`, changing only the priority token and leaving the URL, flags, and any
/// indentation untouched. Errors if no active repo line for `name` is found.
fn repos_text_set_priority(text: &str, name: &str, pri: i32) -> Result<String, String> {
    let mut out = String::with_capacity(text.len() + 4);
    let mut changed = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !changed && !trimmed.is_empty() && !trimmed.starts_with('#') {
            let mut f = trimmed.split_whitespace();
            let pri_tok = f.next();
            let name_tok = f.next();
            let third = f.next();
            // A repo line (not a build-tag line) has a URL or a `mirror` keyword
            // in the third field. Only those carry a priority we want to retune.
            let is_repo_line = third
                .map(|t| t.contains("://") || t == "mirror" || t.starts_with("mirror/"))
                .unwrap_or(false);
            if name_tok == Some(name) && is_repo_line {
                if let Some(pt) = pri_tok {
                    let indent = &line[..line.len() - trimmed.len()];
                    let rest = trimmed[pt.len()..].trim_start_matches([' ', '\t']);
                    out.push_str(indent);
                    out.push_str(&pri.to_string());
                    out.push(' ');
                    out.push_str(rest);
                    out.push('\n');
                    changed = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !changed {
        return Err(format!("could not find an active repo line for '{name}'."));
    }
    Ok(out)
}

/// `pri-repo PRIORITY NAME` — retune one repo's priority in the `repos` file.
/// Validates that NAME is an active repo (suggesting the closest on a typo) and
/// that the new priority is not already taken by another repo (they must stay
/// distinct), then rewrites just that line.
pub(crate) fn cmd_pri_repo(cli: &Cli, cfg: &Config, priority: &str, name: &str) -> Result<Outcome, String> {
    let pri = parse_priority(priority, PRI_REPO_USAGE)?;
    // 1) the repo must exist and be active.
    let target = match cfg.repos.iter().find(|r| r.name == name) {
        Some(r) => r,
        None => {
            let names: Vec<&str> = cfg.repos.iter().map(|r| r.name.as_str()).collect();
            let mut msg = format!("no active repo named '{name}'");
            let at = at_prefix_hint(cfg, name);
            if at.is_empty() {
                match closest(name, names.iter().copied()) {
                    Some(s) => msg.push_str(&format!(" — did you mean '{s}'?")),
                    None if names.is_empty() => msg.push_str("\n  there are no active repos."),
                    None => msg.push_str(&format!("\n  active repos: {}", names.join(", "))),
                }
            } else {
                msg.push_str(&at);
            }
            return Err(msg);
        }
    };
    // Already at this value: nothing to do.
    if target.priority == pri {
        println!(
            "{}",
            ui::dim(&format!("'{name}' is already at priority {pri} — nothing to change."))
        );
        return Ok(Outcome::Ok);
    }
    // 2) priorities must be distinct: refuse if another repo already owns it.
    if let Some(other) = cfg.repos.iter().find(|r| r.name != name && r.priority == pri) {
        return Err(format!(
            "priority {pri} is already used by repo '{}' — pick another value \
             (every repo needs a distinct priority).\n{PRI_REPO_USAGE}",
            other.name
        ));
    }
    // 3) rewrite just that line, then validate the whole file before writing.
    let path = cfg.config_dir.join("repos");
    let current =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let candidate = repos_text_set_priority(&current, name, pri)?;
    config::validate_repos_text(&cfg.config_dir, &candidate).map_err(|e| strip_repos_prefix(&e))?;
    let old = target.priority;
    println!(
        "{}",
        ui::blue(&format!("About to change '{name}' priority: {old} → {pri}"))
    );
    if !confirm("Write it to the repos file?", cli.yes) {
        println!("{}", ui::dim("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }
    std::fs::write(&path, candidate).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{}", ui::green(&format!("Set '{name}' priority: {old} → {pri}.")));
    println!(
        "{}",
        ui::dim("Run `slacker update` then `slacker status` to see the new ordering.")
    );
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_add_repo(
    cli: &Cli,
    cfg: &Config,
    priority: &str,
    name: &str,
    url: &str,
    flags: &[String],
) -> Result<Outcome, String> {
    let priority = parse_priority(priority, ADD_REPO_USAGE)?;
    // Swapped-argument heuristic: a URL in the NAME slot is a common slip.
    if name.contains("://") {
        return Err(format!(
            "'{name}' looks like a URL but is in the NAME position — the order is \
             PRIORITY NAME URL.\n{ADD_REPO_USAGE}"
        ));
    }
    // add-repo takes a web URL only: it must start with http:// or https://.
    let scheme_ok = {
        let l = url.to_ascii_lowercase();
        l.starts_with("http://") || l.starts_with("https://")
    };
    if !scheme_ok {
        return Err(format!(
            "'{url}' must start with http:// or https:// — add-repo only takes web URLs \
             (for a build tag use `add-tag`).\n{ADD_REPO_USAGE}"
        ));
    }
    // Reject a URL another repo already uses. A trailing slash is ignored, so
    // `…/x` and `…/x/` count as the same — like a duplicate name, it is almost
    // always a copy-paste mistake.
    let norm = |u: &str| u.trim_end_matches('/').to_string();
    if let Some(dup) = cfg.repos.iter().find(|r| norm(&r.url) == norm(url)) {
        return Err(format!(
            "that URL is already used by repo '{}': {}\n{ADD_REPO_USAGE}",
            dup.name, dup.url
        ));
    }
    // Credentials over plaintext http would leak on the wire. Refuse unless the
    // user explicitly accepts the risk with the `insecure` flag.
    let is_http = url.to_ascii_lowercase().starts_with("http://");
    let has_creds = flags.iter().any(|f| f.starts_with("credentials="));
    let has_insecure = flags.iter().any(|f| f == "insecure");
    if is_http && has_creds && !has_insecure {
        return Err(format!(
            "refusing to add repo '{name}': it carries credentials over plaintext http, \
             which would expose the login on the network.\n  \
             Fix: use an https:// URL for this repo.\n  \
             Override (at your own risk): re-run with the `insecure` flag to send credentials over http anyway."
        ));
    }
    let mut line = format!("{priority} {name} {url}");
    for f in flags {
        line.push(' ');
        line.push_str(f);
    }
    let path = cfg.config_dir.join("repos");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let candidate = repos_text_with(&current, &line);
    // Full validation: refuse anything that wouldn't load (duplicate priority or
    // name, a second `official`, an unknown flag, a bad verify=, ...).
    config::validate_repos_text(&cfg.config_dir, &candidate)
        .map_err(|e| format!("{}\n{ADD_REPO_USAGE}", strip_repos_prefix(&e)))?;

    println!("{}", ui::blue("About to add this repository:"));
    println!("  {}", ui::white(&line));
    if !confirm("Add it to the repos file?", cli.yes) {
        println!("{}", ui::dim("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }
    std::fs::write(&path, candidate).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{}", ui::green(&format!("Added repo '{name}'.")));

    // Vet it right away (the "enable" action): fetch only its metadata in a
    // sandbox and run safety checks. A repo that fails is quarantined and the
    // user is told plainly, with the override command.
    let trusted = match reload_repo(cfg, name) {
        Ok(r) => apply_vet(cfg, &r),
        Err(e) => {
            println!("{}", ui::yellow(&format!("(could not vet just now: {e})")));
            true
        }
    };
    if trusted {
        println!(
            "{}",
            ui::blue("Run `slacker update` to refresh, then `slacker status` to review.")
        );
    }
    println!(
        "{}",
        ui::dim(&format!("To undo entirely, remove it with:  slacker del-repo {name}"))
    );
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_del_repo(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    let path = cfg.config_dir.join("repos");
    let current = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;

    let mut removed: Vec<String> = Vec::new();
    let kept: Vec<&str> = current
        .lines()
        .filter(|raw| match classify_repos_line(raw) {
            RepoLineKind::Repo(n) if n == name => {
                removed.push((*raw).to_string());
                false
            }
            _ => true,
        })
        .collect();

    if removed.is_empty() {
        let names: Vec<String> = cfg.repos.iter().map(|r| r.name.clone()).collect();
        return Err(format!(
            "no repository named '{name}'{} in {} (configured: {})",
            at_prefix_hint(cfg, name),
            path.display(),
            names.join(", ")
        ));
    }

    let candidate = {
        let mut s = kept.join("\n");
        s.push('\n');
        s
    };
    // Refuse if this would leave no repositories (the config wouldn't load).
    config::validate_repos_text(&cfg.config_dir, &candidate)?;

    let was_official = cfg.repos.iter().any(|r| r.name == name && r.official);
    println!("{}", ui::blue("About to remove this repository:"));
    for r in &removed {
        println!("  {}", ui::red(r.trim()));
    }
    if was_official {
        println!(
            "{}",
            ui::yellow(
                "note: this is the official repo — ChangeLog tracking and the default \
                 install-new source will change."
            )
        );
    }
    // Warn about pins that point at the repo being removed: they become inert
    // (the pinned package just stays put until the repo returns or is re-pinned).
    let orphaned_pins: Vec<&str> = cfg
        .pins()
        .into_iter()
        .filter(|(_, repo)| *repo == name)
        .map(|(pkg, _)| pkg)
        .collect();
    if !orphaned_pins.is_empty() {
        println!(
            "{}",
            ui::yellow(&format!(
                "note: {} package(s) are pinned to '{name}': {}",
                orphaned_pins.len(),
                orphaned_pins.join(", ")
            ))
        );
        println!(
            "  {}",
            ui::dim(
                "their pins will have no effect once it is removed — \
                 `slacker unpin <pkg>` or re-pin elsewhere."
            )
        );
    }
    if !confirm("Remove it from the repos file?", cli.yes) {
        println!("{}", ui::dim("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }
    std::fs::write(&path, candidate).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{}", ui::green(&format!("Removed repo '{name}'.")));
    println!(
        "{}",
        ui::dim("its cached metadata and downloaded packages are left in the cache; \
                 `slacker clean-cache` can remove the packages.")
    );
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_add_tag(
    cli: &Cli,
    cfg: &Config,
    priority: &str,
    name: &str,
    tag: &str,
) -> Result<Outcome, String> {
    let priority = parse_priority(priority, ADD_TAG_USAGE)?;
    // A tag that looks like a URL/mirror would be parsed as a binary repo.
    if tag == "mirror" || tag.contains("://") {
        return Err(format!(
            "'{tag}' looks like a URL — `add-tag` takes a build tag (e.g. _SBo); \
             use `add-repo` for a repository.\n{ADD_TAG_USAGE}"
        ));
    }
    let line = format!("{priority} {name} {tag}");
    let path = cfg.config_dir.join("repos");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let candidate = repos_text_with(&current, &line);
    // Validates format and that the tag isn't already assigned a priority.
    config::validate_repos_text(&cfg.config_dir, &candidate)
        .map_err(|e| format!("{}\n{ADD_TAG_USAGE}", strip_repos_prefix(&e)))?;

    println!("{}", ui::blue("About to add this build-tag priority:"));
    println!("  {}", ui::white(&line));
    if !confirm("Add it to the repos file?", cli.yes) {
        println!("{}", ui::dim("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }
    std::fs::write(&path, candidate).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{}", ui::green(&format!("Added tag priority '{tag}' (priority {priority}).")));
    println!(
        "{}",
        ui::blue("Run `slacker status` to check. If something looks wrong, undo with:")
    );
    println!("  {}", ui::dim(&format!("slacker del-tag {tag}")));
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_del_tag(cli: &Cli, cfg: &Config, tag: &str) -> Result<Outcome, String> {
    let path = cfg.config_dir.join("repos");
    let current = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;

    let mut removed: Vec<String> = Vec::new();
    let kept: Vec<&str> = current
        .lines()
        .filter(|raw| match classify_repos_line(raw) {
            RepoLineKind::Tag(t) if t == tag => {
                removed.push((*raw).to_string());
                false
            }
            _ => true,
        })
        .collect();

    if removed.is_empty() {
        // A common slip: passing the NAME column instead of the TAG column.
        if let Some(tp) = cfg.tag_priorities.iter().find(|t| t.name == tag) {
            return Err(format!(
                "'{tag}' is the NAME column — del-tag takes the TAG; did you mean '{}'?",
                tp.tag
            ));
        }
        let tags: Vec<String> = cfg.tag_priorities.iter().map(|t| t.tag.clone()).collect();
        let have = if tags.is_empty() { "none".to_string() } else { tags.join(", ") };
        return Err(format!(
            "no tag-priority line for '{tag}' in {} (configured tags: {have})",
            path.display()
        ));
    }

    let candidate = {
        let mut s = kept.join("\n");
        s.push('\n');
        s
    };
    config::validate_repos_text(&cfg.config_dir, &candidate)?;

    println!("{}", ui::blue("About to remove this build-tag priority:"));
    for r in &removed {
        println!("  {}", ui::red(r.trim()));
    }
    if !confirm("Remove it from the repos file?", cli.yes) {
        println!("{}", ui::dim("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }
    std::fs::write(&path, candidate).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{}", ui::green(&format!("Removed tag priority '{tag}'.")));
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_vet_repo(cfg: &Config, name: &str) -> Result<Outcome, String> {
    let r = cfg
        .repo_by_name(name)
        .ok_or_else(|| {
            format!(
                "no repo named '{name}' in {}{}",
                cfg.config_dir.join("repos").display(),
                at_prefix_hint(cfg, name)
            )
        })?
        .clone();
    // Force a fresh verdict: drop any prior "trusted" mark so the checks run.
    repo::unmark_trusted(&cfg.state_dir, name);
    apply_vet(cfg, &r);
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_trust_repo(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    // Allow trusting even if the name isn't currently in repos (e.g. a stale
    // marker), but prefer a clear error when the repo truly doesn't exist.
    if cfg.repo_by_name(name).is_none() {
        return Err(format!(
            "no repo named '{name}' in {}{}",
            cfg.config_dir.join("repos").display(),
            at_prefix_hint(cfg, name)
        ));
    }
    if !repo::is_quarantined(&cfg.state_dir, name) {
        println!("{}", ui::dim(&format!("repo '{name}' is not quarantined — nothing to do.")));
        return Ok(Outcome::Ok);
    }
    if let Some(reason) = repo::quarantine_reason(&cfg.state_dir, name) {
        println!("{}", ui::yellow(&format!("'{name}' was frozen because: {reason}")));
    }
    println!(
        "{}",
        ui::red("Trusting it overrides slacker's safety verdict — you accept full responsibility.")
    );
    if !confirm(&format!("Trust repo '{name}' and lift the freeze?"), cli.yes) {
        println!("{}", ui::dim("aborted — repo stays quarantined"));
        return Ok(Outcome::Ok);
    }
    repo::clear_quarantine(&cfg.state_dir, name);
    repo::mark_trusted(&cfg.state_dir, name);
    println!(
        "{}",
        ui::green(&format!("Repo '{name}' is now trusted. Run `slacker update` to fetch it."))
    );
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_distrust_repo(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    let r = cfg
        .repo_by_name(name)
        .ok_or_else(|| {
            format!(
                "no repo named '{name}' in {}{}",
                cfg.config_dir.join("repos").display(),
                at_prefix_hint(cfg, name)
            )
        })?
        .clone();
    if repo::is_quarantined(&cfg.state_dir, name) {
        println!("{}", ui::dim(&format!("repo '{name}' is already quarantined.")));
        return Ok(Outcome::Ok);
    }
    if !confirm(
        &format!("Freeze (quarantine) repo '{name}' so it provides no packages?"),
        cli.yes,
    ) {
        println!("{}", ui::dim("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }
    repo::quarantine(&r, &cfg.cache_dir, &cfg.state_dir, repo::QuarantineKind::Hard, "manually distrusted by the user")?;
    println!(
        "{}",
        ui::green(&format!(
            "Repo '{name}' is now quarantined. Re-enable later with:  slacker trust-repo {name}"
        ))
    );
    Ok(Outcome::Ok)
}

#[cfg(test)]
mod tests {
    #[test]
    fn repos_text_set_priority_retunes_only_the_named_line() {
        let text = "100 slackware mirror official\n\
                    # 80 alienbob https://example.com/a\n\
                    80 conraid https://example.com/c\n\
                    100 SBo _SBo\n";
        let out = super::repos_text_set_priority(text, "conraid", 65).unwrap();
        // conraid's priority changes; URL + other lines untouched.
        assert!(out.contains("65 conraid https://example.com/c"));
        assert!(out.contains("100 slackware mirror official"));
        assert!(out.contains("100 SBo _SBo")); // a build-tag line of the same shape is left alone
        assert!(out.contains("# 80 alienbob")); // commented line untouched
        // A name that is not an active repo line is rejected.
        assert!(super::repos_text_set_priority(text, "alienbob", 60).is_err());
        assert!(super::repos_text_set_priority(text, "nope", 60).is_err());
    }
}
