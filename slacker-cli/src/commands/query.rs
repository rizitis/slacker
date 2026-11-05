//! `search`, `file-search`, `info`: read-only package lookups.

use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::{closest, current_uid, installed_source_label, Outcome};
use crate::{manifest, pkg, repo, system, ui};
use std::collections::HashSet;

/// Pick the candidate a `search` hit should display. `search` returns the
/// priority winner per name, but when the package is INSTALLED we want the
/// candidate matching the installed source (same build tag), so the `[repo]`
/// label and version name where it actually came from — not merely the
/// highest-priority repo.
///
/// `None` means there is no candidate that describes the installed state, and
/// the caller distinguishes the two ways that happens:
///   * nothing installed — the line describes what an `install` would fetch, so
///     the caller shows the priority winner;
///   * installed from a source that is not a configured repo (an `_SBo`,
///     `_local` or otherwise foreign build tag) — the caller labels the line
///     from the installed build tag itself. Falling back to the winner here
///     would print another repo's name and version next to the word
///     "installed", attributing the package to a repo it never came from.
pub(crate) fn search_display<'a>(
    installed: Option<&pkg::PkgId>,
    candidates: &[&'a repo::AvailPkg],
) -> Option<&'a repo::AvailPkg> {
    let i = installed?;
    candidates
        .iter()
        .copied()
        .find(|c| c.id.build_tag() == i.build_tag())
}

pub(crate) fn cmd_search(cfg: &Config, term: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let results = db.search(term);
    if results.is_empty() {
        if db.is_empty() {
            println!("No package metadata yet — run `slacker update` first.");
        } else if let Some(i) = system::installed_by_name_ci(&installed, term) {
            // Nothing in any repo, but it IS on the system: a local or SBo build.
            // Say so — sending the user to `info` without a word about it reads
            // as "this does not exist", which is false.
            //
            // Matched case-insensitively, like `PkgDb::search` above: matching
            // exactly here would apply two different rules in one command, and
            // the builds this branch exists for (SBo/local) are the ones with
            // capitals in their names. The lines below name the package by its
            // real id, not by what was typed, so the suggested `info` command
            // works as printed.
            println!("No repo offers '{}', but it is installed: {}", i.name, i.tag());
            println!(
                "{}",
                ui::dim(&format!(
                    "it came from outside the configured repos, so nothing can upgrade it — \
                     `slacker info {}`.",
                    i.name
                ))
            );
        } else {
            let mut msg = format!("No package named '{term}'.");
            if let Some(s) = closest(term, db.available_names()) {
                msg.push_str(&format!(" Did you mean '{s}'?"));
            }
            println!("{msg}");
            println!(
                "{}",
                ui::dim(&format!(
                    "search matches exact names — try `slacker file-search {term}` for a file, \
                     or `slacker info {term}`."
                ))
            );
        }
        return Ok(Outcome::NothingFound);
    }
    for p in results {
        // `p` is the priority-winner candidate that `search` returns. When the
        // package is installed the line must describe the INSTALLED build: the
        // candidate matching it when some repo ships that build, otherwise the
        // installed id itself — an `_SBo`/local build that no configured repo
        // can account for. The blacklist test stays on the winner `p`, so the
        // frozen/blacklisted marking is unchanged.
        let inst = system::installed_by_name(&installed, &p.id.name);
        let cands = db.candidates(&p.id.name);
        let shown = search_display(inst, &cands);

        // Source label, name and version, taken from whichever of the three the
        // line is really about.
        let (label, name, version) = match (shown, inst) {
            (Some(c), _) => (c.repo.clone(), c.id.name.as_str(), c.id.version.as_str()),
            (None, Some(i)) => (
                installed_source_label(i).to_string(),
                i.name.as_str(),
                i.version.as_str(),
            ),
            (None, None) => (p.repo.clone(), p.id.name.as_str(), p.id.version.as_str()),
        };
        let summary = shown.map_or(p.summary.as_str(), |c| c.summary.as_str());

        let mark = if inst.is_some() {
            ui::green(&format!("{:<11}", "installed"))
        } else {
            ui::red(&format!("{:<11}", "uninstalled"))
        };
        let bl = if p.frozen {
            ui::purple(" [blacklisted]")
        } else {
            String::new()
        };
        // Surface the other repos that ship this name (search shows one winner
        // per name); `info <name>` gives the full per-repo candidate list.
        // Each is named as `<repo> <name>-<version>`: another repo may carry
        // something entirely different (e.g. testing shipping a whole other
        // kernel series), and a bare repo name would hide that until `info` is
        // run. The package name is repeated rather than joined to the repo with
        // a dash, so `testing kernel-generic-7.1.7` cannot be misread as a repo
        // called "testing-7.1.7". `candidates` is priority-ordered, so the first
        // entry seen for a repo is the one that repo would resolve to — later
        // builds of the same name in the same repo are not listed.
        let mut seen_repos: Vec<&str> = Vec::new();
        let mut others: Vec<String> = Vec::new();
        for c in &cands {
            let r = c.repo.as_str();
            if r != label && !seen_repos.contains(&r) {
                seen_repos.push(r);
                others.push(format!("{r} {}-{}", c.id.name, c.id.version));
            }
        }
        let also = if others.is_empty() {
            String::new()
        } else {
            ui::dim(&format!("  (also: {})", others.join(", ")))
        };
        println!(
            "{} {} {}{}  {}{}{}",
            ui::cyan(&format!("[{label}]")),
            mark,
            ui::white(name),
            ui::dim(&format!("-{version}")),
            summary,
            bl,
            also
        );
    }
    Ok(Outcome::Ok)
}

