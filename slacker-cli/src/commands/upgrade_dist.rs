//! upgrade-dist: move a stable system to -current or to a newer stable release.

use crate::cli::Cli;
use crate::commands::clean::cmd_clean_system;
use crate::commands::new_config::cmd_new_config;
use crate::commands::status::cmd_status;
use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::{
    collect, in_graphical_session, package_dest, parallel_fetch, report_batch_failures,
    slackware_dir, summarize_outcomes, update_one_repo, DlItem, InstallAction, Outcome, PlanItem,
    UpdateOutcomes,
};
use crate::{banner, changelog, dist, download, pkg, system, template, ui};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

/// `upgrade-dist` — GATE ONLY for now.
/// Core packages that must be upgraded FIRST, serially, in this order: the C
/// runtime, then the very tools `upgradepkg` relies on to unpack and install the
/// rest. Doing the whole tree before these are in step is how a dist-upgrade
/// bricks a system.
/// Add mkinitrd in core packages as Patrick suggest in ChangeLog:
/// Sat Aug 29 04:22:54 UTC 2026
/// Install mkinitrd early because the kernel might need the latest version.
const DIST_CRITICAL: &[&str] = &[
    "aaa_glibc-solibs",
    "pkgtools",
    "tar",
    "xz",
    "gzip",
    "findutils",
    "mkinitrd",
];

/// The GnuPG verification toolchain: gnupg(2) and the libraries it links.
/// These are deferred to the VERY END of the dist upgrade order so the running
/// `gpg` keeps verifying every other package's per-package signature throughout
/// the run; if gpg or one of its libraries is replaced mid-run, verification
/// silently falls back to md5 ("integrity only (no GPG)"). Kept tight to the
/// GnuPG chain on purpose — extend against the real package .dep if a wider set
/// is ever needed (e.g. libksba, libgcrypt).
const DIST_GPG_LAST: &[&str] = &[
    "gpgme",
    "libassuan",
    "libgpg-error",
    "npth",
    "gnupg",
    "gnupg2",
];

/// Build the dist upgrade set: for every installed package take the TARGET's
/// winning candidate (`db.resolve` = highest-priority repo, i.e. patches over
/// slackware after the transform), IGNORING the priority guard — this is the
/// deliberate dist bypass. blacklist/frozen are already emptied by the transform.
/// A package the target does not provide (e.g. a now-disabled third-party one) is
/// left untouched. Returns `(critical, rest)`: the critical set ordered per
/// `DIST_CRITICAL`, the rest sorted by name.
pub(crate) fn dist_upgrade_sets(db: &PkgDb, installed: &[pkg::PkgId]) -> (Vec<PlanItem>, Vec<PlanItem>) {
    let mut by_name: HashMap<String, PlanItem> = HashMap::new();
    for inst in installed {
        let Some(avail) = db.resolve(&inst.name) else {
            continue; // target has no package by this name — leave it as-is
        };
        // Already at the target's exact version+build: nothing to do.
        if avail.id.version == inst.version && avail.id.build == inst.build {
            continue;
        }
        by_name.insert(
            inst.name.clone(),
            PlanItem {
                pkg: avail.clone(),
                action: InstallAction::Upgrade,
                dep_for: None,
                from: Some(format!("{}-{}-{}", inst.version, inst.arch, inst.build)),
            },
        );
    }
    let mut critical = Vec::new();
    for name in DIST_CRITICAL {
        if let Some(it) = by_name.remove(*name) {
            critical.push(it);
        }
    }
    let mut rest: Vec<PlanItem> = by_name.into_values().collect();
    rest.sort_by(|a, b| a.pkg.id.name.cmp(&b.pkg.id.name));
    // Defer the GnuPG verification toolchain (DIST_GPG_LAST) to the very end, so
    // the working gpg keeps verifying every other package and is only replaced
    // once nothing else remains to install.
    let (mut others, gpg_last): (Vec<PlanItem>, Vec<PlanItem>) = rest
        .into_iter()
        .partition(|it| !DIST_GPG_LAST.contains(&it.pkg.id.name.as_str()));
    others.extend(gpg_last);
    (critical, others)
}

/// The dist install-new set: every package the OFFICIAL repos (slackware +
/// patches after the transform) provide that is not currently installed — the
/// target distribution's newly-added packages. Sorted by name, deduplicated.
fn dist_install_new(
    cfg: &Config,
    db: &PkgDb,
    installed: &[pkg::PkgId],
) -> Result<Vec<PlanItem>, String> {
    // A distribution upgrade takes new packages from the official tree and the
    // `patches` subtree ONLY — never extra/testing/pasture, and never a
    // third-party repo, regardless of priority. `patches` is recognised the same
    // way the rest of the dist code does: a subtree whose URL is the patches/
    // tree. This is deliberately NOT priority-based: a testing repo ranked above
    // official must never be pulled into a dist-upgrade.
    let at: Vec<String> = cfg
        .repos
        .iter()
        .filter(|r| r.official || (r.subtree && r.url.contains("/patches")))
        .map(|r| format!("@{}", r.name))
        .collect();
    if at.is_empty() {
        return Ok(Vec::new());
    }
    let (matched, _misses) = collect(db, &at)?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<PlanItem> = Vec::new();
    for p in matched {
        if system::is_installed(installed, &p.id.name) || !seen.insert(p.id.name.clone()) {
            continue;
        }
        out.push(PlanItem {
            pkg: p.clone(),
            action: InstallAction::Install,
            dep_for: None,
            from: None,
        });
    }
    out.sort_by(|a, b| a.pkg.id.name.cmp(&b.pkg.id.name));
    Ok(out)
}

