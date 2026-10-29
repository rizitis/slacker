//! install, upgrade, reinstall, remove, download, upgrade-all, install-new.

use crate::cli::Cli;
use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::{
    at_prefix_hint, bl_installed, closest, collect, confirm, confirm_conflicts, detect_conflicts,
    execute_plan, expand_with_deps, fetch_and_verify, guard_shell_expansion, hilite_keys,
    hint_freeze_pin, in_graphical_session, installed_source_label, is_foundational,
    note_optional_suggests, parallel_fetch, parse_selection, print_plan, print_table,
    priority_display, reject_mixed_selectors, report_batch_failures, report_conflicts,
    report_pinned_in_plan, show_plan, summarize_outcomes, validate_selector, DlItem, InstallAction,
    Outcome, PlanItem, PlanRow,
};
use crate::{banner, config, newconfig, pkg, repo, system, ui};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

/// Print a resolved plan as a coloured table. `frozen` are blacklisted names
/// left untouched (purple); `protected` are names kept because an installed
/// source of higher-or-equal priority already owns them (blue). Version is
/// never compared — only source priority decides.
/// Heads-up at match time: how many packages matched the pattern(s) but were
/// left out because they are frozen (blacklisted), and which. Keeps the picker's
/// "matched N" count from being confusing. Prints nothing when none were frozen.
fn note_frozen_excluded(frozen: &[String]) {
    if frozen.is_empty() {
        return;
    }
    println!(
        "{}",
        ui::purple(&format!(
            "  {} also matched but {} frozen (skipped): {}",
            frozen.len(),
            if frozen.len() == 1 { "is" } else { "are" },
            frozen.join(", ")
        ))
    );
    println!("{}", ui::dim("    run `unfrozen <name>` to include"));
}

/// install-new counts the frozen packages instead of naming them.
///
/// Every other install path names them, and should: there the user typed a
/// name, so "you asked for this one and it is frozen" is the answer to the
/// question they asked. install-new asks nothing. It offers whatever the repos
/// hold that is not installed, and on a system with a long blacklist the frozen
/// names outnumber the packages actually on offer and used to be printed twice
/// in the same run, burying the only part anybody reads. One dim line keeps the
/// trail for somebody wondering why a package is never offered.
fn note_frozen_count(frozen: &[String]) {
    if frozen.is_empty() {
        return;
    }
    println!(
        "  {}",
        ui::dim(&format!(
            "{} frozen {} skipped (run `frozen` to list them)",
            frozen.len(),
            if frozen.len() == 1 { "package" } else { "packages" }
        ))
    );
}

/// Heads-up at match time: installed packages that matched the pattern but were
/// left out because a pin points them at a repo that does not (yet) provide
/// them — so they don't just vanish from the list without explanation.
fn note_pin_excluded(pins: &[(String, String)]) {
    for (name, repo) in pins {
        println!(
            "{}",
            ui::purple(&format!(
                "  {name} matched but is pinned to '{repo}', which does not provide it (skipped)"
            ))
        );
    }
    if !pins.is_empty() {
        println!(
            "{}",
            ui::dim("    re-pin elsewhere, or `unpin <name>` to use its normal source")
        );
    }
}

/// The major-version token of a Slackware version: its first dot/underscore
/// component when it starts with a digit ("74.2" -> "74", "3.1.4" -> "3",
/// "2.40" -> "2"). None if the version is non-numeric (e.g. a date/hash).
fn major_token(version: &str) -> Option<&str> {
    let first = version.split(['.', '_']).next()?;
    first
        .chars()
        .next()
        .filter(char::is_ascii_digit)
        .map(|_| first)
}

/// After an upgrade plan is shown, advise doing the upgrade from a console when
/// it bumps the MAJOR version of a foundational library. Purely advisory: the
/// caller's confirmation prompt proceeds unchanged. No-op otherwise.
fn advise_console_for_foundational(plan: &[PlanItem]) {
    // (name, from-version, to-version) for foundational major-version bumps.
    let mut hits: Vec<(&str, &str, &str)> = Vec::new();
    for it in plan {
        if !matches!(it.action, InstallAction::Upgrade) {
            continue;
        }
        if !is_foundational(&it.pkg.id.name) {
            continue;
        }
        let Some(from_vab) = &it.from else { continue };
        let old_v = from_vab.split('-').next().unwrap_or("");
        let new_v = it.pkg.id.version.as_str();
        if let (Some(a), Some(b)) = (major_token(old_v), major_token(new_v)) {
            if a != b {
                hits.push((it.pkg.id.name.as_str(), old_v, new_v));
            }
        }
    }
    if hits.is_empty() {
        return;
    }

    let wn = hits.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0).max(7);
    let wf = hits.iter().map(|(_, f, _)| f.len()).max().unwrap_or(0).max(4);
    let wt = hits.iter().map(|(_, _, t)| t.len()).max().unwrap_or(0).max(2);
    let bar = "━".repeat(70);

    println!();
    println!("{}", ui::orange(&format!("  {bar}")));
    println!(
        "{}",
        ui::orange("  ►  FOUNDATIONAL UPGRADE — a core library changes major version")
    );
    println!("{}", ui::orange(&format!("  {bar}")));
    println!();
    println!(
        "     {}   {}   {}",
        ui::white(&format!("{:<wn$}", "Package")),
        ui::white(&format!("{:<wf$}", "From")),
        ui::white("To")
    );
    println!(
        "     {}   {}   {}",
        "─".repeat(wn),
        "─".repeat(wf),
        "─".repeat(wt)
    );
    for (n, f, t) in &hits {
        println!(
            "     {}   {}   {}",
            ui::orange(&format!("{n:<wn$}")),
            ui::white(&format!("{f:<wf$}")),
            ui::white(t)
        );
    }
    println!();
    println!(
        "  {}",
        ui::white("These apply more cleanly with no graphical session running")
    );
    println!(
        "  {}",
        ui::white("(ldconfig/relinking and stateful services).")
    );
    if in_graphical_session() {
        println!(
            "  {}",
            ui::orange("You are in a graphical session — logging out and upgrading from a")
        );
        println!("  {}", ui::orange("console (tty) is recommended."));
    } else {
        println!("  {}", ui::dim("You are on a console — good to go."));
    }
    println!("{}", ui::orange(&format!("  {bar}")));
    println!();
}

