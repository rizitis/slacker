//! `revert-pkg`: roll an official package back to a previous -current build.

use crate::cli::Cli;
use crate::config::Config;
use crate::commands::blacklist::cmd_frozen;
use crate::{
    confirm, hilite_keys, in_graphical_session, is_foundational, package_dest, Outcome,
};
use crate::{config, download, gpg, revert, system, ui};
use std::io::Write;

/// A big red warning banner shown before a dangerous operation on a foundational
/// package (used by revert-pkg). Adapts its advice to whether a graphical
/// session is running; the caller's confirmation still gates the action.
fn danger_banner_foundational(names: &[String]) {
    let bar = "═".repeat(70);
    let red = |s: &str| println!("{}", ui::red(s));
    println!();
    red(&format!("  {bar}"));
    red("   !  DANGER — reverting a foundational package");
    red(&format!("  {bar}"));
    println!();
    println!(
        "  {}",
        ui::white("glibc / openssl / icu4c / aaa* and other core libraries are linked by")
    );
    println!(
        "  {}",
        ui::white("nearly every running binary and service. Downgrading one can leave the")
    );
    println!(
        "  {}",
        ui::white("system unable to run programs — including your shell.")
    );
    println!();
    println!("  {} {}", ui::dim("Package(s):"), ui::white(&names.join(", ")));
    println!();
    if in_graphical_session() {
        red("   You are in a graphical session (startx / display manager).");
        red("   Log out and run this from a console (tty) before proceeding.");
    } else {
        red("   This is a dangerous operation. Proceed only if you are certain.");
    }
    println!("{}", ui::red(&format!("  {bar}")));
    println!();
}

/// Download a cumulative-archive package and its detached `.asc`, then require a
/// GOOD GPG signature from the pinned official Slackware key before returning.
/// md5 is intentionally not used here: the cumulative CHECKSUMS does not list
/// superseded versions, so the per-package GPG signature alone authenticates. A
/// missing, bad, or unpinned-key signature is fatal — nothing is installed.
///
/// `moved_hint` is appended to a failure to download the PACKAGE ITSELF (and to
/// nothing else): the archive listed the package, but this particular build is
/// no longer in the tree, which after a stable release usually means it moved
/// into the release archive. The hint names that location; acting on it is the
/// admin's decision, not slacker's.
fn revert_fetch_and_gpg_verify(
    cfg: &Config,
    official: &config::Repo,
    txz_url: &str,
    dest: &std::path::Path,
    moved_hint: Option<&str>,
) -> Result<(), String> {
    if let Ok(meta) = std::fs::symlink_metadata(dest) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "refusing to write through symlink {}; remove it first",
                dest.display()
            ));
        }
    }
    println!("  {}", ui::dim(&format!("fetching {txz_url}")));
    download::download_to(txz_url, dest).map_err(|e| match moved_hint {
        Some(h) => format!("{e}\n{h}"),
        None => e,
    })?;

    let asc_url = format!("{txz_url}.asc");
    let mut asc = dest.as_os_str().to_os_string();
    asc.push(".asc");
    let asc = std::path::PathBuf::from(asc);
    download::download_to(&asc_url, &asc)
        .map_err(|e| format!("fetch signature {asc_url}: {e}"))?;

    match gpg::verify_detached(official, &cfg.state_dir, dest, &asc)? {
        gpg::Verify::Good(signer) => {
            println!("  {}", ui::green(&format!("verified: gpg ({signer})")));
            Ok(())
        }
        gpg::Verify::Tampered(m) => Err(format!("{m} — refusing to install")),
        gpg::Verify::NoSignature => Err(format!(
            "no GPG signature for {} in the cumulative archive — refusing \
             (revert requires a valid Slackware signature)",
            dest.display()
        )),
        gpg::Verify::Unverifiable(m) => Err(format!("{m} — refusing to install")),
    }
}

/// `(release, archive URL)` of the release archive that holds what has left
/// -current, named from `/etc/os-release` and the configured archive URL, or
/// None when either cannot be determined (an unusual CUMULATIVE_URL, no
/// VERSION_ID). slackware.uk moves the whole -current tree into the release's
/// own archive when a release is made, so this is where a build from the
/// previous development cycle now lives.
///
/// It is only ever PRINTED. Switching to another archive is the admin's
/// decision, taken in slacker.conf, never slacker's at download time.
fn release_archive_url(cfg: &Config) -> Option<(String, String)> {
    let release = system::version_id()?;
    let url = revert::archive_url_for_release(&cfg.cumulative_url, &release)?;
    Some((release, url))
}