/// Non-core packages are downloaded, installed, then DELETED in batches of this
/// many, so the download cache never has to hold the whole distribution at once
/// (the disk-full failure mode that bricked a test VM). Core packages are exempt:
/// they are fetched all together first so a missing core aborts before any
/// install.
const DIST_BATCH: usize = 24;

/// Cheap disk gate run BEFORE the point of no return: a full distribution upgrade
/// needs several GiB, so refuse outright when free space is obviously too low,
/// before the transform touches anything. Floors only — the precise, plan-aware
/// check is [`dist_disk_preflight`], after the target metadata is known.
fn dist_early_disk_gate(cfg: &Config) -> Result<(), String> {
    const MIN_ROOT_GIB: u64 = 5;
    const MIN_CACHE_GIB: u64 = 2;
    let gib = |kb: u64| kb as f64 / 1024.0 / 1024.0;
    if let Some(kb) = avail_kb(std::path::Path::new("/")) {
        if kb < MIN_ROOT_GIB * 1024 * 1024 {
            return Err(format!(
                "only {:.1} GiB free on / — a distribution upgrade needs at least {} GiB there. \
                 Free space and re-run (nothing has been changed).",
                gib(kb),
                MIN_ROOT_GIB
            ));
        }
    }
    if let Some(kb) = avail_kb(&cfg.cache_dir) {
        if kb < MIN_CACHE_GIB * 1024 * 1024 {
            return Err(format!(
                "only {:.1} GiB free on the cache partition — need at least {} GiB for staged \
                 downloads. Free space and re-run (nothing has been changed).",
                gib(kb),
                MIN_CACHE_GIB
            ));
        }
    }
    Ok(())
}

/// Available 1K-blocks on the filesystem holding `path` (POSIX `df -Pk`), or None
/// if it cannot be determined.
fn avail_kb(path: &std::path::Path) -> Option<u64> {
    let out = std::process::Command::new("df").arg("-Pk").arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse::<u64>()
        .ok()
}

/// Plan-aware disk pre-flight that STOPS (returns Err) when there is clearly not
/// enough room, rather than only warning. Checks two things: that `/` can hold
/// the genuinely-new packages (install-new, uncompressed) plus headroom, and that
/// the cache partition can hold at least one download batch (since non-core
/// downloads are now staged in batches of `DIST_BATCH` and deleted as they go,
/// the cache never needs the whole tree). Aborting here is safe: the transform is
/// idempotent, so the user frees space and re-runs.
fn dist_disk_preflight(
    cfg: &Config,
    install_new: &[PlanItem],
    upgrades: &[PlanItem],
) -> Result<(), String> {
    let gib = |kb: u64| kb as f64 / 1024.0 / 1024.0;
    let new_uncompressed_k: u64 =
        install_new.iter().filter_map(|it| it.pkg.size_uncompressed_k).sum();
    let max_compressed_k: u64 = install_new
        .iter()
        .chain(upgrades.iter())
        .filter_map(|it| it.pkg.size_k)
        .max()
        .unwrap_or(0);
    let batch_cache_k = max_compressed_k.saturating_mul(DIST_BATCH as u64);
    let buffer_k: u64 = 1024 * 1024; // 1 GiB headroom for upgrade churn
    let need_root_k = new_uncompressed_k + buffer_k;

    println!("{}", ui::blue("Disk space:"));
    if let Some(kb) = avail_kb(std::path::Path::new("/")) {
        println!(
            "  {} /      {:.1} GiB free (need ~{:.1} GiB for new packages + headroom)",
            ui::dim("·"),
            gib(kb),
            gib(need_root_k)
        );
        if kb < need_root_k {
            return Err(format!(
                "not enough space on / for the new packages: {:.1} GiB free, ~{:.1} GiB needed. \
                 Free space and re-run `slacker upgrade-dist` (safe to re-run).",
                gib(kb),
                gib(need_root_k)
            ));
        }
    }
    if let Some(kb) = avail_kb(&cfg.cache_dir) {
        println!(
            "  {} cache  {:.1} GiB free (staged in batches of {}, ~{:.1} GiB per batch)",
            ui::dim("·"),
            gib(kb),
            DIST_BATCH,
            gib(batch_cache_k)
        );
        if batch_cache_k > 0 && kb < batch_cache_k {
            return Err(format!(
                "not enough space on the cache partition for even one download batch: {:.1} GiB \
                 free, ~{:.1} GiB needed. Free space and re-run.",
                gib(kb),
                gib(batch_cache_k)
            ));
        }
    }
    Ok(())
}

/// Outcome of a [`dist_execute`] pass: how many non-core packages failed to
/// download or install. A pass with ANY failures means the system is only
/// partially upgraded, so clean-system and the second pass must be skipped (you
/// never prune a half-upgraded system). Core failures are fatal and return Err
/// instead, never reaching here.
struct DistReport {
    dl_failed: usize,
    install_failed: usize,
}
impl DistReport {
    fn clean(&self) -> bool {
        self.dl_failed == 0 && self.install_failed == 0
    }
}

