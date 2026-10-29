//! clean-system and clean-cache.

use crate::cli::Cli;
use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::{
    at_prefix_hint, bl_installed, confirm, hilite_keys, parse_selection, print_table, Outcome, PlanRow,
};
use crate::{pkg, system, ui};
use std::collections::HashSet;
use std::io::Write;

/// Whether an installed package should be treated as "pinned to its source" by
/// clean-system: it carries a persistent pin (`pin_repo` is `Some`) AND its
/// installed build actually came from that pinned repo (`installed_tag_repo` —
/// the repo that owns the installed build tag — equals the pin's repo). A pinned
/// name installed from some OTHER repo returns false: the pin is then ignored and
/// the package falls through to the normal foreign-package logic (kept if
/// official, removed if nothing else protects it).
fn is_pinned_from_its_repo(pin_repo: Option<&str>, installed_tag_repo: Option<&str>) -> bool {
    matches!((pin_repo, installed_tag_repo), (Some(p), Some(o)) if p == o)
}

pub(crate) fn cmd_clean_system(cli: &Cli, cfg: &Config) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;

    // Foreign = an installed package no longer part of the distro nor an
    // explicitly-kept source. The decision mirrors how a package is ATTRIBUTED
    // to a repo (the same logic that drives list-repos), so an `immutable` repo
    // keeps exactly the packages slacker attributes to it:
    //
    //   * Tagged install (cf, alien, _SBo, ...): it came from a tagged source,
    //     identified by its build tag. Kept iff that tag is in IGNORE_TAGS, OR
    //     the repo that owns the tag is `immutable`. So `immutable extras` keeps
    //     every package carrying extras' tag — but making `patches` immutable
    //     does NOT keep a `foo` you installed from alienbob (its `alien` tag is
    //     owned by alienbob, not patches), so that `foo` is still foreign.
    //   * Tagless install (empty build tag — the Slackware/official convention):
    //     slacker cannot tell which tagless repo it came from, so it is kept iff
    //     its NAME is in the baseline (official + every `immutable` repo). This
    //     is how a tagless `immutable` repo is kept whole, by name; and a package
    //     that left both official and every immutable repo becomes foreign.
    //
    // Blacklisted packages are always kept. With no baseline repo configured the
    // name set falls back to "any repo" so a third-party-only setup isn't told to
    // remove everything.
    let baseline: HashSet<&str> = cfg
        .repos
        .iter()
        .filter(|r| r.official || r.immutable)
        .map(|r| r.name.as_str())
        .collect();
    let immutable: HashSet<&str> =
        cfg.repos.iter().filter(|r| r.immutable).map(|r| r.name.as_str()).collect();
    let scope: Option<&HashSet<&str>> = if baseline.is_empty() { None } else { Some(&baseline) };

    // Safety: if a baseline repo is configured but its metadata isn't loaded
    // (never updated, or quarantined), packages it would keep could look foreign.
    // Refuse rather than propose mass removal as root. EXCEPTION: a `subtree` repo
    // (patches/extra/testing) can be legitimately empty — e.g. slackware64-current
    // ships an empty patches/ — and an empty repo simply contributes no names, so
    // it cannot cause a wrong removal. The official main tree remains the authority.
    for name in &baseline {
        if !db.has_repo_packages(name) {
            let is_subtree = cfg.repo_by_name(name).is_some_and(|r| r.subtree);
            if is_subtree {
                continue;
            }
            return Err(format!(
                "baseline repo '{name}' has no package data loaded — run `slacker update` first. \
                 Refusing to continue so nothing is wrongly removed."
            ));
        }
    }

    let baseline_names = db.names_provided_by(scope);
    let orphans: Vec<&pkg::PkgId> = installed
        .iter()
        .filter(|p| {
            // Never propose removing slacker itself. clean-system (and the
            // dist-upgrade phase that reuses this logic) must not delete the
            // running package manager — and the _FRG/third-party build of
            // slacker belongs to no official repo, so without this guard it
            // would look "foreign". The name is taken from our own package so a
            // rename of the project keeps the guard correct.
            if p.name.as_str() == env!("CARGO_PKG_NAME") {
                return false;
            }
            if bl_installed(cfg, Some(&db), p) {
                return false; // blacklisted -> always kept
            }
            if !p.is_official_build() {
                // Genuine third-party tag (e.g. `_SBo`, `alien`): kept by
                // IGNORE_TAGS, or if its owning repo is immutable.
                let tag = p.build_tag();
                let owned_by_immutable =
                    db.repo_for_tag(tag).is_some_and(|r| immutable.contains(r));
                !(cfg.is_ignored_tag(tag) || owned_by_immutable)
            } else {
                // Official: tagless (-current) OR a `_slack<version>` stable patch
                // — both kept iff their NAME is in the baseline (official +
                // immutable). A stable system's patched packages carry
                // `_slack15.0` but are the most official packages there are, so
                // they must never be flagged foreign for the sake of that tag.
                !baseline_names.contains(p.name.as_str())
            }
        })
        .collect();

    // Among the foreign packages, separate out ones the user PINNED to a repo
    // and actually installed FROM that repo (the pin's repo owns the installed
    // build tag). A pin is an explicit "take this from here" choice, so removing
    // such a package is offered as a deliberate decision (R/K/P, default Keep)
    // rather than lumped in with the rest. A pinned name whose installed copy is
    // NOT from the pin repo keeps falling through to the normal logic above.
    let (pinned, mut orphans): (Vec<&pkg::PkgId>, Vec<&pkg::PkgId>) =
        orphans.into_iter().partition(|p| {
            is_pinned_from_its_repo(cfg.pinned_repo(&p.name), db.repo_for_tag(p.build_tag()))
        });

    if !pinned.is_empty() {
        println!("{}", ui::blue("Pinned packages found among the foreign set:"));
        println!();
        let pw = pinned.len().to_string().len();
        for (i, p) in pinned.iter().enumerate() {
            let to = cfg.pinned_repo(&p.name).unwrap_or("?");
            println!(
                "  {}) {}{}  {}",
                ui::dim(&format!("{:>pw$}", i + 1, pw = pw)),
                ui::white(&p.name),
                ui::dim(&format!("-{}-{}-{}", p.version, p.arch, p.build)),
                ui::dim(&format!("(pinned to {to})"))
            );
        }
        println!();

        if cli.dry_run {
            println!(
                "{}",
                ui::dim("(dry-run: pinned packages would be offered [R]emove all / [K]eep all / [P]rompt each — kept by default)")
            );
            // default Keep: leave them out of the removal set in dry-run.
        } else if cli.yes {
            // Non-interactive: a pin is explicit intent, so --yes keeps pinned
            // packages rather than sweeping them away. Remove them by hand if
            // that is really wanted.
            println!("{}", ui::blue("Keeping pinned packages (--yes does not remove pinned packages)."));
        } else {
            print!(
                "{}",
                hilite_keys("Pinned packages — [R]emove all · [K]eep all · [P]rompt each (default Keep): ")
            );
            std::io::stdout().flush().ok();
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                return Ok(Outcome::Ok);
            }
            match line.trim() {
                "r" | "R" => {
                    // Fold every pinned package back into the foreign list; they
                    // then appear in the normal plan below (still keepable by no.).
                    orphans.extend(pinned.iter().copied());
                }
                "p" | "P" => {
                    for p in &pinned {
                        let to = cfg.pinned_repo(&p.name).unwrap_or("?");
                        print!(
                            "{}",
                            hilite_keys(&format!(
                                "  remove {}-{}-{}-{} (pinned to {to})? [y/N]: ",
                                p.name, p.version, p.arch, p.build
                            ))
                        );
                        std::io::stdout().flush().ok();
                        let mut ans = String::new();
                        if std::io::stdin().read_line(&mut ans).is_err() {
                            return Ok(Outcome::Ok);
                        }
                        if matches!(ans.trim(), "y" | "Y" | "yes") {
                            orphans.push(p);
                        }
                    }
                }
                // K / k / keep / empty (Enter) / anything unrecognised -> keep all.
                // Keep is the safe default for an explicit pin.
                _ => {
                    println!("{}", ui::blue("Keeping all pinned packages."));
                }
            }
        }
    }

    if orphans.is_empty() {
        if pinned.is_empty() {
            println!("No foreign packages found.");
        } else {
            println!("No further foreign packages to remove.");
        }
        return Ok(Outcome::Ok);
    }

    let header = if scope.is_some() {
        "The following installed packages are no longer part of the official distribution:"
    } else {
        "The following installed packages belong to no configured repo:"
    };
    println!("{}", ui::blue(header));
    println!();
    let width = orphans.len().to_string().len();
    for (i, p) in orphans.iter().enumerate() {
        println!(
            "  {}) {}{}",
            ui::dim(&format!("{:>width$}", i + 1, width = width)),
            ui::white(&p.name),
            ui::dim(&format!("-{}-{}-{}", p.version, p.arch, p.build))
        );
    }
    println!();

    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }

    // Default action is to remove every listed package; the user may keep some
    // by number, or cancel entirely. With --yes we remove them all.
    let to_remove: Vec<&pkg::PkgId> = if cli.yes {
        orphans.clone()
    } else {
        println!("{}", hilite_keys("Enter numbers to KEEP (e.g. 1 3 5 or 2-4), [n] to keep all/cancel,"));
        print!("{}", hilite_keys("or press [Enter] to remove all listed: "));
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return Ok(Outcome::Ok);
        }
        let trimmed = line.trim();
        if matches!(trimmed, "n" | "N" | "none" | "q") {
            println!("Nothing removed.");
            return Ok(Outcome::Ok);
        }
        let keep = parse_selection(trimmed, orphans.len());
        if !trimmed.is_empty() && keep.is_empty() {
            // Non-empty but nothing valid parsed (a typo, or a non-Latin key like
            // Greek 'ν' meant as 'n'). Do NOT fall through to "remove all" — that
            // is the opposite of a mistyped cancel. Abort safely; only an explicit
            // Enter removes everything.
            println!(
                "{}",
                ui::blue(&format!("didn't understand {trimmed:?} — nothing removed (run clean-system again)."))
            );
            return Ok(Outcome::Ok);
        }
        let chosen: Vec<&pkg::PkgId> = orphans
            .iter()
            .enumerate()
            .filter(|(i, _)| !keep.contains(&(i + 1)))
            .map(|(_, p)| *p)
            .collect();
        if chosen.is_empty() {
            println!("Nothing to remove (all kept).");
            return Ok(Outcome::Ok);
        }
        if !keep.is_empty() {
            println!(
                "{}",
                ui::blue(&format!(
                    "Keeping {} package(s); will remove {}:",
                    keep.len(),
                    chosen.len()
                ))
            );
            let rows: Vec<PlanRow> = chosen
                .iter()
                .map(|p| PlanRow {
                    action: "remove",
                    color: ui::red,
                    name: p.name.clone(),
                    version: format!("{}-{}-{}", p.version, p.arch, p.build),
                    repo: {
                        let t = p.build_tag();
                        if t.is_empty() { "-".to_string() } else { t.to_string() }
                    },
                    note: String::new(),
                })
                .collect();
            print_table(&rows);
        }
        chosen
    };

    if !confirm("Remove the selected packages?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    for p in &to_remove {
        system::remove_package(&p.tag())?;
    }
    Ok(Outcome::Ok)
}

