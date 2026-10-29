//! `find-mirror`: probe the official mirror list and propose the fastest fresh mirror.

use crate::config::Config;
use crate::{
    humanize_lag, osuosl_tree_prefix, parse_packages_date, slackware_dir, Outcome,
    FRESHNESS_TIMEOUT_SECS,
};
use crate::{config, download, mirrors, ui};

/// Upstream reference PACKAGES.TXT (osuosl) for THIS system's release and arch,
/// used only to compare the header timestamp against the user's chosen official
/// mirror — no package data is ever trusted from it, only a timestamp is read,
/// so plain http is acceptable. None when the release can't be determined.
///
/// NOTE: on stable this points at the release's ROOT PACKAGES.TXT (the frozen
/// snapshot); security updates land under patches/, so a fully meaningful stable
/// freshness check would read patches/PACKAGES.TXT — a later refinement.
pub(crate) fn upstream_packages_url(arch: &str) -> Option<String> {
    slackware_dir(arch).map(|dir| format!("http://ftp.osuosl.org/pub/slackware/{dir}/PACKAGES.TXT"))
}

/// Hostname of a URL, for matching the user's current mirror among candidates.
fn host_of(url: &str) -> Option<String> {
    let after = url.split("://").nth(1)?;
    Some(after.split('/').next()?.to_string())
}

/// How many of the fastest mirrors `find-mirror` lists in the table.
const FIND_MIRROR_SHOW: usize = 7;
/// How many of the fastest mirrors `find-mirror` proposes as candidate lines.
const FIND_MIRROR_PROPOSE: usize = 3;