/// Drive the install. Core first: fetch+verify ALL core together, abort if any is
/// missing (system still untouched), then install them serially with a
/// post-install package-database check, aborting if a core does not actually land
/// (the disk-full "did not install correctly" case). Then non-core install-new
/// and the rest, processed in batches that are downloaded, installed and DELETED
/// as they go so the cache never holds the whole tree; non-core is best-effort
/// (a failed item is skipped, reported, and never leaves a partial package).
///
/// Note we never parse or second-guess pkgtools' exit status — pkgtools is left
/// exactly as Pat ships it. The safety net is feeding it only verified packages
/// and confirming the *result* (the DB record) for the core, plus stopping on a
/// full disk before it can produce a broken install at all.
fn dist_execute(
    cfg: &Config,
    critical: &[PlanItem],
    install_new: &[PlanItem],
    upgrades: &[PlanItem],
) -> Result<DistReport, String> {
    let noncore: Vec<&PlanItem> = install_new.iter().chain(upgrades.iter()).collect();
    if critical.is_empty() && noncore.is_empty() {
        println!("{}", ui::green("Nothing to upgrade or install — already at the target."));
        return Ok(DistReport { dl_failed: 0, install_failed: 0 });
    }

    // In a dist we always allow installing a package with no installed
    // counterpart (renames across releases, e.g. glibc-solibs -> aaa_glibc-solibs),
    // hence --install-new even for the "upgrade" action.
    let do_install = |dest: &Path, action: InstallAction| match action {
        InstallAction::Install => system::install(dest),
        InstallAction::Upgrade => system::upgrade_install_new(dest),
        InstallAction::Reinstall => system::reinstall(dest),
    };
    let rm_cached = |dest: &Path| {
        let _ = std::fs::remove_file(dest);
    };

    let mut dl_failed: Vec<(String, String)> = Vec::new();
    let mut install_failed: Vec<(String, String)> = Vec::new();
    let mut done = 0usize;

    // ---- CORE: fetch + verify all together; a missing core aborts now. ----
    if !critical.is_empty() {
        let mut citems: Vec<DlItem> = Vec::with_capacity(critical.len());
        for it in critical {
            let r = cfg.repo_by_name(&it.pkg.repo).ok_or("internal repo lookup failed")?;
            let dest = package_dest(cfg, &it.pkg.repo, &it.pkg.filename)?;
            citems.push(DlItem { repo: r, pkg: &it.pkg, dest });
        }
        println!(
            "{}",
            ui::blue(&format!(
                "Core: downloading + verifying {} package(s) before touching the system...",
                citems.len()
            ))
        );
        let outc = parallel_fetch(cfg, &citems, cfg.max_parallel);
        let (ready, _dlf) = summarize_outcomes(&outc, citems.len());
        for (i, it) in citems.iter().enumerate() {
            if !ready[i] {
                return Err(format!(
                    "core package '{}' failed download/verify — aborting before any install \
                     (system untouched). Fix the mirror/network/disk and re-run \
                     `slacker upgrade-dist` (safe to re-run).",
                    it.pkg.id.name
                ));
            }
        }
        // ---- Phase 0: install core serially, ABORT on failure, with a
        //      post-install record check. ----
        println!("{}", ui::blue("Phase 0 — core packages (in order):"));
        for (i, it) in citems.iter().enumerate() {
            do_install(&it.dest, critical[i].action).map_err(|e| {
                format!(
                    "Phase 0: installing core '{}' failed: {e}\nThe system may be in a partial \
                     state — resolve this package by hand before continuing.",
                    it.pkg.id.name
                )
            })?;
            let record = pkg::strip_pkg_ext(&it.pkg.filename);
            if !system::record_present(&cfg.pkg_db_dir, record) {
                return Err(format!(
                    "Phase 0: core '{}' did not register as installed (no package-database \
                     record '{record}') — the install did not take, most often a full disk. \
                     Aborting before touching anything else; free space and re-run \
                     `slacker upgrade-dist` (safe to re-run).",
                    it.pkg.id.name
                ));
            }
            rm_cached(&it.dest);
            println!("  {} {}", ui::green("✓"), it.pkg.id.name);
        }
    }

    // ---- Non-core: download -> install -> DELETE, in batches, best-effort. ----
    let n_new = install_new.len();
    if n_new > 0 {
        println!("{}", ui::blue(&format!("Phase 1 — install-new ({n_new}):")));
    }
    let mut announced_phase2 = false;
    let total_noncore = noncore.len();
    let mut idx = 0usize;
    while idx < total_noncore {
        let end = (idx + DIST_BATCH).min(total_noncore);
        let mut batch: Vec<DlItem> = Vec::with_capacity(end - idx);
        for it in &noncore[idx..end] {
            let r = cfg.repo_by_name(&it.pkg.repo).ok_or("internal repo lookup failed")?;
            let dest = package_dest(cfg, &it.pkg.repo, &it.pkg.filename)?;
            batch.push(DlItem { repo: r, pkg: &it.pkg, dest });
        }
        let outc = parallel_fetch(cfg, &batch, cfg.max_parallel);
        let (ready, mut dlf) = summarize_outcomes(&outc, batch.len());
        dl_failed.append(&mut dlf);
        for (b, it) in batch.iter().enumerate() {
            let gi = idx + b;
            if !announced_phase2 && gi >= n_new && !upgrades.is_empty() {
                println!(
                    "{}",
                    ui::blue(&format!("Phase 2 — upgrade the rest ({}):", upgrades.len()))
                );
                announced_phase2 = true;
            }
            if !ready[b] {
                continue; // download/verify failed; counted in dl_failed
            }
            match do_install(&it.dest, noncore[gi].action) {
                Ok(()) => done += 1,
                Err(e) => install_failed.push((it.pkg.id.name.clone(), e)),
            }
            rm_cached(&it.dest); // free space whether it installed or not
        }
        idx = end;
    }

    let attempted = critical.len() + total_noncore;
    let ok = critical.len() + done;
    report_batch_failures(attempted, ok, &dl_failed, &install_failed);
    Ok(DistReport {
        dl_failed: dl_failed.len(),
        install_failed: install_failed.len(),
    })
}

