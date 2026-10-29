//! `history` and `show-changelog`: the package change log and a repo's ChangeLog.

use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::Outcome;
use crate::{changelog, history, pkg, repo, ui};
use std::io::Write;

pub(crate) fn cmd_show_changelog(cfg: &Config, repo_name: Option<&str>) -> Result<Outcome, String> {
    // Which repo's ChangeLog: an explicitly named one, else the tracked
    // (official) repo as before.
    let r = match repo_name {
        Some(name) => cfg
            .repos
            .iter()
            .find(|r| r.name == name)
            .ok_or_else(|| format!("no repo named '{name}'"))?,
        None => changelog::changelog_repo(&cfg.repos).ok_or("no repo configured")?,
    };
    // Always fetch the ChangeLog fresh so the user sees current content (a
    // cached copy can be stale — `update` only refreshes the official one). The
    // official repo passes cache=false: its cached ChangeLog is the check-updates
    // baseline owned by `update`, and refreshing it here would desync that.
    // Non-official repos refresh their cached copy as an offline fallback. If the
    // fetch fails (offline), fall back to a cached copy when one exists.
    let text = match repo::fetch_changelog_text(r, &cfg.cache_dir, !r.official) {
        Ok((t, url)) => {
            // A subtree shares the distribution's ChangeLog, and some
            // publishers keep one ChangeLog above the per-release trees. When
            // the text did not come from the repo's own path, say where it did.
            if url != r.join_url(repo::CHANGELOG) {
                println!("{}", ui::dim(&format!("(from {url})")));
            }
            t
        }
        Err(e) => match changelog::cached_changelog(r, &cfg.cache_dir) {
            Some(t) => {
                println!("{}", ui::dim(&format!("(could not refresh, showing cached copy: {e})")));
                t
            }
            None => {
                println!("No ChangeLog available for '{}' ({e}).", r.name);
                return Ok(Outcome::NothingFound);
            }
        },
    };
    if text.trim().is_empty() {
        println!("ChangeLog for '{}' is empty.", r.name);
        return Ok(Outcome::NothingFound);
    }
    page_output(&text);
    Ok(Outcome::Ok)
}

/// Source label for a single package id, mirroring installed-attribution
/// precedence: official (empty tag) -> repo that serves the tag -> declared
/// tag-rule name -> the raw tag itself.
fn source_of(cfg: &Config, db: &PkgDb, pkg: &pkg::PkgId) -> String {
    let tag = pkg.build_tag();
    if tag.is_empty() {
        return cfg.official_repo_name().unwrap_or("official").to_string();
    }
    if let Some(r) = db.repo_for_tag(tag) {
        return r.to_string();
    }
    if let Some(tp) = cfg.tag_priorities.iter().find(|tp| tp.tag == tag) {
        return tp.name.clone();
    }
    tag.to_string()
}

pub(crate) fn cmd_history(
    cfg: &Config,
    name: Option<&str>,
    last: Option<usize>,
    since: Option<&str>,
    installed_only: bool,
    removed_only: bool,
    upgraded_only: bool,
) -> Result<Outcome, String> {
    let tl = history::collect(&cfg.adm_dir);
    let clock = &tl.clock;

    // --installed is a current-state view read straight from packages/, so it is
    // always complete regardless of how each package last changed (a package
    // whose last action was an upgrade is still listed here).
    let mut events: Vec<history::Event> = if installed_only {
        tl.current
            .iter()
            .map(|(pkg, when)| history::Event {
                when: *when,
                pkg: pkg.clone(),
                kind: history::EventKind::Installed { reinstall: false },
            })
            .collect()
    } else {
        tl.events
    };

    if let Some(n) = name {
        events.retain(|e| e.pkg.name == n);
    }
    if removed_only {
        events.retain(|e| {
            matches!(e.kind, history::EventKind::Removed | history::EventKind::Upgraded { .. })
        });
    }
    if upgraded_only {
        events.retain(|e| matches!(e.kind, history::EventKind::Upgraded { .. }));
    }
    if let Some(s) = since {
        events.retain(|e| clock.local_date(e.when).as_str() >= s);
    }
    events.sort_by(|a, b| b.when.cmp(&a.when)); // newest first
    if let Some(n) = last {
        events.truncate(n);
    }
    if events.is_empty() {
        println!("No matching package history.");
        return Ok(Outcome::NothingFound);
    }

    // Available-package DB resolves which repo serves a build tag (best-effort:
    // if metadata is missing the source falls back to the tag-rule name or tag).
    let (db, _missing) = PkgDb::load_available(cfg);
    page_output(&render_history(&events, clock, cfg, &db));
    Ok(Outcome::Ok)
}