/// True when the package filename of a file-search hit names a build that is
/// actually installed — the EXACT name-version-arch-build, not merely some other
/// build of the same name. `installed_tags` is the set of installed full tags. A
/// non-package filename (e.g. a `*.tar.xz` source tarball listed in a MANIFEST)
/// doesn't parse to a PkgId and is reported not installed, which is correct.
fn hit_build_installed(package_filename: &str, installed_tags: &HashSet<String>) -> bool {
    pkg::PkgId::parse(package_filename)
        .map(|p| installed_tags.contains(&p.tag()))
        .unwrap_or(false)
}

pub(crate) fn cmd_file_search(cfg: &Config, filename: &str) -> Result<Outcome, String> {
    // MANIFEST is fetched lazily (it is large); make sure it's present. Track
    // repos whose MANIFEST we could neither find nor fetch, so we can explain
    // rather than silently return "not found".
    let mut unavailable: Vec<String> = Vec::new();
    for r in &cfg.repos {
        let mpath = repo::meta_path(r, &cfg.cache_dir, repo::MANIFEST);
        if mpath.exists() {
            continue;
        }
        match repo::ensure_manifest(r, &cfg.cache_dir) {
            Ok(true) => {}
            Ok(false) | Err(_) => unavailable.push(r.name.clone()),
        }
    }

    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    // Index installed packages by their full tag (name-version-arch-build) so a
    // hit is marked "installed" only when THAT EXACT build is on the system —
    // not merely when some other build of the same name is. file-search lists
    // every repo's copy of a file (e.g. slackware's xf86-input-libinput AND
    // testing's xlibre snapshot); keying on name alone made every candidate of
    // an installed name read "installed", which was misleading.
    let installed_tags: HashSet<String> = installed.iter().map(|i| i.tag()).collect();
    let hits = manifest::file_search(&cfg.repos, &cfg.cache_dir, filename)?;
    let found = !hits.is_empty();

    // file-search is a plain substring match over MANIFEST paths, not a glob.
    // If the query carries shell-wildcard characters the user almost certainly
    // meant them as globs, so flag that they are taken literally.
    let has_glob = filename.contains(['*', '?', '[', ']']);
    if has_glob {
        eprintln!(
            "{}",
            ui::yellow("note: file-search matches a literal substring, not a glob —")
        );
        eprintln!("      '*', '?', '[' and ']' are treated as ordinary characters, not wildcards.");
    }

    for h in hits {
        // A source tarball or other non-package filename won't parse to a PkgId
        // and so is never "installed" — correct, since it isn't a package.
        let mark = if hit_build_installed(&h.package, &installed_tags) {
            ui::green(&format!("{:<11}", "installed"))
        } else {
            ui::red(&format!("{:<11}", "uninstalled"))
        };
        println!(
            "{} {} {}: {}",
            ui::cyan(&format!("[{}]", h.repo)),
            mark,
            ui::white(&h.package),
            h.path
        );
    }

    // If some repos had no usable MANIFEST, say so — the first fetch writes into
    // the root-owned cache, so a non-root run can't do it and would otherwise
    // look like an empty result.
    if !unavailable.is_empty() {
        let list = unavailable.join(", ");
        eprintln!();
        if current_uid() == Some(0) {
            eprintln!("note: could not fetch the MANIFEST for: {list} (network or server error);");
            eprintln!("      results above may be incomplete — try again later.");
        } else {
            eprintln!("note: the MANIFEST for: {list} is not cached yet, and downloading it");
            eprintln!("      needs root (it is written into {}).", cfg.cache_dir.display());
            eprintln!("      run once as root: slacker file-search {filename}");
        }
    }

    if !found {
        if unavailable.is_empty() {
            println!("No package ships a file matching '{filename}'.");
            if has_glob {
                let stripped: String = filename
                    .chars()
                    .filter(|c| !matches!(c, '*' | '?' | '[' | ']'))
                    .collect();
                if !stripped.is_empty() && stripped != filename {
                    println!("Wildcards aren't supported here — try `slacker file-search {stripped}`.");
                }
            }
        }
        return Ok(Outcome::NothingFound);
    }
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_info(cfg: &Config, name: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let candidates = db.candidates(name);
    // Read the installed set BEFORE deciding there is nothing to report: a
    // locally built package (an `_SBo` or own build) exists on the system while
    // no configured repo offers it, and saying "no package named X" about
    // something sitting in the package database is simply wrong.
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let inst = system::installed_by_name(&installed, name);
    if candidates.is_empty() {
        if let Some(i) = inst {
            println!("{} {}", ui::blue("Installed:"), ui::green(&i.tag()));
            println!(
                "{}",
                ui::dim(&format!(
                    "No repo offers '{name}', so nothing can upgrade it — it came from \
                     outside the configured repos (a local or SBo build)."
                ))
            );
            // The pkgtools record already holds the description, sizes and file
            // list in readable form; point at it rather than reimplementing a
            // parser for a file the system publishes. `pkg_db_dir` is used (not
            // a hardcoded path) so the hint stays correct under a custom
            // PKG_DB_DIR.
            println!(
                "{}",
                ui::dim(&format!(
                    "package details: `cat {}`",
                    cfg.pkg_db_dir.join(i.tag()).display()
                ))
            );
            return Ok(Outcome::Ok);
        }
        if db.is_empty() {
            println!("No package metadata yet — run `slacker update` first.");
        } else {
            let mut msg = format!("No package named '{name}' in any repo.");
            if let Some(s) = closest(name, db.available_names()) {
                msg.push_str(&format!(" Did you mean '{s}'?"));
            }
            println!("{msg}");
        }
        return Ok(Outcome::NothingFound);
    }
    match inst {
        Some(i) => println!("{} {}", ui::blue("Installed:"), ui::green(&i.tag())),
        None => println!("{} {}", ui::blue("Installed:"), ui::dim("(none)")),
    }
    println!("{}", ui::blue("Available candidates (highest priority first):"));
    for p in candidates {
        let csize = p.size_k.map(|k| format!("{k} K")).unwrap_or_else(|| "?".into());
        let usize_ = p.size_uncompressed_k.map(|k| format!("{k} K")).unwrap_or_else(|| "?".into());
        let md5 = if p.md5.is_some() { "md5 ok" } else { "no md5" };
        let bl = if p.frozen {
            ui::purple("  [blacklisted]")
        } else {
            String::new()
        };
        println!(
            "  {} {}{}{}",
            ui::cyan(&format!("[{}]", p.repo)),
            ui::white(&p.id.name),
            ui::dim(&format!("-{}-{}-{}", p.id.version, p.id.arch, p.id.build)),
            bl
        );
        println!(
            "        {}",
            ui::dim(&format!(
                "series: {}   compressed: {csize}   uncompressed: {usize_}   {md5}",
                p.series
            ))
        );
        if !p.description.is_empty() {
            for line in p.description.lines() {
                println!("        {line}");
            }
        } else if !p.summary.is_empty() {
            println!("        {}", p.summary);
        }
    }
    Ok(Outcome::Ok)
}

#[cfg(test)]
mod tests {
    use super::hit_build_installed;
    use std::collections::HashSet;

    #[test]
    fn file_search_marks_only_the_exact_installed_build() {
        // Two repos ship the same file under the same package NAME but different
        // builds; only the build actually installed must read "installed".
        let installed_tags: HashSet<String> =
            ["xf86-input-libinput-1.5.0-x86_64-1".to_string()].into_iter().collect();
        // the installed slackware build
        assert!(hit_build_installed(
            "xf86-input-libinput-1.5.0-x86_64-1.txz",
            &installed_tags
        ));
        // a different (testing) build of the same name -> NOT installed
        assert!(!hit_build_installed(
            "xf86-input-libinput-20260202_4eb6691-x86_64-1.txz",
            &installed_tags
        ));
        // a source tarball (doesn't parse to a package) -> NOT installed
        assert!(!hit_build_installed(
            "xf86-input-libinput-20260202_4eb6691.tar.xz",
            &installed_tags
        ));
        // unrelated name -> NOT installed
        assert!(!hit_build_installed("bash-5.2.21-x86_64-1.txz", &installed_tags));
    }
}
