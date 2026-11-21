//! fix-upgrade: finish an `upgradepkg` that was cut off (power loss, crash,
//! a failed first install pass).
//!
//! `upgradepkg` renames the old record to `<pkg>-upgraded-<T>`, installs the new
//! package, removes the renamed record with `removepkg`, and installs the new
//! package a second time. A record still in the package DB means it stopped
//! somewhere in between. This runs the same steps with the pkgtools, in the
//! same order:
//!
//!   1. `upgradepkg --install-new --reinstall FILE`: the package goes back in
//!      place with a complete record (reinstalled if a half-written record of
//!      it is there, installed if none is).
//!   2. `removepkg RECORD`: removes what only the old version had.
//!   3. `upgradepkg --reinstall FILE`: the second pass of upgradepkg.
//!
//! Step 1 comes before step 2 because the record of an install cut off halfway
//! may have no file list yet, and `removepkg` keeps only the files that other
//! records list.
//!
//! slacker does not pick the package to use. It lists every candidate the
//! repos have (frozen ones left out, as the admin froze them), with its
//! priority, whether it is the very build that is installed, and whether it
//! would move the package to a lower-priority source; the user picks one, or
//! skips (the default). With --yes, where nobody is asked, only a candidate
//! that is exactly the installed build is taken. The file is then fetched and
//! checked like any install (cache, download, md5/gpg). For a package no repo
//! has, or one that was skipped, the steps are printed to run by hand.
//!
//! An install cut off before its file list was written leaves a record with
//! no file list (installpkg writes the header first and the list only after
//! tar has extracted everything). removepkg cannot remove what such a record
//! does not list, and `slacker status` cannot tell what the package owns. The
//! fix is one `upgradepkg --reinstall FILE`, with the package chosen as above:
//! the package is installed again and its record written whole; the removepkg
//! step inside upgradepkg removes nothing, as the record lists nothing. A
//! record of a package that also has a `-upgraded-` record is fixed as an
//! unfinished upgrade instead (its step 1 is the same reinstall).
//!
//! An install cut off after the unpack but before doinst.sh ran leaves a
//! complete record, no script in scripts/, and installpkg's own directory
//! `ROOT/installpkg-<cookie>` holding doinst.sh and slack-desc (installpkg
//! runs the script from there, copies it to scripts/, then removes the
//! directory). The slack-desc names the package; when no record of it has
//! that script in scripts/, it is fixed the same way, with one
//! `upgradepkg --reinstall FILE`, which runs the script.
//!
//! The directories themselves are not removed: no pkgtool removes them, so
//! slacker prints the two commands installpkg would have run
//! (`rm -f DIR/do*inst.sh DIR/slack-*`, `rmdir DIR`) for the admin to run.
//!
//! One case needs no package file: a reinstall (or upgrade) whose new record
//! is complete and lists every file and symlink the leftover record lists.
//! Then `removepkg` of the leftover would delete nothing, and only the record
//! has to go: `removepkg --skip-douninst RECORD`. The uninstall script is
//! skipped because the package of that name stays installed and no second
//! install pass follows to set up again what the script undoes.

use crate::cli::Cli;
use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::{
    confirm, enforce_arch_match, enforce_package_list, enforce_release_match, fetch_and_verify,
    installed_source_label, package_dest, InstallAction, Outcome, PlanItem,
};
use std::io::Write;
use crate::{pkg, revert, system, ui};

/// One package to fix: every leftover record of that name, the package now
/// installed under that name (if any), and what the repos offer for it.
struct Fix<'a> {
    name: String,
    records: Vec<String>,
    /// Every record's files and symlinks are listed by the installed one:
    /// only the records have to go.
    record_only: bool,
    now: Option<pkg::PkgId>,
    /// The old version, from the first record.
    old: pkg::PkgId,
    /// Candidates the user can pick from, highest priority first.
    cands: Vec<&'a crate::repo::AvailPkg>,
    /// Repos whose candidate is frozen (not offered).
    frozen: Vec<String>,
    /// The candidate chosen, if any.
    chosen: Option<&'a crate::repo::AvailPkg>,
    /// An install cut off before its file list, or before its install script
    /// ran (no `-upgraded-` record): fixed with one `upgradepkg --reinstall`.
    incomplete: bool,
    /// What is wrong with it, for an incomplete one.
    why: String,
    /// Why it cannot be fixed here even with a package file, if so.
    blocked: Option<String>,
}