/// Read-only: for packages in the plan whose *name* is also offered by other
/// repos, list those alternatives. The plan already holds the priority/pin
/// winner that `collect` chose; this only surfaces what was NOT applied so the
/// user can answer "n" at the prompt and pin / switch a repo if they would
/// rather have a different build. It never reads or changes the plan's choice.
/// Returns, per package: (name, chosen-source, [other-sources]).
pub(crate) fn plan_alternatives(db: &PkgDb, plan: &[PlanItem]) -> Vec<(String, String, Vec<String>)> {
    let src = |a: &repo::AvailPkg| {
        format!(
            "{} {}-{}-{} ({})",
            a.repo,
            a.id.version,
            a.id.arch,
            a.id.build,
            db.repo_priority(&a.repo),
        )
    };
    let mut out = Vec::new();
    for item in plan {
        let cands = db.candidates(&item.pkg.id.name);
        if cands.len() < 2 {
            continue; // only one repo offers this name — nothing to surface
        }
        let others: Vec<String> =
            cands.iter().filter(|c| c.repo != item.pkg.repo).map(|c| src(c)).collect();
        if !others.is_empty() {
            out.push((item.pkg.id.name.clone(), src(&item.pkg), others));
        }
    }
    out
}

/// Direct-`.dep` differences of an alternative build relative to the chosen one:
/// `(added, removed)` — deps the alternative declares that the chosen does not,
/// and deps the chosen declares that the alternative does not. Pure (no I/O), so
/// it is unit-tested; both inputs are expected sorted and de-duplicated.
fn dep_delta(chosen: &[String], alt: &[String]) -> (Vec<String>, Vec<String>) {
    let added = alt.iter().filter(|d| !chosen.contains(d)).cloned().collect();
    let removed = chosen.iter().filter(|d| !alt.contains(d)).cloned().collect();
    (added, removed)
}

/// Fetch and normalise a candidate's declared dependencies from its own repo's
/// `.dep` file (one small request; a 404 simply yields an empty list, i.e. "no
/// declared deps"). Sorted and de-duplicated so `dep_delta` output is stable.
fn fetch_dep_names(cfg: &Config, avail: &repo::AvailPkg) -> Vec<String> {
    let Some(r) = cfg.repos.iter().find(|r| r.name == avail.repo) else {
        return Vec::new();
    };
    let mut v = repo::fetch_dep(r, avail);
    v.sort();
    v.dedup();
    v
}

/// Styled informational note, shown after the plan and before the prompt, so a
/// cross-repo collision is visible before the user commits. For every planned
/// package (target or dependency) whose *name* is also offered by other repos,
/// it lists each alternative build with its version (flagged same/different) and
/// \(em when dependency resolution is on \(em an on-the-fly diff of that repo's
/// declared `.dep` against the chosen one. Purely informational: priority (or a
/// pin) still decides; answer "n" to switch. Empty -> prints nothing.
fn show_plan_alternatives(cfg: &Config, db: &PkgDb, plan: &[PlanItem], resolve: bool) {
    if plan_alternatives(db, plan).is_empty() {
        return;
    }
    let verstr = |a: &repo::AvailPkg| format!("{}-{}-{}", a.id.version, a.id.arch, a.id.build);
    println!(
        "\n{}",
        ui::blue("Also offered by other repos (kept the priority winner — answer 'n' to pin a different build):")
    );
    for item in plan {
        let cands = db.candidates(&item.pkg.id.name);
        let others: Vec<&repo::AvailPkg> =
            cands.iter().copied().filter(|c| c.repo != item.pkg.repo).collect();
        if cands.len() < 2 || others.is_empty() {
            continue;
        }
        let wr = cands.iter().map(|&c| c.repo.len()).max().unwrap_or(0);
        let wv = cands.iter().map(|&c| verstr(c).len()).max().unwrap_or(0);
        let row = |a: &repo::AvailPkg, prio: i32| {
            format!(
                "    {}  {}  {}",
                ui::white(&format!("{:<wr$}", a.repo)),
                ui::dim(&format!("{:<wv$}", verstr(a))),
                ui::dim(&format!("(prio {prio})")),
            )
        };
        println!("  {}", ui::cyan(&item.pkg.id.name));
        println!(
            "{}  {}",
            row(&item.pkg, db.repo_priority(&item.pkg.repo)),
            ui::green("\u{2190} installing")
        );
        let chosen_deps = if resolve { fetch_dep_names(cfg, &item.pkg) } else { Vec::new() };
        for &c in &others {
            let same = c.id.version == item.pkg.id.version;
            let vtag = if same {
                ui::dim("same version")
            } else {
                ui::yellow(&format!(
                    "different version ({} vs {})",
                    c.id.version, item.pkg.id.version
                ))
            };
            println!("{}  {}", row(c, db.repo_priority(&c.repo)), vtag);
            if resolve {
                let alt_deps = fetch_dep_names(cfg, c);
                let (added, removed) = dep_delta(&chosen_deps, &alt_deps);
                if added.is_empty() && removed.is_empty() {
                    println!("        {}", ui::dim("deps: identical"));
                } else {
                    let mut parts: Vec<String> = Vec::new();
                    for a in &added {
                        parts.push(ui::green(&format!("+{a}")));
                    }
                    for r in &removed {
                        parts.push(ui::red(&format!("-{r}")));
                    }
                    println!("        {} {}", ui::dim("deps:"), parts.join("  "));
                }
            }
        }
    }
}

/// If `m` is a `repo:name` pin whose repo is unknown, return the corrected
/// `closest_repo:name`; used so a mistyped pin repo (`conrad:vlc`) is caught as
/// `conraid:vlc`. None when there is no pin, the repo is valid, or nothing close.
fn fix_pin_repo(db: &PkgDb, m: &str) -> Option<String> {
    let (r, name) = m.split_once(':')?;
    let repos = db.all_repos();
    if repos.iter().any(|x| x == r) {
        return None; // repo part is fine — the name is the problem
    }
    let s = closest(r, repos.iter().map(|x| x.as_str()))?;
    Some(format!("{s}:{name}"))
}