pub(crate) fn cmd_find_mirror(config_dir: &std::path::Path) -> Result<Outcome, String> {
    // Config is optional here — find-mirror is meant to run before you have set a
    // mirror. If it loads, its arch is used; otherwise x86_64 is assumed.
    let cfg = Config::load_dir(config_dir).ok();
    // Release + arch directory for THIS system (current, or a stable VERSION_ID;
    // slackware64 vs slackware for 32-bit). The probe path, the freshness
    // reference and the suggested line all key off it, so find-mirror now works
    // on -current AND stable alike.
    // find-mirror runs precisely when the mirror config is still broken, so the
    // full Config may not load. Fall back to standalone arch detection (NOT a
    // blind x86_64) — otherwise a 32-bit box gets a slackware64-current line.
    let detected = cfg
        .as_ref()
        .map(|c| c.arch.clone())
        .unwrap_or_else(|| config::system_arch(config_dir));
    let arch = detected.as_str();
    // The official mirror list and the osuosl reference carry x86 trees only.
    // On Slackware ARM, probing them would rank mirrors by another
    // architecture's tree and propose lines for it, so stop with directions
    // instead of a wrong answer.
    if osuosl_tree_prefix(arch).is_none() {
        return Err(format!(
            "find-mirror only knows the official Slackware mirror list, which has no {arch} trees — \
             for Slackware ARM pick a mirror yourself (project home: https://arm.slackware.com) \
             and make it the single active line in {}",
            config_dir.join("mirrors").display()
        ));
    }
    let dir = slackware_dir(arch).ok_or(
        "cannot determine your Slackware release/arch — check /etc/os-release and the `arch` line in slacker.conf",
    )?;
    let subpath = format!("{dir}/PACKAGES.TXT");
    println!("{}", ui::blue("Finding the fastest up-to-date Slackware mirror"));

    let candidates = mirrors::fetch_https_mirrors()?;
    if candidates.is_empty() {
        return Err("no https mirrors found in the mirror list — its format may have changed".into());
    }
    println!("  {}", ui::dim(&format!("probing {} https mirrors in parallel ...", candidates.len())));

    // Upstream reference for freshness; fail-open — if osuosl is unreachable we
    // still rank by speed, just without the freshness filter. Keep the raw line
    // too, so we can show upstream's own timestamp as proof of the check. The
    // release/arch are already resolved above (works on -current and stable).
    let upstream_line = upstream_packages_url(arch).and_then(|u| {
        download::first_line(&u, std::time::Duration::from_secs(FRESHNESS_TIMEOUT_SECS)).ok()
    });
    let upstream = upstream_line.as_deref().and_then(parse_packages_date);

    let probed = mirrors::probe_all(&candidates, &subpath);
    let reachable = probed.len();
    // Show the fastest few, fastest first; propose the top FIND_MIRROR_PROPOSE below.
    let ranked = mirrors::rank(probed, upstream, FIND_MIRROR_SHOW);
    if ranked.is_empty() {
        return Err(format!(
            "probed {} mirror(s) but none returned a usable PACKAGES.TXT",
            candidates.len()
        ));
    }

    // Where does the user's own official mirror sit, if it shows up?
    let current_host = cfg
        .as_ref()
        .and_then(|c| c.repos.iter().find(|r| r.official))
        .and_then(|r| host_of(&r.url));

    println!();
    if upstream.is_some() {
        let date = upstream_line
            .as_deref()
            .and_then(|l| l.split_once(';'))
            .map(|(_, d)| d.trim())
            .unwrap_or("?");
        println!(
            "  {} {}",
            ui::green("\u{2713}"),
            ui::dim(&format!("freshness validated against upstream osuosl ({date})"))
        );
    } else {
        println!(
            "  {}",
            ui::yellow("upstream unreachable — freshness unchecked, ranked by speed only")
        );
    }
    println!(
        "  {} {}",
        ui::blue(&format!("Top {} mirrors:", ranked.len())),
        ui::dim(&format!("({} of {} reachable)", reachable, candidates.len()))
    );

    for (i, m) in ranked.iter().enumerate() {
        let fresh = match upstream {
            Some(up) => {
                let lag = up - m.pkg_epoch;
                if lag <= 0 {
                    "in sync".to_string()
                } else {
                    format!("{} behind", humanize_lag(lag))
                }
            }
            None => "?".to_string(),
        };
        let yours = if host_of(&m.base_url).as_deref() == current_host.as_deref() {
            ui::green(" (yours)")
        } else {
            String::new()
        };
        println!(
            "  {} {} {} {} {}{}",
            ui::cyan(&format!("{:>2}.", i + 1)),
            ui::white(&format!("{:<3}", m.country)),
            ui::green(&format!("{:>7}", format!("{}ms", m.latency_ms))),
            ui::dim(&format!("{:<13}", fresh)),
            ui::cyan(&m.base_url),
            yours,
        );
    }

    // Suggestion — printed only, never written: slacker does not switch mirrors
    // for you. Each line is the one the `mirrors` file expects (the release
    // root for this arch).
    //
    // Only mirrors that are actually IN SYNC are offered. find-mirror promises
    // an up-to-date mirror, and `status` sends people here precisely because
    // theirs is behind, so proposing one that is itself behind defeats the
    // command — being under the 48h staleness cut makes a mirror worth listing
    // (with its lag, above), not worth switching to. Without an upstream
    // reference nothing can be judged, so there the fastest are offered as
    // before, unchecked.
    println!();
    let proposable = mirrors::proposable(&ranked, upstream);
    if proposable.is_empty() {
        println!(
            "{}",
            ui::yellow("None of these mirrors is in sync with upstream right now.")
        );
        println!(
            "  {}",
            ui::dim(
                "Keep the mirror you have and probe again later — the list above shows how far \
                 behind each one is."
            )
        );
        return Ok(Outcome::Ok);
    }
    let propose = FIND_MIRROR_PROPOSE.min(proposable.len());
    if propose == 1 {
        let best = proposable[0];
        println!(
            "{}",
            ui::green(&format!("Fastest: {} ({}ms)", best.base_url, best.latency_ms))
        );
        println!("  {}", ui::dim("to use it, make this the single active line in your `mirrors` file:"));
    } else {
        println!(
            "{}",
            ui::green(&format!(
                "Fastest {propose} — put ONE of these as the single active line in your `mirrors` file:"
            ))
        );
    }
    for m in proposable.iter().take(propose) {
        println!(
            "    {}",
            ui::white(&format!("{}/{}/", m.base_url.trim_end_matches('/'), dir))
        );
    }
    println!("  {}", ui::dim("(slacker will not change your mirror automatically)"));
    Ok(Outcome::Ok)
}