/// Roll an official package back to a previous -current version (rollback). See
/// the `RevertPkg` command doc for the user-facing description. Guards: the
/// feature switch, a -current-only check (fail-closed), and a -current archive
/// URL. Then: list previous official versions from removed-packages, pick one,
/// locate it in the cumulative archive, GPG-verify, downgrade, offer to freeze.
pub(crate) fn cmd_revert_pkg(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    // GUARD 1 — feature switch.
    if !cfg.revert_enabled {
        return Err("revert-pkg is disabled (set REVERT=on in slacker.conf to enable it)".into());
    }
    // GUARD 2 — -current only (fail-closed if the codename is missing/unknown).
    match system::version_codename().as_deref() {
        Some("current") => {}
        other => {
            return Err(format!(
                "revert-pkg works only on Slackware -current (VERSION_CODENAME=current).\n\
                 This system reports VERSION_CODENAME={}. Refusing: the cumulative archive holds \
                 -current packages, and mixing them into a stable release would break it.",
                other.unwrap_or("<unset>")
            ));
        }
    }
    // GUARD 3 — the configured archive must itself be a -current tree.
    if !cfg.cumulative_url.contains("-current") {
        return Err(format!(
            "CUMULATIVE_URL does not point at a -current archive: {}\n\
             Refusing, to avoid mixing a stable archive into a -current system.",
            cfg.cumulative_url
        ));
    }

    // Read removed-packages records, most-recently-removed first.
    let dir = cfg.adm_dir.join("removed_packages");
    let rd = std::fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut recs: Vec<(std::time::SystemTime, String)> = Vec::new();
    for ent in rd.flatten() {
        let fname = ent.file_name().to_string_lossy().into_owned();
        let mtime = ent
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        recs.push((mtime, fname));
    }
    recs.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    let names: Vec<&str> = recs.iter().map(|(_, n)| n.as_str()).collect();

    let candidates = revert::previous_official_versions(&names, name, 10);
    if candidates.is_empty() {
        println!(
            "No previous official versions of '{name}' found in {}.",
            dir.display()
        );
        println!(
            "  {}",
            ui::dim(
                "(revert covers official Slackware packages this system has upgraded; \
                 third-party packages are not in the cumulative archive)"
            )
        );
        return Ok(Outcome::NothingFound);
    }

    // The archive is consulted BEFORE anything is offered. slackware.uk rotates
    // it at every stable release (the -current tree is moved into the release's
    // own archive, the fresh release set is copied back in its place), so "this
    // archive can serve nothing older" is a normal state for a while after a
    // release. Learning that after someone has picked a version is the wrong
    // order, and the three failure shapes read very differently to a user.
    let release_archive = release_archive_url(cfg);
    let pkgs_url = format!("{}/PACKAGES.TXT", cfg.cumulative_url.trim_end_matches('/'));
    println!("  {}", ui::dim(&format!("fetching {pkgs_url}")));
    let bytes = download::get_bytes(&pkgs_url).map_err(|e| {
        let mut msg = format!(
            "fetch cumulative PACKAGES.TXT: {e}\n\
             (check CUMULATIVE_URL in slacker.conf: {})",
            cfg.cumulative_url
        );
        if let Some((rel, base)) = &release_archive {
            msg.push_str(&format!(
                "\n(packages from before the {rel} release are kept at {base})"
            ));
        }
        msg
    })?;
    let locations = match revert::classify_index(&String::from_utf8_lossy(&bytes)) {
        revert::ArchiveIndex::Packages(m) => m,
        revert::ArchiveIndex::Empty => {
            println!(
                "The cumulative archive lists no packages: {}",
                cfg.cumulative_url
            );
            println!(
                "  {}",
                ui::dim(
                    "(nothing to revert to yet — a new -current cycle starts with a fresh tree, \
                     and the previous cycle's packages move to the release archive)"
                )
            );
            if let Some((rel, base)) = &release_archive {
                println!(
                    "  {}",
                    ui::dim(&format!(
                        "packages from before the {rel} release are kept at {base} — \
                         point CUMULATIVE_URL there to reach them"
                    ))
                );
            }
            return Ok(Outcome::NothingFound);
        }
        revert::ArchiveIndex::NotIndex => {
            return Err(format!(
                "{pkgs_url} did not return a package list — check CUMULATIVE_URL in slacker.conf"
            ));
        }
    };

    // Same reasoning: if the archive does not carry this package at all, say so
    // now rather than after a version has been chosen.
    if !locations.contains_key(name) {
        return Err(format!(
            "could not locate '{name}' in the cumulative archive (not in its PACKAGES.TXT — it may \
             have left -current, or live under extra/ or patches/, which revert does not cover)"
        ));
    }

    // Choose the target version: with -y the most recent previous, else interactive.
    println!(
        "{}",
        ui::blue(&format!(
            "Previous official versions of '{name}' available to revert to:"
        ))
    );
    for (i, id) in candidates.iter().enumerate() {
        println!(
            "  {}) {}",
            ui::dim(&format!("{:>3}", i + 1)),
            ui::white(&id.tag())
        );
    }
    let target = if cli.yes {
        println!(
            "  {}",
            ui::dim("(-y: selecting the most recent previous version)")
        );
        &candidates[0]
    } else {
        print!(
            "{} ",
            hilite_keys("Enter a number to revert to (or [n] to cancel):")
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return Ok(Outcome::Ok);
        }
        let t = line.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("n") {
            println!("Cancelled.");
            return Ok(Outcome::Ok);
        }
        match t.parse::<usize>() {
            Ok(n) if (1..=candidates.len()).contains(&n) => &candidates[n - 1],
            _ => return Err(format!("invalid selection {t:?}")),
        }
    };

    // Danger gate: reverting a foundational package (glibc/openssl/icu4c/aaa*)
    // gets a big banner and an extra confirmation, adapted to graphical/tty.
    if is_foundational(name) {
        danger_banner_foundational(&[name.to_string()]);
        if !cli.dry_run
            && !confirm(
                &format!("Revert the foundational package '{name}' anyway?"),
                cli.yes,
            )
        {
            println!("Cancelled.");
            return Ok(Outcome::Ok);
        }
    }

    // Locate it in the cumulative archive (series from the archive's PACKAGES.TXT,
    // fetched and classified above).
    let url =
        revert::cumulative_url_for(&cfg.cumulative_url, &locations, target).ok_or_else(|| {
            format!(
                "could not locate '{}' in the cumulative archive (not in its PACKAGES.TXT — it may \
                 have left -current, or live under extra/ or patches/, which revert does not cover)",
                target.name
            )
        })?;

    if cli.dry_run {
        println!("{} {}", ui::green("would revert to"), ui::white(&target.tag()));
        println!("  {} {url}", ui::dim("from"));
        println!("(dry-run: nothing downloaded or installed)");
        return Ok(Outcome::Ok);
    }

    // Download + GPG-verify against the pinned official Slackware key.
    let official = cfg.repos.iter().find(|r| r.official).ok_or(
        "no official repo is configured — needed to verify the cumulative package's Slackware signature",
    )?;
    let dest = package_dest(cfg, &official.name, &format!("{}.txz", target.tag()))?;

    // Where this build would be if it has already left -current: same relative
    // path, in the archive of the release named by /etc/os-release. Only shown
    // if the download fails; slacker does not switch archives by itself.
    let moved_hint = release_archive
        .as_ref()
        .and_then(|(rel, base)| {
            revert::cumulative_url_for(base, &locations, target).map(|moved| {
                format!(
                    "  this build may have left -current at the {rel} release; the archive keeps \
                     moved packages here:\n    {moved}\n  \
                     to revert to it, point CUMULATIVE_URL at {base} in slacker.conf"
                )
            })
        });
    revert_fetch_and_gpg_verify(cfg, official, &url, &dest, moved_hint.as_deref())?;

    // Downgrade.
    println!("{} {}", ui::blue("Reverting to"), ui::white(&target.tag()));
    system::reinstall(&dest)?;

    // Offer to freeze so a later upgrade-all won't pull it forward again.
    println!();
    if confirm(
        &format!("Freeze '{name}' now so upgrade-all won't upgrade it again?"),
        cli.yes,
    ) {
        cmd_frozen(cli, cfg, &[name.to_string()])?;
    } else {
        println!(
            "  {}",
            ui::dim("Not frozen — a later upgrade-all may pull this package forward again.")
        );
    }
    Ok(Outcome::Ok)
}