/// Refuse to act when any argument matched nothing.
///
/// The details were already printed by the caller's miss report (with typo and
/// pin suggestions), so this only states that nothing was done and names the
/// arguments at fault.
///
/// Acting on the rest is the worst outcome available: the user asked for N
/// things, the tool understood N-1, and a partial result looks exactly like a
/// complete one. `download fontforge ponce` (a forgotten `:`) fetched fontforge
/// from *conraid* while the user was reading "no match for 'ponce'", and a
/// mistyped `remove` argument would take out the packages that did match — with
/// nothing to undo it. Re-running a corrected command costs seconds; a partial
/// install, upgrade or removal costs much more.
///
/// This generalises the judgement already made in `guard_shell_expansion`,
/// which refuses a whole command for the same reason in one narrow case.
/// `-y` does NOT bypass it: "don't ask me" is not "act on what you did not
/// understand".
///
/// It belongs at the command level, on user-supplied patterns only. Inside
/// `collect` it would break the internal `@repo`-only callers (`install-new`,
/// `dist_install_new`), which legitimately ignore misses: an empty repo — a
/// `patches` tree on -current, say — yields a miss and must stay harmless.
pub(crate) fn abort_on_misses(misses: &[String]) -> Result<(), String> {
    if misses.is_empty() {
        return Ok(());
    }
    let what = if misses.len() == 1 { "argument" } else { "arguments" };
    Err(format!(
        "nothing was done — {} {what} matched no package: {}\n  \
         fix or drop {} and run the command again",
        misses.len(),
        misses.join(", "),
        if misses.len() == 1 { "it" } else { "them" },
    ))
}

/// When a bare argument matched no package but names a REPO or a build TAG, the
/// user almost certainly meant a selector and lost its punctuation — a dropped
/// `:` in `download fontforge ponce`, or a missing `@` in `upgrade conraid`.
/// Suggesting the closest package name there is actively misleading (`ponce` ->
/// `poke`), so name what the word really is and give both selector forms.
///
/// Returns `None` for anything else, leaving the ordinary typo help in place.
/// A `repo:name` argument is not considered: it already carries its repo, and
/// `fix_pin_repo` handles a mistyped one.
pub(crate) fn selector_miss_hint(db: &PkgDb, miss: &str) -> Option<String> {
    if miss.starts_with('@') || miss.contains(':') {
        return None;
    }
    if db.is_repo(miss) {
        return Some(format!(
            "it is a repo, not a package; for one package from it use \
             '{miss}:<package>', for all of it use '@{miss}'"
        ));
    }
    if db.tag_in_use(miss) {
        return Some(format!(
            "it is a build tag, not a package; for the packages carrying it use \
             '@{miss}'"
        ));
    }
    None
}

/// Report package-name misses with guidance instead of a bare "no match":
///   * if the database is empty the real problem is missing metadata, so point
///     at `slacker update` (printed once);
///   * a `repo:name` pin with a mistyped repo suggests the closest repo;
///   * otherwise suggest the closest available name (the pin is kept on the
///     suggestion, so `alienbob:cvlc` -> `alienbob:vlc`).
/// Printed to stderr, matching the previous "no match for ..." behaviour.
fn report_pkg_misses(db: &PkgDb, misses: &[String]) {
    if misses.is_empty() {
        return;
    }
    if db.is_empty() {
        eprintln!("no repository metadata yet — run `slacker update` first");
        return;
    }
    for m in misses {
        if let Some(hint) = selector_miss_hint(db, m) {
            eprintln!("no match for '{m}' — {hint}");
            continue;
        }
        if let Some(fixed) = fix_pin_repo(db, m) {
            eprintln!("no match for '{m}' — did you mean '{fixed}'?");
            continue;
        }
        let (pin, name) = match m.split_once(':') {
            Some((r, n)) => (Some(r), n),
            None => (None, m.as_str()),
        };
        let mut msg = format!("no match for '{m}'");
        if let Some(s) = closest(name, db.available_names()) {
            let suggestion = pin.map_or_else(|| s.clone(), |r| format!("{r}:{s}"));
            msg.push_str(&format!(" — did you mean '{suggestion}'?"));
        }
        eprintln!("{msg}");
    }
}

/// Report names that name no *installed* package (for upgrade/reinstall/remove):
///   * a `repo:name` pin with a mistyped repo suggests the closest repo;
///   * if the name exists in a repo, it simply was not installed — point at
///     `slacker install`;
///   * otherwise suggest the closest installed name (typo help).
fn report_installed_misses(db: &PkgDb, installed: &[pkg::PkgId], misses: &[String]) {
    for m in misses {
        if let Some(hint) = selector_miss_hint(db, m) {
            eprintln!("'{m}' is not installed — {hint}");
            continue;
        }
        if let Some(fixed) = fix_pin_repo(db, m) {
            eprintln!("'{m}' is not installed — did you mean '{fixed}'?");
            continue;
        }
        let name = m.rsplit(':').next().unwrap_or(m); // strip any repo: pin
        if !db.candidates(name).is_empty() {
            eprintln!("'{m}' is not installed — install it first with `slacker install {name}`");
        } else {
            let mut msg = format!("'{m}' is not installed");
            if let Some(s) = closest(name, installed.iter().map(|p| p.name.as_str())) {
                msg.push_str(&format!(" — did you mean '{s}'?"));
            }
            eprintln!("{msg}");
        }
    }
}

/// Resolve upgrade/reinstall PATTERNs into available candidates, restricted to
/// *installed* packages and honouring build-tag source priority.
///
/// Two guarantees that `collect` (used for fresh installs) does not provide:
///
/// 1. For `@repo`/`@tag` the source that matters is where a package was built
///    (its build tag), not which repos merely ship that name. `@conraid` selects
///    the installed packages carrying conraid's tag (`cf`) and upgrades each
///    from conraid — it does NOT pull in an SBo-built `webkit2gtk4.1` just
///    because conraid also publishes that name. (Mirrors how `remove` treats @.)
///
/// 2. No selection may migrate an installed package to a *lower*-priority
///    source (e.g. an `_SBo` package, priority 100, down to conraid's 80). The
///    explicit `repo:name` pin is the deliberate override and bypasses this;
///    everything else — bare name, substring, series, `@repo`, `@tag` — is
///    held to the same source-priority rule `upgrade-all` uses.
fn collect_installed_targets<'a>(
    db: &'a PkgDb,
    installed: &[pkg::PkgId],
    tag_prios: &[crate::config::TagPriority],
    patterns: &[String],
) -> Result<(Vec<&'a repo::AvailPkg>, Vec<String>, Vec<String>), String> {
    for pat in patterns {
        validate_selector(db, pat)?;
    }
    reject_mixed_selectors(db, patterns)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut protected = Vec::new(); // names kept because their source has priority
    let mut misses = Vec::new(); // plain names that matched no installed package
    for pat in patterns {
        if let Some(rest) = pat.strip_prefix('@') {
            // @repo -> installed packages whose build tag belongs to that repo
            // @_tag -> installed packages carrying that build tag
            let is_repo = db.is_repo(rest);
            let tags: HashSet<String> = if is_repo {
                db.repo_build_tags(rest)
            } else {
                std::iter::once(rest.to_string()).collect()
            };
            for inst in installed {
                if !tags.contains(inst.build_tag()) || !seen.insert(inst.name.clone()) {
                    continue;
                }
                // @repo: candidate from that same repo (no migration). @tag: the
                // winning candidate for the name.
                let cand = if is_repo {
                    db.resolve(&format!("{rest}:{}", inst.name))
                } else {
                    db.resolve(&inst.name)
                };
                if let Some(c) = cand {
                    if db.upgrade_respects_priority(inst, c, tag_prios) {
                        out.push(c);
                    } else {
                        protected.push(kept_detail(db, inst, c, tag_prios));
                    }
                }
            }
        } else {
            // Plain name / substring / series, or an explicit `repo:name` pin.
            // Only the pin bypasses the source-priority guard.
            let pinned = pat.split_once(':').is_some();
            let mut hit = false;
            for p in db.match_pattern(pat) {
                let Some(inst) = installed.iter().find(|i| i.name == p.id.name) else {
                    continue;
                };
                hit = true;
                if !seen.insert(p.id.name.clone()) {
                    continue;
                }
                if !pinned && !db.upgrade_respects_priority(inst, p, tag_prios) {
                    protected.push(kept_detail(db, inst, p, tag_prios));
                    continue;
                }
                out.push(p);
            }
            // A plain name that matched no installed package is a miss — the
            // caller turns it into a "not installed / did you mean" hint.
            if !hit {
                misses.push(pat.clone());
            }
        }
    }
    guard_shell_expansion(&misses)?;
    Ok((out, protected, misses))
}