/// Delete downloaded package files (.txz) from CACHE_DIR/packages. Repo
/// metadata and GPG keys live under CACHE_DIR/repos and are never touched.
pub(crate) fn cmd_clean_cache(cli: &Cli, cfg: &Config, repos: &[String]) -> Result<Outcome, String> {
    let pkg_root = cfg.cache_dir.join("packages");
    if !pkg_root.is_dir() {
        println!("Cache is already empty (no {} directory).", pkg_root.display());
        return Ok(Outcome::NothingFound);
    }

    // Validate any named repos against the config so a typo can't silently
    // match nothing.
    if !repos.is_empty() {
        for name in repos {
            if cfg.repo_by_name(name).is_none() {
                return Err(format!(
                    "clean-cache: unknown repo '{name}'{}",
                    at_prefix_hint(cfg, name)
                ));
            }
        }
    }

    // Collect the per-repo package directories to clean.
    let mut targets: Vec<std::path::PathBuf> = Vec::new();
    if repos.is_empty() {
        for entry in std::fs::read_dir(&pkg_root).map_err(|e| format!("read {}: {e}", pkg_root.display()))? {
            let p = entry.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                targets.push(p);
            }
        }
    } else {
        for name in repos {
            let d = pkg_root.join(name);
            if d.is_dir() {
                targets.push(d);
            }
        }
    }

    // Tally files and total size.
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut total: u64 = 0;
    for dir in &targets {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
            let p = entry.map_err(|e| e.to_string())?.path();
            // Only delete regular files, never follow symlinks or recurse.
            let meta = std::fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
            if meta.file_type().is_file() {
                total += meta.len();
                files.push(p);
            }
        }
    }

    if files.is_empty() {
        println!("No cached packages to remove.");
        return Ok(Outcome::NothingFound);
    }

    let mib = total as f64 / (1024.0 * 1024.0);
    let scope = if repos.is_empty() {
        "all repos".to_string()
    } else {
        repos.join(", ")
    };
    println!(
        "This will delete {} cached package file(s) ({:.1} MiB) from {} under {}.",
        files.len(),
        mib,
        scope,
        pkg_root.display()
    );
    println!("(Repo metadata and GPG keys are not affected.)");

    if cli.dry_run {
        println!("(dry-run: nothing deleted)");
        return Ok(Outcome::Ok);
    }
    if !confirm("Proceed?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    let mut removed = 0;
    for f in &files {
        match std::fs::remove_file(f) {
            Ok(()) => removed += 1,
            Err(e) => eprintln!("could not remove {}: {e}", f.display()),
        }
    }
    println!("Removed {removed} file(s), freed {mib:.1} MiB.");
    Ok(Outcome::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_from_its_repo_predicate() {
        // pinned to a repo AND installed from that same repo -> treated as pinned
        assert!(is_pinned_from_its_repo(Some("alienbob"), Some("alienbob")));
        // pinned to one repo but installed from another -> pin ignored
        assert!(!is_pinned_from_its_repo(Some("alienbob"), Some("conraid")));
        // no pin at all -> not pinned
        assert!(!is_pinned_from_its_repo(None, Some("alienbob")));
        // pinned, but the installed build's tag is owned by no known repo -> not
        assert!(!is_pinned_from_its_repo(Some("alienbob"), None));
        // neither -> not
        assert!(!is_pinned_from_its_repo(None, None));
    }
}