impl Fix<'_> {
    /// The build a reinstall would restore: what is installed now, or else
    /// the old version.
    fn reference(&self) -> &pkg::PkgId {
        self.now.as_ref().unwrap_or(&self.old)
    }
}

/// The FILE LIST of a pkgtools record, or None if it has none (unreadable, or
/// written by an install that was cut off before the list).
fn file_list(record: &std::path::Path) -> Option<std::collections::HashSet<String>> {
    let text = std::fs::read_to_string(record).ok()?;
    let mut lines = text.lines();
    lines.by_ref().find(|l| *l == "FILE LIST:")?;
    Some(lines.filter(|l| !l.is_empty()).map(str::to_string).collect())
}

/// The symlinks an install script creates, read the way removepkg reads them
/// (`( cd DIR ; rm -rf NAME )` lines), as DIR/NAME. A missing script has none.
fn script_links(script: &std::path::Path) -> std::collections::HashSet<String> {
    let text = std::fs::read_to_string(script).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("( cd ")?.strip_suffix(" )")?;
            let (dir, name) = rest.split_once(" ; rm -rf ")?;
            Some(format!("{}/{}", dir.trim(), name.trim()))
        })
        .collect()
}

/// True when `removepkg` of `record` would delete nothing: the installed
/// record `live` has a non-empty file list holding every file of `record`,
/// and its install script every symlink of `record`'s.
fn record_only(db_dir: &std::path::Path, record: &str, live: &str) -> bool {
    let scripts = db_dir.parent().map(|p| p.join("scripts"));
    let (Some(old), Some(new)) = (file_list(&db_dir.join(record)), file_list(&db_dir.join(live))) else {
        return false;
    };
    if new.is_empty() || !old.is_subset(&new) {
        return false;
    }
    match scripts {
        Some(s) => script_links(&s.join(record)).is_subset(&script_links(&s.join(live))),
        None => false,
    }
}

fn manual_steps(records: &[String], file: &str) -> Vec<String> {
    let mut v = vec![format!("upgradepkg --install-new --reinstall {file}")];
    for r in records {
        v.push(format!("removepkg {r}"));
    }
    v.push(format!("upgradepkg --reinstall {file}"));
    v
}

/// The steps for one fix: the three of an unfinished upgrade, or the one
/// reinstall of a record with no file list.
fn steps_for(f: &Fix, file: &str) -> Vec<String> {
    if f.incomplete {
        vec![format!("upgradepkg --reinstall {file}")]
    } else {
        manual_steps(&f.records, file)
    }
}

/// One line per candidate for the user to choose from.
fn candidate_line(db: &PkgDb, cfg: &Config, f: &Fix, c: &crate::repo::AvailPkg) -> String {
    let reference = f.reference();
    let mut notes: Vec<String> = Vec::new();
    if c.id == *reference {
        notes.push("same build as installed".to_string());
    } else if f.incomplete {
        notes.push("another build: files only the cut-off install put on disk stay".to_string());
    }
    if f.now.is_some() && !db.upgrade_respects_priority(reference, c, &cfg.tag_priorities) {
        notes.push(format!(
            "lower priority than {}: the package changes source",
            installed_source_label(reference)
        ));
    }
    format!(
        "{:<10} {:<40} prio {:<4} {}",
        c.repo,
        c.id.tag(),
        db.repo_priority(&c.repo),
        notes.join("; ")
    )
}

/// Ask which candidate to use. Empty input, `s`, or anything not a listed
/// number skips the package.
fn ask_choice(n: usize) -> Option<usize> {
    print!("  {} ", ui::blue("Choose [s]:"));
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return None;
    }
    match line.trim().parse::<usize>() {
        Ok(k) if (1..=n).contains(&k) => Some(k - 1),
        _ => None,
    }
}