/// One "kept (priority)" detail line, explaining why an installed package was
/// not replaced — e.g. `webkit2gtk4.1  installed _SBo (100) — conraid (80) not
/// applied`. Source priority alone decides; versions are never compared.
///
/// A package whose build tag has no priority at all is a different case: it is
/// not outranking anything, it simply cannot be ranked, so it is kept as a
/// fail-safe. That is a configuration gap the user can close, so the line grows
/// a hint naming the two ways out — give the tag a priority, or remove the
/// package and install it from a repo.
fn kept_detail(
    db: &PkgDb,
    inst: &pkg::PkgId,
    cand: &repo::AvailPkg,
    tag_prios: &[crate::config::TagPriority],
) -> String {
    let prio = db.installed_priority(inst, tag_prios);
    let mut line = format!(
        "{}  installed {} ({}) — {} ({}) not applied",
        inst.name,
        installed_source_label(inst),
        priority_display(prio),
        cand.repo,
        db.repo_priority(&cand.repo),
    );
    let tag = inst.build_tag();
    if prio == i32::MAX && !tag.is_empty() {
        line.push_str(&format!(
            "\n      you installed {} from {}, but that tag has no priority in `repos`, \
             so it cannot be ranked.\n      It is kept for safety. Either give the tag a \
             priority (e.g. `100  {}  {}`) or `removepkg {}` and install it from a repo.",
            inst.name,
            tag,
            tag.trim_start_matches('_'),
            tag,
            inst.name,
        ));
    }
    line
}

