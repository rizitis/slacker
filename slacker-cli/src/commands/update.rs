//! update: refresh repository metadata (and the stock-db with resolve-stock).

use crate::cli::Cli;
use crate::config::Config;
use crate::{
    hilite_keys, parse_selection, update_one_repo, warn_unverified_repos, OlderList, Outcome, UpdateOutcomes,
};
use crate::{changelog, config, download, gpg, repo, stockdb, ui};
use std::io::Write;

/// resolve-stock: download the stock dependency DB and report whether it is in
/// sync with -current. Never aborts — prints one visible line and returns.
fn refresh_stock_db(cfg: &Config) {
    let base = cfg.stock_db_url.trim_end_matches('/');
    if base.is_empty() {
        println!(
            "  {} {}",
            ui::yellow("!"),
            ui::yellow("resolve-stock is on but STOCK_DB_URL is unset \u{2014} cannot fetch stock-db")
        );
        return;
    }
    let bytes = match download::get_bytes(&format!("{base}/depgraph.db")) {
        Ok(b) => b,
        Err(e) => {
            println!(
                "  {} {}",
                ui::red("\u{2717}"),
                ui::red(&format!(
                    "stock-db download failed \u{2014} resolve-stock may be incomplete ({e})"
                ))
            );
            return;
        }
    };
    if let Err(e) = stockdb::validate_and_write(&bytes, &cfg.stock_db_path()) {
        println!("  {} {}", ui::red("\u{2717}"), ui::red(&format!("stock-db invalid \u{2014} {e}")));
        return;
    }
    // Freshness: stock-db README stamp vs the (now fresh) official ChangeLog head.
    let stamp = download::get_bytes(&format!("{base}/README.md"))
        .ok()
        .and_then(|b| stockdb::readme_stamp(&String::from_utf8_lossy(&b)));
    let head = changelog::changelog_repo(&cfg.repos)
        .and_then(|r| changelog::cached_changelog(r, &cfg.cache_dir))
        .and_then(|c| stockdb::changelog_head(&c));
    match (stamp, head) {
        (Some(s), Some(h)) if s == h => println!(
            "  {} {}",
            ui::green("\u{2713}"),
            ui::green("stock-db updated (in sync with -current)")
        ),
        (Some(_), Some(_)) => println!(
            "  {} {}",
            ui::yellow("!"),
            ui::yellow(
                "stock-db updated but behind -current \u{2014} resolve-stock deps may be incomplete"
            )
        ),
        _ => println!(
            "  {} {}",
            ui::green("\u{2713}"),
            ui::dim("stock-db updated (could not verify sync with -current)")
        ),
    }
}