/// Resolves the running release (from `/etc/os-release`) and the target release
/// (the explicit argument). Runs the fail-closed direction check, applies the
/// repo/blacklist transform, then drives the phased upgrade: refresh metadata for
/// the target, take the target's version of every package (priority bypassed),
/// and install core-first, then install-new, then the rest.
pub(crate) fn cmd_upgrade_dist(cli: &Cli, cfg: &Config, target_arg: &str) -> Result<Outcome, String> {
    // A little ceremony for the distribution-scale operation on the official tree.
    banner::show();
    // Running side: codename is authoritative for -current; VERSION_ID names a
    // stable. An unrecognisable os-release is a refusal, never a guess.
    let running = dist::parse_release_from_os(
        system::version_id().as_deref(),
        system::version_codename().as_deref(),
    )
    .ok_or(
        "could not identify this system's Slackware release from /etc/os-release \
         (need VERSION_CODENAME=current or a numeric VERSION_ID)",
    )?;

    // Target side: the explicit argument — where you are GOING, not where you are.
    let target = dist::parse_target(target_arg).ok_or_else(|| {
        format!(
            "unrecognised upgrade-dist target {target_arg:?}; use `current` or a \
             newer stable version like `15.1`"
        )
    })?;

    // Fail-closed routing: only whitelisted directions return Ok.
    let route = dist::dist_route(&running, &target)?;

    println!(
        "{}",
        ui::blue(&format!(
            "upgrade-dist: {} -> {}",
            dist::show(&running),
            dist::show(&target)
        ))
    );
    match &route {
        dist::Route::StableToCurrent => println!("  {}", ui::green("route allowed: 15.0 -> -current")),
        dist::Route::StableToStable(n) => {
            println!("  {}", ui::green(&format!("route allowed: 15.0 -> {n}")))
        }
    }

    // The release-segment rewrite: replace the running release directory with the
    // target's everywhere it appears in the mirror/repo URLs (so `mirror` and
    // `mirror/patches` repos auto-follow, and any literal slackware URL is moved).
    let running_seg = slackware_dir(&cfg.arch)
        .ok_or(
            "cannot determine this system's slackware release directory — \
             check /etc/os-release and the `arch` line in slacker.conf",
        )?;
    let prefix = running_seg
        .rsplit_once('-')
        .map(|(p, _)| p)
        .unwrap_or(running_seg.as_str());
    let target_seg = format!("{prefix}-{}", dist::release_suffix(&target));

    // Local upgrade-dist source (DISTRO_UPGRADE_MIRROR): when set, the dist pulls
    // the target release from this local mirror / mounted ISO. Validate it now,
    // before anything is touched — a bad/missing local mirror STOPS here.
    let local_mirror = cfg.distro_upgrade_mirror.as_deref();
    if let Some(m) = local_mirror {
        dist_validate_local_mirror(m, &target)?;
    }

    if cli.dry_run {
        println!("{}", ui::blue("dry run — showing what would change, nothing is touched:"));
        dist_transform(cfg, &running_seg, &target_seg, local_mirror, true)?;
        println!(
            "{}",
            ui::dim(
                "  then: disk gate, save an escape kit (config backup + installed-set template), \
                 refresh metadata, fetch+verify core first, phase0 core \
                 (aaa_glibc-solibs -> pkgtools/tar/xz/gzip/findutils), then install-new and the rest \
                 staged in batches (download -> install -> delete), clean-system, a second \
                 upgrade-all+install-new, new-config, status, kernel/boot reminder. \
                 clean-system and the second pass are skipped if anything fails to install."
            )
        );
        return Ok(Outcome::Ok);
    }

    // Cheap disk gate BEFORE the point of no return: refuse outright if free
    // space is obviously too low for a whole-distribution upgrade, before the
    // transform touches anything.
    dist_early_disk_gate(cfg)?;

    // ---- POINT OF NO RETURN ----
    let suffix = dist::release_suffix(&target);
    let bar = "=".repeat(70);
    println!("{}", ui::red(&bar));
    println!("{}", ui::red("  DISTRIBUTION UPGRADE — this is not a normal update."));
    println!(
        "{}",
        ui::red(&format!(
            "  It re-points your mirror/repos to {target_seg} and will take the target's"
        ))
    );
    println!("{}", ui::red("  version of EVERY package, IGNORING source priority, the blacklist,"));
    println!("{}", ui::red("  and frozen packages. The blacklist is backed up and emptied."));
    println!("{}", ui::red("  A half-finished dist-upgrade can leave the system unbootable."));
    println!("{}", ui::red("  Take a VM snapshot / full backup first."));
    if in_graphical_session() {
        println!("{}", ui::red(&bar));
        println!(
            "{}",
            ui::red("  You are in a GRAPHICAL session. Run a dist-upgrade from a console:")
        );
        println!(
            "{}",
            ui::red("  log out of X/Wayland, switch to a tty (e.g. Ctrl+Alt+F2), and run it")
        );
        println!(
            "{}",
            ui::red("  there. Proceeding from a graphical session is at your own risk.")
        );
    }
    println!("{}", ui::red(&bar));

    if !cli.yes {
        print!(
            "{}",
            ui::blue(&format!("Type `{suffix}` exactly to proceed (anything else aborts): "))
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() || line.trim() != suffix {
            println!("{}", ui::blue("aborted — nothing changed."));
            return Ok(Outcome::Ok);
        }
    }

    // ---- Phase -1: escape kit — runs BEFORE the transform, while the repos
    // still point at the running release, so the package snapshot resolves the
    // current (15.0) official set. Backs up the config files verbatim and writes
    // a template of the installed packages, so there is always something to fall
    // back on if the upgrade goes wrong. Best-effort: a backup failure warns but
    // does not stop the upgrade.
    if let Err(e) = dist_backup(cfg) {
        println!("{}", ui::yellow(&format!("escape-kit backup incomplete: {e} (continuing)")));
    }

    // Apply the transform for real.
    dist_transform(cfg, &running_seg, &target_seg, local_mirror, false)?;
    println!(
        "{}",
        ui::green(&format!("re-pointed mirror/repos to {target_seg}; blacklist backed up + emptied."))
    );

    // The repos file changed, so re-read the configuration before doing anything
    // that depends on it (metadata refresh, plan building).
    let cfg = Config::load_dir(&cfg.config_dir)
        .map_err(|e| format!("reload config after transform: {e}"))?;

    // Refresh metadata for the target release (the cache still holds the old
    // release's PACKAGES.TXT until now). The user already committed at the point
    // of no return, so DON'T show the normal per-repo update menu (which, on
    // -current, lists "patches updates available" and only confuses the flow).
    // One clear confirmation, default YES (Enter proceeds), then update all repos
    // silently.
    println!(
        "{}",
        ui::blue(&format!("Connected to the {} repositories.", dist::show(&target)))
    );
    let proceed = if cli.yes {
        true
    } else {
        print!(
            "{} {}{}{}{}{} ",
            ui::blue("Proceed with the distribution upgrade?"),
            ui::blue("["),
            ui::white("Y"),
            ui::blue("/"),
            ui::white("n"),
            ui::blue("]")
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        let t = line.trim().to_lowercase();
        t.is_empty() || t == "y" || t == "yes" // default YES
    };
    if !proceed {
        println!(
            "{}",
            ui::blue(
                "aborted — the mirror/repos were already re-pointed; restore from the escape kit \
                 (printed above) to undo."
            )
        );
        return Ok(Outcome::Ok);
    }
    // Update ALL active repos non-interactively (best-effort: an empty
    // current/patches printing FAILED must not abort the dist).
    let changelog_repo = changelog::changelog_repo(&cfg.repos).map(|r| r.name.clone());
    let mut up = UpdateOutcomes::default();
    for r in &cfg.repos {
        let track = changelog_repo.as_deref() == Some(r.name.as_str());
        update_one_repo(&cfg, r, track, &mut up);
    }

    // Build the dist plan against the refreshed cache: the target's version of
    // every installed package (priority bypassed), plus the target's new packages.
    let db = PkgDb::load(&cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (critical, upgrades) = dist_upgrade_sets(&db, &installed);
    let install_new = dist_install_new(&cfg, &db, &installed)?;

    let self_upgrade = critical
        .iter()
        .chain(install_new.iter())
        .chain(upgrades.iter())
        .any(|it| it.pkg.id.name == env!("CARGO_PKG_NAME"));

    println!(
        "{}",
        ui::blue(&format!(
            "Plan: {} core, {} new, {} other upgrade(s).",
            critical.len(),
            install_new.len(),
            upgrades.len()
        ))
    );
    // Plan-aware disk pre-flight that STOPS if there is clearly not enough room
    // (safe to re-run: the transform is idempotent).
    dist_disk_preflight(&cfg, &install_new, &upgrades)?;

    // Download (staged in batches) + install core-first -> install-new -> rest.
    let report = dist_execute(&cfg, &critical, &install_new, &upgrades)?;

    if report.clean() {
        // ---- Phase 3: remove packages no longer in the distribution ----
        // clean-system anchors membership on the OFFICIAL main-tree PACKAGES.TXT
        // (the full distribution), so an empty current/patches or a patches subset
        // on N+1 cannot mislead it; it is interactive, so the user reviews the
        // list (e.g. the now-disabled third-party packages) and confirms before
        // anything is removed.
        println!();
        println!("{}", ui::blue("Phase 3 — remove packages no longer in the distribution:"));
        cmd_clean_system(cli, &cfg)?;

        // ---- Phase 4: a second upgrade-all + install-new, for certainty ----
        // After the core upgrades and the clean-up, run the selection once more so
        // anything that shifted is caught. Normally this finds nothing.
        println!();
        println!("{}", ui::blue("Phase 4 — second pass (upgrade-all + install-new) to be sure:"));
        let db2 = PkgDb::load(&cfg)?;
        let installed2 = system::installed_packages(&cfg.pkg_db_dir)?;
        let (critical2, upgrades2) = dist_upgrade_sets(&db2, &installed2);
        let new2 = dist_install_new(&cfg, &db2, &installed2)?;
        let _ = dist_execute(&cfg, &critical2, &new2, &upgrades2)?;
    } else {
        // Some packages failed to download or install: the system is only
        // partially upgraded. Skip clean-system and the second pass — pruning a
        // half-upgraded system is exactly how the test VM proposed deleting live
        // packages. The transform is idempotent, so re-running after fixing the
        // cause (usually disk space or a mirror) is safe and resumes cleanly.
        println!();
        println!(
            "{}",
            ui::yellow(&format!(
                "{} download and {} install failure(s) — the upgrade is INCOMPLETE.",
                report.dl_failed, report.install_failed
            ))
        );
        println!(
            "{}",
            ui::yellow(
                "Skipping clean-system and the second pass: a partially-upgraded system must \
                 not be pruned (it would propose removing packages that simply did not upgrade \
                 yet). Fix the cause (free disk space / check the mirror) and re-run \
                 `slacker upgrade-dist` — it is safe to re-run and will resume."
            )
        );
    }

    // ---- Phase 5: merge .new config files ----
    println!();
    println!("{}", ui::blue("Phase 5 — config files (.new):"));
    cmd_new_config(cli, true)?;

    // ---- Phase 6: final report ----
    println!();
    cmd_status(&cfg.config_dir)?;

    // ---- Boot reminder ----
    // Follows the "before you reboot" section of Patrick's upgrade HOWTO: build
    // the initrd first (geninitrd also refreshes grub.cfg), then reinstall
    // whichever bootloader is in use. Getting this wrong is the one way a
    // finished dist-upgrade still leaves an unbootable system.
    println!();
    println!("{}", ui::green(&format!("Distribution upgrade to {target_seg} complete.")));
    println!("{}", ui::red("DO NOT REBOOT until the bootloader knows about the new kernel."));
    println!("{}", ui::blue("Before rebooting:"));
    println!(
        "  1) {}  {}",
        ui::white("geninitrd"),
        ui::dim("— builds the initrd for the new kernel (and updates grub.cfg)")
    );
    println!("  2) {}", ui::white("reinstall your bootloader:"));
    println!(
        "       {}   {}",
        ui::white("LILO:            lilo"),
        ui::dim("(check /etc/lilo.conf points at a valid kernel first)")
    );
    println!("       {}", ui::white("GRUB (UEFI):     grub-install"));
    println!(
        "       {}   {}",
        ui::white("GRUB (BIOS):     grub-install --target=i386-pc /dev/sda"),
        ui::dim("(your boot disk)")
    );
    println!(
        "       {}   {}",
        ui::white("ELILO/UEFI:      eliloconfig"),
        ui::dim("(installs kernel + initrd to the EFI System Partition)")
    );
    println!(
        "  {}",
        ui::dim("booting from a USB stick? copy the new kernel onto it in place of the old one.")
    );

    // If a local mirror / ISO drove the upgrade, the active mirror now points at
    // that local source — fine for the upgrade just done, but useless for future
    // updates once it is unmounted/removed. Tell the user to set a remote mirror.
    if let Some(m) = local_mirror {
        println!();
        println!(
            "{}",
            ui::yellow(&format!("Your mirror now points at the local source: {m}"))
        );
        println!(
            "  {}",
            ui::white(
                "set a remote -current mirror for future updates (uncomment one in `mirrors`, \
                 or run `slacker find-mirror`), and you can clear DISTRO_UPGRADE_MIRROR."
            )
        );
    }

    if self_upgrade {
        println!();
        println!("{}", ui::blue("slacker upgraded itself during the dist-upgrade; re-run it to continue."));
        return Ok(Outcome::SelfUpgrade);
    }
    Ok(Outcome::Ok)
}

/// Phase -1 escape kit. Before the dist touches anything, save enough to recover:
///  * the config files (`slacker.conf`, `mirrors`, `repos`, `blacklist`) copied
///    verbatim into a timestamped `dist-backup-<stamp>/` under the config dir, so
///    the original release's mirror/repo URLs (which the transform rewrites in
///    place) can be restored;
///  * a `dist-backup-<stamp>` template of every installed package that a repo
///    knows about (orphans/third-party-not-in-a-repo are skipped, exactly like
///    `generate-template`), written to the normal `templates/` dir AND copied into
///    the backup dir — a manifest of what was installed, reusable via
///    `install-template`.
/// Must be called while the repos still point at the running release. Best-effort
/// at the call site (a failure warns, the upgrade continues).
fn dist_backup(cfg: &Config) -> Result<(), String> {
    let stamp = backup_stamp();
    let dir = cfg.config_dir.join(format!("dist-backup-{stamp}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    // 1) config files, verbatim
    let mut saved: Vec<&str> = Vec::new();
    for f in ["slacker.conf", "mirrors", "repos", "blacklist"] {
        let src = cfg.config_dir.join(f);
        if src.exists() {
            std::fs::copy(&src, dir.join(f)).map_err(|e| format!("back up {f}: {e}"))?;
            saved.push(f);
        }
    }

    // 2) template of the installed set known to a repo (skip orphans)
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let orphans: HashSet<&str> =
        db.orphans(&installed).into_iter().map(|p| p.name.as_str()).collect();
    let names: Vec<String> = installed
        .iter()
        .map(|p| p.name.clone())
        .filter(|n| !orphans.contains(n.as_str()))
        .collect();
    let tmpl_name = format!("dist-backup-{stamp}");
    let tmpl_path = template::generate(&cfg.config_dir, &tmpl_name, &names)?;
    // self-contained kit: also drop the template inside the backup dir
    let _ = std::fs::copy(&tmpl_path, dir.join(format!("{tmpl_name}.template")));

    println!(
        "{}",
        ui::green(&format!(
            "Escape kit saved to {}: configs ({}), template '{}' ({} packages).",
            dir.display(),
            saved.join(", "),
            tmpl_name,
            names.len()
        ))
    );
    println!(
        "  {}",
        ui::dim(&format!(
            "to undo before reboot: restore the files from {} into {}",
            dir.display(),
            cfg.config_dir.display()
        ))
    );
    Ok(())
}

/// A sortable, human-readable timestamp for backup names (`YYYYmmdd-HHMMSS`).
/// Uses the system `date` if available; falls back to UNIX epoch seconds so a
/// backup name is always produced.
fn backup_stamp() -> String {
    if let Ok(out) = std::process::Command::new("date").arg("+%Y%m%d-%H%M%S").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("epoch-{secs}")
}

/// Replace the running release directory segment with the target's in the
/// `mirrors` and `repos` files (so `mirror`/`mirror/patches` repos and any
/// literal slackware URL follow the dist target), then back up the blacklist to
/// `blacklist.bak` and empty it. `dry_run` prints the changes without touching
/// anything.
fn dist_transform(
    cfg: &Config,
    running_seg: &str,
    target_seg: &str,
    local_mirror: Option<&str>,
    dry_run: bool,
) -> Result<(), String> {
    let cd = &cfg.config_dir;

    // 1) mirrors: either set the active line to a LOCAL mirror (DISTRO_UPGRADE_
    // MIRROR — local http/file mirror or mounted ISO), or, in the normal case,
    // re-point the active mirror's release directory to the target.
    {
        let path = cd.join("mirrors");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let new = match local_mirror {
                Some(m) => set_active_mirror_line(&text, m),
                None => dist_rewrite_text(&text, running_seg, target_seg),
            };
            let label = match local_mirror {
                Some(m) => format!("use local mirror {m}"),
                None => format!("-> {target_seg}"),
            };
            if new != text {
                if dry_run {
                    println!("  {}", ui::white("mirrors"));
                    for (o, n) in text.lines().zip(new.lines()).filter(|(o, n)| o != n) {
                        println!("    {}", ui::red(&format!("- {o}")));
                        println!("    {}", ui::green(&format!("+ {n}")));
                    }
                    // set_active_mirror_line may also APPEND a line (no zip pair).
                    if local_mirror.is_some() && new.lines().count() > text.lines().count() {
                        for extra in new.lines().skip(text.lines().count()) {
                            println!("    {}", ui::green(&format!("+ {extra}")));
                        }
                    }
                } else {
                    refuse_through_symlink(&path)?;
                    std::fs::write(&path, &new)
                        .map_err(|e| format!("write {}: {e}", path.display()))?;
                    println!("  {}", ui::dim(&format!("rewrote mirrors ({label})")));
                }
            } else if dry_run {
                println!("  {} {}", ui::white("mirrors"), ui::dim("(no change needed)"));
            }
        }
    }

    // 2) repos: DISABLE every active non-mirror repo (a literal `://` URL — a
    // third-party repo with its own versioning slacker cannot safely re-point).
    // `mirror`/`mirror/<subpath>` repos follow the rewritten mirror and stay;
    // build-tag priority lines stay. The user re-enables/fixes third-party repos
    // by hand after the dist-upgrade.
    {
        let path = cd.join("repos");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let (new, disabled) = comment_nonmirror_repos(&text);
            if disabled.is_empty() {
                if dry_run {
                    println!("  {} {}", ui::white("repos"), ui::dim("(no third-party repos to disable)"));
                }
            } else if dry_run {
                println!("  {}", ui::white("repos"));
                for line in &disabled {
                    println!("    {}", ui::purple(&format!("# {line}   (would disable — non-mirror repo)")));
                }
            } else {
                refuse_through_symlink(&path)?;
                std::fs::write(&path, &new).map_err(|e| format!("write {}: {e}", path.display()))?;
                println!(
                    "  {}",
                    ui::dim(&format!("disabled {} non-mirror repo(s) in repos", disabled.len()))
                );
            }
        }
    }

    // 3) blacklist: back up and empty.
    let bl = cd.join("blacklist");
    if let Ok(text) = std::fs::read_to_string(&bl) {
        let rules = text
            .lines()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#')
            })
            .count();
        if dry_run {
            println!(
                "  {} {}",
                ui::white("blacklist"),
                ui::dim(&format!("(would back up {rules} rule(s) -> blacklist.bak and empty it)"))
            );
        } else {
            // Preserve the ORIGINAL backup across re-runs: only create
            // blacklist.bak if it does not already exist. A re-run (e.g. after a
            // disk-space stop) would otherwise copy the now-empty blacklist over
            // the real backup and lose the user's rules. Either way the live
            // blacklist is emptied so the dist ignores it.
            let bak = cd.join("blacklist.bak");
            if bak.exists() {
                println!(
                    "  {}",
                    ui::dim("blacklist.bak already exists (kept); emptied blacklist")
                );
            } else {
                std::fs::copy(&bl, &bak).map_err(|e| format!("back up blacklist: {e}"))?;
                println!(
                    "  {}",
                    ui::dim(&format!(
                        "backed up {rules} blacklist rule(s) -> blacklist.bak, emptied blacklist"
                    ))
                );
            }
            std::fs::write(&bl, "").map_err(|e| format!("empty blacklist: {e}"))?;
        }
    }
    Ok(())
}

/// Pure: swap every occurrence of the running release directory segment for the
/// target's. Lines using the `mirror` keyword carry no segment and pass through
/// untouched (they follow the rewritten mirrors file).
fn dist_rewrite_text(text: &str, running_seg: &str, target_seg: &str) -> String {
    text.replace(running_seg, target_seg)
}

/// Pure: make `url` the sole ACTIVE line in a mirrors file. Every existing active
/// (uncommented, non-blank) line is commented out, then `url` is appended as the
/// one active line. Used by the local-mirror (DISTRO_UPGRADE_MIRROR) dist path so
/// downloads come from the local http/file mirror or mounted ISO. Idempotent: if
/// `url` is already the only active line, the text is returned unchanged.
fn set_active_mirror_line(text: &str, url: &str) -> String {
    let already_sole_active = {
        let mut actives = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'));
        actives.next() == Some(url) && actives.next().is_none()
    };
    if already_sole_active {
        return text.to_string();
    }
    let mut out: Vec<String> = Vec::with_capacity(text.lines().count() + 2);
    for line in text.lines() {
        let t = line.trim_start();
        if !t.is_empty() && !t.starts_with('#') {
            out.push(format!("# {line}"));
        } else {
            out.push(line.to_string());
        }
    }
    out.push("# --- upgrade-dist: local mirror (DISTRO_UPGRADE_MIRROR) ---".to_string());
    out.push(url.to_string());
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

/// Validate a local upgrade-dist mirror (DISTRO_UPGRADE_MIRROR) BEFORE the point
/// of no return. Two checks: (1) if the mirror URL contains a recognisable
/// `slackware*-<release>` segment it MUST match the target (catches a mirror
/// left pointing at the wrong release); a mounted-ISO/path with no such segment
/// can't be checked this way, so we warn and rely on the explicit target plus
/// the typed confirmation. (2) the mirror must actually serve a PACKAGES.TXT.
/// Any failure returns Err so the caller STOPS without touching anything.
fn dist_validate_local_mirror(mirror: &str, target: &dist::Release) -> Result<(), String> {
    println!("{}", ui::blue(&format!("Local upgrade-dist mirror: {mirror}")));
    match dist::parse_release_from_url(mirror) {
        Some(r) if &r == target => {
            println!("  {}", ui::green(&format!("path names the target release ({})", dist::show(target))));
        }
        Some(r) => {
            return Err(format!(
                "the local mirror path names release '{}', but you asked to upgrade to '{}'. \
                 Point DISTRO_UPGRADE_MIRROR at the {} tree (or fix the target).",
                dist::show(&r),
                dist::show(target),
                dist::show(target),
            ));
        }
        None => {
            println!(
                "  {}",
                ui::yellow(&format!(
                    "could not confirm the release from the path (e.g. a mounted ISO) — make \
                     sure it is the {} tree; proceeding on your explicit target + confirmation",
                    dist::show(target)
                ))
            );
        }
    }
    // Reachability: the mirror must serve a PACKAGES.TXT.
    let probe = format!("{}/PACKAGES.TXT", mirror.trim_end_matches('/'));
    match download::first_line(&probe, std::time::Duration::from_secs(20)) {
        Ok(_) => {
            println!("  {}", ui::green("PACKAGES.TXT is reachable"));
            Ok(())
        }
        Err(e) => Err(format!(
            "the local mirror does not serve a readable PACKAGES.TXT at {probe} ({e}). \
             Check the path/mount and that it is a full Slackware tree."
        )),
    }
}

/// Pure: comment out every ACTIVE repos-file line that is a non-mirror repo —
/// one whose third field is a literal URL (`://`), i.e. a third-party repo.
/// `mirror`/`mirror/<subpath>` repos and build-tag priority lines (third field
/// not a URL) are left untouched, as are already-commented/blank lines. Returns
/// the rewritten text and the list of lines that were disabled.
fn comment_nonmirror_repos(text: &str) -> (String, Vec<String>) {
    let mut out: Vec<String> = Vec::with_capacity(text.lines().count());
    let mut disabled: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        let is_active = !t.is_empty() && !t.starts_with('#');
        let third_is_url = t.split_whitespace().nth(2).is_some_and(|f| f.contains("://"));
        if is_active && third_is_url {
            out.push(format!("#{line}"));
            disabled.push(line.trim().to_string());
        } else {
            out.push(line.to_string());
        }
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    (joined, disabled)
}

/// Refuse to write through a symlink at `path` (so a planted symlink cannot
/// redirect a config write elsewhere).
fn refuse_through_symlink(path: &std::path::Path) -> Result<(), String> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "refusing to write through symlink {}; remove it first",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dist_rewrite_swaps_release_segment() {
        let txt = "100 slackware mirror official\n\
                   200 patches mirror/patches subtree immutable\n\
                   # http://ftp.cc.uoc.gr/slackware/slackware64-15.0/\n\
                   http://ftp.cc.uoc.gr/slackware/slackware64-15.0/";
        // 15.0 -> current: literal slackware64-15.0 URLs move; `mirror` keyword
        // lines (no segment) are untouched and just follow the mirrors file.
        let out = dist_rewrite_text(txt, "slackware64-15.0", "slackware64-current");
        assert!(out.contains("slackware64-current/"));
        assert!(!out.contains("slackware64-15.0"));
        assert!(out.contains("200 patches mirror/patches subtree immutable")); // unchanged
        // 15.0 -> 15.1 likewise.
        let out2 = dist_rewrite_text(
            "http://m/slackware64-15.0/patches/",
            "slackware64-15.0",
            "slackware64-15.1",
        );
        assert_eq!(out2, "http://m/slackware64-15.1/patches/");
    }

    #[test]
    fn dist_disables_only_nonmirror_repos() {
        let txt = "100 slackware mirror official\n\
                   200 patches mirror/patches subtree immutable\n\
                   60 alienbob https://slackware.nl/people/alien/sbrepos/15.0/x86_64\n\
                   #80 conraid https://slackers.it/repository/slackware64-15.0\n\
                   100 SBo _SBo\n";
        let (out, disabled) = comment_nonmirror_repos(txt);
        // only the active literal-URL repo (alienbob) is disabled
        assert_eq!(disabled.len(), 1);
        assert!(disabled[0].contains("alienbob"));
        assert!(out.contains("#60 alienbob https://"));
        // mirror / mirror-subtree / tag-priority / already-commented stay as-is
        assert!(out.contains("100 slackware mirror official"));
        assert!(out.contains("200 patches mirror/patches subtree immutable"));
        assert!(out.contains("100 SBo _SBo"));
        assert!(out.contains("#80 conraid")); // not double-commented
        assert!(!out.contains("##80 conraid"));
        assert!(out.ends_with('\n')); // trailing newline preserved
    }

    #[test]
    fn set_active_mirror_line_comments_and_appends() {
        // A typical mirrors file: one active line plus commented alternatives.
        let txt = "# choose one\n\
                   https://mirrors.slackware.com/slackware/slackware64-15.0/\n\
                   # https://other/slackware64-15.0/\n";
        let out = set_active_mirror_line(txt, "file:///mnt/iso");
        // the old active line is now commented out
        assert!(out.contains("# https://mirrors.slackware.com/slackware/slackware64-15.0/"));
        // the local mirror is the sole active (uncommented, non-blank) line
        let active: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert_eq!(active, vec!["file:///mnt/iso"]);
        // idempotent: applying again changes nothing
        assert_eq!(set_active_mirror_line(&out, "file:///mnt/iso"), out);
    }

    #[test]
    fn set_active_mirror_line_on_all_commented() {
        // r-tz's shipped template has every line commented: appending makes the
        // local mirror the only active line.
        let txt = "# a\n# b\n";
        let out = set_active_mirror_line(txt, "http://nas/slackware64-current");
        let active: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert_eq!(active, vec!["http://nas/slackware64-current"]);
    }
}