pub(crate) fn cmd_install(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (matched, misses) = collect(&db, patterns)?;
    report_pkg_misses(&db, &misses);
    // A miss means the argument named no package in any repo; do not install a
    // subset of what was asked for.
    abort_on_misses(&misses)?;
    // install = only packages that are not already installed and not blacklisted
    let mut frozen = Vec::new();
    let mut already = Vec::new();
    let todo: Vec<_> = matched
        .into_iter()
        .filter(|p| {
            if p.frozen {
                frozen.push(p.id.name.clone());
                return false;
            }
            if system::is_installed(&installed, &p.id.name) {
                already.push(p.id.name.clone());
                return false;
            }
            true
        })
        .collect();

    if todo.is_empty() {
        // Still show why nothing will happen (frozen / already installed).
        show_plan(&[], &frozen, &already);
        // If the only reason is that they are already installed, nudge toward the
        // command that actually does something with an installed package.
        if frozen.is_empty() && !already.is_empty() {
            let one = &already[0];
            println!(
                "{}",
                ui::dim(&format!(
                    "already installed — use `slacker upgrade {one}` for a newer build, \
                     or `slacker reinstall {one}`."
                ))
            );
        }
        println!("Nothing to install.");
        return Ok(Outcome::NothingFound);
    }
    note_frozen_excluded(&frozen);
    // Did the picker actually prompt? (same condition select_packages uses to
    // skip asking). Only then print the "-> selected:" closer below, so the
    // numbered prompt is never read as referring to the dependency plan.
    let picker_asked = todo.len() > 1 && !cli.yes && !cli.dry_run;
    let todo = select_packages(todo, "install", &installed, cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    if picker_asked {
        note_selected(&todo);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Install)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes, cli.dry_run)?;
    // `already` is shown via the same blue "kept" line as priority skips — both
    // mean "installed, leaving it alone".
    show_plan(&plan, &frozen, &already);
    report_pinned_in_plan(cfg, &plan);
    hint_freeze_pin();
    show_plan_alternatives(cfg, &db, &plan, resolve);
    note_optional_suggests(&plan, resolve);
    let conflicts = detect_conflicts(&plan, &installed, resolve);
    report_conflicts(&conflicts);
    advise_console_for_foundational(&plan);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm_conflicts("Proceed with installation?", &conflicts, cli.yes)? {
        return Ok(Outcome::Ok);
    }
    let before_cfgs: HashSet<PathBuf> = newconfig::find_new_configs(&newconfig::default_roots())
        .into_iter()
        .map(|nc| nc.new_file)
        .collect();
    execute_plan(cfg, &plan, cli.yes)?;
    report_pending_configs(&before_cfgs);
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_upgrade(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (cands, protected, misses) =
        collect_installed_targets(&db, &installed, &cfg.tag_priorities, patterns)?;
    report_installed_misses(&db, &installed, &misses);
    // A miss means the argument named nothing installed; do not upgrade a
    // subset of what was asked for.
    abort_on_misses(&misses)?;
    let mut frozen = Vec::new();
    let todo: Vec<_> = cands
        .into_iter()
        .filter(|p| {
            if p.frozen {
                frozen.push(p.id.name.clone());
                return false;
            }
            // `upgrade` acts only on packages that actually have a newer
            // revision available — or the same version from a HIGHER-priority
            // source, a genuine source migration, kept consistent with
            // `upgrade-all`. A package already at the offered version+build from
            // an equal-priority source has nothing to do, so it is dropped here.
            // Without this, `upgrade @repo` lists every installed package in the
            // repo (and would re-run upgradepkg over each), not just those with a
            // pending update. `reinstall` shares collect_installed_targets and
            // deliberately KEEPS the same-version candidate — which is why this
            // filter lives here, in the upgrade command, not in that function.
            if let Some(inst) = system::installed_by_name(&installed, &p.id.name) {
                let higher = db.repo_priority(&p.repo)
                    > db.installed_priority(inst, &cfg.tag_priorities);
                if !higher && !inst.is_other_revision_of(&p.id) {
                    return false;
                }
            }
            true
        })
        .collect();
    if todo.is_empty() {
        show_plan(&[], &frozen, &protected);
        println!("Nothing to upgrade.");
        return Ok(Outcome::NothingFound);
    }
    let mut pin_excluded: Vec<(String, String)> = patterns
        .iter()
        .flat_map(|pat| db.pin_excluded(pat, &installed))
        .collect();
    pin_excluded.sort();
    pin_excluded.dedup();
    note_pin_excluded(&pin_excluded);
    note_frozen_excluded(&frozen);
    let todo = select_packages(todo, "upgrade", &installed, cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Upgrade)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes, cli.dry_run)?;
    show_plan(&plan, &frozen, &protected);
    report_pinned_in_plan(cfg, &plan);
    hint_freeze_pin();
    note_optional_suggests(&plan, resolve);
    let conflicts = detect_conflicts(&plan, &installed, resolve);
    report_conflicts(&conflicts);
    advise_console_for_foundational(&plan);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm_conflicts("Proceed with upgrade?", &conflicts, cli.yes)? {
        return Ok(Outcome::Ok);
    }
    let before_cfgs: HashSet<PathBuf> = newconfig::find_new_configs(&newconfig::default_roots())
        .into_iter()
        .map(|nc| nc.new_file)
        .collect();
    execute_plan(cfg, &plan, cli.yes)?;
    report_pending_configs(&before_cfgs);
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_reinstall(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (cands, protected, misses) =
        collect_installed_targets(&db, &installed, &cfg.tag_priorities, patterns)?;
    report_installed_misses(&db, &installed, &misses);
    // A miss means the argument named nothing installed; do not reinstall a
    // subset of what was asked for.
    abort_on_misses(&misses)?;
    let mut frozen = Vec::new();
    let todo: Vec<_> = cands
        .into_iter()
        .filter(|p| {
            if p.frozen {
                frozen.push(p.id.name.clone());
                return false;
            }
            true
        })
        .collect();
    if todo.is_empty() {
        show_plan(&[], &frozen, &protected);
        println!("Nothing to reinstall.");
        return Ok(Outcome::NothingFound);
    }
    let mut pin_excluded: Vec<(String, String)> = patterns
        .iter()
        .flat_map(|pat| db.pin_excluded(pat, &installed))
        .collect();
    pin_excluded.sort();
    pin_excluded.dedup();
    note_pin_excluded(&pin_excluded);
    note_frozen_excluded(&frozen);
    let todo = select_packages(todo, "reinstall", &installed, cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Reinstall)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes, cli.dry_run)?;
    show_plan(&plan, &frozen, &protected);
    report_pinned_in_plan(cfg, &plan);
    hint_freeze_pin();
    note_optional_suggests(&plan, resolve);
    let conflicts = detect_conflicts(&plan, &installed, resolve);
    report_conflicts(&conflicts);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm_conflicts("Proceed with reinstall?", &conflicts, cli.yes)? {
        return Ok(Outcome::Ok);
    }
    execute_plan(cfg, &plan, cli.yes)?;
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_remove(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    // Load the package DB only if an @repo/@tag selector is used (needs it to
    // map a repo to the build tags its packages carry).
    let db = if patterns.iter().any(|p| p.starts_with('@')) {
        Some(PkgDb::load(cfg)?)
    } else {
        None
    };

    let mut todo: Vec<&pkg::PkgId> = Vec::new();
    let mut frozen: Vec<String> = Vec::new();
    let mut misses: Vec<String> = Vec::new();
    let mut miss_pats: Vec<String> = Vec::new(); // raw patterns, for the shell-expansion gate
    let mut seen = HashSet::new();

    for pat in patterns {
        if let Some(rest) = pat.strip_prefix('@') {
            // @repo  -> installed packages whose build tag belongs to that repo
            // @_tag  -> installed packages carrying that build tag
            let db = db.as_ref().expect("db loaded for @ selector");
            validate_selector(db, pat)?;
            let tags: HashSet<String> = if db.is_repo(rest) {
                db.repo_build_tags(rest)
            } else {
                std::iter::once(rest.to_string()).collect()
            };
            for inst in &installed {
                if tags.contains(inst.build_tag()) && seen.insert(inst.name.clone()) {
                    if bl_installed(cfg, Some(db), inst) {
                        frozen.push(inst.name.clone());
                        continue;
                    }
                    todo.push(inst);
                }
            }
            continue;
        }
        // plain name / substring match against installed names
        let term = pat.split_once(':').map(|x| x.1).unwrap_or(pat);
        let mut hit = false;
        for inst in &installed {
            if inst.name == term || inst.name.contains(term) {
                hit = true;
                if !seen.insert(inst.name.clone()) {
                    continue;
                }
                if bl_installed(cfg, db.as_ref(), inst) {
                    frozen.push(inst.name.clone());
                    continue;
                }
                todo.push(inst);
            }
        }
        // Named something that is not installed: say so (with a typo hint over
        // the installed set) rather than letting it vanish into "Nothing to remove".
        if !hit {
            miss_pats.push(pat.clone());
            let mut msg = format!("'{pat}' is not installed");
            if let Some(s) = closest(term, installed.iter().map(|p| p.name.as_str())) {
                msg.push_str(&format!(" — did you mean '{s}'?"));
            }
            misses.push(msg);
        }
    }
    guard_shell_expansion(&miss_pats)?;
    for m in &misses {
        eprintln!("{m}");
    }
    // A miss means the argument named nothing installed. Removal is the one
    // action with nothing to undo it, so a mistyped argument must not be
    // allowed to take out the packages that did match. `miss_pats` holds the
    // raw arguments; `misses` holds their already-printed explanations.
    abort_on_misses(&miss_pats)?;
    if todo.is_empty() {
        if !frozen.is_empty() {
            println!("{}", ui::purple("  frozen (blacklisted — left unchanged):"));
            for n in &frozen {
                println!("    {}", ui::white(n));
            }
        }
        println!("Nothing to remove.");
        return Ok(Outcome::NothingFound);
    }
    note_frozen_excluded(&frozen);
    let todo = select_packages_pkgid(todo, "remove", cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let rows: Vec<PlanRow> = todo
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
    if !frozen.is_empty() {
        println!("{}", ui::purple("  frozen (blacklisted — left unchanged):"));
        for n in &frozen {
            println!("    {}", ui::white(n));
        }
    }
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("Proceed with removal?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    for p in &todo {
        system::remove_package(&p.tag())?;
    }
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_download(
    cli: &Cli,
    cfg: &Config,
    patterns: &[String],
    output: Option<&str>,
) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let (matched, misses) = collect(&db, patterns)?;
    report_pkg_misses(&db, &misses);
    // A miss means the argument named nothing downloadable; do not fetch a
    // different package than the one that was asked for.
    abort_on_misses(&misses)?;
    if matched.is_empty() {
        println!("Nothing to download.");
        return Ok(Outcome::NothingFound);
    }

    // Where to save: a user-given directory, or the package cache by default.
    let out_dir = output.map(std::path::PathBuf::from);
    if let Some(dir) = &out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let dest_label = match &out_dir {
        Some(d) => d.display().to_string(),
        None => cfg.cache_dir.join("packages").display().to_string(),
    };

    // dry-run: list everything that matches, change nothing.
    if cli.dry_run {
        for p in &matched {
            println!(
                "  {} {} {}",
                ui::green("would download"),
                ui::cyan(&format!("[{}]", p.repo)),
                ui::white(&p.filename)
            );
        }
        println!("(dry-run: nothing downloaded)");
        return Ok(Outcome::Ok);
    }

    // Present the matched packages and let the user choose which to fetch — the
    // same numbered-selection UI as install/upgrade/reinstall. A single match,
    // or `-y`, takes everything without a prompt.
    let matched = select_packages(matched, "download", &[], cli.yes, cli.dry_run);
    if matched.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    println!(
        "{}",
        ui::blue(&format!(
            "Downloading {} package(s) into {dest_label}.",
            matched.len()
        ))
    );

    // Build the work list (repo + safe destination per package). An unsafe
    // filename is a path-traversal red flag — a hard error, not a skip.
    let mut items: Vec<DlItem> = Vec::with_capacity(matched.len());
    for p in &matched {
        let r = cfg.repo_by_name(&p.repo).ok_or("internal repo lookup failed")?;
        if !pkg::is_safe_filename(&p.filename) {
            return Err(format!(
                "repo '{}' supplied an unsafe package filename {:?} — refusing \
                 (possible path-traversal attack)",
                p.repo, p.filename
            ));
        }
        let dest = match &out_dir {
            Some(d) => d.join(&p.filename),
            None => system::cached_pkg_path(&cfg.cache_dir, &p.repo, &p.filename),
        };
        items.push(DlItem { repo: r, pkg: p, dest });
    }

    // Single package: keep the original verbose, serial output.
    if items.len() == 1 {
        let it = &items[0];
        fetch_and_verify(cfg, it.repo, it.pkg, &it.dest, false)?;
        println!(
            "{} {}",
            ui::green("downloaded:"),
            ui::dim(&it.dest.display().to_string())
        );
        return Ok(Outcome::Ok);
    }

    // Several packages: download + verify in parallel, best-effort. A package
    // that fails is skipped and reported; the rest still land on disk.
    let outcomes = parallel_fetch(cfg, &items, cfg.max_parallel);
    let (ready, dl_failed) = summarize_outcomes(&outcomes, items.len());
    let ok = ready.iter().filter(|&&b| b).count();
    report_batch_failures(items.len(), ok, &dl_failed, &[]);
    if dl_failed.is_empty() {
        println!(
            "{}",
            ui::green(&format!("Downloaded {ok} package(s) into {dest_label}."))
        );
    }
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_upgrade_all(cli: &Cli, cfg: &Config) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    // upgrades_for already treats a frozen candidate as absent and falls through
    // to the next non-frozen candidate by priority (never below the installed
    // source's priority). `held` are packages whose only newer candidate is
    // frozen — reported as frozen/skipped so the user sees the freeze took
    // effect, rather than a silent drop.
    let (ups, held) = db.upgrades_for(&installed, &cfg.tag_priorities);
    note_frozen_excluded(&held);
    if ups.is_empty() {
        println!("Everything is up to date.");
        return Ok(Outcome::Ok);
    }
    // Let the user deselect upgrades before anything is resolved or applied.
    // This is the same select-before-resolve step install/upgrade/reinstall use:
    // dependencies are then computed only for what is kept, so the dep resolver
    // is never re-run on a trimmed plan. Skipped under --yes/--dry-run or a
    // single upgrade (select_packages returns the input unchanged there).
    let chosen = select_packages(
        ups.iter().map(|u| u.available).collect(),
        "upgrade",
        &installed,
        cli.yes,
        cli.dry_run,
    );
    if chosen.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let self_upgrade = chosen.iter().any(|a| a.id.name == "slacker");
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots: Vec<_> =
        chosen.iter().map(|&a| (a.clone(), InstallAction::Upgrade)).collect();

    // Resolve dependencies up front, so the complete plan — including any new
    // packages pulled in as dependencies — is shown *before* we ask to proceed.
    // In a dry-run we keep installed versions for conflicts (no prompts).
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes, cli.dry_run)?;

    print_plan(&plan);
    report_pinned_in_plan(cfg, &plan);
    hint_freeze_pin();

    note_optional_suggests(&plan, resolve);
    let conflicts = detect_conflicts(&plan, &installed, resolve);
    report_conflicts(&conflicts);
    advise_console_for_foundational(&plan);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm_conflicts("Proceed with upgrade-all?", &conflicts, cli.yes)? {
        return Ok(Outcome::Ok);
    }
    let before_cfgs: HashSet<PathBuf> = newconfig::find_new_configs(&newconfig::default_roots())
        .into_iter()
        .map(|nc| nc.new_file)
        .collect();
    execute_plan(cfg, &plan, cli.yes)?;
    report_pending_configs(&before_cfgs);
    // A nod to the official tree when this upgrade actually touched it (a package
    // from the main slackware repo or one of its subtrees: patches/extra/...).
    if plan
        .iter()
        .any(|it| cfg.repo_by_name(&it.pkg.repo).is_some_and(|r| r.official || r.subtree))
    {
        banner::show();
    }
    if self_upgrade {
        println!("slacker upgraded itself; please re-run.");
        return Ok(Outcome::SelfUpgrade);
    }
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_install_new(cli: &Cli, cfg: &Config, repos: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;

    // Which repos to scan for newly-added packages:
    //   - no argument  -> the official repo(s) PLUS any Slackware distribution
    //     subtree (extra/patches/testing/pasture) the user has ranked ABOVE the
    //     base official repo. A subtree is part of the official distribution;
    //     ranking it over the base is an explicit "I want its packages" (e.g.
    //     patches at 200, or conraid's testing bumped to 105), so install-new
    //     treats it as official too. A subtree left at or below the base stays
    //     opt-in. `collect` then dedups per name to the priority winner (and
    //     falls back off a frozen candidate), so the higher subtree wins only
    //     when it actually offers the package and is not frozen.
    //   - repo name(s) -> exactly those, so the user can opt in to any repo
    //     explicitly (e.g. `slacker install-new alienbob`, or a low-ranked
    //     subtree the default scan skips).
    let selected: Vec<&config::Repo> = if repos.is_empty() {
        let off_prio = cfg.official_repo_priority();
        cfg.repos
            .iter()
            .filter(|r| {
                r.official || (r.subtree && off_prio.is_some_and(|op| r.priority > op))
            })
            .collect()
    } else {
        let mut out = Vec::new();
        for name in repos {
            match cfg.repos.iter().find(|r| &r.name == name) {
                Some(r) => out.push(r),
                None => {
                    return Err(format!(
                        "install-new: unknown repo '{name}'{}",
                        at_prefix_hint(cfg, name)
                    ))
                }
            }
        }
        out
    };
    if selected.is_empty() {
        return Err("install-new: no official repo configured; name a repo explicitly".into());
    }

    // install-new offers every package the selected repos provide that is NOT
    // already installed and NOT frozen — the same "fill what's missing" logic as
    // `install @<repo>` (collect -> match_pattern, then drop installed/frozen).
    // Compared against the live installed set, it catches both genuinely-new
    // distribution packages AND anything the user removed, robustly across any
    // number of updates.
    //
    // NOTE: this replaced the earlier "names added since the last update"
    // behaviour, which diffed PACKAGES.TXT.prev. That baseline is overwritten on
    // every `update` (so additions were lost after a second update) and never
    // caught packages the user had removed. The old prev-diff machinery
    // (repo::previous_names + PkgDb::newly_added, and the PACKAGES.TXT.prev that
    // `update_repo` still keeps) is intentionally LEFT IN PLACE, unused, in case
    // it is needed again later or elsewhere — see the notes at those functions.
    //
    // Scope is official-only by default (immutable/third-party repos are not
    // pulled in unless named); the blacklist filter keeps frozen packages out.
    let at: Vec<String> = selected.iter().map(|r| format!("@{}", r.name)).collect();
    let (matched, _misses) = collect(&db, &at)?;
    let mut frozen = Vec::new();
    let todo: Vec<_> = matched
        .into_iter()
        .filter(|p| {
            if p.frozen {
                frozen.push(p.id.name.clone());
                return false;
            }
            !system::is_installed(&installed, &p.id.name)
        })
        .collect();
    if todo.is_empty() {
        println!("No new packages to install.");
        note_frozen_count(&frozen);
        return Ok(Outcome::NothingFound);
    }
    // Let the user deselect before resolving deps (same select-before-resolve
    // step the other install paths use; skipped under --yes/--dry-run/single).
    note_frozen_count(&frozen);
    let todo = select_packages(todo, "install", &installed, cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Install)).collect();
    // Resolve dependencies up front so any extra packages pulled in are shown
    // before we ask to proceed (dry-run keeps installed versions, no prompts).
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes, cli.dry_run)?;
    // No frozen list here: it was already counted above, once per run.
    show_plan(&plan, &[], &[]);
    report_pinned_in_plan(cfg, &plan);
    hint_freeze_pin();
    show_plan_alternatives(cfg, &db, &plan, resolve);
    note_optional_suggests(&plan, resolve);
    let conflicts = detect_conflicts(&plan, &installed, resolve);
    report_conflicts(&conflicts);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm_conflicts("Install new packages?", &conflicts, cli.yes)? {
        return Ok(Outcome::Ok);
    }
    execute_plan(cfg, &plan, cli.yes)?;
    Ok(Outcome::Ok)
}