pub(crate) fn cmd_update(cli: &Cli, cfg: &Config, mode: Option<&str>) -> Result<Outcome, String> {
    if mode == Some("gpg") {
        let mut newly = 0;
        for r in cfg.repos_by_priority() {
            if repo::is_quarantined(&cfg.state_dir, &r.name) {
                println!("{}", ui::dim(&format!("Skipping '{}' (quarantined).", r.name)));
                continue;
            }
            print!("Importing GPG key for '{}' ... ", r.name);
            std::io::stdout().flush().ok();
            match gpg::import_key(r, &cfg.state_dir) {
                Ok(gpg::ImportOutcome::NewlyPinned(fpr)) => {
                    newly += 1;
                    println!("{}", ui::green("pinned (first time)"));
                    println!("    {}", ui::white(&format!("fingerprint: {fpr}")));
                }
                Ok(gpg::ImportOutcome::AlreadyTrusted) => println!("{}", ui::dim("ok (already pinned)")),
                Err(e) => println!("{}", ui::red(&format!("skipped ({})", e.message()))),
            }
        }
        if newly > 0 {
            println!("\n{}", ui::yellow("A key was pinned for the first time."));
            println!("  {}", ui::dim("Verify each fingerprint above against the repository's officially published"));
            println!("  {}", ui::dim("key before trusting it — slacker will refuse the repo if its key ever changes."));
        }
        return Ok(Outcome::Ok);
    }

    // ---- check phase: see which repos actually changed, without touching the
    // cache (so unchanged repos keep their metadata, including the MANIFEST). ----
    let all_repos = cfg.repos_by_priority();

    // Grandfather: a repo that already has cached metadata was accepted before
    // this run (or by a prior slacker), so treat it as trusted. This keeps
    // existing working repos from being re-vetted or frozen by a transient
    // network failure on the first update; only genuinely new/unreachable repos
    // get vetted below.
    for r in &all_repos {
        if !repo::is_quarantined(&cfg.state_dir, &r.name)
            && !repo::is_trusted(&cfg.state_dir, &r.name)
            && repo::meta_path(r, &cfg.cache_dir, repo::PACKAGES_TXT).exists()
        {
            repo::mark_trusted(&cfg.state_dir, &r.name);
        }
    }

    for r in &all_repos {
        if repo::is_hard_quarantined(&cfg.state_dir, &r.name) {
            println!(
                "{}",
                ui::yellow(&format!(
                    "Skipping '{}' — frozen (run `slacker trust-repo {}` to use it, or `del-repo {}`).",
                    r.name, r.name, r.name
                ))
            );
        } else if repo::is_quarantined(&cfg.state_dir, &r.name) {
            // Soft (was unreachable): retried below, not skipped.
            println!(
                "{}",
                ui::dim(&format!("'{}' was unreachable before — retrying.", r.name))
            );
        }
    }
    // Hard-frozen repos are skipped; soft-frozen ones are kept so update retries
    // them (and they recover on their own if they come up clean).
    let repos: Vec<&config::Repo> = all_repos
        .into_iter()
        .filter(|r| !repo::is_hard_quarantined(&cfg.state_dir, &r.name))
        .collect();
    println!("Checking repositories for updates ...");
    let statuses: Vec<changelog::UpdateStatus> = repos
        .iter()
        .map(|r| changelog::check_repo_updates(r, &cfg.cache_dir, r.checks_packages_txt(&cfg.verify)))
        .collect();

    // An unreachable repo is still offered: the failure may be transient, and
    // attempting it surfaces the real error instead of silently skipping it.
    // Its label must not claim an update is coming, though.
    let needs = |s: &changelog::UpdateStatus| {
        matches!(
            s,
            changelog::UpdateStatus::Pending
                | changelog::UpdateStatus::Unknown
                | changelog::UpdateStatus::Unreachable
        )
    };
    let wname = repos.iter().map(|r| r.name.len()).chain(std::iter::once(10)).max().unwrap();

    println!(
        "  {}  {}  {}  {}",
        ui::blue(&format!("{:>2}", "#")),
        ui::blue(&format!("{:<wname$}", "Repo")),
        ui::blue(&format!("{:>4}", "Pri")),
        ui::blue("Status"),
    );
    println!("  {}", ui::dim(&"-".repeat(2 + 2 + wname + 2 + 4 + 2 + 17)));

    let mut needing: Vec<&config::Repo> = Vec::new();
    for (r, s) in repos.iter().zip(statuses.iter()) {
        let (num, status_txt) = if needs(s) {
            needing.push(*r);
            let txt = match s {
                changelog::UpdateStatus::Unknown => ui::yellow("new (will update)"),
                changelog::UpdateStatus::Unreachable => ui::red("unreachable (will retry)"),
                _ => ui::yellow("updates available"),
            };
            (ui::cyan(&format!("{:>2}", needing.len())), txt)
        } else {
            (ui::dim(&format!("{:>2}", "-")), ui::green("up-to-date"))
        };
        println!(
            "  {}  {}  {}  {}",
            num,
            ui::white(&format!("{:<wname$}", r.name)),
            ui::dim(&format!("{:>4}", r.priority)),
            status_txt,
        );
    }

    if needing.is_empty() {
        println!("\n{}", ui::green("All up-to-date — no news is good news."));
        fill_missing_manifests(cfg, &repos, &[]);
        // Even when no repo changed, the stock-db must still be checked/
        // fetched: in a container it may be missing or stale regardless of
        // repo state (repos up-to-date != stock-db present & in sync).
        if cfg.resolve_stock {
            refresh_stock_db(cfg);
        }
        warn_unverified_repos(cfg);
        return Ok(Outcome::Ok);
    }

    // ---- selection: update many, one, or none ----
    // --yes: non-interactive (CI / containers) — take them all, exactly like
    // pressing Enter at the prompt. Without this, the unconditional stdin read
    // below blocks forever in an interactive container where every repo is
    // fresh, and only *accidentally* proceeds when stdin is closed (CI EOF).
    let trimmed_owned: String;
    let trimmed: &str = if cli.yes {
        println!(
            "\n{}",
            ui::dim(&format!("--yes: updating all {} repo(s).", needing.len()))
        );
        ""
    } else {
        print!(
            "\n{} ",
            hilite_keys(&format!(
                "{} repo(s) to update. Update [a]ll / numbers (e.g. 1 2) / [n]one? [a]:",
                needing.len()
            ))
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        trimmed_owned = line.trim().to_string();
        &trimmed_owned
    };
    let chosen: Vec<&config::Repo> = match trimmed.to_lowercase().as_str() {
        "n" | "no" | "none" | "q" => {
            println!("Nothing updated.");
            return Ok(Outcome::Ok);
        }
        "" | "a" | "all" => needing.clone(),
        _ => {
            let sel = parse_selection(trimmed, needing.len());
            if sel.is_empty() {
                println!("Nothing selected.");
                return Ok(Outcome::Ok);
            }
            needing
                .iter()
                .enumerate()
                .filter(|(i, _)| sel.contains(&(i + 1)))
                .map(|(_, r)| *r)
                .collect()
        }
    };

    // ---- update phase: only the chosen repos (their MANIFEST is refreshed by
    // update_one_repo; every other repo is left untouched, except that one
    // with no MANIFEST in the cache gets it). ----
    let changelog_repo = changelog::changelog_repo(&cfg.repos).map(|r| r.name.clone());
    let mut out = UpdateOutcomes::default();
    println!();
    for r in &chosen {
        let track = changelog_repo.as_deref() == Some(r.name.as_str());
        update_one_repo(cfg, r, track, OlderList::Ask(cli.yes), &mut out);
    }
    let updated: Vec<&str> = chosen.iter().map(|r| r.name.as_str()).collect();
    fill_missing_manifests(cfg, &repos, &updated);

    if !out.failed_verify.is_empty() {
        println!(
            "\n{}",
            ui::red(&format!(
                "{} repo(s) failed verification and were skipped: {}.",
                out.failed_verify.len(),
                out.failed_verify.join(", ")
            ))
        );
        println!("  {}", ui::dim("If you trust one, set `verify=none` (or `verify=md5`) for it in the repos file,"));
        println!("  {}", ui::dim("or import its key with `slacker update gpg`, then run `slacker update` again."));
    }
    if !out.inconsistent.is_empty() {
        println!(
            "\n{}",
            ui::red(&format!(
                "{} repo(s) were NOT updated: {}.",
                out.inconsistent.len(),
                out.inconsistent.join(", ")
            ))
        );
        println!("  {}", ui::dim("The mirror served a PACKAGES.TXT that does not match its CHECKSUMS.md5."));
        println!("  {}", ui::dim("The mirror is mid-sync or serves a stale cached copy. Their previous metadata"));
        println!("  {}", ui::dim("was kept unchanged. Run `slacker update` again later, or choose another mirror."));
    }
    if !out.older.is_empty() {
        println!(
            "\n{}",
            ui::yellow(&format!(
                "{} repo(s) were NOT updated: the mirror's list is older than yours: {}.",
                out.older.len(),
                out.older.join(", ")
            ))
        );
        for l in [
            "Your newer metadata was kept. The mirror is behind, or you switched to an older one:",
            "run `slacker update` later or use another mirror (`slacker status` shows the official",
            "mirror's freshness). To take the older list on purpose, answer y when `slacker update`",
            "asks (without --yes).",
        ] {
            println!("  {}", ui::dim(l));
        }
    }
    if !out.hard_frozen.is_empty() {
        println!(
            "\n{}",
            ui::red(&format!(
                "{} repo(s) were FROZEN (unsafe — malicious metadata or bad signature): {}.",
                out.hard_frozen.len(),
                out.hard_frozen.join(", ")
            ))
        );
        println!("  {}", ui::dim("These will stay frozen until you run `slacker trust-repo NAME` (only if you are"));
        println!("  {}", ui::dim("sure), or `slacker del-repo NAME` to remove them."));
    }
    if !out.soft_frozen.is_empty() {
        println!(
            "\n{}",
            ui::yellow(&format!(
                "{} repo(s) were unreachable and frozen for now: {}.",
                out.soft_frozen.len(),
                out.soft_frozen.join(", ")
            ))
        );
        println!("  {}", ui::dim("The next `slacker update` retries them automatically; they recover on their own"));
        println!("  {}", ui::dim("once reachable. If one is gone for good, `slacker del-repo NAME`."));
    }
    if cfg.resolve_stock {
        refresh_stock_db(cfg);
    }
    warn_unverified_repos(cfg);
    Ok(Outcome::Ok)
}

/// An up-to-date repo is not fetched again, so one that has no MANIFEST in the
/// cache (from before `update` kept it, or a fetch that failed) gets it here,
/// checked against the CHECKSUMS.md5 already in the cache (verified when that
/// was fetched). Repos with a MANIFEST, frozen ones, and the ones in `done` are
/// left alone. Only a fetched or refused MANIFEST is printed: a repo that
/// serves none is tried again, quietly, on each update.
fn fill_missing_manifests(cfg: &Config, repos: &[&config::Repo], done: &[&str]) {
    for r in repos {
        if done.contains(&r.name.as_str())
            || repo::is_quarantined(&cfg.state_dir, &r.name)
            || !repo::meta_path(r, &cfg.cache_dir, repo::PACKAGES_TXT).exists()
            || repo::meta_path(r, &cfg.cache_dir, repo::MANIFEST).exists()
        {
            continue;
        }
        let what = match repo::refresh_manifest(r, &cfg.cache_dir, r.checks_packages_txt(&cfg.verify)) {
            repo::ManifestRefresh::Fetched => ui::green("ok"),
            repo::ManifestRefresh::Unchanged => continue,
            repo::ManifestRefresh::Missing => continue,
            repo::ManifestRefresh::Mismatch => ui::yellow("does not match the md5 in CHECKSUMS.md5: not kept"),
        };
        println!("  '{}': {} ... {}", r.name, repo::MANIFEST_BZ2, what);
    }
}