fn render_history(
    events: &[history::Event],
    clock: &history::LocalClock,
    cfg: &Config,
    db: &PkgDb,
) -> String {
    use history::EventKind;
    let wn = events.iter().map(|e| e.pkg.name.len()).max().unwrap_or(7).max(7);
    let mut out = String::new();
    for e in events {
        let date = ui::dim(&clock.format(e.when));
        // `src_pkg` is the build whose source label the line carries. For an
        // upgrade that is the REPLACEMENT, not `e.pkg`: the old build is what
        // left, and labelling the line with it says a package moved to a repo
        // it actually moved away from (an _SBo build replaced by an official
        // one read as "[SBo]"). Every other kind describes a single build, so
        // `e.pkg` is already the right one.
        let (sym, label, detail, src_pkg) = match &e.kind {
            EventKind::Installed { reinstall } => {
                let mut d = format!("{}-{}", e.pkg.version, e.pkg.build);
                if *reinstall {
                    d.push_str(&ui::yellow(" (reinstall)"));
                }
                (ui::green("+"), "installed", d, &e.pkg)
            }
            EventKind::Removed => (
                ui::red("\u{2212}"),
                "removed",
                format!("{}-{}", e.pkg.version, e.pkg.build),
                &e.pkg,
            ),
            EventKind::Upgraded { to } => match to {
                // upgradepkg over the same id is a rebuild / reinstall in place,
                // not a version change — show it as such, not "X -> X".
                Some(p) if p.tag() == e.pkg.tag() => (
                    ui::cyan("\u{21BB}"),
                    "reinstalled",
                    format!("{}-{}", e.pkg.version, e.pkg.build),
                    p,
                ),
                Some(p) => (
                    ui::cyan("\u{2191}"),
                    "upgraded",
                    format!(
                        "{}-{} {} {}-{}",
                        e.pkg.version, e.pkg.build, ui::dim("\u{2192}"), p.version, p.build
                    ),
                    p,
                ),
                // The successor could not be identified, so there is no new
                // build to name: fall back to the old one rather than guess.
                None => (
                    ui::cyan("\u{2191}"),
                    "upgraded",
                    format!("{}-{} {} ?", e.pkg.version, e.pkg.build, ui::dim("\u{2192}")),
                    &e.pkg,
                ),
            },
        };
        out.push_str(&format!(
            "{date}  {sym} {label:<11}  {name}  {detail}  {src}\n",
            name = ui::white(&format!("{:<wn$}", e.pkg.name)),
            src = ui::dim(&format!("[{}]", source_of(cfg, db, src_pkg))),
        ));
    }
    out
}

/// Print text through a pager when stdout is a terminal, so long output (the
/// ChangeLog, history) opens at the top — newest first — and is scrollable and
/// quittable like slackpkg. Output that fits one screen is printed inline and
/// the pager exits immediately (`-F`); the alternate screen is not used so short
/// output stays visible and the terminal is left clean (`-X`); colours pass
/// through (`-R`). Falls back to a plain print when not a TTY (piped/redirected)
/// or when no pager is available.
fn page_output(text: &str) {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        let pager = std::env::var("PAGER").unwrap_or_else(|_| "less -FRX".to_string());
        let mut parts = pager.split_whitespace();
        if let Some(cmd) = parts.next() {
            let args: Vec<&str> = parts.collect();
            let mut command = std::process::Command::new(cmd);
            command.args(&args).stdin(std::process::Stdio::piped());
            // Give `less` the same sensible defaults even when invoked via a bare
            // `PAGER=less`, without overriding a LESS the user set themselves.
            if std::env::var_os("LESS").is_none() {
                command.env("LESS", "FRX");
            }
            if let Ok(mut child) = command.spawn() {
                if let Some(mut stdin) = child.stdin.take() {
                    // Feed the pager from a scoped thread so the main thread can
                    // wait on it immediately. A body larger than the pipe buffer
                    // (~64 KiB) would otherwise block write_all here while `less`
                    // sits paused for keypresses, and the main thread would never
                    // reach wait() — so `q` could not be handled cleanly. With the
                    // writer detached, `q` makes `less` exit, wait() returns, and
                    // the writer unblocks on EPIPE. A scoped thread borrows `text`
                    // directly (no copy, however large) and is always joined.
                    std::thread::scope(|s| {
                        s.spawn(move || {
                            let _ = stdin.write_all(text.as_bytes());
                            // stdin dropped here -> EOF for the pager
                        });
                        let _ = child.wait();
                    });
                } else {
                    let _ = child.wait();
                }
                return;
            }
        }
    }
    print!("{text}");
}