/// Parse a keep-selection like "1 3 5", "1,3,5" or "2-4" into a set of 1-based
/// indices, ignoring anything out of range or unparseable.
/// Like `select_packages` but for installed `PkgId`s (used by remove).
fn select_packages_pkgid<'a>(
    pkgs: Vec<&'a pkg::PkgId>,
    verb: &str,
    assume_yes: bool,
    dry_run: bool,
) -> Vec<&'a pkg::PkgId> {
    if pkgs.len() <= 1 || assume_yes || dry_run {
        return pkgs;
    }
    println!("{}", ui::blue(&format!("'{verb}' matched {} packages:", pkgs.len())));
    for (i, p) in pkgs.iter().enumerate() {
        println!(
            "  {}) {}{}",
            ui::dim(&format!("{:>3}", i + 1)),
            ui::white(&p.name),
            ui::dim(&format!("-{}-{}-{}", p.version, p.arch, p.build))
        );
    }
    print!(
        "{} ",
        hilite_keys(&format!(
            "Enter numbers to {verb} (e.g. 1 3 5 or 2-4), [Enter] for all, [n] to cancel:"
        ))
    );
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return Vec::new();
    }
    let t = line.trim();
    if t.eq_ignore_ascii_case("n") {
        return Vec::new();
    }
    if t.is_empty() {
        return pkgs;
    }
    let sel = parse_selection(t, pkgs.len());
    if sel.is_empty() {
        println!("    {}", ui::blue(&format!("didn't understand {t:?} — nothing selected.")));
        return Vec::new();
    }
    pkgs.into_iter()
        .enumerate()
        .filter(|(i, _)| sel.contains(&(i + 1)))
        .map(|(_, p)| p)
        .collect()
}