pub(crate) fn cmd_fix_upgrade(cli: &Cli, cfg: &Config) -> Result<Outcome, String> {
    let leftovers = system::leftover_upgrade_records(&cfg.pkg_db_dir);
    let incomplete = system::incomplete_records(&cfg.pkg_db_dir);
    let tmps = system::installpkg_tmp_dirs(&system::system_root(&cfg.adm_dir));
    if leftovers.is_empty() && incomplete.is_empty() && tmps.is_empty() {
        println!(
            "Nothing to fix: no `-upgraded-` record and no record without a file list in {}, no directory installpkg left in {}.",
            cfg.pkg_db_dir.display(),
            system::system_root(&cfg.adm_dir).display()
        );
        return Ok(Outcome::NothingFound);
    }
    // While a pkgtool runs, its `-upgraded-` record, or a record with no file
    // list yet, is expected; do not race it.
    let running = system::pkgtools_running();
    if !running.is_empty() {
        let list: Vec<String> = running.iter().map(|(p, t)| format!("{t} (PID {p})")).collect();
        return Err(format!(
            "running now: {}; its records are not leftovers yet. Wait for it to finish, then run `slacker fix-upgrade` again. Nothing was changed.",
            list.join(", ")
        ));
    }

    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;

    // Group the records by package name (two cut-off upgrades of one package
    // are fixed in one go).
    let mut fixes: Vec<Fix> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    for rec in &leftovers {
        let Some(old) = pkg::PkgId::parse(revert::strip_upgraded_suffix(rec)) else {
            unreadable.push(rec.clone());
            continue;
        };
        let now = system::installed_by_name(&installed, &old.name).cloned();
        let only = now.as_ref().is_some_and(|n| record_only(&cfg.pkg_db_dir, rec, &n.tag()));
        if let Some(f) = fixes.iter_mut().find(|f| f.name == old.name) {
            f.records.push(rec.clone());
            f.record_only &= only;
            continue;
        }
        let all = db.candidates(&old.name);
        let frozen = all.iter().filter(|c| c.frozen).map(|c| c.repo.clone()).collect();
        let cands = all.into_iter().filter(|c| !c.frozen).collect();
        fixes.push(Fix {
            name: old.name.clone(),
            records: vec![rec.clone()],
            record_only: only,
            now,
            old,
            cands,
            frozen,
            chosen: None,
            incomplete: false,
            why: String::new(),
            blocked: None,
        });
    }
    let not_run = system::scripts_not_run(&cfg.pkg_db_dir, &tmps);
    let mut cut: Vec<(String, String)> = incomplete.iter().map(|r| (r.clone(), "no file list".to_string())).collect();
    for (r, d) in &not_run {
        cut.push((r.clone(), format!("install script did not run ({} is still there)", d.display())));
    }
    for (rec, why) in &cut {
        let Some(id) = pkg::PkgId::parse(rec) else {
            unreadable.push(rec.clone());
            continue;
        };
        // Two records of one name: which one upgradepkg would replace is not
        // for slacker to guess.
        let same_name = installed.iter().filter(|p| p.name == id.name).count();
        let blocked = (same_name > 1).then(|| format!("{same_name} records of {} are installed", id.name));
        let all = db.candidates(&id.name);
        let frozen = all.iter().filter(|c| c.frozen).map(|c| c.repo.clone()).collect();
        let cands = all.into_iter().filter(|c| !c.frozen).collect();
        fixes.push(Fix {
            name: id.name.clone(),
            records: vec![rec.clone()],
            record_only: false,
            now: Some(id.clone()),
            old: id,
            cands,
            frozen,
            chosen: None,
            incomplete: true,
            why: why.clone(),
            blocked,
        });
    }

    // ---------- what is found ----------
    if fixes.iter().any(|f| !f.incomplete) {
        println!("{}", ui::blue("Unfinished upgrades (records upgradepkg left in the package DB):"));
    }
    for f in fixes.iter().filter(|f| !f.incomplete) {
        for r in &f.records {
            println!("  {}", ui::white(r));
        }
        match &f.now {
            Some(n) => println!(
                "    {}",
                ui::dim(&format!("installed now: {} (source {})", n.tag(), installed_source_label(n)))
            ),
            None => println!("    {}", ui::dim(&format!("{} is not installed now", f.name))),
        }
    }
    if fixes.iter().any(|f| f.incomplete) {
        println!("{}", ui::blue("Installs cut off:"));
    }
    for f in fixes.iter().filter(|f| f.incomplete) {
        println!("  {}  {}", ui::white(&f.records[0]), ui::dim(&f.why));
        if let Some(b) = &f.blocked {
            println!("    {}", ui::yellow(b));
        }
    }
    for r in &unreadable {
        println!("  {}", ui::yellow(&format!("{r}: not a package name slacker can read; check it by hand.")));
    }

    // Only the record goes: nothing to choose.
    if fixes.iter().any(|f| f.record_only) {
        println!(
            "\n{}",
            ui::blue("Only the leftover record has to go (the installed record lists all its files; nothing is deleted from disk):")
        );
        for f in fixes.iter().filter(|f| f.record_only) {
            for r in &f.records {
                println!("  removepkg --skip-douninst {r}");
            }
        }
    }

    // ---------- the user picks the package ----------
    let ask = !cli.yes && !cli.dry_run;
    for f in fixes.iter_mut().filter(|f| !f.record_only && f.blocked.is_none() && !f.cands.is_empty()) {
        println!("\n{}", ui::white(&format!("{}: which package to reinstall it from?", f.name)));
        for (i, c) in f.cands.iter().enumerate() {
            println!("  {}) {}", i + 1, candidate_line(&db, cfg, f, c));
        }
        if !f.frozen.is_empty() {
            println!("  {}", ui::dim(&format!("frozen, not offered: {}", f.frozen.join(", "))));
        }
        println!("  s) skip, I will do it myself");
        f.chosen = if ask {
            ask_choice(f.cands.len()).map(|k| f.cands[k])
        } else if cli.yes {
            // Nobody to ask: only the very build that is installed.
            let exact = f.cands.iter().copied().find(|c| c.id == *f.reference());
            match exact {
                Some(c) => println!("  {}", ui::dim(&format!("--yes: {} from {} (same build)", c.id.tag(), c.repo))),
                None => println!("  {}", ui::dim("--yes: no candidate is the installed build; skipped")),
            }
            exact
        } else {
            None
        };
    }

    // Everything not fixed here: the commands to run by hand.
    let print_manual = |fixes: &[Fix]| {
        for f in fixes.iter().filter(|f| !f.record_only && f.chosen.is_none()) {
            let file = format!("{}.t?z", f.reference().tag());
            let why = if let Some(b) = &f.blocked {
                b.as_str()
            } else if f.cands.is_empty() && f.frozen.is_empty() {
                "no repository has it"
            } else if f.cands.is_empty() {
                "only frozen candidates"
            } else if cli.dry_run {
                "dry-run: nothing was asked"
            } else {
                "skipped"
            };
            println!(
                "\n{}",
                ui::yellow(&format!(
                    "{}: not fixed here ({why}). Build or find the package file {file}, then run as root:",
                    f.name
                ))
            );
            for step in steps_for(f, &file) {
                println!("    {step}");
            }
        }
    };

    // The directories installpkg left: printed only, never removed here.
    let print_dirs = || {
        if tmps.is_empty() {
            return;
        }
        println!(
            "\n{}",
            ui::yellow("Directories installpkg left (not removed by slacker). After the fixes above, run as root:")
        );
        for t in &tmps {
            let what = match &t.package {
                Some(p) => format!("package {p}"),
                None => "no slack-desc: package unknown".to_string(),
            };
            println!("  {}", ui::dim(&format!("{} ({what})", t.dir.display())));
            let d = t.dir.display();
            println!("    rm -f {d}/do*inst.sh {d}/slack-*");
            println!("    rmdir {d}");
            if !t.other.is_empty() {
                println!(
                    "    {}",
                    ui::yellow(&format!("it also holds {}: rmdir fails until you check those", t.other.join(", ")))
                );
            }
        }
    };

    let todo: Vec<&Fix> = fixes.iter().filter(|f| !f.record_only && f.chosen.is_some()).collect();
    let only: Vec<&Fix> = fixes.iter().filter(|f| f.record_only).collect();
    if todo.is_empty() && only.is_empty() {
        print_manual(&fixes);
        print_dirs();
        return Ok(Outcome::Ok);
    }

    if !todo.is_empty() {
        println!("\n{}", ui::blue("slacker will run:"));
        for f in &todo {
            let c = f.chosen.expect("filtered");
            println!("  {} {}", ui::white(&f.name), ui::dim(&format!("with {} from {}", c.id.tag(), c.repo)));
            for (i, step) in steps_for(f, &c.filename).iter().enumerate() {
                println!("    {}. {step}", i + 1);
            }
        }
        // The same guards as any install: release, architecture, package list.
        let plan: Vec<PlanItem> = todo
            .iter()
            .map(|f| PlanItem {
                pkg: f.chosen.expect("filtered").clone(),
                action: InstallAction::Reinstall,
                dep_for: None,
                from: f.now.as_ref().map(|n| format!("{}-{}-{}", n.version, n.arch, n.build)),
            })
            .collect();
        enforce_release_match(cfg, &plan, cli.yes)?;
        enforce_arch_match(cfg, &plan, cli.yes)?;
        enforce_package_list(cfg, &plan)?;
    }

    if cli.dry_run {
        print_manual(&fixes);
        print_dirs();
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    println!();
    if !confirm("Proceed?", cli.yes) {
        println!("Nothing changed.");
        return Ok(Outcome::Ok);
    }

    // ---------- run ----------
    let mut failed: Vec<(String, String)> = Vec::new();
    for f in &only {
        for r in &f.records {
            if !cfg.pkg_db_dir.join(r).exists() {
                continue;
            }
            println!("{}", ui::blue(&format!("{}: removepkg --skip-douninst", f.name)));
            let res = system::remove_record_only(r).and_then(|()| {
                if cfg.pkg_db_dir.join(r).exists() {
                    Err(format!("removepkg left {r} in place"))
                } else {
                    Ok(())
                }
            });
            if let Err(e) = res {
                println!("{}", ui::red(&format!("{}: {e}", f.name)));
                failed.push((f.name.clone(), e));
            }
        }
    }
    for f in &todo {
        let c = f.chosen.expect("filtered");
        let res = (|| -> Result<(), String> {
            let repo = cfg.repo_by_name(&c.repo).ok_or("internal repo lookup failed")?;
            let dest = package_dest(cfg, &c.repo, &c.filename)?;
            fetch_and_verify(cfg, repo, c, &dest, false)?;
            if f.incomplete {
                println!("{}", ui::blue(&format!("{}: upgradepkg --reinstall", f.name)));
                system::reinstall(&dest, cfg.terse)?;
                let rec = cfg.pkg_db_dir.join(c.id.tag());
                if !rec.exists() || system::record_lacks_file_list(&rec) {
                    return Err(format!("{} has no complete record after upgradepkg", c.id.tag()));
                }
                return Ok(());
            }
            println!("{}", ui::blue(&format!("[1/3] {}: upgradepkg --install-new --reinstall", f.name)));
            system::reinstall_or_install(&dest, cfg.terse)?;
            // Step 1 must have left a record of the package before anything is
            // removed, or removepkg would take files the package needs.
            if !system::record_present(&cfg.pkg_db_dir, &c.id.tag()) {
                return Err(format!("no record {} after step 1; stopped before removepkg", c.id.tag()));
            }
            println!("{}", ui::blue(&format!("[2/3] {}: removepkg", f.name)));
            for r in &f.records {
                // removepkg exits 0 even when it finds nothing to remove, so
                // check the record before and after.
                if !cfg.pkg_db_dir.join(r).exists() {
                    continue;
                }
                system::remove_package(r)?;
                if cfg.pkg_db_dir.join(r).exists() {
                    return Err(format!("removepkg left {r} in place"));
                }
            }
            println!("{}", ui::blue(&format!("[3/3] {}: upgradepkg --reinstall", f.name)));
            system::reinstall(&dest, cfg.terse)?;
            Ok(())
        })();
        if let Err(e) = res {
            println!("{}", ui::red(&format!("{}: {e}", f.name)));
            failed.push((f.name.clone(), e));
        }
    }
    print_manual(&fixes);
    print_dirs();

    let mut left = system::leftover_upgrade_records(&cfg.pkg_db_dir);
    left.extend(system::incomplete_records(&cfg.pkg_db_dir));
    left.extend(system::scripts_not_run(&cfg.pkg_db_dir, &tmps).into_iter().map(|(r, _)| r));
    if failed.is_empty() && left.is_empty() && !tmps.is_empty() {
        println!("\n{}", ui::green("Done: the packages are fixed; the directories above are yours to remove."));
        Ok(Outcome::Ok)
    } else if failed.is_empty() && left.is_empty() {
        println!("\n{}", ui::green("Done: no unfinished upgrade and no record without a file list is left."));
        Ok(Outcome::Ok)
    } else if failed.is_empty() {
        println!("\n{}", ui::yellow(&format!("{} record(s) are still there (see above).", left.len())));
        Ok(Outcome::Ok)
    } else {
        Err(format!(
            "not fixed: {}. `slacker status` shows what is left.",
            failed.iter().map(|f| f.0.as_str()).collect::<Vec<_>>().join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{manual_steps, record_only};

    #[test]
    fn record_only_when_the_installed_record_lists_everything() {
        let root = std::env::temp_dir().join("slacker_fix_upgrade_record_only");
        let _ = std::fs::remove_dir_all(&root);
        let (db, sc) = (root.join("packages"), root.join("scripts"));
        std::fs::create_dir_all(&db).unwrap();
        std::fs::create_dir_all(&sc).unwrap();
        let rec = "foo-1-x86_64-2_SBo-upgraded-2026-07-16,23:44:04";
        let live = "foo-1-x86_64-2_SBo";
        let list = "PACKAGE NAME: x\nFILE LIST:\n./\nusr/\nusr/lib64/libfoo.so.1.0\n";
        std::fs::write(db.join(rec), list).unwrap();
        std::fs::write(db.join(live), list).unwrap();
        std::fs::write(sc.join(rec), "( cd usr/lib64 ; rm -rf libfoo.so.1 )\n").unwrap();
        std::fs::write(sc.join(live), "( cd usr/lib64 ; rm -rf libfoo.so.1 )\n").unwrap();
        assert!(record_only(&db, rec, live));
        // a symlink only the old record makes would be deleted
        std::fs::write(sc.join(live), "").unwrap();
        assert!(!record_only(&db, rec, live));
        std::fs::write(sc.join(live), "( cd usr/lib64 ; rm -rf libfoo.so.1 )\n").unwrap();
        // a file only the old record lists would be deleted
        std::fs::write(db.join(rec), format!("{list}usr/lib64/libold.so\n")).unwrap();
        assert!(!record_only(&db, rec, live));
        // an installed record cut off before its file list
        std::fs::write(db.join(rec), list).unwrap();
        std::fs::write(db.join(live), "PACKAGE NAME: x\n").unwrap();
        assert!(!record_only(&db, rec, live));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn steps_are_upgradepkgs_order() {
        let recs = vec!["foo-1-x86_64-1-upgraded-2026-11-21,17:31:16".to_string()];
        assert_eq!(
            manual_steps(&recs, "foo-2-x86_64-1.txz"),
            vec![
                "upgradepkg --install-new --reinstall foo-2-x86_64-1.txz",
                "removepkg foo-1-x86_64-1-upgraded-2026-11-21,17:31:16",
                "upgradepkg --reinstall foo-2-x86_64-1.txz",
            ]
        );
    }
}