/// When a pattern matched more than one package, show a numbered list and let
/// the user pick which to act on: Enter = all, numbers/ranges (`1 3 5` or
/// `2-4`) = those only, `n` = cancel. A single match is returned unchanged.
/// With --yes (or in a dry-run preview) all matches are kept without asking.
fn select_packages<'a>(
    pkgs: Vec<&'a repo::AvailPkg>,
    verb: &str,
    installed: &[pkg::PkgId],
    assume_yes: bool,
    dry_run: bool,
) -> Vec<&'a repo::AvailPkg> {
    if pkgs.len() <= 1 || assume_yes || dry_run {
        return pkgs;
    }
    println!("{}", ui::blue(&format!("'{verb}' matched {} packages:", pkgs.len())));
    for (i, p) in pkgs.iter().enumerate() {
        // The target build: name in white, the rest dim (as before).
        let target = format!(
            "{}{}",
            ui::white(&p.id.name),
            ui::dim(&format!("-{}-{}-{}", p.id.version, p.id.arch, p.id.build))
        );
        // When this name is already installed as a DIFFERENT build, show the
        // full-tag transition `old -> new`, so the user sees exactly which build
        // is being replaced — the case that matters when a name is offered by
        // several repos and an accidental cross-repo change is the worry. A
        // fresh install (nothing installed by this name) or a same-build
        // reinstall shows the target alone. `download` passes an empty installed
        // set here, since a download replaces nothing.
        let shown = match system::installed_by_name(installed, &p.id.name) {
            Some(inst) if inst.tag() != p.id.tag() => {
                format!("{} {}", ui::dim(&format!("{} ->", inst.tag())), target)
            }
            _ => target,
        };
        println!(
            "  {}) {} {}",
            ui::dim(&format!("{:>3}", i + 1)),
            ui::cyan(&format!("[{}]", p.repo)),
            shown
        );
    }
    print!(
        "{} ",
        hilite_keys(&format!(
            "Enter numbers to {verb} (e.g. 1 3 5 or 2-4), [Enter] for all, [n] to cancel:"
        ))
    );
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return Vec::new();
    }
    let t = line.trim();
    if t.eq_ignore_ascii_case("n") {
        return Vec::new();
    }
    if t.is_empty() {
        return pkgs;
    }
    let sel = parse_selection(t, pkgs.len());
    if sel.is_empty() {
        println!("    {}", ui::blue(&format!("didn't understand {t:?} — nothing selected.")));
        return Vec::new();
    }
    pkgs.into_iter()
        .enumerate()
        .filter(|(i, _)| sel.contains(&(i + 1)))
        .map(|(_, p)| p)
        .collect()
}

/// Close the numbered picker visually (plain `install` only): one dim line
/// confirming what was selected. Without it, the "Enter numbers ..." prompt
/// sits directly above the (unnumbered) dependency plan and reads as if the
/// numbers applied to the deps — with resolve-stock, official installs now
/// print large dep tables, so that misreading would hit everyone. The other
/// picker users (upgrade / reinstall / download / install-new) keep their
/// output exactly as before.
fn note_selected(chosen: &[&repo::AvailPkg]) {
    if chosen.is_empty() {
        return;
    }
    let names: Vec<String> =
        chosen.iter().map(|p| format!("{} ({})", p.id.name, p.repo)).collect();
    println!("  {}", ui::dim(&format!("-> selected: {}", names.join(", "))));
}

/// After packages are applied, report pending *.new config files, separating
/// ones created by this run from leftovers already on disk, and point at
/// `slacker new-config`. Silent when there are none. `before` is the set of
/// .new paths that existed prior to the operation.
fn report_pending_configs(before: &HashSet<PathBuf>) {
    let current = newconfig::find_new_configs(&newconfig::default_roots());
    if current.is_empty() {
        return;
    }
    let (old, fresh): (Vec<_>, Vec<_>) =
        current.iter().partition(|nc| before.contains(&nc.new_file));
    if !fresh.is_empty() {
        println!(
            "\n{}",
            ui::blue("New configuration files were installed (your current ones were kept):")
        );
        for nc in &fresh {
            println!("  {}", ui::white(&nc.new_file.display().to_string()));
        }
    }
    if !old.is_empty() {
        println!("\n{}", ui::blue("Configuration files still pending from before:"));
        for nc in &old {
            println!("  {}", ui::white(&nc.new_file.display().to_string()));
        }
    }
    println!(
        "\n{}",
        ui::blue("Run `slacker new-config` to keep, overwrite, or merge them.")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_pin_repo_catches_mistyped_repo() {
        let db = crate::pkgdb::PkgDb::for_test(
            Vec::new(),
            &[("slackware", 100), ("alienbob", 80), ("conraid", 70)],
            Some(100),
        );
        // mistyped repo in a pin -> corrected, name preserved
        assert_eq!(super::fix_pin_repo(&db, "conrad:vlc"), Some("conraid:vlc".into()));
        assert_eq!(super::fix_pin_repo(&db, "aleinbob:vlc"), Some("alienbob:vlc".into()));
        // valid repo -> the repo is fine (name handled elsewhere)
        assert_eq!(super::fix_pin_repo(&db, "alienbob:cvlc"), None);
        // no pin at all -> nothing to fix
        assert_eq!(super::fix_pin_repo(&db, "vlc"), None);
        // unrecognisable repo -> no suggestion
        assert_eq!(super::fix_pin_repo(&db, "zzzzzz:vlc"), None);
    }

    #[test]
    fn dep_delta_reports_added_and_removed() {
        // alt swaps qt5 for qt6, keeps ffmpeg -> +qt6 / -qt5
        let chosen = vec!["ffmpeg".to_string(), "qt5".to_string()];
        let alt = vec!["ffmpeg".to_string(), "qt6".to_string()];
        let (added, removed) = dep_delta(&chosen, &alt);
        assert_eq!(added, vec!["qt6".to_string()]);
        assert_eq!(removed, vec!["qt5".to_string()]);
        // identical lists -> no delta
        let (a2, r2) = dep_delta(&chosen, &chosen);
        assert!(a2.is_empty() && r2.is_empty());
        // alt declares no deps -> everything the chosen pulls is "removed"
        let (a3, r3) = dep_delta(&chosen, &[]);
        assert!(a3.is_empty());
        assert_eq!(r3, vec!["ffmpeg".to_string(), "qt5".to_string()]);
    }

    #[test]
    fn major_token_reads_first_numeric_component() {
        assert_eq!(major_token("74.2"), Some("74"));
        assert_eq!(major_token("3.1.4"), Some("3"));
        assert_eq!(major_token("2.40"), Some("2"));
        assert_eq!(major_token("76"), Some("76"));
        // Non-numeric (date/hash-style) versions have no major token.
        assert_eq!(major_token("20240101"), Some("20240101"));
        assert_eq!(major_token("git"), None);
    }

    #[test]
    fn major_bump_detected_across_versions() {
        // A real icu4c-style bump.
        assert_ne!(major_token("74.2"), major_token("76.1"));
        // A patch bump is NOT a major bump.
        assert_eq!(major_token("3.1.4"), major_token("3.2.0"));
    }
}
