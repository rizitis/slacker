//! slacker — a minimal Slackware package manager with full slackpkg parity,
//! combined with slackpkg+ multi-repo priority resolution.

mod banner;
mod banner2;
mod changelog;
mod config;
mod dist;
mod download;
mod gpg;
mod history;
mod manifest;
mod mirrors;
mod newconfig;
mod pkg;
mod pkgdb;
mod repo;
mod revert;
mod system;
mod template;
mod ui;

use clap::{Parser, Subcommand};
use config::Config;
use pkgdb::PkgDb;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// slackpkg-compatible exit statuses.
enum Outcome {
    Ok,            // 0
    NothingFound,  // 20
    SelfUpgrade,   // 50
    Pending,       // 100
}

/// Help colouring that matches slacker's own palette: blue section headers and
/// usage, cyan command/flag literals, white placeholders. clap only emits these
/// on a real TTY (and honours NO_COLOR), the same rule the rest of the tool uses.
const HELP_STYLES: clap::builder::Styles = clap::builder::Styles::styled()
    .header(clap::builder::styling::AnsiColor::Blue.on_default().bold())
    .usage(clap::builder::styling::AnsiColor::Blue.on_default().bold())
    .literal(clap::builder::styling::AnsiColor::Cyan.on_default())
    .placeholder(clap::builder::styling::AnsiColor::White.on_default());

#[derive(Parser)]
#[command(
    name = "slacker",
    version,
    about = "slackpkg + slackpkg+ in one, minimal Rust tool",
    styles = HELP_STYLES
)]
struct Cli {
    /// Directory holding the plain-text config files.
    #[arg(long, global = true, default_value = "/etc/slacker")]
    config_dir: PathBuf,

    /// Assume "yes" to confirmation prompts.
    #[arg(short = 'y', long, global = true)]
    yes: bool,

    /// Show what would happen without changing the system.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Do not read .dep files / pull in dependencies for this run.
    #[arg(long, global = true)]
    no_deps: bool,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Refresh metadata from every repo. `update gpg` imports repo GPG keys.
    Update { mode: Option<String> },
    /// Find a package by its exact name (case-insensitive).
    Search { pattern: String },
    /// Find which package ships a file (uses MANIFEST).
    FileSearch { filename: String },
    /// Show details and per-repo candidates for a package.
    Info { name: String },
    /// List configured repositories with priority, verify policy and how many
    /// installed packages came from each.
    ListRepos,
    /// Health-check the whole setup and report what to do next — the setup doctor.
    ///
    /// Runs even when the configuration is broken, since diagnosing that is its
    /// job. Checks the environment (pkgtools and helper tools on PATH, config-file
    /// syntax), then mirror, repos, priorities, verification, GPG keys, metadata
    /// freshness, blacklist, the package admin dir, installed-package sources,
    /// pending `.new` configs, and (if online) reachability and updates.
    Status,
    /// Install new packages (refuses already-installed ones).
    Install { patterns: Vec<String> },
    /// Upgrade installed packages to the newest available revision.
    Upgrade { patterns: Vec<String> },
    /// Reinstall the currently installed version.
    Reinstall { patterns: Vec<String> },
    /// Remove installed packages.
    Remove { patterns: Vec<String> },
    /// Revert an official package to a previous -current version (rollback).
    ///
    /// Lists the package's earlier versions recorded in the system's
    /// removed-packages, lets you pick one, then fetches that exact build from
    /// the cumulative -current archive (GPG-verified against the pinned Slackware
    /// key) and downgrades to it with `upgradepkg --reinstall`. Official packages
    /// only (the archive does not carry third-party repos), and only on -current.
    RevertPkg { name: String },
    /// Download package files without installing. Saved to the cache by
    /// default, or to a directory given with -o/--output.
    Download {
        patterns: Vec<String>,
        /// Directory to save into (default: the slacker package cache).
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Upgrade every installed package that has a newer revision.
    UpgradeAll,
    /// Distribution upgrade: migrate a Slackware 15.0 system to -current (or to
    /// the next stable). UNLIKE every other command it deliberately ignores
    /// source priority and the blacklist and takes the target distribution's
    /// version of every package.
    ///
    /// TARGET is where you are going, not where you are: `current` for the
    /// rolling release, or a newer stable version like `15.1`. The running
    /// release is read from /etc/os-release; the target comes from this argument
    /// (not your mirror), and upgrade-dist re-points the mirror and repos itself.
    ///
    /// Allowed directions only: 15.0 -> -current, and 15.0 -> a newer stable.
    /// Every other direction (running on -current, going backward, an unknown
    /// release) is refused. After a typed point-of-no-return confirmation it
    /// writes an escape-kit template, re-points the mirror and every mirror/
    /// subtree repo, comments out un-re-pointable third-party repos, empties the
    /// blacklist, then upgrades every installed package to the target (core
    /// first, the GnuPG chain last), runs install-new + clean-system + a second
    /// pass, and ends with a kernel/boot reminder. --dry-run shows it all and
    /// changes nothing; --yes runs non-interactively.
    UpgradeDist {
        /// Where to upgrade TO: `current` or a newer stable version (e.g. `15.1`).
        target: String,
    },
    /// Install every package the selected repos provide that isn't already
    /// installed — the "fill what's missing" counterpart to `install @repo`:
    /// catches packages new to the distribution and ones you removed, correct
    /// across any number of updates. Honours source/tag priority and the blacklist
    /// (frozen packages are skipped); a newer build/version of an installed package
    /// is an upgrade (use `upgrade-all`), not new. Default: official repo(s) only;
    /// name repos to use those instead.
    InstallNew { repos: Vec<String> },
    /// Remove installed packages no longer in the official baseline — the
    /// official repo plus any `immutable` repo. slackpkg-style: a package the
    /// distribution dropped is removed even if a third-party repo still ships
    /// the name. Kept by the blacklist, an `IGNORE_TAGS` build tag, or an
    /// immutable repo.
    CleanSystem,
    /// Delete downloaded package files from the cache. Repo metadata and GPG
    /// keys are never touched. Optionally limit to named repos.
    CleanCache { repos: Vec<String> },
    /// Handle leftover *.new configuration files.
    NewConfig,
    /// Check every configured repo for pending updates (exit 100 if any).
    CheckUpdates,
    /// Print a repo's cached ChangeLog. With no argument, the official (tracked)
    /// repo; name a repo to fetch and show that one instead.
    ShowChangelog { repo: Option<String> },
    /// Show a chronological log of package changes — installed, upgraded and
    /// removed, and when — newest first. Derived from the pkgtools admin
    /// directories, so it also reflects changes made outside slacker.
    History {
        /// Limit to a single package name.
        name: Option<String>,
        /// Show only the most recent N events.
        #[arg(long)]
        last: Option<usize>,
        /// Show only events on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// Show only packages currently installed (with their install date).
        #[arg(long)]
        installed: bool,
        /// Show only packages that left the system (removed or upgraded away).
        #[arg(long)]
        removed: bool,
        /// Show only upgrade / reinstall events.
        #[arg(long)]
        upgraded: bool,
    },
    /// Find the fastest up-to-date Slackware mirror for your location: probes the
    /// official mirror list over HTTP and ranks reachable, fresh mirrors by speed,
    /// lists the 7 fastest and proposes the 3 fastest. Works on -current and stable. Does not change
    /// your configuration.
    FindMirror,
    /// Snapshot installed packages into a template.
    GenerateTemplate { name: String },
    /// Install all packages listed in a template.
    InstallTemplate { name: String },
    /// Remove all packages listed in a template.
    RemoveTemplate { name: String },
    /// Delete a template file (does not touch installed packages).
    DeleteTemplate { name: String },
    /// Add one or more blacklist rules ("freeze"). Each argument is one rule:
    /// a glob (`*`, `?`) or a regex, a `series/`, or `@repo PATTERN` (quote rules
    /// with spaces). Run with no argument to list the current frozen rules.
    Frozen { names: Vec<String> },
    /// Remove one or more blacklist rules ("unfreeze"). Each argument must match
    /// an existing rule EXACTLY, as a literal string — special characters like
    /// `.*`, `*`, `-` or `/` are compared verbatim, never as a pattern. Run with
    /// no argument to list the current rules. Quote rules with spaces.
    Unfrozen { names: Vec<String> },
    /// Pin a package to ONE repo, regardless of priority: `pin repo:package`
    /// (e.g. `pin alienbob:vlc`). From then on every command (incl. upgrade-all)
    /// sources that package only from that repo. Stored in the `blacklist` file
    /// as `@repo 100% package`. A freeze on the same package always wins. Run
    /// with no argument to list the current pins.
    Pin { spec: Option<String> },
    /// Remove a pin: `unpin package` (e.g. `unpin vlc`). The package returns to
    /// normal priority-based resolution. Run with no argument to list the
    /// current pins.
    Unpin { names: Vec<String> },
    /// Add a binary repository to the `repos` file:
    /// `add-repo PRIORITY NAME URL [official] [immutable] [subtree] [verify=...]`.
    /// URL must be http:// or https:// and unique. Separate words, no quotes
    /// (quote only a URL that contains shell-special characters). `immutable`
    /// keeps every package attributed to the repo out of clean-system.
    /// `subtree` marks a Slackware distribution subtree (extra/, patches/, ...)
    /// whose packages download from the parent (root) URL.
    AddRepo {
        priority: String,
        name: String,
        url: String,
        /// Optional flags: `official`, `immutable`, `subtree`, and/or
        /// `verify=gpg,md5,...`.
        flags: Vec<String>,
    },
    /// Remove a binary repository (by name) from the `repos` file.
    DelRepo { name: String },
    /// Change a repo's priority in the `repos` file: `pri-repo PRIORITY NAME`.
    /// Refuses if NAME is not an active repo (suggesting the closest match), or
    /// if PRIORITY is already used by another repo (priorities must be distinct).
    PriRepo { priority: String, name: String },
    /// Add a build-tag priority line to the `repos` file:
    /// `add-tag PRIORITY NAME TAG` (e.g. `add-tag 100 SBo _SBo`; no quotes).
    AddTag { priority: String, name: String, tag: String },
    /// Remove a build-tag priority line (by its TAG) from the `repos` file.
    DelTag { tag: String },
    /// Re-run the safety vetting on a repo on demand (fetches metadata only).
    /// Quarantines it if it fails, or clears a prior quarantine if it now passes.
    VetRepo { name: String },
    /// Trust a quarantined repo, lifting its freeze so it can be used again.
    /// This overrides slacker's safety verdict — at your own responsibility.
    TrustRepo { name: String },
    /// Manually quarantine (freeze) a repo so it provides no packages until you
    /// `trust-repo` it again.
    DistrustRepo { name: String },
}

/// Restore the default SIGPIPE disposition (SIG_DFL) on Unix. Rust sets SIGPIPE
/// to SIG_IGN at startup, which turns a closed downstream pipe into an EPIPE that
/// `println!` then PANICS on ("failed printing to stdout") — e.g.
/// `slacker install x --dry-run | head` aborts with a panic once `head` exits.
/// Resetting to SIG_DFL makes the process terminate quietly on a broken pipe,
/// the normal Unix behaviour. Done with a raw libc `signal` call so no extra
/// crate dependency is pulled in (libc is always linked).
#[cfg(unix)]
fn restore_default_sigpipe() {
    extern "C" {
        fn signal(signum: core::ffi::c_int, handler: usize) -> usize;
    }
    const SIGPIPE: core::ffi::c_int = 13; // Linux/glibc
    const SIG_DFL: usize = 0;
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_default_sigpipe() {}

fn main() -> ExitCode {
    restore_default_sigpipe();
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => return clap_error_exit(e),
    };
    match run(&cli) {
        Ok(Outcome::Ok) => ExitCode::SUCCESS,
        Ok(Outcome::NothingFound) => ExitCode::from(20),
        Ok(Outcome::SelfUpgrade) => ExitCode::from(50),
        Ok(Outcome::Pending) => ExitCode::from(100),
        Err(e) => {
            eprintln!("slacker: {} {e}", ui::err_label());
            ExitCode::FAILURE
        }
    }
}

/// Show a clap parse error. A single-argument command (`history`, `search`,
/// `info`, `file-search`, `revert-pkg`, …) cannot accept the flood of filenames
/// the shell produces from a bare `*`, so clap rejects it with a terse
/// "unexpected argument"; a bare `slacker *` (no subcommand) has its first
/// expanded filename taken as the subcommand, giving "unrecognized subcommand".
/// In both cases, detect the shell-expanded flood and print the same friendly
/// explanation the install/remove path uses; the command never ran, so nothing
/// changed. Any other parse error defers to clap's own output.
fn clap_error_exit(e: clap::Error) -> ExitCode {
    use clap::error::ErrorKind;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let glob_flood = matches!(
        e.kind(),
        ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand
    ) && looks_shell_expanded(&args);
    if glob_flood {
        eprintln!(
            "{}",
            ui::yellow(&format!(
                "note: your shell expanded a glob into {} arguments before slacker ran.",
                args.len()
            ))
        );
        eprintln!(
            "{}",
            ui::dim("      slacker has no `*` wildcard — quote the pattern, or use `@repo` where a whole repo is allowed. Nothing ran.")
        );
        return ExitCode::from(2);
    }
    e.exit() // clap prints its own usage/help and exits
}

fn run(cli: &Cli) -> Result<Outcome, String> {
    // Banner No. 2 (the labyrinth masthead) heads every command, on a terminal
    // only (show() no-ops when stdout is piped, so scripts/pipes stay clean).
    banner2::show();
    // `status` must work even when the configuration is broken — diagnosing
    // exactly that is its job — so it loads resiliently on its own and is
    // dispatched before the strict load below (which aborts on those problems).
    if matches!(cli.command, Cmd::Status) {
        return cmd_status(&cli.config_dir);
    }
    // find-mirror helps you pick your FIRST mirror, so it must run before a mirror
    // is configured. Like status, it is dispatched before the strict load below; it
    // reads the config only opportunistically (for the "(yours)" marker) and never
    // requires a valid one.
    if matches!(cli.command, Cmd::FindMirror) {
        return cmd_find_mirror(&cli.config_dir);
    }
    let cfg = Config::load_dir(&cli.config_dir)?;
    migrate_state(&cfg);
    let privileged = requires_privilege(&cli.command);
    if privileged {
        ensure_privileged(&cli.command)?;
    }
    // Mutating commands take an exclusive lock so two cannot run at once and
    // corrupt the cache or the package database. The lock is an flock() held by
    // the kernel: it is released automatically when this process exits, even on
    // crash or kill -9, so a dead slacker never locks you out. Queries take no
    // lock and run freely in parallel.
    let _lock = if privileged {
        Some(acquire_lock()?)
    } else {
        None
    };
    match &cli.command {
        Cmd::Update { mode } => cmd_update(&cfg, mode.as_deref()),
        Cmd::Search { pattern } => cmd_search(&cfg, pattern),
        Cmd::FileSearch { filename } => cmd_file_search(&cfg, filename),
        Cmd::Info { name } => cmd_info(&cfg, name),
        Cmd::ListRepos => cmd_list_repos(&cfg),
        Cmd::Status => unreachable!("status is dispatched before config load"),
        Cmd::Install { patterns } => cmd_install(cli, &cfg, patterns),
        Cmd::Upgrade { patterns } => cmd_upgrade(cli, &cfg, patterns),
        Cmd::Reinstall { patterns } => cmd_reinstall(cli, &cfg, patterns),
        Cmd::Remove { patterns } => cmd_remove(cli, &cfg, patterns),
        Cmd::RevertPkg { name } => cmd_revert_pkg(cli, &cfg, name),
        Cmd::Download { patterns, output } => cmd_download(cli, &cfg, patterns, output.as_deref()),
        Cmd::UpgradeAll => cmd_upgrade_all(cli, &cfg),
        Cmd::UpgradeDist { target } => cmd_upgrade_dist(cli, &cfg, target),
        Cmd::InstallNew { repos } => cmd_install_new(cli, &cfg, repos),
        Cmd::CleanSystem => cmd_clean_system(cli, &cfg),
        Cmd::CleanCache { repos } => cmd_clean_cache(cli, &cfg, repos),
        Cmd::NewConfig => cmd_new_config(cli),
        Cmd::CheckUpdates => cmd_check_updates(&cfg),
        Cmd::ShowChangelog { repo } => cmd_show_changelog(&cfg, repo.as_deref()),
        Cmd::FindMirror => unreachable!("find-mirror is dispatched before config load"),
        Cmd::History { name, last, since, installed, removed, upgraded } => {
            cmd_history(&cfg, name.as_deref(), *last, since.as_deref(), *installed, *removed, *upgraded)
        }
        Cmd::GenerateTemplate { name } => cmd_generate_template(&cfg, name),
        Cmd::InstallTemplate { name } => cmd_install_template(cli, &cfg, name),
        Cmd::RemoveTemplate { name } => cmd_remove_template(cli, &cfg, name),
        Cmd::DeleteTemplate { name } => cmd_delete_template(cli, &cfg, name),
        Cmd::Frozen { names } => cmd_frozen(&cli, &cfg, names),
        Cmd::Unfrozen { names } => cmd_unfrozen(&cli, &cfg, names),
        Cmd::Pin { spec } => cmd_pin(&cli, &cfg, spec.as_deref()),
        Cmd::Unpin { names } => cmd_unpin(&cli, &cfg, names),
        Cmd::AddRepo { priority, name, url, flags } => {
            cmd_add_repo(cli, &cfg, priority, name, url, flags)
        }
        Cmd::DelRepo { name } => cmd_del_repo(cli, &cfg, name),
        Cmd::PriRepo { priority, name } => cmd_pri_repo(cli, &cfg, priority, name),
        Cmd::AddTag { priority, name, tag } => cmd_add_tag(cli, &cfg, priority, name, tag),
        Cmd::DelTag { tag } => cmd_del_tag(cli, &cfg, tag),
        Cmd::VetRepo { name } => cmd_vet_repo(&cfg, name),
        Cmd::TrustRepo { name } => cmd_trust_repo(cli, &cfg, name),
        Cmd::DistrustRepo { name } => cmd_distrust_repo(cli, &cfg, name),
    }
}

// ---- helpers -------------------------------------------------------------

/// Holds the open lock file. Dropping it closes the fd, which releases the
/// flock; the kernel also releases it automatically if the process dies.
struct Lock {
    _file: std::fs::File,
}

/// Take an exclusive, non-blocking flock on /run/slacker.lock. If another
/// slacker holds it, fail fast with its PID. The lock lives only as long as the
/// holding process: a crash or kill never leaves a stale lock behind (the file
/// may remain, but it carries no lock without a live owner).
fn acquire_lock() -> Result<Lock, String> {
    use std::io::Write as _;
    use std::os::unix::io::AsRawFd;

    const LOCK_PATH: &str = "/run/slacker.lock";
    const LOCK_EX: i32 = 2; // exclusive
    const LOCK_NB: i32 = 4; // non-blocking
    extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(LOCK_PATH)
        .map_err(|e| format!("cannot open lock file {LOCK_PATH}: {e}"))?;

    let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if rc != 0 {
        let who = std::fs::read_to_string(LOCK_PATH)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .map(|p| format!(" (PID {p})"))
            .unwrap_or_default();
        return Err(format!(
            "another slacker is already running{who}; wait for it to finish and try again"
        ));
    }

    // We hold the lock: record our PID for the message the *next* caller sees.
    let mut f = &file;
    let _ = f.set_len(0);
    let _ = write!(f, "{}", std::process::id());
    let _ = f.flush();

    Ok(Lock { _file: file })
}

/// Commands that write to root-owned locations (the package database under
/// /var/lib/pkgtools, the cache under /var/cache/slacker, or config under
/// /etc/slacker) and therefore need root. Pure queries are free for anyone.
fn requires_privilege(cmd: &Cmd) -> bool {
    match cmd {
        // read-only: search the metadata, print info, no writes to root dirs
        Cmd::Search { .. }
        | Cmd::Info { .. }
        | Cmd::ListRepos
        | Cmd::Status
        | Cmd::FileSearch { .. }
        | Cmd::CheckUpdates
        | Cmd::ShowChangelog { .. }
        | Cmd::History { .. }
        | Cmd::FindMirror => false,
        // everything else writes to a root-owned location
        _ => true,
    }
}

/// The user's numeric uid, via `id -u` (no extra crate needed).
fn current_uid() -> Option<u32> {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
}

/// Whether the current user belongs to the `wheel` group, via `id -nG`.
fn in_wheel() -> bool {
    std::process::Command::new("id")
        .arg("-nG")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.split_whitespace().any(|g| g == "wheel"))
        .unwrap_or(false)
}

/// Stop, with a clear message, if a privileged command is run without root.
/// Only uid 0 can actually write to the root-owned directories; wheel
/// membership just tailors the hint (you can use sudo) versus (ask an admin).
fn ensure_privileged(cmd: &Cmd) -> Result<(), String> {
    if current_uid() == Some(0) {
        return Ok(());
    }
    let name = command_name(cmd);
    let hint = if in_wheel() {
        format!("run it with: sudo slacker {name} ...")
    } else {
        "you are not in the 'wheel' group; ask a system administrator".to_string()
    };
    Err(format!(
        "'{name}' modifies the system or cache and must be run as root — {hint}"
    ))
}

/// Short command name for messages.
fn command_name(cmd: &Cmd) -> &'static str {
    match cmd {
        Cmd::Update { .. } => "update",
        Cmd::Search { .. } => "search",
        Cmd::FileSearch { .. } => "file-search",
        Cmd::Info { .. } => "info",
        Cmd::ListRepos => "list-repos",
        Cmd::Status => "status",
        Cmd::Install { .. } => "install",
        Cmd::Upgrade { .. } => "upgrade",
        Cmd::Reinstall { .. } => "reinstall",
        Cmd::Remove { .. } => "remove",
        Cmd::RevertPkg { .. } => "revert-pkg",
        Cmd::Download { .. } => "download",
        Cmd::UpgradeAll => "upgrade-all",
        Cmd::UpgradeDist { .. } => "upgrade-dist",
        Cmd::InstallNew { .. } => "install-new",
        Cmd::CleanSystem => "clean-system",
        Cmd::CleanCache { .. } => "clean-cache",
        Cmd::NewConfig => "new-config",
        Cmd::CheckUpdates => "check-updates",
        Cmd::ShowChangelog { .. } => "show-changelog",
        Cmd::History { .. } => "history",
        Cmd::FindMirror => "find-mirror",
        Cmd::GenerateTemplate { .. } => "generate-template",
        Cmd::InstallTemplate { .. } => "install-template",
        Cmd::RemoveTemplate { .. } => "remove-template",
        Cmd::DeleteTemplate { .. } => "delete-template",
        Cmd::Frozen { .. } => "frozen",
        Cmd::Unfrozen { .. } => "unfrozen",
        Cmd::Pin { .. } => "pin",
        Cmd::Unpin { .. } => "unpin",
        Cmd::AddRepo { .. } => "add-repo",
        Cmd::DelRepo { .. } => "del-repo",
        Cmd::PriRepo { .. } => "pri-repo",
        Cmd::AddTag { .. } => "add-tag",
        Cmd::DelTag { .. } => "del-tag",
        Cmd::VetRepo { .. } => "vet-repo",
        Cmd::TrustRepo { .. } => "trust-repo",
        Cmd::DistrustRepo { .. } => "distrust-repo",
    }
}

fn confirm(prompt: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true;
    }
    print!(
        "{} {}{}{}{}{} ",
        ui::blue(prompt),
        ui::blue("["),
        ui::white("y"),
        ui::blue("/"),
        ui::white("N"),
        ui::blue("]")
    );
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    // Only an explicit yes proceeds; Enter (the capital-N default) or anything
    // else cancels. Callers that want a cancellation message print their own.
    matches!(line.trim(), "y" | "Y" | "yes")
}

/// What pkgtool action a planned package needs.
#[derive(Clone, Copy, PartialEq)]
enum InstallAction {
    Install,
    Upgrade,
    Reinstall,
}

/// One package in the resolved install plan (dependencies come before the
/// packages that need them).
struct PlanItem {
    pkg: repo::AvailPkg,
    action: InstallAction,
    /// For a pulled-in dependency, the name of the package that needs it.
    dep_for: Option<String>,
    /// For an upgrade/reinstall of an installed package, the version-arch-build
    /// currently installed, so the plan can show the `old -> new` transition.
    from: Option<String>,
}

enum DepChoice {
    Skip,
    Replace,
    SkipAll,
    Abort,
}

/// A dependency that is already installed from a source of higher-or-equal
/// priority than the repo pulling it in. By the priority rule it is kept; we
/// record it so the user can be shown the choice rather than keeping it
/// silently.
struct ProtectedDep {
    dep: String,
    needed_by: String,
    installed: pkg::PkgId,
    offered: repo::AvailPkg,
}

enum KeepChoice {
    Keep,
    Replace,
    KeepAll,
    Quit,
}

/// Ask what to do when a dependency is installed but differs from the version
/// this repo offers (i.e. it likely came from another source). With --yes we
/// keep the installed one (non-destructive).
fn ask_dep_conflict(
    dep: &str,
    needed_by: &str,
    installed: &pkg::PkgId,
    offered: &repo::AvailPkg,
    assume_yes: bool,
) -> DepChoice {
    println!(
        "\n{}",
        ui::blue(&format!("  Dependency conflict for '{dep}' (needed by '{needed_by}'):"))
    );
    println!("    {}           {}", ui::blue("installed:"), ui::white(&installed.tag()));
    println!(
        "    {}  {}",
        ui::blue(&format!("{} provides:", offered.repo)),
        ui::white(&offered.id.tag())
    );
    if assume_yes {
        println!("    {}", ui::blue("(--yes: keeping the installed version)"));
        return DepChoice::Skip;
    }
    println!("    {}", hilite_keys("[s]kip      keep the installed version (default)"));
    println!(
        "    {}",
        hilite_keys(&format!("[r]eplace   install the {}'s version instead", offered.repo))
    );
    println!("    {}", hilite_keys("skip-[a]ll  keep installed for this and all later conflicts"));
    println!("    {}", hilite_keys("a[b]ort     cancel the whole operation, change nothing more"));
    loop {
        print!("  {} ", hilite_keys("Choice [s/r/a/b]:"));
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return DepChoice::Skip;
        }
        match line.trim() {
            "" | "s" | "S" => return DepChoice::Skip,
            "r" | "R" => return DepChoice::Replace,
            "a" | "A" => return DepChoice::SkipAll,
            "b" | "B" => return DepChoice::Abort,
            other => println!(
                "    {}",
                ui::blue(&format!("'{other}' is not a choice — type s, r, a, or b (Enter = skip)."))
            ),
        }
    }
}

/// Show the dependencies kept by the priority rule (already installed from a
/// source of higher-or-equal priority than the repo that pulled them in) and
/// let the user keep each (default) or replace it with that repo's version.
/// Returns the offered packages the user chose to install instead. With
/// `assume_yes` nothing is asked and everything is kept (the table is still
/// shown, for information).
fn resolve_protected_deps(
    db: &PkgDb,
    tag_prios: &[crate::config::TagPriority],
    protected: &[ProtectedDep],
    assume_yes: bool,
) -> Result<Vec<repo::AvailPkg>, String> {
    let inst_src = |p: &ProtectedDep| {
        let tag = p.installed.build_tag();
        let src = if tag.is_empty() { "official" } else { tag };
        format!("{} ({})", src, db.installed_priority(&p.installed, tag_prios))
    };
    let off_src =
        |p: &ProtectedDep| format!("{} ({})", p.offered.repo, db.repo_priority(&p.offered.repo));

    let wnum = protected.len().to_string().len().max(1);
    let wdep = protected.iter().map(|p| p.dep.len()).chain(std::iter::once(10)).max().unwrap();
    let wkept =
        protected.iter().map(|p| inst_src(p).len()).chain(std::iter::once(16)).max().unwrap();
    let woff = protected.iter().map(|p| off_src(p).len()).chain(std::iter::once(7)).max().unwrap();

    println!(
        "\n{}",
        ui::blue("These dependencies are already installed from a higher-or-equal priority source:")
    );
    println!(
        "  {}  {}  {}  {}",
        ui::blue(&format!("{:>wnum$}", "#")),
        ui::blue(&format!("{:<wdep$}", "Dependency")),
        ui::blue(&format!("{:<wkept$}", "Installed (kept)")),
        ui::blue(&format!("{:<woff$}", "Offered")),
    );
    println!("  {}", ui::dim(&"-".repeat(wnum + 2 + wdep + 2 + wkept + 2 + woff)));
    for (i, p) in protected.iter().enumerate() {
        println!(
            "  {}  {}  {}  {}",
            ui::cyan(&format!("{:>wnum$}", i + 1)),
            ui::white(&format!("{:<wdep$}", p.dep)),
            ui::green(&format!("{:<wkept$}", inst_src(p))),
            ui::yellow(&format!("{:<woff$}", off_src(p))),
        );
    }

    if assume_yes {
        println!("{}", ui::blue("(--yes: keeping the installed versions)"));
        return Ok(Vec::new());
    }

    // Per-dependency choice; the default is to keep the installed version.
    let mut replace = Vec::new();
    let mut keep_all = false;
    for p in protected {
        if keep_all {
            break;
        }
        match ask_protected_dep(p, &inst_src(p), &off_src(p)) {
            KeepChoice::Keep => {}
            KeepChoice::Replace => replace.push(p.offered.clone()),
            KeepChoice::KeepAll => keep_all = true,
            // Mirror the conflict prompt's a[b]ort: stop the whole operation,
            // change nothing more.
            KeepChoice::Quit => return Err("aborted by user".into()),
        }
    }
    Ok(replace)
}

/// Per-dependency prompt for a priority-protected dependency. Default = keep.
fn ask_protected_dep(p: &ProtectedDep, inst_src: &str, off_src: &str) -> KeepChoice {
    println!("\n  {}", ui::blue(&format!("'{}' (needed by '{}'):", p.dep, p.needed_by)));
    println!("    {}", hilite_keys(&format!("[k]eep      keep the installed {inst_src} (default)")));
    println!("    {}", hilite_keys(&format!("[r]eplace   install {off_src} instead")));
    println!("    {}", hilite_keys("keep-[a]ll  keep this and every remaining one"));
    println!("    {}", hilite_keys("[q]uit      cancel the whole operation, change nothing"));
    loop {
        print!("  {} ", hilite_keys("Choice [k/r/a/q]:"));
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return KeepChoice::Keep;
        }
        match line.trim() {
            "" | "k" | "K" => return KeepChoice::Keep,
            "r" | "R" => return KeepChoice::Replace,
            "a" | "A" => return KeepChoice::KeepAll,
            "q" | "Q" => return KeepChoice::Quit,
            other => println!(
                "    {}",
                ui::blue(&format!("'{other}' is not a choice — type k, r, a, or q (Enter = keep)."))
            ),
        }
    }
}

/// Expand a set of root packages into a full, ordered install plan by reading
/// each package's `.dep` file and pulling in dependencies from the *same* repo.
/// Dependencies are placed before the packages that need them.
fn expand_with_deps(
    cfg: &Config,
    db: &PkgDb,
    installed: &[pkg::PkgId],
    roots: Vec<(repo::AvailPkg, InstallAction)>,
    resolve: bool,
    assume_yes: bool,
) -> Result<Vec<PlanItem>, String> {
    let mut plan = Vec::new();
    let mut planned: HashSet<String> = HashSet::new();
    let mut visiting: HashSet<String> = HashSet::new();
    let mut skip_all = false;
    let mut protected: Vec<ProtectedDep> = Vec::new();
    // Names already scheduled as roots (e.g. every package upgrade-all will
    // upgrade). A dependency whose name is here will be satisfied by its own
    // root entry, so we must not prompt about it as a "conflict".
    let root_names: HashSet<String> =
        roots.iter().map(|(p, _)| p.id.name.clone()).collect();
    for (pkg, action) in roots {
        add_with_deps(
            cfg, db, installed, pkg, action, None, resolve, assume_yes, &root_names,
            &mut plan, &mut planned, &mut visiting, &mut skip_all, &mut protected,
        )?;
    }
    // Dependencies kept by the priority rule: show them and let the user replace
    // any with the version offered by the repo that pulled them in. Anything the
    // user keeps (the default) just stays installed.
    if !protected.is_empty() {
        let replace = resolve_protected_deps(db, &cfg.tag_priorities, &protected, assume_yes)?;
        for o in replace {
            add_with_deps(
                cfg, db, installed, o, InstallAction::Upgrade, None, resolve, assume_yes,
                &root_names, &mut plan, &mut planned, &mut visiting, &mut skip_all,
                &mut Vec::new(),
            )?;
        }
    }
    Ok(plan)
}

#[allow(clippy::too_many_arguments)]
fn add_with_deps(
    cfg: &Config,
    db: &PkgDb,
    installed: &[pkg::PkgId],
    pkg: repo::AvailPkg,
    action: InstallAction,
    dep_for: Option<String>,
    resolve: bool,
    assume_yes: bool,
    root_names: &HashSet<String>,
    plan: &mut Vec<PlanItem>,
    planned: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
    skip_all: &mut bool,
    protected: &mut Vec<ProtectedDep>,
) -> Result<(), String> {
    let name = pkg.id.name.clone();
    if planned.contains(&name) {
        return Ok(());
    }
    if !visiting.insert(name.clone()) {
        return Ok(()); // already on the stack: dependency cycle, stop recursing
    }

    if resolve {
        if let Some(repo) = cfg.repo_by_name(&pkg.repo) {
            for dep in repo::fetch_dep(repo, &pkg) {
                // A blacklisted dependency is never pulled in — whether it would
                // come fresh from this repo, or is already installed/frozen.
                let bl = db
                    .resolve(&format!("{}:{}", pkg.repo, dep))
                    .map_or(false, |o| bl_avail(cfg, o))
                    || system::installed_by_name(installed, &dep)
                        .map_or(false, |i| bl_installed(cfg, Some(db), i));
                if bl {
                    continue;
                }
                // A dependency that is itself a root (will be upgraded anyway)
                // or already planned needs no handling here.
                if root_names.contains(&dep) || planned.contains(&dep) {
                    continue;
                }
                // What the *same* repo offers for this dependency name.
                let offered = db.resolve(&format!("{}:{}", pkg.repo, dep)).cloned();
                let inst = system::installed_by_name(installed, &dep);
                match (inst, offered) {
                    // installed, and this repo offers the exact same build: satisfied.
                    (Some(i), Some(o)) if i.tag() == o.id.tag() => {}
                    // installed but differs from what this repo offers: likely another source — ask.
                    (Some(i), Some(o)) => {
                        if db.installed_outranks(i, &o, &cfg.tag_priorities) {
                            // The installed dependency comes from a source of
                            // higher-or-equal priority than this repo offers, so
                            // it is kept by the priority rule. Record it (once)
                            // so the caller can show the choice instead of
                            // keeping it silently.
                            if !protected.iter().any(|p| p.dep == dep) {
                                protected.push(ProtectedDep {
                                    dep: dep.clone(),
                                    needed_by: name.clone(),
                                    installed: i.clone(),
                                    offered: o,
                                });
                            }
                        } else {
                            let choice = if *skip_all {
                                DepChoice::Skip
                            } else {
                                ask_dep_conflict(&dep, &name, i, &o, assume_yes)
                            };
                            match choice {
                                DepChoice::Skip => {}
                                DepChoice::SkipAll => *skip_all = true,
                                DepChoice::Replace => add_with_deps(
                                    cfg, db, installed, o, InstallAction::Upgrade, Some(name.clone()),
                                    resolve, assume_yes, root_names, plan, planned, visiting, skip_all,
                                    protected,
                                )?,
                                DepChoice::Abort => return Err("aborted by user".into()),
                            }
                        }
                    }
                    // installed, this repo doesn't offer it: assume satisfied (e.g. a core package).
                    (Some(_), None) => {}
                    // not installed, this repo offers it: pull it in as a new install.
                    (None, Some(o)) => add_with_deps(
                        cfg, db, installed, o, InstallAction::Install, Some(name.clone()),
                        resolve, assume_yes, root_names, plan, planned, visiting, skip_all,
                        protected,
                    )?,
                    // not installed and not offered here: can't satisfy, warn and move on.
                    (None, None) => eprintln!(
                        "  warning: dependency '{dep}' of '{name}' not found in repo '{}'",
                        pkg.repo
                    ),
                }
            }
        }
    }

    visiting.remove(&name);
    // Record the currently-installed version for upgrades/reinstalls so the plan
    // can show the `old -> new` transition. A fresh install has no "from".
    let from = match action {
        InstallAction::Upgrade | InstallAction::Reinstall => {
            system::installed_by_name(installed, &pkg.id.name)
                .map(|i| format!("{}-{}-{}", i.version, i.arch, i.build))
        }
        InstallAction::Install => None,
    };
    if planned.insert(name) {
        plan.push(PlanItem { pkg, action, dep_for, from });
    }
    Ok(())
}

/// One row of the plan table.
struct PlanRow {
    action: &'static str,
    color: fn(&str) -> String,
    name: String,
    version: String,
    repo: String,
    note: String,
}

/// Render plan rows as an aligned, coloured table:
///   Action | Package | Version | Repo
/// The action label is coloured per row (green install/upgrade, yellow
/// reinstall, red remove), the package name is always white, the version is
/// dim, the repo is cyan, and the rules/separators are dim. Prints nothing for
/// an empty slice.
fn print_table(rows: &[PlanRow]) {
    if rows.is_empty() {
        return;
    }
    let wa = rows.iter().map(|r| r.action.len()).chain(std::iter::once(6)).max().unwrap();
    let wn = rows.iter().map(|r| r.name.len()).chain(std::iter::once(7)).max().unwrap();
    let wv = rows.iter().map(|r| r.version.len()).chain(std::iter::once(7)).max().unwrap();
    let wr = rows.iter().map(|r| r.repo.len()).chain(std::iter::once(4)).max().unwrap();
    let sep = ui::dim(" | ");
    println!(
        "  {}{}{}{}{}{}{}",
        ui::blue(&format!("{:<wa$}", "Action")),
        sep,
        ui::blue(&format!("{:<wn$}", "Package")),
        sep,
        ui::blue(&format!("{:<wv$}", "Version")),
        sep,
        ui::blue(&format!("{:<wr$}", "Repo")),
    );
    let dash = |n: usize| "-".repeat(n);
    println!(
        "  {}",
        ui::dim(&format!("{}-+-{}-+-{}-+-{}", dash(wa), dash(wn), dash(wv), dash(wr)))
    );
    for r in rows {
        let line = format!(
            "  {}{}{}{}{}{}{}",
            (r.color)(&format!("{:<wa$}", r.action)),
            sep,
            ui::white(&format!("{:<wn$}", r.name)),
            sep,
            ui::dim(&format!("{:<wv$}", r.version)),
            sep,
            ui::cyan(&format!("{:<wr$}", r.repo)),
        );
        if r.note.is_empty() {
            println!("{line}");
        } else {
            println!("{line}  {}", ui::blue(&r.note));
        }
    }
}

/// Split an available package into the table columns (name / version / repo).
fn plan_row(it: &PlanItem) -> PlanRow {
    let (action, color): (&'static str, fn(&str) -> String) = match it.action {
        InstallAction::Install if it.dep_for.is_some() => ("new dep", ui::green),
        InstallAction::Install => ("install", ui::green),
        InstallAction::Upgrade => ("upgrade", ui::green),
        InstallAction::Reinstall => ("reinstall", ui::yellow),
    };
    let to = format!("{}-{}-{}", it.pkg.id.version, it.pkg.id.arch, it.pkg.id.build);
    // Show the transition for an upgrade (a reinstall has from == to, so it
    // collapses to a single version). Plain ASCII arrow keeps byte length equal
    // to display width, so the dim version column stays aligned.
    let version = match &it.from {
        Some(f) if *f != to => format!("{f} -> {to}"),
        _ => to,
    };
    PlanRow {
        action,
        color,
        name: it.pkg.id.name.clone(),
        version,
        repo: it.pkg.repo.clone(),
        note: it.dep_for.as_ref().map(|p| format!("for {p}")).unwrap_or_default(),
    }
}

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

/// Informational, shown with the plan just before the confirmation prompt:
/// optional companions a repo SUGGESTS for something being installed or
/// upgraded. The SUGGESTS field is printed verbatim: slapt-get repos may put a
/// free-text note there rather than a clean package list, so slacker shows it
/// as-is and never tries to act on it. Gated on dependency resolution (the same
/// switch that honours a repo's dependency metadata); a repo that ships no
/// SUGGESTS, or vanilla Slackware, prints nothing.
fn note_optional_suggests(plan: &[PlanItem], resolve: bool) {
    if !resolve {
        return;
    }
    let mut printed = false;
    for it in plan {
        if !matches!(
            it.action,
            InstallAction::Install | InstallAction::Upgrade | InstallAction::Reinstall
        ) {
            continue;
        }
        let note = it.pkg.suggests.trim();
        if note.is_empty() {
            continue;
        }
        printed = true;
        println!(
            "{}",
            ui::blue(&format!(
                "  {} — the repo maintainers suggest (acting on it is up to you):",
                it.pkg.id.name
            ))
        );
        println!("{}", ui::blue(&format!("    {note}")));
    }
    if printed {
        println!(
            "{}",
            ui::dim("    optional only — install any you want yourself; slacker won't pull them in")
        );
    }
}

/// A repo-declared conflict that is currently installed: the package about to
/// be installed/upgraded, and the installed package it declares a conflict with.
struct Conflict {
    installing: String,
    installed: String,
}

/// Detect repo-declared `PACKAGE CONFLICTS:` (from PACKAGES.TXT) that are
/// currently installed. One-directional by necessity: only the conflicts the
/// package *being installed* declares are knowable — an installed package
/// carries no PACKAGES.TXT metadata. Gated on dependency resolution; de-duped on
/// (installing, installed). A package never conflicts with its own new version.
fn detect_conflicts(plan: &[PlanItem], installed: &[pkg::PkgId], resolve: bool) -> Vec<Conflict> {
    if !resolve {
        return Vec::new();
    }
    let mut out: Vec<Conflict> = Vec::new();
    for it in plan {
        if !matches!(
            it.action,
            InstallAction::Install | InstallAction::Upgrade | InstallAction::Reinstall
        ) {
            continue;
        }
        for c in &it.pkg.conflicts {
            if *c == it.pkg.id.name || system::installed_by_name(installed, c.as_str()).is_none() {
                continue;
            }
            if !out.iter().any(|x| x.installing == it.pkg.id.name && x.installed == *c) {
                out.push(Conflict {
                    installing: it.pkg.id.name.clone(),
                    installed: c.clone(),
                });
            }
        }
    }
    out
}

/// Print the conflict ATTENTION block (red). Shown with the plan, in dry-run and
/// before the confirmation alike.
fn report_conflicts(conflicts: &[Conflict]) {
    if conflicts.is_empty() {
        return;
    }
    println!(
        "{}",
        ui::red(&format!(
            "  ATTENTION: {} package conflict{} with what is already installed:",
            conflicts.len(),
            if conflicts.len() == 1 { "" } else { "s" }
        ))
    );
    for c in conflicts {
        println!(
            "{}",
            ui::red(&format!(
                "    {} conflicts with the installed {}",
                c.installing, c.installed
            ))
        );
    }
}

/// Conflict-aware confirmation. With no conflicts it is the normal yes/no
/// prompt. With conflicts it offers a 3-way choice: continue anyway, removepkg
/// the conflicting installed package(s) first then continue, or abort. With
/// `assume_yes` it warns and proceeds — never auto-removing a package, never
/// silently aborting an automated run. Returns whether to proceed; runs the
/// chosen removals itself. Reverse dependencies of a removed package are NOT
/// chased — removing a conflict is at the user's explicit request.
fn confirm_conflicts(
    prompt: &str,
    conflicts: &[Conflict],
    assume_yes: bool,
) -> Result<bool, String> {
    if conflicts.is_empty() {
        return Ok(confirm(prompt, assume_yes));
    }
    if assume_yes {
        println!(
            "{}",
            ui::yellow(
                "--yes: proceeding despite the conflict(s); the conflicting installed package(s) are left in place."
            )
        );
        return Ok(true);
    }
    // Distinct installed packages we would remove if asked.
    let mut to_remove: Vec<String> = Vec::new();
    for c in conflicts {
        if !to_remove.contains(&c.installed) {
            to_remove.push(c.installed.clone());
        }
    }
    println!("    {}", hilite_keys("[c]ontinue   install anyway, leave the conflicting package(s)"));
    println!(
        "    {}",
        hilite_keys(&format!(
            "[r]emove     removepkg {} first, then continue",
            to_remove.join(", ")
        ))
    );
    println!("    {}", hilite_keys("[a]bort      cancel, change nothing (default)"));
    loop {
        print!("  {} ", hilite_keys("Choice [c/r/a]:"));
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return Ok(false);
        }
        match line.trim() {
            "c" | "C" => return Ok(true),
            "r" | "R" => {
                for name in &to_remove {
                    println!("{}", ui::blue(&format!("  removing conflicting {name} ...")));
                    system::remove_package(name)?;
                }
                return Ok(true);
            }
            "" | "a" | "A" => return Ok(false),
            other => println!(
                "    {}",
                ui::blue(&format!("'{other}' is not a choice — type c, r, or a (Enter = abort)."))
            ),
        }
    }
}

/// One-time migration of persistent security state from the old CACHE_DIR home
/// to STATE_DIR. Earlier versions kept the GPG keyring + TOFU `.fpr` pins and
/// the quarantine/ + trusted/ markers under CACHE_DIR; those now live under
/// STATE_DIR so an FHS-disposable /var/cache sweep cannot wipe the trust
/// anchors. Runs only when STATE_DIR does not yet hold a given subdir AND the
/// old cache location does — so an established install is never disturbed, and
/// re-pinning (the first-contact event we must avoid) never happens on upgrade.
fn migrate_state(cfg: &Config) {
    migrate_state_dirs(&cfg.cache_dir, &cfg.state_dir);
}

fn migrate_state_dirs(cache_dir: &Path, state_dir: &Path) {
    if cache_dir == state_dir {
        return;
    }
    for sub in ["gpg", "quarantine", "trusted"] {
        let old = cache_dir.join(sub);
        let new = state_dir.join(sub);
        if !old.is_dir() || new.exists() {
            continue;
        }
        if let Some(parent) = new.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Atomic rename when on one filesystem; copy+remove across devices
        // (/var/cache and /var/lib may be separate mounts).
        let moved = std::fs::rename(&old, &new).is_ok()
            || (copy_dir_recursive(&old, &new).is_ok() && std::fs::remove_dir_all(&old).is_ok());
        // The GPG keyring must stay private (gpg refuses a world-readable home);
        // a cross-device copy would otherwise create it with default perms.
        if moved && sub == "gpg" {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o700));
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn show_plan(plan: &[PlanItem], frozen: &[String], protected: &[String]) {
    let rows: Vec<PlanRow> = plan.iter().map(plan_row).collect();
    print_table(&rows);

    if !frozen.is_empty() {
        println!("{}", ui::purple("  frozen (blacklisted — left unchanged):"));
        for n in frozen {
            println!("    {}", ui::white(n));
        }
    }
    if !protected.is_empty() {
        println!("{}", ui::blue("  kept (installed from a higher/equal-priority source):"));
        for n in protected {
            println!("    {}", ui::white(n));
        }
    }
}

/// Informational note (mirrors how frozen packages are surfaced): list any plan
/// package whose name is pinned, so the user sees the pin steered the source.
/// No-op when nothing in the plan is pinned.
fn report_pinned_in_plan(cfg: &Config, plan: &[PlanItem]) {
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    for it in plan {
        if let Some(repo) = cfg.pinned_repo(&it.pkg.id.name) {
            if seen.insert(it.pkg.id.name.clone()) {
                lines.push(format!("{} -> {}", it.pkg.id.name, repo));
            }
        }
    }
    if !lines.is_empty() {
        println!(
            "{}",
            ui::blue("  pinned (taken only from their repo, ignoring priority):")
        );
        for l in &lines {
            println!("    {}", ui::white(l));
        }
    }
}

/// One-line discoverability tip shown with a plan: how to keep a package from
/// changing (freeze) or take it only from one repo (pin).
fn hint_freeze_pin() {
    println!(
        "  {}",
        ui::dim(
            "tip: keep a package unchanged with `frozen <name>`, or take it from one repo with `pin repo:name`"
        )
    );
}

/// Read-only: for packages in the plan whose *name* is also offered by other
/// repos, list those alternatives. The plan already holds the priority/pin
/// winner that `collect` chose; this only surfaces what was NOT applied so the
/// user can answer "n" at the prompt and pin / switch a repo if they would
/// rather have a different build. It never reads or changes the plan's choice.
/// Returns, per package: (name, chosen-source, [other-sources]).
fn plan_alternatives(db: &PkgDb, plan: &[PlanItem]) -> Vec<(String, String, Vec<String>)> {
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

/// Print just the action part of a plan (no skip categories). Used by commands
/// that don't compute frozen/priority skips themselves.
fn print_plan(plan: &[PlanItem]) {
    show_plan(plan, &[], &[]);
}

/// Build the on-disk package path, refusing any repo-supplied filename that is
/// not a safe basename. This is the second line of defence behind the parser
/// filter: even if a path-like filename ever reached here, slacker (as root)
/// must never write or install through it.
fn package_dest(cfg: &Config, repo: &str, filename: &str) -> Result<std::path::PathBuf, String> {
    if !pkg::is_safe_filename(filename) {
        return Err(format!(
            "repo '{repo}' supplied an unsafe package filename {filename:?} — refusing \
             (possible path-traversal attack)"
        ));
    }
    let dest = system::cached_pkg_path(&cfg.cache_dir, repo, filename);
    // Confirm the result really stays inside the per-repo package directory.
    let base = cfg.cache_dir.join("packages").join(repo);
    if dest.parent() != Some(base.as_path()) {
        return Err(format!(
            "refusing package path outside the cache for repo '{repo}': {}",
            dest.display()
        ));
    }
    Ok(dest)
}

/// One unit of download work: a package, the repo it comes from, and the
/// on-disk path it must land at. The repo/package references borrow from the
/// caller's config/plan (which outlive the parallel scope), so package metadata
/// is never cloned just to hand it to a worker.
struct DlItem<'a> {
    repo: &'a config::Repo,
    pkg: &'a repo::AvailPkg,
    dest: std::path::PathBuf,
}

/// Result of fetching+verifying one item, tagged with its index in the input
/// list so outcomes can be reordered after concurrent (out-of-order) completion.
struct DlOutcome {
    idx: usize,
    name: String,
    /// Ok = verified and on disk (the checks that ran); Err = a reason it was
    /// not made ready. A failed item is never installed.
    result: Result<Vec<String>, String>,
}

/// Compact trailing marker for a successful download line, mirroring the honesty
/// of the verbose path: nothing for a GPG-authenticated package, an "integrity
/// only" note when just md5/sha matched, and "verify off" when disabled.
fn verify_suffix(checks: &[String]) -> &'static str {
    if checks.is_empty() {
        "(verify off) "
    } else if checks.iter().any(|c| c.starts_with("gpg")) {
        ""
    } else {
        "(integrity only) "
    }
}

/// Split fetch outcomes into a ready-flag per input index and a list of
/// (name, reason) download/verify failures. Pure, so it is unit-tested without
/// threads or a network.
fn summarize_outcomes(outcomes: &[DlOutcome], total: usize) -> (Vec<bool>, Vec<(String, String)>) {
    let mut ready = vec![false; total];
    let mut failed = Vec::new();
    for o in outcomes {
        match &o.result {
            Ok(_) if o.idx < total => ready[o.idx] = true,
            Ok(_) => {}
            Err(e) => failed.push((o.name.clone(), e.clone())),
        }
    }
    (ready, failed)
}

/// Download + verify every item concurrently, bounded by `max_parallel`, writing
/// each package to its OWN destination (no shared files, so no races). Prints a
/// live ✓/✗ counter as items complete and returns one outcome per item, ordered
/// to match `items`. Network phase only — nothing is installed here.
fn parallel_fetch(cfg: &Config, items: &[DlItem], max_parallel: usize) -> Vec<DlOutcome> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    let total = items.len();
    if total == 0 {
        return Vec::new();
    }
    let workers = max_parallel.clamp(1, total);
    let next = AtomicUsize::new(0);

    let mut outcomes: Vec<DlOutcome> = std::thread::scope(|s| {
        let (tx, rx) = mpsc::channel::<DlOutcome>();
        for _ in 0..workers {
            let tx = tx.clone();
            let next = &next;
            let items = &items;
            s.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= total {
                    break;
                }
                let it = &items[i];
                let result = fetch_and_verify(cfg, it.repo, it.pkg, &it.dest, true);
                let _ = tx.send(DlOutcome {
                    idx: i,
                    name: it.pkg.id.name.clone(),
                    result,
                });
            });
        }
        drop(tx); // receive loop ends once every worker has finished and dropped its sender

        let mut got: Vec<DlOutcome> = Vec::with_capacity(total);
        let mut done = 0usize;
        while let Ok(o) = rx.recv() {
            done += 1;
            match &o.result {
                Ok(checks) => println!(
                    "  {} {} {}[{done}/{total}]",
                    ui::green("✓"),
                    o.name,
                    verify_suffix(checks)
                ),
                Err(e) => println!(
                    "  {} {} [{done}/{total}]  {}",
                    ui::red("✗"),
                    o.name,
                    e.lines().next().unwrap_or("download failed")
                ),
            }
            got.push(o);
        }
        got
    });

    outcomes.sort_by_key(|o| o.idx);

    // Verification summary for the batch. The per-item ✓ lines stay terse, so
    // make the overall authentication level explicit here: how many packages
    // were GPG-authenticated vs only integrity-checked (md5/sha, no signature)
    // vs not verified at all. Failures are reported separately by the caller.
    let mut gpg = 0usize;
    let mut integrity = 0usize;
    let mut off = 0usize;
    for o in &outcomes {
        match &o.result {
            Ok(checks) if checks.iter().any(|c| c.starts_with("gpg")) => gpg += 1,
            Ok(checks) if checks.is_empty() => off += 1,
            Ok(_) => integrity += 1,
            Err(_) => {}
        }
    }
    let verified = gpg + integrity + off;
    if verified > 0 {
        if integrity == 0 && off == 0 {
            println!(
                "  {}",
                ui::green(&format!(
                    "all {verified} package(s) GPG-verified (signature + checksum)"
                ))
            );
        } else {
            let mut parts: Vec<String> = Vec::new();
            if gpg > 0 {
                parts.push(format!("{gpg} GPG-verified"));
            }
            if integrity > 0 {
                parts.push(format!("{integrity} integrity-only (no GPG)"));
            }
            if off > 0 {
                parts.push(format!("{off} unverified"));
            }
            println!(
                "  {}",
                ui::yellow(&format!("{verified} package(s): {}", parts.join(", ")))
            );
        }
    }

    outcomes
}

/// End-of-run report for a batch: stays quiet when everything succeeded (the
/// per-item lines already told the story), otherwise lists what was skipped and
/// why. Download/verify failures leave any installed version untouched; install
/// failures are reported as-is. Never aborts — purely informational.
fn report_batch_failures(
    total: usize,
    installed: usize,
    dl_failed: &[(String, String)],
    install_failed: &[(String, String)],
) {
    if dl_failed.is_empty() && install_failed.is_empty() {
        return;
    }
    let bad = dl_failed.len() + install_failed.len();
    println!();
    println!(
        "{}",
        ui::yellow(&format!(
            "{installed} of {total} package(s) completed; {bad} had problems:"
        ))
    );
    for (name, reason) in dl_failed {
        let r = reason.lines().next().unwrap_or("download/verify failed");
        // A GPG failure is a refusal (possible tampering); make it stand out.
        if r.to_ascii_lowercase().contains("gpg") || r.to_ascii_lowercase().contains("signature") {
            println!("  {} {}  GPG check failed — refused: {r}", ui::red("✗"), name);
        } else {
            println!("  {} {}  download/verify failed: {r}", ui::red("✗"), name);
        }
        println!(
            "      {}",
            ui::dim("(installed version, if any, was left unchanged)")
        );
    }
    for (name, reason) in install_failed {
        let r = reason.lines().next().unwrap_or("install failed");
        println!("  {} {}  install failed: {r}", ui::red("✗"), name);
    }
    println!();
    println!(
        "  {}",
        ui::dim("Re-run the same command to retry these; packages already done are skipped.")
    );
}

/// Download, verify and install/upgrade/reinstall every item in a plan.
///
/// A single item keeps the original verbose, serial path (nothing to parallelise
/// and nothing to skip past). For several items the work splits into two phases:
/// first every package is downloaded and verified concurrently, then the verified
/// ones are installed serially through the unchanged pkgtools path. The flow is
/// best-effort — a package that fails to download/verify is skipped (its installed
/// version left intact) and reported at the end, rather than aborting the whole
/// batch. A package that fails GPG/md5 verification is never installed.
/// True if `name` is a boot-critical kernel package — upgrading it means the
/// bootloader and (if used) the initrd must be refreshed before the next reboot,
/// or the machine may not boot.
fn is_kernel_pkg(name: &str) -> bool {
    name.starts_with("kernel-generic")
        || name.starts_with("kernel-huge")
        || name == "kernel-modules"
        || name.starts_with("kernel-modules-")
}

/// After a plan that upgraded/installed a kernel, remind the user to refresh the
/// bootloader and initrd before rebooting (Slackware does not do this
/// automatically). No-op if the plan touched no kernel package.
fn kernel_reboot_reminder(plan: &[PlanItem]) {
    if !plan.iter().any(|it| is_kernel_pkg(&it.pkg.id.name)) {
        return;
    }
    println!();
    println!("{}", ui::yellow("/// !!!! --- A kernel package was upgraded.---- !!!! ///"));
    println!(
        "  {}",
        ui::white(
            "Before rebooting: make sure the bootloader is updated; if you use an initrd or custom hooks, make sure everything is in place."
        )
    );
    println!(
        "  {}",
        ui::dim(
            "LILO: run `lilo`  |  ELILO/UEFI: `eliloconfig`  |  GRUB: `update-grub`  \
             |  This message is a reminder; generally speaking, you should not need to act on it."
        )
    );
}

/// Detect the Slackware release a repo URL targets, for the release-mismatch
/// guard. First the precise `slackware{arch}-<suffix>` segment (official tree /
/// conraid); failing that, a bare `current` segment or a clean `X.Y` version
/// segment (e.g. alienbob's `sbrepos/current/x86_64` or `.../15.0/...`). None
/// when the URL carries no release token (SBo and other release-agnostic repos)
/// — those are never flagged.
fn repo_release_token(url: &str) -> Option<dist::Release> {
    if let Some(r) = dist::parse_release_from_url(url) {
        return Some(r);
    }
    for seg in url.split(['/', '\\']) {
        if seg.eq_ignore_ascii_case("current") {
            return Some(dist::Release::Current);
        }
        if is_release_version_segment(seg) {
            return Some(dist::Release::Stable(seg.to_string()));
        }
    }
    None
}

/// A clean `X.Y` version path segment (e.g. `15.0`, `14.2`), used only to spot a
/// stable-release directory in a third-party repo URL. Requires exactly one dot
/// with digits on both sides, so bare integers and longer dotted strings (a
/// package version like `3.0.23`) are NOT mistaken for a release.
fn is_release_version_segment(s: &str) -> bool {
    let mut parts = s.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(a), Some(b), None) => {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|c| c.is_ascii_digit())
                && b.bytes().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// The running system's release, from `/etc/os-release`. None if it cannot be
/// determined (then the release-mismatch guard stays silent — fail-open).
fn system_release() -> Option<dist::Release> {
    dist::parse_release_from_os(
        system::version_id().as_deref(),
        system::version_codename().as_deref(),
    )
}

/// Refuse a plan that would install/upgrade packages from a repo built for a
/// DIFFERENT Slackware release than the running system (e.g. an alienbob
/// `-current` repo left active on a 15.0 system). GPG/md5 authenticate WHO built
/// a package, not WHICH RELEASE it targets, so this is the only thing standing
/// between the user and a release-mixing foot-gun that can break the system.
/// HARD REFUSE unless `allow` (passed from `--yes`). Suppressed for upgrade-dist,
/// which has its own execution path and deliberately changes release.
fn enforce_release_match(cfg: &Config, plan: &[PlanItem], allow: bool) -> Result<(), String> {
    let Some(sys) = system_release() else {
        return Ok(()); // can't tell the system release — don't block
    };
    let mut bad: Vec<(String, String, String)> = Vec::new(); // (pkg, repo, repo_release)
    for it in plan {
        let Some(r) = cfg.repo_by_name(&it.pkg.repo) else {
            continue;
        };
        if let Some(rel) = repo_release_token(&r.url) {
            if rel != sys {
                bad.push((it.pkg.id.name.clone(), r.name.clone(), dist::show(&rel)));
            }
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    let sys_show = dist::show(&sys);
    println!(
        "{}",
        ui::red(&format!(
            "release mismatch: this system is Slackware {sys_show}, but these packages come from \
             a repo built for a different release:"
        ))
    );
    for (pkg, repo, rel) in &bad {
        println!(
            "  {}  {}  {}",
            ui::white(pkg),
            ui::cyan(repo),
            ui::red(&format!("(targets {rel})"))
        );
    }
    if allow {
        println!(
            "{}",
            ui::yellow("--yes given: proceeding despite the release mismatch (at your own risk).")
        );
        return Ok(());
    }
    Err(format!(
        "refusing to mix releases — installing {}-release packages on a {sys_show} system can \
         break it. Point the repo at the {sys_show} tree, or pass --yes to override.",
        bad.first().map(|b| b.2.as_str()).unwrap_or("another"),
    ))
}

/// The architecture *family* of a Slackware arch token. All 32-bit x86 variants
/// (i386/i486/i586/i686/x86) are one family — they are mutually runnable, and the
/// distro base, our own build and third-party repos may each pick a different one
/// (e.g. the base is i586 while slacker is labelled i686). Every other token is
/// compared as-is (x86_64, aarch64, arm, ...). `noarch` is handled by the caller.
fn arch_family(a: &str) -> &str {
    match a {
        "i386" | "i486" | "i586" | "i686" | "x86" => "x86",
        other => other,
    }
}

/// Is a package built for `pkg_arch` safe to install on a `sys_arch` system?
/// `noarch` always is; otherwise the families must match. (Note: multilib does
/// NOT need an exception — compat32 packages carry an `x86_64` arch field, with
/// `compat32` only in the name/build, so they pass trivially on x86_64.)
fn arch_compatible(sys_arch: &str, pkg_arch: &str) -> bool {
    pkg_arch == "noarch" || arch_family(sys_arch) == arch_family(pkg_arch)
}

/// Refuse a plan that would install packages built for a DIFFERENT CPU
/// architecture than the running system (e.g. an `x86_64` mirror left active on
/// a 32-bit box, or the reverse). Such a package simply cannot run. Like the
/// release guard this is fail-closed; `allow` (from --yes) is a deliberately
/// quiet escape hatch and is not surfaced in the refusal message.
fn enforce_arch_match(cfg: &Config, plan: &[PlanItem], allow: bool) -> Result<(), String> {
    let sys = &cfg.arch;
    let mut bad: Vec<(String, String, String)> = Vec::new(); // (pkg, repo, pkg_arch)
    for it in plan {
        if !arch_compatible(sys, &it.pkg.id.arch) {
            bad.push((it.pkg.id.name.clone(), it.pkg.repo.clone(), it.pkg.id.arch.clone()));
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    println!(
        "{}",
        ui::red(&format!(
            "architecture mismatch: this system is {sys}, but these packages are built for a \
             different architecture:"
        ))
    );
    for (pkg, repo, a) in &bad {
        println!("  {}  {}  {}", ui::white(pkg), ui::cyan(repo), ui::red(&format!("({a})")));
    }
    if allow {
        println!(
            "{}",
            ui::yellow("proceeding despite the architecture mismatch (at your own risk).")
        );
        return Ok(());
    }
    Err(format!(
        "refusing: installing {}-architecture packages on a {sys} system cannot work. \
         Check that the repo/mirror points at the right Slackware tree for this arch.",
        bad.first().map(|b| b.2.as_str()).unwrap_or("another"),
    ))
}

fn execute_plan(cfg: &Config, plan: &[PlanItem], allow_release_mismatch: bool) -> Result<(), String> {
    if plan.is_empty() {
        return Ok(());
    }

    // Release-mismatch guard (fail-closed unless --yes). Runs before any download
    // or install. upgrade-dist does NOT use execute_plan, so it is unaffected.
    enforce_release_match(cfg, plan, allow_release_mismatch)?;

    // Architecture-mismatch guard (fail-closed; the same --yes is a quiet override).
    enforce_arch_match(cfg, plan, allow_release_mismatch)?;

    // Resolve repo + safe destination for every item first. A repo-lookup miss
    // or an unsafe (path-traversal) filename is a hard error, not a transient
    // download failure, so it bails the whole command.
    let mut items: Vec<DlItem> = Vec::with_capacity(plan.len());
    for it in plan {
        let r = cfg
            .repo_by_name(&it.pkg.repo)
            .ok_or("internal repo lookup failed")?;
        let dest = package_dest(cfg, &it.pkg.repo, &it.pkg.filename)?;
        items.push(DlItem {
            repo: r,
            pkg: &it.pkg,
            dest,
        });
    }

    // Single item: original verbose, serial behaviour (abort-on-error is moot
    // with nothing else to continue to).
    if items.len() == 1 {
        let it = &items[0];
        fetch_and_verify(cfg, it.repo, it.pkg, &it.dest, false)?;
        match plan[0].action {
            InstallAction::Install => system::install(&it.dest)?,
            InstallAction::Upgrade => system::upgrade_only(&it.dest)?,
            InstallAction::Reinstall => system::reinstall(&it.dest)?,
        }
        kernel_reboot_reminder(plan);
        return Ok(());
    }

    // PHASE 1 — download + verify everything in parallel.
    let workers = cfg.max_parallel.min(items.len());
    println!(
        "Downloading {} package(s) ({workers} parallel)...",
        items.len()
    );
    let outcomes = parallel_fetch(cfg, &items, cfg.max_parallel);
    let (ready, dl_failed) = summarize_outcomes(&outcomes, items.len());

    // PHASE 2 — install the verified packages serially (unchanged pkgtools path).
    // Best-effort: an install error on one does not stop the rest.
    let mut install_failed: Vec<(String, String)> = Vec::new();
    let mut installed = 0usize;
    for (i, it) in items.iter().enumerate() {
        if !ready[i] {
            continue;
        }
        let res = match plan[i].action {
            InstallAction::Install => system::install(&it.dest),
            InstallAction::Upgrade => system::upgrade_only(&it.dest),
            InstallAction::Reinstall => system::reinstall(&it.dest),
        };
        match res {
            Ok(()) => installed += 1,
            Err(e) => install_failed.push((it.pkg.id.name.clone(), e)),
        }
    }

    report_batch_failures(plan.len(), installed, &dl_failed, &install_failed);
    kernel_reboot_reminder(plan);
    Ok(())
}

/// Download a package (if needed) and verify md5 before use.
/// Message shown when a required verification method is not provided by a repo,
/// telling the user exactly where to relax the policy.
fn verify_unavailable_error(repo: &str, check: config::Check, config_dir: &std::path::Path) -> String {
    let what = match check {
        config::Check::Gpg => "a GPG signature (CHECKSUMS.md5.asc)",
        config::Check::Md5 => "an md5 checksum (CHECKSUMS.md5)",
        config::Check::Sha => "a SHA-256 checksum (CHECKSUMS.sha256)",
    };
    format!(
        "repo '{repo}': '{}' verification is required, but this repo does not provide {what}.\n\
         To continue for this repo without '{}', either add a `verify=` flag (omitting '{}') to\n\
         its line in {}, or change VERIFY in {}.",
        check.label(),
        check.label(),
        check.label(),
        config_dir.join("repos").display(),
        config_dir.join("slacker.conf").display(),
    )
}

fn fetch_and_verify(
    cfg: &Config,
    repo: &config::Repo,
    p: &repo::AvailPkg,
    dest: &std::path::Path,
    quiet: bool,
) -> Result<Vec<String>, String> {
    let policy = repo.verify_policy(&cfg.verify);

    // Guard against a symlink planted at the destination (e.g. in a shared
    // output directory like /tmp): never write through it. symlink_metadata
    // does not follow the link, so a dangling symlink is caught too.
    if let Ok(meta) = std::fs::symlink_metadata(dest) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "refusing to write through symlink {}; remove it first",
                dest.display()
            ));
        }
    }
    let need = if dest.exists() {
        match &p.md5 {
            Some(m) => download::md5_file(dest)? != *m,
            None => false,
        }
    } else {
        true
    };
    if need {
        let url = p.url(repo);
        if !quiet {
            println!("  fetching {url}");
        }
        download::download_to(&url, dest)?;
    }

    // Track which verifications actually ran, to report them on success.
    let mut checks: Vec<String> = Vec::new();

    // Per-package GPG signature, when the policy wants gpg. Slackware repos ship
    // a detached `<pkg>.asc` next to each package; verifying it directly is a
    // stronger check than the md5-via-signed-CHECKSUMS chain (md5 is weak). The
    // signature is fetched best-effort: present-and-good passes, present-and-bad
    // is fatal, absent falls back to md5/sha unless gpg is explicitly required.
    if policy.wants(config::Check::Gpg) {
        let asc_url = format!("{}.asc", p.url(repo));
        let mut asc = dest.as_os_str().to_os_string();
        asc.push(".asc");
        let asc = std::path::PathBuf::from(asc);
        let _ = download::download_to(&asc_url, &asc);
        match gpg::verify_detached(repo, &cfg.state_dir, dest, &asc) {
            Ok(gpg::Verify::Good(signer)) => checks.push(format!("gpg ({signer})")),
            Ok(gpg::Verify::Tampered(m)) => {
                // A bad or key-substituted package signature is always fatal.
                return Err(format!("{m} — refusing to install"));
            }
            Ok(gpg::Verify::NoSignature) | Ok(gpg::Verify::Unverifiable(_)) => {
                if policy.requires(config::Check::Gpg) {
                    return Err(verify_unavailable_error(
                        &p.repo,
                        config::Check::Gpg,
                        &cfg.config_dir,
                    ));
                }
                // best-effort: fall through to md5/sha below.
            }
            Err(e) => return Err(e),
        }
    }

    // Integrity: md5 and/or sha. The two are alternatives — at least ONE must
    // be present and pass. Any present-and-checked hash that mismatches is
    // fatal. If a method is explicitly required (Required policy) but absent,
    // that is fatal with guidance. If neither md5 nor sha is available at all
    // (and integrity wasn't switched off), we stop: the repo's checksum file is
    // missing or broken.
    let want_md5 = policy.wants(config::Check::Md5);
    let want_sha = policy.wants(config::Check::Sha);

    if want_md5 || want_sha {
        let mut any_checked = false;

        if want_md5 {
            match &p.md5 {
                Some(expected) => {
                    let got = download::md5_file(dest)?;
                    if &got != expected {
                        return Err(format!(
                            "md5 mismatch for {}: expected {expected}, got {got} \
                             (the package may be corrupt or the checksum file is wrong)",
                            p.filename
                        ));
                    }
                    any_checked = true;
                    checks.push("md5".into());
                }
                None => {
                    if policy.requires(config::Check::Md5) {
                        return Err(verify_unavailable_error(
                            &p.repo,
                            config::Check::Md5,
                            &cfg.config_dir,
                        ));
                    }
                }
            }
        }

        if want_sha {
            match &p.sha {
                Some(expected) => {
                    let got = download::sha256_file(dest)?;
                    if &got != expected {
                        return Err(format!(
                            "sha256 mismatch for {}: expected {expected}, got {got} \
                             (the package may be corrupt or the checksum file is wrong)",
                            p.filename
                        ));
                    }
                    any_checked = true;
                    checks.push("sha".into());
                }
                None => {
                    if policy.requires(config::Check::Sha) {
                        return Err(verify_unavailable_error(
                            &p.repo,
                            config::Check::Sha,
                            &cfg.config_dir,
                        ));
                    }
                }
            }
        }

        // best-available ("all"): if neither hash was available, refuse rather
        // than install something we could not check at all.
        if !any_checked && !policy.requires(config::Check::Md5) && !policy.requires(config::Check::Sha) {
            return Err(format!(
                "no usable checksum (md5 or sha) for {} in repo '{}': the repo's \
                 checksum file may be missing or broken. Fix the repo, or relax \
                 verification for it with a `verify=` flag in {} (or VERIFY in {}).",
                p.filename,
                p.repo,
                cfg.config_dir.join("repos").display(),
                cfg.config_dir.join("slacker.conf").display(),
            ));
        }
    }

    if !quiet {
        if checks.is_empty() {
            println!("  {}", ui::dim("(verification is disabled for this repo)"));
        } else if checks.iter().any(|c| c.starts_with("gpg")) {
            println!("  {}", ui::green(&format!("verified: {}", checks.join(" + "))));
        } else {
            // md5/sha prove the bytes match the repo's OWN checksum file, which —
            // without a GPG signature on that file — a malicious or MITM'd repo
            // controls too. Be honest: this is integrity, not authenticity.
            println!(
                "  {}",
                ui::yellow(&format!(
                    "integrity only: {} — NOT cryptographically authenticated (no GPG signature; \
                     enable gpg for this repo)",
                    checks.join(" + ")
                ))
            );
        }
    }
    Ok(checks)
}

/// Edit distance (Levenshtein) for "did you mean" suggestions.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Pick the closest candidate to `term` within a small edit distance.
fn closest<'a>(term: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for c in candidates {
        let d = edit_distance(term, c);
        if d <= 2 && best.as_ref().map_or(true, |(bd, _)| d < *bd) {
            best = Some((d, c.to_string()));
        }
    }
    best.map(|(_, s)| s)
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

/// Heuristic: do these unmatched arguments look like a shell-expanded glob —
/// the shell turned a bare `*`/`?` into a list of filenames before slacker ran —
/// rather than a handful of mistyped names? Two structural tells: a flood of
/// arguments (nobody types that many package names), or arguments containing
/// whitespace (a package selector never does). Either way we collapse the
/// per-argument flood into one explanatory note.
fn looks_shell_expanded(misses: &[String]) -> bool {
    const FLOOD: usize = 8;
    if misses.len() >= FLOOD {
        return true;
    }
    misses.len() >= 2 && misses.iter().filter(|m| m.contains(char::is_whitespace)).count() >= 2
}

/// One consolidated note replacing a flood of "no match" lines when the shell
/// clearly expanded a glob. slacker has no `*` wildcard for selecting packages —
/// a whole repo is `@repo`, a literal pattern must be quoted.
fn note_shell_expansion(misses: &[String]) {
    let n = misses.len();
    let sample: Vec<String> = misses.iter().take(3).map(|m| format!("'{m}'")).collect();
    let more = if n > 3 {
        format!(", … (+{} more)", n - 3)
    } else {
        String::new()
    };
    eprintln!(
        "{}",
        ui::yellow(&format!(
            "note: {n} of your arguments match no package — e.g. {}{more}.",
            sample.join(", ")
        ))
    );
    eprintln!(
        "{}",
        ui::dim("      this looks like a shell-expanded `*`/`?` glob (slacker never reads your directory).")
    );
    eprintln!(
        "{}",
        ui::dim("      slacker has no `*` wildcard for selecting packages — use `@repo` for a whole repo (e.g. `@gnome`), or quote a literal pattern.")
    );
}

/// Safety gate for install/upgrade/remove/reinstall/download. If the UNMATCHED
/// arguments look like a shell-expanded glob, refuse the WHOLE command before any
/// plan or picker is built — even arguments that DID match are almost certainly
/// accidental, because a short stray filename like `go` substring-matches pango,
/// cargo, dragon, … and a careless `[Enter]=all` would wreck the system. The
/// deliberate, single-term picker (e.g. `reinstall emacs` -> emacs, emacspeak)
/// is unaffected: it produces no misses, so the gate stays open.
fn guard_shell_expansion(misses: &[String]) -> Result<(), String> {
    if looks_shell_expanded(misses) {
        note_shell_expansion(misses);
        return Err("refusing to act on shell-expanded arguments — nothing was changed".into());
    }
    Ok(())
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

/// Validate an `@repo` / `@_tag` selector. Returns a helpful error if it names
/// neither a known repo nor a build tag actually in use.
fn validate_selector(db: &PkgDb, pattern: &str) -> Result<(), String> {
    let Some(rest) = pattern.strip_prefix('@') else {
        return Ok(());
    };
    if rest.is_empty() {
        return Err("empty selector '@': use @repo (e.g. @gnome) or @_tag (e.g. @_SBo)".into());
    }
    if db.is_repo(rest) || db.tag_in_use(rest) {
        return Ok(());
    }
    let repos = db.all_repos();
    let tags = db.all_build_tags();
    let mut msg = format!("unknown repo or tag '@{rest}'");
    let cands = repos.iter().map(|s| s.as_str()).chain(tags.iter().map(|s| s.as_str()));
    if let Some(s) = closest(rest, cands) {
        msg.push_str(&format!("; did you mean '@{s}'?"));
    }
    msg.push_str(&format!("\n  available repos: {}", repos.join(", ")));
    if !tags.is_empty() {
        msg.push_str(&format!("\n  available tags:  {}", tags.join(", ")));
    }
    Err(msg)
}

/// Expand patterns into winning packages, reporting patterns that matched
/// nothing.
fn collect<'a>(
    db: &'a PkgDb,
    patterns: &[String],
) -> Result<(Vec<&'a repo::AvailPkg>, Vec<String>), String> {
    for pat in patterns {
        validate_selector(db, pat)?;
    }
    // When more than one pattern yields the same package name, pick a single
    // winner by an explicit precedence (see `collect_prefers`): an explicit
    // `repo:name` pin beats a non-pinned candidate; otherwise the higher-priority
    // repo wins; a true tie keeps the first one seen. First-appearance order of
    // names is preserved for stable output. For a SINGLE pattern this changes
    // nothing — `match_pattern` already returns one priority-correct candidate
    // per name, so no name is seen twice and nothing is ever replaced.
    let mut order: Vec<String> = Vec::new();
    let mut chosen: HashMap<String, (&'a repo::AvailPkg, bool)> = HashMap::new();
    let mut misses = Vec::new();
    for pat in patterns {
        // A pin is `repo:name` (never `@repo`): the deliberate override of source
        // priority, so it must win over a non-pinned candidate of the same name.
        let is_pin = !pat.starts_with('@') && pat.split_once(':').is_some();
        let matched = db.match_pattern(pat);
        if matched.is_empty() {
            misses.push(pat.clone());
        }
        for p in matched {
            let replace = match chosen.get(&p.id.name) {
                None => {
                    order.push(p.id.name.clone());
                    true
                }
                Some(&(cur, cur_pin)) => collect_prefers(
                    is_pin,
                    db.repo_priority(&p.repo),
                    cur_pin,
                    db.repo_priority(&cur.repo),
                ),
            };
            if replace {
                chosen.insert(p.id.name.clone(), (p, is_pin));
            }
        }
    }
    guard_shell_expansion(&misses)?;
    let pkgs = order.iter().map(|n| chosen[n].0).collect();
    Ok((pkgs, misses))
}

/// Should a newly-seen candidate replace the current winner for the same name,
/// when two patterns in one `collect` both match it? Precedence, highest first:
///   1. an explicit `repo:name` pin beats a non-pinned candidate (a pin is the
///      deliberate override of source priority);
///   2. otherwise the candidate from the higher-priority repo wins;
///   3. a true tie (same pin-ness and priority — e.g. two pins of the same name)
///      keeps the first one seen, respecting the order the user listed them.
fn collect_prefers(new_pin: bool, new_prio: i32, cur_pin: bool, cur_prio: i32) -> bool {
    match (new_pin, cur_pin) {
        (true, false) => true,                 // a pin beats a non-pin
        (false, true) => false,                // a non-pin never displaces a pin
        (true, true) => false,                 // two pins: keep the first listed
        (false, false) => new_prio > cur_prio, // both non-pinned: higher priority
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
fn kept_detail(
    db: &PkgDb,
    inst: &pkg::PkgId,
    cand: &repo::AvailPkg,
    tag_prios: &[crate::config::TagPriority],
) -> String {
    let tag = inst.build_tag();
    let src = if tag.is_empty() { "official" } else { tag };
    format!(
        "{}  installed {} ({}) — {} ({}) not applied",
        inst.name,
        src,
        db.installed_priority(inst, tag_prios),
        cand.repo,
        db.repo_priority(&cand.repo),
    )
}

// ---- blacklist helpers ---------------------------------------------------

/// Is an available candidate blacklisted? Matched against its full id, series
/// and candidate repo.
fn bl_avail(cfg: &Config, p: &repo::AvailPkg) -> bool {
    cfg.blacklist_hit(&p.id.tag(), Some(p.series.as_str()), Some(p.repo.as_str()))
}

/// Is an installed package blacklisted (frozen)? Matched against its full id,
/// the series looked up from the db, and its source repo — the official repo
/// for an empty build tag, otherwise the repo owning that tag. `db` may be None
/// when a command hasn't loaded it; then only plain regex rules can match.
fn bl_installed(cfg: &Config, db: Option<&PkgDb>, i: &pkg::PkgId) -> bool {
    let series = db.and_then(|d| d.series_of(&i.name));
    let tag = i.build_tag();
    let repo = if tag.is_empty() {
        cfg.official_repo_name()
    } else {
        db.and_then(|d| d.repo_for_tag(tag))
    };
    cfg.blacklist_hit(&i.tag(), series, repo)
}

/// Frozen if either the candidate or the installed copy of the same name is
/// blacklisted (so an `@repo`-scoped rule on the installed source still freezes
/// even when the winning candidate now comes from a different repo).
fn bl_frozen(cfg: &Config, db: &PkgDb, installed: &[pkg::PkgId], p: &repo::AvailPkg) -> bool {
    bl_avail(cfg, p)
        || system::installed_by_name(installed, &p.id.name)
            .map_or(false, |i| bl_installed(cfg, Some(db), i))
}

// ---- commands ------------------------------------------------------------

/// Warn about active repos whose effective verify policy performs NO checks at
/// all — either global `VERIFY=none` with no per-repo override, or an explicit
/// `verify=none` on the repo line. Shown after `update` and in `check-updates`.
/// Names of active repos whose effective verify policy performs NO integrity
/// check at all (global `none` with no per-repo override, or `verify=none`).
fn unverified_repo_names(cfg: &Config) -> Vec<String> {
    cfg.repos
        .iter()
        .filter(|r| {
            let p = r.verify_policy(&cfg.verify);
            !p.wants(config::Check::Gpg)
                && !p.wants(config::Check::Md5)
                && !p.wants(config::Check::Sha)
        })
        .map(|r| r.name.clone())
        .collect()
}

/// Repos fetched over plaintext http:// — a network attacker can rewrite their
/// metadata and packages in transit, which (without a pinned GPG key) is the
/// same as a malicious repo.
fn insecure_http_repos(cfg: &Config) -> Vec<String> {
    cfg.repos
        .iter()
        .filter(|r| r.url.to_ascii_lowercase().starts_with("http://"))
        .map(|r| r.name.clone())
        .collect()
}

fn warn_unverified_repos(cfg: &Config) {
    let bare = unverified_repo_names(cfg);
    if bare.is_empty() {
        return;
    }
    let who = if bare.len() == cfg.repos.len() {
        "ALL active repos".to_string()
    } else {
        bare.join(", ")
    };
    println!(
        "\n{}",
        ui::purple(&format!(
            "WARNING: verification is OFF for {who} — packages from there install completely unchecked."
        ))
    );
    println!(
        "{}",
        ui::blue(
            "Are you sure? For protection set VERIFY=all in slacker.conf (or verify=gpg,md5 on the \
             repo line in repos), then run `slacker update gpg` to import keys."
        )
    );
}

/// Fetch one repo's metadata and run its GPG verification. On a verification
/// failure the repo's metadata is discarded and its name pushed to `failed`,
/// rather than aborting the whole run.
/// What happened to each repo during an update, for the end-of-run summary.
#[derive(Default)]
struct UpdateOutcomes {
    /// GPG verification failed / key missing — fixable via verify policy or key.
    failed_verify: Vec<String>,
    /// Unreachable and frozen — retried automatically on the next update.
    soft_frozen: Vec<String>,
    /// Actively distrusted (malicious / bad signature) — needs `trust-repo`.
    hard_frozen: Vec<String>,
}

fn update_one_repo(
    cfg: &Config,
    r: &config::Repo,
    track_changelog: bool,
    out: &mut UpdateOutcomes,
) {
    println!("{}", ui::blue(&format!("Updating '{}' (priority {}):", r.name, r.priority)));

    // Has this repo already been accepted (vetted, or trusted by the user)? An
    // established repo is not re-vetted here and a transient fetch failure does
    // NOT freeze it; an untrusted one (newly added / never vetted) is vetted now.
    let trusted = repo::is_trusted(&cfg.state_dir, &r.name);

    if let Err(e) = repo::update_repo(r, &cfg.cache_dir, track_changelog) {
        if trusted {
            // Established repo: most likely a transient network problem.
            println!("{}", ui::red(&format!("  FAILED: {e}")));
        } else {
            // Never-vetted repo we cannot even reach: SOFT-freeze it. The next
            // update retries it automatically; if it comes up clean it recovers
            // on its own, with no command needed.
            let _ = repo::quarantine(
                r,
                &cfg.cache_dir,
                &cfg.state_dir,
                repo::QuarantineKind::Soft,
                &format!("could not fetch metadata: {e}"),
            );
            println!("{}", ui::red(&format!("  FAILED: {e}")));
            println!(
                "{}",
                ui::yellow(&format!(
                    "  '{}' is unreachable — FROZEN for now; the next `slacker update` will retry it.",
                    r.name
                ))
            );
            out.soft_frozen.push(r.name.clone());
        }
        return;
    }

    // A repo advertising path-traversal filenames is malicious — HARD-freeze it
    // even if it was previously trusted; this never auto-recovers.
    let bad = malicious_filename_count(cfg, r);
    if bad > 0 {
        let _ = repo::quarantine(
            r,
            &cfg.cache_dir,
            &cfg.state_dir,
            repo::QuarantineKind::Hard,
            &format!("advertises {bad} unsafe/path-traversal filename(s) — malicious"),
        );
        println!(
            "{}",
            ui::red(&format!(
                "  MALICIOUS: '{}' advertises {bad} path-traversal filename(s) — FROZEN and discarded.",
                r.name
            ))
        );
        out.hard_frozen.push(r.name.clone());
        return;
    }

    // Reachable and not malicious: clear any prior (soft) quarantine and trust
    // it, so a recovered repo comes back and isn't re-vetted next time.
    repo::clear_quarantine(&cfg.state_dir, &r.name);
    if !trusted {
        repo::mark_trusted(&cfg.state_dir, &r.name);
    }

    let policy = r.verify_policy(&cfg.verify);
    let requires_gpg = policy.requires(config::Check::Gpg);
    if policy.wants(config::Check::Gpg) {
        // Re-check the served key against the pin on EVERY update (not just first
        // contact): if a repo ever serves a different key than the pinned one,
        // that is caught here as KeyChanged and the repo is frozen. On first
        // contact the key is pinned (TOFU) and its fingerprint shown.
        match gpg::import_key(r, &cfg.state_dir) {
            Ok(gpg::ImportOutcome::NewlyPinned(fpr)) => {
                println!("  {}", ui::green("GPG: pinned key (first contact)"));
                println!("    {}", ui::white(&format!("fingerprint: {fpr}")));
                println!("    {}", ui::yellow("verify this matches the repo's published key"));
            }
            Ok(gpg::ImportOutcome::AlreadyTrusted) => {}
            Err(gpg::ImportError::KeyChanged(m)) => {
                // The repo serves a different key than the pinned one: hostile.
                let _ = repo::quarantine(
                    r,
                    &cfg.cache_dir,
                    &cfg.state_dir,
                    repo::QuarantineKind::Hard,
                    &format!("GPG key changed: {m}"),
                );
                println!("{}", ui::red(&format!("  GPG: {m}")));
                println!("{}", ui::red(&format!("  '{}' has been FROZEN (possible tampering).", r.name)));
                out.hard_frozen.push(r.name.clone());
                return;
            }
            Err(gpg::ImportError::Other(m)) => {
                if requires_gpg {
                    println!("{}", ui::red(&format!("  GPG: {m}")));
                    println!("{}", ui::red("  this repo's metadata was discarded and will NOT be used."));
                    repo::invalidate_metadata(r, &cfg.cache_dir);
                    out.failed_verify.push(r.name.clone());
                    return;
                }
                // best-effort: couldn't (re-)fetch the key; if we already have it
                // in the keyring, verification below still works, else md5.
                println!("  {}", ui::dim(&format!("GPG: key unavailable ({m}) — using md5")));
            }
        }
        match gpg::verify_checksums(r, &cfg.cache_dir, &cfg.state_dir) {
            Ok(gpg::Verify::Good(signer)) => {
                println!("  {}", ui::green(&format!("GPG: good signature ({signer})")))
            }
            Ok(gpg::Verify::NoSignature) => {
                if requires_gpg {
                    println!("{}", ui::red("  GPG: required signature is missing — this repo will NOT be used."));
                    repo::invalidate_metadata(r, &cfg.cache_dir);
                    out.failed_verify.push(r.name.clone());
                } else {
                    println!("  {}", ui::dim("GPG: no signature provided (skipped)"));
                }
            }
            Ok(gpg::Verify::Tampered(m)) => {
                // Bad signature / key-substitution: hostile regardless of policy.
                let _ = repo::quarantine(
                    r,
                    &cfg.cache_dir,
                    &cfg.state_dir,
                    repo::QuarantineKind::Hard,
                    &format!("GPG verification failed: {m}"),
                );
                println!("{}", ui::red(&format!("  GPG: {m}")));
                println!("{}", ui::red(&format!("  '{}' has been FROZEN (possible tampering).", r.name)));
                out.hard_frozen.push(r.name.clone());
            }
            Ok(gpg::Verify::Unverifiable(m)) => {
                if requires_gpg {
                    println!("{}", ui::red(&format!("  GPG: {m} — this repo will NOT be used.")));
                    repo::invalidate_metadata(r, &cfg.cache_dir);
                    out.failed_verify.push(r.name.clone());
                } else {
                    // Can't authenticate, but not proven hostile: fall back to md5.
                    println!("  {}", ui::dim(&format!("GPG: {m} — using md5")));
                }
            }
            Err(e) => {
                // gpg itself failed to run — treat as a verification failure.
                println!("{}", ui::red(&format!("  GPG: {e}")));
                repo::invalidate_metadata(r, &cfg.cache_dir);
                out.failed_verify.push(r.name.clone());
            }
        }
    } else {
        println!("  {}", ui::dim("GPG: skipped (verify policy)"));
    }
}

/// Count how many `PACKAGE NAME:` entries in a repo's cached PACKAGES.TXT carry
/// an unsafe / path-traversal filename. A nonzero count means the repo is
/// actively advertising the arbitrary-write attack and must not be used.
fn malicious_filename_count(cfg: &Config, r: &config::Repo) -> usize {
    let pkgs_path = repo::meta_path(r, &cfg.cache_dir, repo::PACKAGES_TXT);
    match std::fs::read_to_string(&pkgs_path) {
        Ok(text) => text
            .lines()
            .filter_map(|l| l.strip_prefix("PACKAGE NAME:"))
            .map(|s| s.trim())
            .filter(|f| !f.is_empty() && !pkg::is_safe_filename(f))
            .count(),
        Err(_) => 0,
    }
}

/// Safety-vet a repo by fetching ONLY its metadata (no packages, nothing
/// installed) and running the full set of checks. Returns the list of problems
/// found; an empty list means the repo passed. On a clean pass the repo's key
/// is also pinned as a side effect. This is the thorough probe used by add-repo
/// and `vet-repo`, so an inexperienced user can't unknowingly wire in a hostile
/// source.
fn vet_repo(cfg: &Config, r: &config::Repo) -> Vec<String> {
    let mut issues: Vec<String> = Vec::new();
    let policy = r.verify_policy(&cfg.verify);

    // 1. Transport: plaintext http is tamperable in flight.
    if r.url.to_ascii_lowercase().starts_with("http://") {
        issues.push("served over plaintext http — a network attacker could tamper with it".into());
    }

    // 2. Fetch metadata only (downloads are size-capped; no packages fetched).
    if let Err(e) = repo::update_repo(r, &cfg.cache_dir, r.official) {
        issues.push(format!("could not fetch metadata: {e}"));
        return issues; // nothing else is checkable without metadata
    }

    // 3. Scan the raw PACKAGES.TXT for path-traversal filenames. A repo that
    //    advertises those is actively trying the arbitrary-write attack.
    let bad = malicious_filename_count(cfg, r);
    if bad > 0 {
        issues.push(format!(
            "advertises {bad} unsafe/path-traversal filename(s) — this repo is malicious"
        ));
    }

    // 4. It must actually contain installable packages for this arch.
    match repo::load_repo(r, &cfg.cache_dir, &cfg.arch) {
        Ok(p) if p.is_empty() => issues.push("no usable packages found in PACKAGES.TXT".into()),
        Ok(_) => {}
        Err(e) => issues.push(format!("metadata unreadable: {e}")),
    }

    // 5. Authentication: unless the repo's policy explicitly relaxes to md5/none
    //    (the user opting out of gpg), it must ship a signature that verifies
    //    against a key we can pin. md5 alone is integrity, not authenticity.
    if policy.wants(config::Check::Gpg) {
        let asc = repo::meta_path(r, &cfg.cache_dir, repo::CHECKSUMS_ASC);
        if !asc.exists() {
            issues.push(
                "ships no GPG signature (CHECKSUMS.md5.asc) — packages cannot be authenticated \
                 (set verify=md5 on this repo if you accept md5-only integrity)"
                    .into(),
            );
        } else {
            if let Err(e) = gpg::import_key(r, &cfg.state_dir) {
                issues.push(format!("GPG key problem: {}", e.message()));
            }
            match gpg::verify_checksums(r, &cfg.cache_dir, &cfg.state_dir) {
                Ok(gpg::Verify::Good(_)) => {}
                Ok(gpg::Verify::NoSignature) => {
                    issues.push("signature file is present but empty or unreadable".into())
                }
                Ok(gpg::Verify::Tampered(m)) => issues.push(m),
                Ok(gpg::Verify::Unverifiable(m)) => issues.push(m),
                Err(e) => issues.push(format!("GPG error: {e}")),
            }
        }
    }

    issues
}

/// Run the vetting probe on a repo and act on the verdict: a clean repo is
/// trusted (and any prior quarantine lifted); a repo that fails is FROZEN
/// (quarantined) with a clear explanation and the override command. Returns
/// true if the repo ended up trusted.
fn apply_vet(cfg: &Config, r: &config::Repo) -> bool {
    println!(
        "{}",
        ui::blue(&format!(
            "Vetting '{}' (safety check — fetches metadata only, installs nothing) ...",
            r.name
        ))
    );
    let issues = vet_repo(cfg, r);
    if issues.is_empty() {
        repo::clear_quarantine(&cfg.state_dir, &r.name);
        repo::mark_trusted(&cfg.state_dir, &r.name);
        println!(
            "  {}",
            ui::green("passed: metadata looks safe and verification is in order.")
        );
        return true;
    }

    // Unreachable is a soft freeze (retried next update); anything else the
    // probe found (malicious, unsigned, bad signature) is a hard freeze.
    let unreachable_only = issues.iter().all(|i| i.starts_with("could not fetch metadata"));
    let kind = if unreachable_only {
        repo::QuarantineKind::Soft
    } else {
        repo::QuarantineKind::Hard
    };
    let reason = issues.join("; ");
    let _ = repo::quarantine(r, &cfg.cache_dir, &cfg.state_dir, kind, &reason);
    let bar = "=".repeat(66);
    println!("{}", ui::red(&bar));
    println!("{}", ui::red(&format!("I do NOT trust repo '{}' — it has been FROZEN (quarantined).", r.name)));
    println!("{}", ui::dim("Reasons:"));
    for i in &issues {
        println!("  {}", ui::yellow(&format!("- {i}")));
    }
    println!("{}", ui::red("While quarantined it provides NO packages."));
    if unreachable_only {
        println!(
            "{}",
            ui::blue("It was only unreachable — the next `slacker update` will retry it automatically.")
        );
    } else {
        println!(
            "{}",
            ui::white(&format!(
                "If you are certain you trust it, override with:  slacker trust-repo {}",
                r.name
            ))
        );
        println!("{}", ui::dim("Doing so is entirely at your own responsibility."));
    }
    println!("{}", ui::red(&bar));
    false
}

/// Re-load config from disk and return an owned clone of the named repo, for
/// commands that have just rewritten the repos file.
fn reload_repo(cfg: &Config, name: &str) -> Result<config::Repo, String> {
    let fresh = config::Config::load_dir(&cfg.config_dir)?;
    fresh
        .repo_by_name(name)
        .cloned()
        .ok_or_else(|| format!("internal: repo '{name}' not found after writing config"))
}

fn cmd_update(cfg: &Config, mode: Option<&str>) -> Result<Outcome, String> {
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
            println!(
                "\n{}",
                ui::yellow(
                    "A key was pinned for the first time. Verify each fingerprint above against \
                     the repository's officially published key before trusting it — slacker will \
                     refuse the repo if its key ever changes."
                )
            );
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
        .map(|r| changelog::check_repo_updates(*r, &cfg.cache_dir))
        .collect();

    let needs = |s: &changelog::UpdateStatus| {
        matches!(s, changelog::UpdateStatus::Pending | changelog::UpdateStatus::Unknown)
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
        warn_unverified_repos(cfg);
        return Ok(Outcome::Ok);
    }

    // ---- selection: update many, one, or none ----
    print!(
        "\n{} ",
        hilite_keys(&format!(
            "{} repo(s) have updates. Update [a]ll / numbers (e.g. 1 2) / [n]one? [a]:",
            needing.len()
        ))
    );
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    let trimmed = line.trim();
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

    // ---- update phase: only the chosen repos (their stale MANIFEST is dropped
    // by update_repo; every other repo is left untouched). ----
    let changelog_repo = changelog::changelog_repo(&cfg.repos).map(|r| r.name.clone());
    let mut out = UpdateOutcomes::default();
    println!();
    for r in &chosen {
        let track = changelog_repo.as_deref() == Some(r.name.as_str());
        update_one_repo(cfg, *r, track, &mut out);
    }

    if !out.failed_verify.is_empty() {
        println!(
            "\n{}",
            ui::red(&format!(
                "{} repo(s) failed verification and were skipped: {}.",
                out.failed_verify.len(),
                out.failed_verify.join(", ")
            ))
        );
        println!(
            "{}",
            ui::blue(
                "If you trust one, set `verify=none` (or `verify=md5`) for it in the repos file, \
                 or import its key with `slacker update gpg`, then run `slacker update` again."
            )
        );
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
        println!(
            "{}",
            ui::blue(
                "These will stay frozen until you run `slacker trust-repo NAME` (only if you are \
                 sure), or `slacker del-repo NAME` to remove them."
            )
        );
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
        println!(
            "{}",
            ui::blue(
                "The next `slacker update` retries them automatically; they recover on their own \
                 once reachable. If one is gone for good, `slacker del-repo NAME`."
            )
        );
    }
    warn_unverified_repos(cfg);
    Ok(Outcome::Ok)
}

/// Pick the candidate a `search` hit should display. `search` returns the
/// priority winner per name, but when the package is INSTALLED we want the
/// candidate matching the installed source (same build tag), so the `[repo]`
/// label and version name where it actually came from — not merely the
/// highest-priority repo. Falls back to the winner when nothing is installed,
/// or when the installed source is no longer offered (e.g. a local/`_SBo` build
/// with no matching candidate).
fn search_display<'a>(
    winner: &'a repo::AvailPkg,
    installed: Option<&pkg::PkgId>,
    candidates: &[&'a repo::AvailPkg],
) -> &'a repo::AvailPkg {
    match installed {
        Some(i) => candidates
            .iter()
            .copied()
            .find(|c| c.id.build_tag() == i.build_tag())
            .unwrap_or(winner),
        None => winner,
    }
}

fn cmd_search(cfg: &Config, term: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let results = db.search(term);
    if results.is_empty() {
        if db.is_empty() {
            println!("No package metadata yet — run `slacker update` first.");
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
        // `p` is the priority-winner candidate that `search` returns. If this
        // package is installed, show the candidate matching the INSTALLED source
        // instead, so the [repo] label and version name where it actually came
        // from rather than merely the highest-priority repo. The blacklist test
        // stays on the winner `p`, so the frozen/blacklisted marking is
        // unchanged.
        let inst = system::installed_by_name(&installed, &p.id.name);
        let cands = db.candidates(&p.id.name);
        let shown = search_display(p, inst, &cands);

        let mark = if inst.is_some() {
            ui::green(&format!("{:<11}", "installed"))
        } else {
            ui::red(&format!("{:<11}", "uninstalled"))
        };
        let bl = if bl_frozen(cfg, &db, &installed, p) {
            ui::purple(" [blacklisted]")
        } else {
            String::new()
        };
        // Surface the other repos that ship this name (search shows one winner
        // per name); `info <name>` gives the full per-repo candidate list.
        let mut others: Vec<&str> = Vec::new();
        for c in &cands {
            let r = c.repo.as_str();
            if r != shown.repo && !others.contains(&r) {
                others.push(r);
            }
        }
        let also = if others.is_empty() {
            String::new()
        } else {
            ui::dim(&format!("  (also: {})", others.join(", ")))
        };
        println!(
            "{} {} {}{}  {}{}{}",
            ui::cyan(&format!("[{}]", shown.repo)),
            mark,
            ui::white(&shown.id.name),
            ui::dim(&format!("-{}", shown.id.version)),
            shown.summary,
            bl,
            also
        );
    }
    Ok(Outcome::Ok)
}

fn cmd_file_search(cfg: &Config, filename: &str) -> Result<Outcome, String> {
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
    let hits = manifest::file_search(&cfg.repos, &cfg.cache_dir, filename)?;
    let found = !hits.is_empty();

    // file-search is a plain substring match over MANIFEST paths, not a glob.
    // If the query carries shell-wildcard characters the user almost certainly
    // meant them as globs, so flag that they are taken literally.
    let has_glob = filename.contains(|c| matches!(c, '*' | '?' | '[' | ']'));
    if has_glob {
        eprintln!(
            "{}",
            ui::yellow("note: file-search matches a literal substring, not a glob —")
        );
        eprintln!("      '*', '?', '[' and ']' are treated as ordinary characters, not wildcards.");
    }

    for h in hits {
        let pkgname = pkg::PkgId::parse(&h.package).map(|p| p.name).unwrap_or_else(|| h.package.clone());
        let mark = if system::is_installed(&installed, &pkgname) {
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
            eprintln!("      run once as: sudo slacker file-search {filename}");
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

fn cmd_info(cfg: &Config, name: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let candidates = db.candidates(name);
    if candidates.is_empty() {
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
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    match system::installed_by_name(&installed, name) {
        Some(inst) => println!("{} {}", ui::blue("Installed:"), ui::green(&inst.tag())),
        None => println!("{} {}", ui::blue("Installed:"), ui::dim("(none)")),
    }
    println!("{}", ui::blue("Available candidates (highest priority first):"));
    for p in candidates {
        let csize = p.size_k.map(|k| format!("{k} K")).unwrap_or_else(|| "?".into());
        let usize_ = p.size_uncompressed_k.map(|k| format!("{k} K")).unwrap_or_else(|| "?".into());
        let md5 = if p.md5.is_some() { "md5 ok" } else { "no md5" };
        let bl = if bl_frozen(cfg, &db, &installed, p) {
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

/// Short label for an effective verify policy, e.g. "all", "none", "gpg,md5".
fn verify_label(p: &config::VerifyPolicy) -> String {
    match p {
        config::VerifyPolicy::All => "all".to_string(),
        config::VerifyPolicy::None => "none".to_string(),
        config::VerifyPolicy::Required(v) => {
            v.iter().map(|c| c.label()).collect::<Vec<_>>().join(",")
        }
    }
}

/// Attribute each installed package to a source by its build tag (pure core).
/// An empty tag is the official repo; a tag the `resolve_repo` closure maps to a
/// binary repo goes there; a tag matching a declared tag-priority rule (e.g.
/// `_SBo`) maps to that rule's name; any other tag is itself the source. An
/// installed package is never "untracked" — its build tag is the user's own
/// choice and a legitimate source. Returns (per-repo, per-tag-rule-by-name,
/// per-other-tag).
fn attribute_tags(
    official_repo: Option<&str>,
    tag_priorities: &[config::TagPriority],
    resolve_repo: impl Fn(&str) -> Option<String>,
    installed: &[pkg::PkgId],
) -> (
    HashMap<String, usize>,
    HashMap<String, usize>,
    HashMap<String, usize>,
) {
    let mut per_repo: HashMap<String, usize> = HashMap::new();
    let mut per_rule: HashMap<String, usize> = HashMap::new();
    let mut per_tag: HashMap<String, usize> = HashMap::new();
    for p in installed {
        let tag = p.build_tag();
        let repo = if tag.is_empty() {
            official_repo.map(|s| s.to_string())
        } else {
            resolve_repo(tag)
        };
        if let Some(r) = repo {
            *per_repo.entry(r).or_default() += 1;
        } else if tag.is_empty() {
            // Empty tag but no official repo configured: a real edge.
            *per_tag.entry("(no official repo)".to_string()).or_default() += 1;
        } else if let Some(tp) = tag_priorities.iter().find(|tp| tp.tag == tag) {
            *per_rule.entry(tp.name.clone()).or_default() += 1;
        } else {
            *per_tag.entry(tag.to_string()).or_default() += 1;
        }
    }
    (per_repo, per_rule, per_tag)
}

/// Attribute installed packages using the available-package DB to resolve which
/// repo serves a build tag. With no DB, all maps are empty.
fn installed_attribution(
    cfg: &Config,
    db: Option<&PkgDb>,
    installed: &[pkg::PkgId],
) -> (
    HashMap<String, usize>,
    HashMap<String, usize>,
    HashMap<String, usize>,
) {
    match db {
        Some(db) => attribute_tags(
            cfg.official_repo_name(),
            &cfg.tag_priorities,
            |t| db.repo_for_tag(t).map(|s| s.to_string()),
            installed,
        ),
        None => (HashMap::new(), HashMap::new(), HashMap::new()),
    }
}

/// `list-repos`: show every configured repository with its priority, effective
/// verify policy, URL, and how many installed packages came from it; then the
/// build-tag priority lines and a grand total. Per-repo counts need the package
/// DB (from `update`); without it the repo list still prints, counts as `?`.
fn cmd_list_repos(cfg: &Config) -> Result<Outcome, String> {
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (db, missing) = PkgDb::load_available(cfg);
    let (per_repo, per_rule, per_other) = installed_attribution(cfg, Some(&db), &installed);

    let repos = cfg.repos_by_priority();
    // Only a repo whose metadata is missing shows `?`; every other repo gets its
    // real count. This isolates an un-updated / wrong-URL repo instead of blanking
    // the whole column.
    let count_str = |name: &str| -> String {
        if missing.iter().any(|m| m == name) {
            "?".to_string()
        } else {
            per_repo.get(name).copied().unwrap_or(0).to_string()
        }
    };
    let once = std::iter::once;
    let wn = repos.iter().map(|r| r.name.len()).chain(once(4)).max().unwrap();
    let wi = repos.iter().map(|r| count_str(&r.name).len()).chain(once(4)).max().unwrap();
    let wv = repos
        .iter()
        .map(|r| verify_label(r.verify_policy(&cfg.verify)).len())
        .chain(once(6))
        .max()
        .unwrap();
    let sep = ui::dim(" | ");

    println!("{}", ui::blue("Configured repositories (highest priority first):"));
    println!(
        "  {}{}{}{}{}{}{}{}{}",
        ui::blue(&format!("{:>4}", "Pri")),
        sep,
        ui::blue(&format!("{:<wn$}", "Name")),
        sep,
        ui::blue(&format!("{:>wi$}", "Inst")),
        sep,
        ui::blue(&format!("{:<wv$}", "Verify")),
        sep,
        ui::blue("URL"),
    );
    println!(
        "  {}",
        ui::dim(&format!(
            "{}-+-{}-+-{}-+-{}-+-{}",
            "-".repeat(4),
            "-".repeat(wn),
            "-".repeat(wi),
            "-".repeat(wv),
            "-".repeat(3)
        ))
    );
    for r in &repos {
        let pol = r.verify_policy(&cfg.verify);
        let vcolor: fn(&str) -> String = match pol {
            config::VerifyPolicy::None => ui::red,
            config::VerifyPolicy::All => ui::green,
            config::VerifyPolicy::Required(_) => ui::yellow,
        };
        let mut line = format!(
            "  {}{}{}{}{}{}{}{}{}",
            ui::dim(&format!("{:>4}", r.priority)),
            sep,
            ui::white(&format!("{:<wn$}", r.name)),
            sep,
            ui::cyan(&format!("{:>wi$}", count_str(&r.name))),
            sep,
            vcolor(&format!("{:<wv$}", verify_label(pol))),
            sep,
            ui::dim(&r.url),
        );
        if r.official {
            line.push_str(&ui::cyan("  (official)"));
        }
        if r.immutable {
            line.push_str(&ui::cyan("  (immutable)"));
        }
        if r.subtree {
            line.push_str(&ui::cyan("  (subtree)"));
        }
        if repo::is_hard_quarantined(&cfg.state_dir, &r.name) {
            line.push_str(&ui::red("  [FROZEN]"));
        } else if repo::is_quarantined(&cfg.state_dir, &r.name) {
            line.push_str(&ui::yellow("  [unreachable — retrying]"));
        }
        println!("{line}");
    }

    if !cfg.tag_priorities.is_empty() {
        println!();
        println!("{}", ui::blue("Build-tag priorities:"));
        let rule_inst = |t: &config::TagPriority| per_rule.get(&t.name).copied().unwrap_or(0);
        let wtn = cfg.tag_priorities.iter().map(|t| t.name.len()).chain(once(4)).max().unwrap();
        let wtt = cfg.tag_priorities.iter().map(|t| t.tag.len()).chain(once(3)).max().unwrap();
        let wti = cfg
            .tag_priorities
            .iter()
            .map(|t| rule_inst(t).to_string().len())
            .chain(once(4))
            .max()
            .unwrap();
        println!(
            "  {}{}{}{}{}{}{}",
            ui::blue(&format!("{:>4}", "Pri")),
            sep,
            ui::blue(&format!("{:<wtn$}", "Name")),
            sep,
            ui::blue(&format!("{:<wtt$}", "Tag")),
            sep,
            ui::blue(&format!("{:>wti$}", "Inst")),
        );
        for t in &cfg.tag_priorities {
            let inst = rule_inst(t);
            let mut line = format!(
                "  {}{}{}{}{}{}{}",
                ui::dim(&format!("{:>4}", t.priority)),
                sep,
                ui::white(&format!("{:<wtn$}", t.name)),
                sep,
                ui::cyan(&format!("{:<wtt$}", t.tag)),
                sep,
                ui::cyan(&format!("{:>wti$}", inst)),
            );
            // A declared rule that matches no installed package is worth a quiet
            // heads-up (often a typo in the tag), but it is not an error.
            if inst == 0 {
                line.push_str(&ui::yellow("  (declared, no installed package)"));
            }
            println!("{line}");
        }
    }

    println!();
    println!(
        "{} {}",
        ui::blue("Total installed packages:"),
        ui::white(&installed.len().to_string())
    );
    if !per_other.is_empty() {
        let total_other: usize = per_other.values().sum();
        let mut items: Vec<(&String, &usize)> = per_other.iter().collect();
        items.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let breakdown =
            items.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ");
        println!(
            "{} {} {}",
            ui::blue("Installed under other build tags:"),
            ui::white(&total_other.to_string()),
            ui::dim(&format!("[{breakdown}]")),
        );
    }
    if !missing.is_empty() {
        println!(
            "{}",
            ui::yellow(&format!(
                "no metadata yet for: {} (shown as ?) — run `slacker update`, or check the repo URL",
                missing.join(", ")
            ))
        );
    }
    Ok(Outcome::Ok)
}

/// Human-friendly "time since" for a file mtime, e.g. "3m", "2h", "5d".
fn ago(t: std::time::SystemTime) -> String {
    match t.elapsed() {
        Ok(d) => {
            let s = d.as_secs();
            if s < 90 {
                format!("{s}s")
            } else if s < 90 * 60 {
                format!("{}m", s / 60)
            } else if s < 36 * 3600 {
                format!("{}h", s / 3600)
            } else {
                format!("{}d", s / 86400)
            }
        }
        Err(_) => "just now".to_string(),
    }
}

/// `status`: a one-shot health check of the whole setup. Every line reports a
/// real, verifiable fact (config is already structurally validated at load, so
/// here we check initialisation and reachability) and ends with a truthful
/// verdict plus concrete next steps. Read-only; safe to run any time.
/// True if an executable `name` is found in any of `dirs`. Split out from
/// [`tool_on_path`] so it can be unit-tested without touching the real `$PATH`.
fn tool_in_dirs(name: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| {
        let p = dir.join(name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            p.is_file()
        }
    })
}

/// True if `name` resolves to an executable on the current `$PATH` — i.e. a
/// `Command::new(name)` would find it. Used by `status` to flag a missing
/// external tool (pkgtools, gpg, bzip2, ...) up front, before its absence
/// surfaces as a confusing failure in the middle of an operation.
fn tool_on_path(name: &str) -> bool {
    match std::env::var_os("PATH") {
        Some(path) => tool_in_dirs(name, &std::env::split_paths(&path).collect::<Vec<_>>()),
        None => false,
    }
}

/// What auditing slacker's OWN files turned up. Counts are totals across the
/// whole tree; the `sample_*` vectors keep up to `AUDIT_SAMPLE` offenders each
/// so the report can name a few without flooding the screen.
#[derive(Default)]
struct PathAudit {
    checked: usize,
    not_root: usize,
    world_writable: usize,
    group_writable: usize,
    symlinks: usize,
    unreadable: usize,
    sample_not_root: Vec<PathBuf>,
    sample_world_writable: Vec<PathBuf>,
    sample_group_writable: Vec<PathBuf>,
    sample_symlinks: Vec<(PathBuf, PathBuf)>,
}

const AUDIT_SAMPLE: usize = 3;

fn push_capped<T>(v: &mut Vec<T>, item: T) {
    if v.len() < AUDIT_SAMPLE {
        v.push(item);
    }
}

impl PathAudit {
    /// No ownership/permission/symlink problems at all.
    fn clean(&self) -> bool {
        self.not_root == 0
            && self.world_writable == 0
            && self.group_writable == 0
            && self.symlinks == 0
    }
    /// A problem that lets a non-root user redirect or rewrite slacker's state —
    /// i.e. it undermines the trust model, not just tidiness.
    fn severe(&self) -> bool {
        self.world_writable > 0 || self.symlinks > 0
    }
}

#[cfg(unix)]
fn check_perms(a: &mut PathAudit, path: &Path, meta: &std::fs::Metadata) {
    use std::os::unix::fs::MetadataExt;
    if meta.uid() != 0 {
        a.not_root += 1;
        push_capped(&mut a.sample_not_root, path.to_path_buf());
    }
    let mode = meta.mode();
    if mode & 0o002 != 0 {
        a.world_writable += 1;
        push_capped(&mut a.sample_world_writable, path.to_path_buf());
    } else if mode & 0o020 != 0 {
        a.group_writable += 1;
        push_capped(&mut a.sample_group_writable, path.to_path_buf());
    }
}
#[cfg(not(unix))]
fn check_perms(_a: &mut PathAudit, _path: &Path, _meta: &std::fs::Metadata) {}

/// Audit slacker's OWN files (config + cache + state) for tampering and unsafe
/// permissions. On Slackware these are all owned by root and not writable by
/// anyone else; the trust state in the cache (pinned GPG fingerprints,
/// trusted/quarantine markers) is security-critical, so a world-writable file
/// or an unexpected symlink there is a red flag — slacker itself creates no
/// symlinks under these paths. The walk uses lstat and NEVER follows a symlink,
/// so a planted link cannot lure it outside the tree. Only the roots are
/// followed, since an admin may legitimately mount one via a symlink.
fn audit_owned_paths(roots: &[&Path]) -> PathAudit {
    let mut a = PathAudit::default();
    let mut stack: Vec<PathBuf> = Vec::new();
    for r in roots {
        // Follow the root (it may be a legitimate symlink to a real directory);
        // entries inside are what we scrutinise for stray links.
        if let Ok(meta) = std::fs::metadata(r) {
            a.checked += 1;
            check_perms(&mut a, r, &meta);
            if meta.is_dir() {
                stack.push(r.to_path_buf());
            }
        }
    }
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => {
                // Almost always a root-only (0700) subdir we cannot enter as a
                // non-root user; report it so the user can re-run under sudo.
                a.unreadable += 1;
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            a.checked += 1;
            if meta.file_type().is_symlink() {
                a.symlinks += 1;
                let target = std::fs::read_link(&path).unwrap_or_default();
                push_capped(&mut a.sample_symlinks, (path, target));
                continue; // never follow a symlink
            }
            check_perms(&mut a, &path, &meta);
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    a
}

/// Format up to `AUDIT_SAMPLE` offending paths, with a "(+N more)" tail.
fn fmt_path_sample(sample: &[PathBuf], total: usize) -> String {
    let mut s = sample.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
    if total > sample.len() {
        s.push_str(&format!(" (+{} more)", total - sample.len()));
    }
    s
}

/// Same, for symlinks shown as `link -> target`.
fn fmt_symlink_sample(sample: &[(PathBuf, PathBuf)], total: usize) -> String {
    let mut s = sample
        .iter()
        .map(|(l, t)| format!("{} -> {}", l.display(), t.display()))
        .collect::<Vec<_>>()
        .join(", ");
    if total > sample.len() {
        s.push_str(&format!(" (+{} more)", total - sample.len()));
    }
    s
}

/// `status`, the setup doctor. It deliberately does its OWN, resilient loading
/// (it is dispatched in `run` BEFORE the strict `Config::load_dir`): a broken
/// configuration is the very thing it must diagnose, so it first checks the
/// environment and validates the config files, and only then — if they are
/// sound — runs the full health report against a loaded `Config`.
fn cmd_status(config_dir: &std::path::Path) -> Result<Outcome, String> {
    let ok = ui::green("\u{2713}"); // ✓
    let bad = ui::red("\u{2717}"); // ✗
    let warn = ui::yellow("!");
    let srow = |mark: &str, label: &str, detail: &str| {
        println!("  {} {} {}", mark, ui::white(&format!("{:<10}", label)), detail);
    };

    println!("{}", ui::blue("Environment"));

    // The config directory must exist before anything else can be read.
    if !config_dir.exists() {
        srow(&bad, "Config dir", &ui::red(&format!("{} does not exist", config_dir.display())));
        println!(
            "\n{}",
            ui::yellow("! create it with a `repos` file (see `man slacker`), then re-run `slacker status`.")
        );
        return Err("configuration directory is missing".into());
    }

    // External tools slacker shells out to. The pkgtools are essential — without
    // them install/upgrade/remove cannot run at all; the rest each degrade one
    // feature when absent. Looked up on $PATH, not executed.
    let missing_pt: Vec<&str> =
        ["installpkg", "upgradepkg", "removepkg"].into_iter().filter(|&t| !tool_on_path(t)).collect();
    if missing_pt.is_empty() {
        srow(&ok, "Pkgtools", &ui::dim("installpkg, upgradepkg, removepkg present"));
    } else {
        srow(
            &bad,
            "Pkgtools",
            &ui::red(&format!(
                "MISSING: {} — install/upgrade/remove cannot run",
                missing_pt.join(", ")
            )),
        );
    }
    let missing_aux: Vec<String> = [
        ("gpg", "signature verification"),
        ("bzip2", "file-search (MANIFEST)"),
        ("sha256sum", "sha256 checks"),
        ("diff", "new-config diffs"),
    ]
    .into_iter()
    .filter(|&(t, _)| !tool_on_path(t))
    .map(|(t, why)| format!("{t} ({why})"))
    .collect();
    if missing_aux.is_empty() {
        srow(&ok, "Tools", &ui::dim("gpg, bzip2, sha256sum, diff present"));
    } else {
        srow(&warn, "Tools", &ui::yellow(&format!("missing: {}", missing_aux.join(", "))));
    }

    // Config syntax + cross-checks, via the SAME validator the `add-repo`/`add-tag`
    // editors use. This catches a line that does not start with a priority number,
    // duplicate priorities, a second `official`, a bad `verify=`, a `mirror` line
    // with no active mirror, duplicate names/tags, and an empty repo set — the
    // problems that otherwise abort every other command before it can start.
    let repos_text = std::fs::read_to_string(config_dir.join("repos")).unwrap_or_default();
    match config::validate_repos_text(config_dir, &repos_text) {
        Ok(()) => srow(&ok, "Config", &ui::dim("repos and mirrors parse and cross-check cleanly")),
        Err(e) => {
            srow(&bad, "Config", &ui::red(&e));
            println!(
                "\n{}",
                ui::yellow("! fix the `repos`/`mirrors` problem above, then re-run `slacker status`.")
            );
            return Err("configuration is invalid".into());
        }
    }

    // Everything needed to load is present and valid — run the full report.
    let cfg = Config::load_dir(config_dir)?;
    migrate_state(&cfg);
    println!();
    status_full(&cfg)
}

/// The Slackware distribution directory segment for this system, e.g.
/// `slackware64-current`, `slackware64-15.0`, `slackware-15.0`. The arch prefix
/// is `slackware64` on x86_64 and `slackware` otherwise (the official 32-bit
/// tree); the release is `current` on -current, else the numeric `VERSION_ID`
/// read from /etc/os-release (e.g. `15.0`). Returns None when the release cannot
/// be determined (not -current and no VERSION_ID), so callers fail open rather
/// than build a wrong path.
///
/// This is the single place that turns "what release/arch am I on" into a
/// distribution path; anywhere slacker would otherwise hardcode
/// `slackware64-current` should derive it here so a non-current system gets its
/// own version substituted. (Slackware ARM uses slackwarearm/slackwareaarch64
/// and lives on different mirrors; only the x86 trees are reachable on the
/// osuosl reference, so a wrong ARM path simply fails open below.)
pub(crate) fn slackware_dir(arch: &str) -> Option<String> {
    slackware_dir_parts(
        arch,
        system::version_codename().as_deref(),
        system::version_id().as_deref(),
    )
}

/// Pure core of [`slackware_dir`], split out so the release/arch logic is
/// unit-testable without reading /etc/os-release.
fn slackware_dir_parts(arch: &str, codename: Option<&str>, version_id: Option<&str>) -> Option<String> {
    let prefix = if arch == "x86_64" { "slackware64" } else { "slackware" };
    let release = if is_current_codename(codename) {
        "current".to_string()
    } else {
        // Stable: the numeric VERSION_ID. -current is identified by the codename
        // (a real -current reports VERSION_ID=15.0 with NO suffix), so any
        // trailing development `+` seen here is spurious — e.g. a hand-made
        // os-release that wrote "15.0+" — and is part of no mirror directory.
        // Strip it: slackware64-15.0, never slackware64-15.0+.
        let v = version_id?
            .trim()
            .trim_matches('"')
            .trim_end_matches('+')
            .trim();
        if v.is_empty() {
            return None;
        }
        v.to_string()
    };
    Some(format!("{prefix}-{release}"))
}

/// -current is marked by `VERSION_CODENAME=current` in /etc/os-release; that
/// codename is the reliable signal. A real -current reports VERSION_ID=15.0 with
/// no suffix, while stable releases carry a different codename (e.g. "stable").
pub(crate) fn is_current_codename(codename: Option<&str>) -> bool {
    codename
        .map(|c| c.trim().eq_ignore_ascii_case("current"))
        .unwrap_or(false)
}

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

/// osuosl reference PACKAGES.TXT for the FRESHNESS check. On -current the main
/// tree moves, so we read the release root; on STABLE the root is a frozen
/// snapshot and the `patches/` subtree is what actually changes when security
/// updates land, so we read patches/PACKAGES.TXT. `dir` is the
/// `slackware{64}-{release}` segment.
fn osuosl_freshness_url(dir: &str, is_current: bool) -> String {
    let sub = if is_current { "" } else { "/patches" };
    format!("http://ftp.osuosl.org/pub/slackware/{dir}{sub}/PACKAGES.TXT")
}

/// The (upstream, your-mirror) PACKAGES.TXT pair the freshness check compares,
/// or None if the release/arch can't be determined or there is no official
/// mirror. -current → the main-tree root on both sides; STABLE → the `patches/`
/// tree on both sides (the frozen root never moves; patches/ is where the server
/// publishes security updates, so that is the timestamp that signals freshness).
/// On stable the "your" side prefers a configured `patches` subtree repo, falling
/// back to deriving patches/ from the official mirror base.
fn freshness_urls(cfg: &Config) -> Option<(String, String)> {
    let official = cfg.repos.iter().find(|r| r.official)?;
    // Freshness checks YOUR MIRROR, so key everything off the mirror's own
    // release (read from its URL), not the running system's os-release — the two
    // can disagree mid dist-upgrade (mirror already re-pointed to -current while
    // os-release still says 15.0), and comparing across releases is meaningless.
    let mirror_release = dist::parse_release_from_url(&official.url)?;
    let is_current = matches!(mirror_release, dist::Release::Current);
    let prefix = if cfg.arch == "x86_64" { "slackware64" } else { "slackware" };
    let dir = format!("{prefix}-{}", dist::release_suffix(&mirror_release));
    let upstream = osuosl_freshness_url(&dir, is_current);

    let mine = if is_current {
        official.join_url(repo::PACKAGES_TXT)
    } else {
        match cfg.repos.iter().find(|r| r.subtree && r.url.contains("/patches")) {
            Some(p) => p.join_url(repo::PACKAGES_TXT),
            None => format!(
                "{}/patches/{}",
                official.url.trim_end_matches('/'),
                repo::PACKAGES_TXT
            ),
        }
    };
    Some((upstream, mine))
}

/// A mirror lagging upstream by more than this is reported as stale.
pub(crate) const FRESHNESS_MAX_LAG_SECS: i64 = 48 * 3600;

/// How long to wait on each freshness probe before giving up (fail-open).
const FRESHNESS_TIMEOUT_SECS: u64 = 8;

/// Parse the timestamp from a PACKAGES.TXT header line into epoch seconds.
/// The line is always `PACKAGES.TXT;  Wed Jun 24 22:11:34 UTC 2026` — a
/// `date -u` stamp (`%a %b %e %H:%M:%S UTC %Y`). Any deviation returns None so
/// the caller fails open instead of guessing.
pub(crate) fn parse_packages_date(line: &str) -> Option<i64> {
    let ts = line.split_once(';')?.1.trim(); // "Wed Jun 24 22:11:34 UTC 2026"
    let mut f = ts.split_whitespace();
    let _weekday = f.next()?;
    let mon = month_num(f.next()?)?;
    let day: i64 = f.next()?.parse().ok()?;
    let hms = f.next()?;
    if f.next()? != "UTC" {
        return None;
    }
    let year: i64 = f.next()?.parse().ok()?;
    let mut hp = hms.split(':');
    let h: i64 = hp.next()?.parse().ok()?;
    let mi: i64 = hp.next()?.parse().ok()?;
    let s: i64 = hp.next()?.parse().ok()?;
    if hp.next().is_some() {
        return None; // trailing junk after seconds
    }
    Some(crate::history::to_naive_epoch((year, mon, day, h, mi, s)))
}

fn month_num(m: &str) -> Option<i64> {
    Some(match m {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

/// True when `upstream` leads `mirror` by more than the staleness threshold.
pub(crate) fn mirror_is_stale(upstream_epoch: i64, mirror_epoch: i64) -> bool {
    upstream_epoch - mirror_epoch > FRESHNESS_MAX_LAG_SECS
}

/// Render a lag in seconds as a compact "Nd Mh" / "Nh" / "Nm" string.
fn humanize_lag(secs: i64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    if days >= 1 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours >= 1 {
        format!("{hours}h")
    } else {
        format!("{}m", (secs % 3600) / 60)
    }
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

fn cmd_find_mirror(config_dir: &std::path::Path) -> Result<Outcome, String> {
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
    // for you. Offer the fastest few; each line is the one the `mirrors` file
    // expects (the release root for this arch).
    println!();
    let propose = FIND_MIRROR_PROPOSE.min(ranked.len());
    if propose == 1 {
        let best = &ranked[0];
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
    for m in ranked.iter().take(propose) {
        println!(
            "    {}",
            ui::white(&format!("{}/{}/", m.base_url.trim_end_matches('/'), dir))
        );
    }
    println!("  {}", ui::dim("(slacker will not change your mirror automatically)"));
    Ok(Outcome::Ok)
}

fn status_full(cfg: &Config) -> Result<Outcome, String> {
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (db, missing_meta) = PkgDb::load_available(cfg);
    let repos = cfg.repos_by_priority();
    // State flags feeding the ordered "next steps" recipe at the end.
    let mut gpg_missing = false;
    let mut metadata_incomplete = false;
    let mut pending = false;
    let mut unreachable = false;
    let mut tampered: Vec<String> = Vec::new();
    let mut security_problem = false;

    let ok = ui::green("\u{2713}"); // ✓
    let bad = ui::red("\u{2717}"); // ✗
    let warn = ui::yellow("!");
    let info = ui::blue("\u{00B7}"); // ·
    let srow = |mark: &str, label: &str, detail: &str| {
        println!("  {} {} {}", mark, ui::white(&format!("{:<10}", label)), detail);
    };

    // ---------- Setup ----------
    println!("{}", ui::blue("Setup"));

    match cfg.repos.iter().find(|r| r.official) {
        Some(r) => srow(&ok, "Mirror", &ui::dim(&r.url)),
        None => srow(&info, "Mirror", &ui::dim("no official repo configured (third-party only)")),
    }

    // Mirror freshness vs upstream. Every other row reads local state; this is
    // the one that touches the network. On -current we compare the main-tree
    // PACKAGES.TXT; on STABLE the release root is frozen, so we compare the
    // patches/ PACKAGES.TXT — that is the timestamp that moves when the server
    // publishes security updates. Short timeout, FAILS OPEN: an unreachable or
    // unparseable side never yields a false "stale" verdict and never blocks.
    if let Some((up_url, mine_url)) = freshness_urls(cfg) {
        let t = std::time::Duration::from_secs(FRESHNESS_TIMEOUT_SECS);
        let up = download::first_line(&up_url, t)
            .ok()
            .and_then(|l| parse_packages_date(&l));
        let mine = download::first_line(&mine_url, t)
            .ok()
            .and_then(|l| parse_packages_date(&l));
        match (up, mine) {
            (Some(up), Some(mine)) if mirror_is_stale(up, mine) => srow(
                &warn,
                "Freshness",
                &ui::yellow(&format!(
                    "your mirror is {} behind upstream — run `slacker find-mirror`",
                    humanize_lag(up - mine)
                )),
            ),
            (Some(_), Some(_)) => {
                srow(&ok, "Freshness", &ui::dim("mirror is in sync with upstream"))
            }
            _ => srow(
                &info,
                "Freshness",
                &ui::dim("could not check (upstream or mirror unreachable)"),
            ),
        }
    }

    srow(&ok, "Repos", &ui::dim(&format!("{} configured, priorities distinct", repos.len())));

    if !cfg.tag_priorities.is_empty() {
        let tags =
            cfg.tag_priorities.iter().map(|t| t.tag.as_str()).collect::<Vec<_>>().join(", ");
        srow(&ok, "Tag rules", &ui::dim(&format!("{} ({tags})", cfg.tag_priorities.len())));
    }

    // Verification policy
    let unverified = unverified_repo_names(cfg);
    if unverified.is_empty() {
        srow(&ok, "Verify", &ui::green("every repo verifies packages"));
    } else {
        srow(&warn, "Verify", &ui::yellow(&format!("OFF for: {}", unverified.join(", "))));
    }

    // Transport security
    let insecure = insecure_http_repos(cfg);
    if insecure.is_empty() {
        srow(&ok, "Transport", &ui::dim("all repos use https or file"));
    } else {
        srow(
            &warn,
            "Transport",
            &ui::yellow(&format!(
                "plaintext http (MITM-able): {} — prefer https",
                insecure.join(", ")
            )),
        );
    }

    // Release match: any active repo built for a different Slackware release than
    // this system is a foot-gun (release-mixing) — installing from it is refused.
    if let Some(sys) = system_release() {
        let mism: Vec<String> = cfg
            .repos
            .iter()
            .filter_map(|r| {
                repo_release_token(&r.url)
                    .filter(|rel| *rel != sys)
                    .map(|rel| format!("{} → {}", r.name, dist::show(&rel)))
            })
            .collect();
        if mism.is_empty() {
            srow(&ok, "Repo release", &ui::dim(&format!("all match this system ({})", dist::show(&sys))));
        } else {
            srow(
                &warn,
                "Repo release",
                &ui::yellow(&format!(
                    "MISMATCH (installs refused without --yes): {} — system is {}",
                    mism.join(", "),
                    dist::show(&sys)
                )),
            );
        }
    }

    // Arch match: a repo whose packages are ALL built for a foreign architecture
    // (ignoring noarch) is the wrong tree for this system — installs from it are
    // refused. Judged from the package arch field, never the URL (folder names
    // lie: alienbob uses /x86_64/, conraid slackware64-current, SBo none).
    {
        let sys_fam = arch_family(&cfg.arch);
        let foreign: Vec<String> = cfg
            .repos
            .iter()
            .filter_map(|r| {
                let archs = db.repo_archs(&r.name);
                let non_noarch: Vec<&str> =
                    archs.iter().copied().filter(|a| *a != "noarch").collect();
                if non_noarch.is_empty() {
                    return None; // only noarch, or no metadata — nothing to judge
                }
                if non_noarch.iter().any(|a| arch_family(a) == sys_fam) {
                    return None; // provides at least some packages for this arch
                }
                Some(format!("{} ({})", r.name, non_noarch.join("/")))
            })
            .collect();
        if foreign.is_empty() {
            srow(&ok, "Repo arch", &ui::dim(&format!("all match this system ({})", cfg.arch)));
        } else {
            srow(
                &warn,
                "Repo arch",
                &ui::yellow(&format!(
                    "MISMATCH (installs refused): {} — system is {}",
                    foreign.join(", "),
                    cfg.arch
                )),
            );
        }
    }

    // Frozen repos: hard (needs trust-repo) vs soft (unreachable, auto-retried)
    let hard: Vec<&config::Repo> = cfg
        .repos
        .iter()
        .filter(|r| repo::is_hard_quarantined(&cfg.state_dir, &r.name))
        .collect();
    let soft: Vec<&config::Repo> = cfg
        .repos
        .iter()
        .filter(|r| {
            repo::is_quarantined(&cfg.state_dir, &r.name)
                && !repo::is_hard_quarantined(&cfg.state_dir, &r.name)
        })
        .collect();
    if hard.is_empty() && soft.is_empty() {
        srow(&ok, "Repo trust", &ui::dim("no repos are frozen"));
    } else {
        if !hard.is_empty() {
            let names: Vec<String> = hard.iter().map(|r| r.name.clone()).collect();
            srow(
                &warn,
                "Repo trust",
                &ui::red(&format!(
                    "FROZEN (unsafe, unused): {} — trust with `trust-repo NAME`, or `del-repo NAME`",
                    names.join(", ")
                )),
            );
            for r in &hard {
                if let Some(reason) = repo::quarantine_reason(&cfg.state_dir, &r.name) {
                    println!("             {}", ui::dim(&format!("{}: {reason}", r.name)));
                }
            }
        }
        if !soft.is_empty() {
            let names: Vec<String> = soft.iter().map(|r| r.name.clone()).collect();
            srow(
                &warn,
                "Repo trust",
                &ui::yellow(&format!(
                    "unreachable (will retry on next update): {}",
                    names.join(", ")
                )),
            );
        }
    }

    // GPG keys — verify EMPIRICALLY. A repo is "covered" if its cached CHECKSUMS
    // signature verifies against our keyring, regardless of WHICH key signed it:
    // extras/testing/patches reuse slackware's key, so they verify without a key
    // of their own. Only repos whose policy wants gpg are relevant, and the
    // keyring is root-only, so a non-root run can't check.
    let need_gpg: Vec<&config::Repo> = repos
        .iter()
        .filter(|r| r.verify_policy(&cfg.verify).wants(config::Check::Gpg))
        .copied()
        .collect();
    if !need_gpg.is_empty() {
        let keyring = cfg.state_dir.join("gpg");
        let keyring_has_keys = std::fs::read_dir(&keyring)
            .map(|d| {
                d.flatten().any(|e| e.file_name().to_string_lossy().ends_with("-GPG-KEY"))
            })
            .unwrap_or(false);
        if current_uid() != Some(0) {
            srow(&info, "GPG keys", &ui::dim("cannot verify without root"));
        } else if !keyring_has_keys {
            srow(&warn, "GPG keys", &ui::yellow("none imported yet"));
            gpg_missing = true;
        } else {
            // Keyring populated and we're root: actually verify each repo's
            // signature. Good = covered (by any key); NoSignature = none cached
            // yet / repo ships none (md5 covers it); "no public key" = its signer
            // really isn't imported; BADSIG = tampering.
            let mut verified: Vec<&str> = Vec::new();
            let mut nosig: Vec<&str> = Vec::new();
            let mut nokey: Vec<&str> = Vec::new();
            let mut errd: Vec<&str> = Vec::new();
            for r in &need_gpg {
                // A repo with no cached metadata has nothing to verify yet —
                // don't count it as "md5-covered"; the Metadata row already
                // flags it. (e.g. a freshly added or wrong-URL repo.)
                if missing_meta.iter().any(|m| m == &r.name) {
                    continue;
                }
                match gpg::verify_checksums(r, &cfg.cache_dir, &cfg.state_dir) {
                    Ok(gpg::Verify::Good(_)) => verified.push(r.name.as_str()),
                    Ok(gpg::Verify::NoSignature) => nosig.push(r.name.as_str()),
                    Ok(gpg::Verify::Tampered(_)) => tampered.push(r.name.clone()),
                    Ok(gpg::Verify::Unverifiable(m)) if m.contains("no public key") => {
                        nokey.push(r.name.as_str())
                    }
                    Ok(gpg::Verify::Unverifiable(_)) => errd.push(r.name.as_str()),
                    Err(_) => errd.push(r.name.as_str()),
                }
            }
            if !tampered.is_empty() {
                srow(&bad, "GPG keys", &ui::red(&format!("BAD signature: {}", tampered.join(", "))));
            } else if !nokey.is_empty() {
                srow(&warn, "GPG keys", &ui::yellow(&format!("no imported key verifies: {}", nokey.join(", "))));
                gpg_missing = true;
            } else if !verified.is_empty() {
                let extra = if nosig.is_empty() {
                    String::new()
                } else {
                    format!(" (+{} via md5)", nosig.len())
                };
                srow(&ok, "GPG keys", &ui::green(&format!("{} repo(s) verify{extra}", verified.len())));
            } else if !nosig.is_empty() {
                srow(&info, "GPG keys", &ui::dim("no signatures fetched yet (run `slacker update`)"));
            } else {
                srow(&info, "GPG keys", &ui::dim(&format!("could not verify: {}", errd.join(", "))));
            }
        }
    }

    // Metadata freshness — has `update` been run, and how long ago?
    let mut cached = 0usize;
    let mut oldest: Option<std::time::SystemTime> = None;
    for r in &repos {
        let p = repo::meta_path(r, &cfg.cache_dir, repo::PACKAGES_TXT);
        if let Ok(meta) = std::fs::metadata(&p) {
            cached += 1;
            if let Ok(mtime) = meta.modified() {
                oldest = Some(match oldest {
                    Some(o) if o < mtime => o,
                    _ => mtime,
                });
            }
        }
    }
    if cached == 0 {
        srow(&bad, "Metadata", &ui::red("not downloaded yet"));
        metadata_incomplete = true;
    } else if cached < repos.len() {
        srow(&warn, "Metadata", &ui::yellow(&format!("cached for {}/{} repos", cached, repos.len())));
        metadata_incomplete = true;
    } else {
        let age = oldest.map(ago).unwrap_or_else(|| "?".into());
        srow(&ok, "Metadata", &ui::dim(&format!("cached for all repos (oldest {age} old)")));
    }

    // Blacklist — show valid rules, and surface any the parser had to skip so a
    // typo'd rule is visible instead of silently vanishing. The comment/blank
    // handling mirrors config's own parse_lines, so the count matches the loader.
    let n_bl = cfg.blacklist.len();
    let bl_invalid = std::fs::read_to_string(cfg.config_dir.join("blacklist"))
        .map(|t| {
            t.lines()
                .map(|l| match l.find('#') {
                    Some(i) => l[..i].trim(),
                    None => l.trim(),
                })
                .filter(|l| !l.is_empty())
                .filter(|l| config::parse_blacklist_rule(l).is_err())
                .count()
        })
        .unwrap_or(0);
    if bl_invalid == 0 {
        srow(if n_bl == 0 { &info } else { &ok }, "Blacklist", &ui::dim(&format!("{n_bl} rule(s)")));
    } else {
        srow(
            &warn,
            "Blacklist",
            &ui::yellow(&format!("{n_bl} valid, {bl_invalid} invalid — check syntax in `blacklist`")),
        );
    }

    // Pins (`@repo 100% pkg`) — flag any whose repo is not active, since such a
    // pin has no effect until that repo exists.
    let pins = cfg.pins();
    if !pins.is_empty() {
        let active: HashSet<&str> = cfg.repos.iter().map(|r| r.name.as_str()).collect();
        let dangling: Vec<String> = pins
            .iter()
            .filter(|(_, repo)| !active.contains(repo))
            .map(|(pkg, repo)| format!("{pkg} -> {repo}"))
            .collect();
        if dangling.is_empty() {
            srow(&ok, "Pins", &ui::dim(&format!("{} pin(s)", pins.len())));
        } else {
            srow(
                &warn,
                "Pins",
                &ui::yellow(&format!(
                    "{} pin(s), {} to an inactive repo (no effect): {}",
                    pins.len(),
                    dangling.len(),
                    dangling.join(", ")
                )),
            );
            println!(
                "             {}",
                ui::dim("activate the repo, re-pin elsewhere, or `slacker unpin <pkg>`")
            );
        }
    }

    // Leftover `*.new` config files from past upgrades, waiting to be reconciled.
    let pending_new = newconfig::find_new_configs(&newconfig::default_roots()).len();
    if pending_new > 0 {
        srow(
            &warn,
            "Configs",
            &ui::yellow(&format!("{pending_new} pending .new file(s) — run `slacker new-config`")),
        );
    }

    // Integrity of slacker's OWN files: ownership, write-exposure, stray symlinks.
    // Scoped strictly to config + cache + state (NOT the system package DB, which
    // the pkgtools own). A compromise of the state dir defeats GPG pinning and the
    // trust markers, so it is reported prominently with the exact fix.
    let mut audit_roots: Vec<&Path> = vec![cfg.config_dir.as_path(), cfg.cache_dir.as_path()];
    if cfg.state_dir != cfg.cache_dir {
        audit_roots.push(cfg.state_dir.as_path());
    }
    let audit = audit_owned_paths(&audit_roots);
    if audit.severe() {
        security_problem = true;
    }
    let fix_own = format!("sudo chown -R root:root {} {}", cfg.config_dir.display(), cfg.cache_dir.display());
    let fix_w = format!("sudo chmod -R go-w {} {}", cfg.config_dir.display(), cfg.cache_dir.display());
    if audit.clean() {
        let tail = if audit.unreadable > 0 {
            format!(
                " ({} checked, {} dir(s) need root to read — re-run with sudo for a full audit)",
                audit.checked, audit.unreadable
            )
        } else {
            format!(" ({} checked)", audit.checked)
        };
        srow(
            if audit.unreadable > 0 { &info } else { &ok },
            "Integrity",
            &ui::dim(&format!("config + cache + state root-owned, no world-writable paths, no stray symlinks{tail}")),
        );
    } else {
        if audit.symlinks > 0 {
            srow(
                &bad,
                "Symlinks",
                &ui::red(&format!(
                    "{} unexpected symlink(s) — slacker plants none here; investigate before trusting the cache: {}",
                    audit.symlinks,
                    fmt_symlink_sample(&audit.sample_symlinks, audit.symlinks)
                )),
            );
        }
        if audit.world_writable > 0 {
            srow(
                &bad,
                "Writable",
                &ui::red(&format!(
                    "{} world-writable path(s) — anyone can tamper (e.g. {}); fix: {fix_w}",
                    audit.world_writable,
                    fmt_path_sample(&audit.sample_world_writable, audit.world_writable)
                )),
            );
        }
        if audit.group_writable > 0 {
            srow(
                &warn,
                "Writable",
                &ui::yellow(&format!(
                    "{} group-writable path(s) (e.g. {}); fix: {fix_w}",
                    audit.group_writable,
                    fmt_path_sample(&audit.sample_group_writable, audit.group_writable)
                )),
            );
        }
        if audit.not_root > 0 {
            srow(
                &warn,
                "Ownership",
                &ui::yellow(&format!(
                    "{} path(s) not owned by root (e.g. {}); fix: {fix_own}",
                    audit.not_root,
                    fmt_path_sample(&audit.sample_not_root, audit.not_root)
                )),
            );
        }
        if audit.unreadable > 0 {
            srow(
                &info,
                "Integrity",
                &ui::dim(&format!(
                    "{} dir(s) need root to read — re-run with sudo for a full audit",
                    audit.unreadable
                )),
            );
        }
    }

    // ---------- Installed ----------
    println!("\n{}", ui::blue("Installed"));
    if !cfg.pkg_db_dir.exists() {
        srow(
            &bad,
            "Admin dir",
            &ui::red(&format!(
                "{} does not exist — slacker sees no installed packages",
                cfg.pkg_db_dir.display()
            )),
        );
    }
    let (per_repo, per_rule, per_other) = installed_attribution(cfg, Some(&db), &installed);
    srow(&ok, "Packages", &ui::white(&installed.len().to_string()));
    let mut parts: Vec<String> = repos
        .iter()
        .map(|r| {
            // Isolate a repo with no metadata as `?` instead of a misleading 0;
            // the rest report their real counts.
            if missing_meta.iter().any(|m| m == &r.name) {
                format!("{} ?", r.name)
            } else {
                format!("{} {}", r.name, per_repo.get(&r.name).copied().unwrap_or(0))
            }
        })
        .collect();
    // Declared tag-priority rules (SBo, local, ...) are named sources too.
    let mut seen_rule = std::collections::HashSet::new();
    for tp in &cfg.tag_priorities {
        if seen_rule.insert(tp.name.as_str()) {
            parts.push(format!("{} {}", tp.name, per_rule.get(&tp.name).copied().unwrap_or(0)));
        }
    }
    // Any remaining build tags are legitimate sources, shown by their tag.
    let mut others: Vec<(&String, &usize)> = per_other.iter().collect();
    others.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (t, c) in others {
        parts.push(format!("{t} {c}"));
    }
    srow(&info, "By source", &ui::dim(&parts.join(", ")));
    if !missing_meta.is_empty() {
        srow(
            &info,
            "",
            &ui::yellow(&format!(
                "? = no metadata yet ({}) — run `slacker update`, or check the repo URL",
                missing_meta.join(", ")
            )),
        );
    }

    // ---------- Online ----------
    println!("\n{}", ui::blue("Online"));
    let probe_repo = cfg.repos.iter().find(|r| r.official).or_else(|| repos.first().copied());
    match probe_repo {
        None => srow(&info, "Connection", &ui::dim("no repo to probe")),
        Some(r) => {
            // The same small file check-updates compares: ChangeLog for the
            // official repo, otherwise CHECKSUMS.md5.
            let file = if r.official { repo::CHANGELOG } else { repo::CHECKSUMS };
            let url = r.join_url(file);
            match download::get_bytes(&url) {
                Ok(remote) => {
                    srow(
                        &ok,
                        "Reachable",
                        &ui::dim(&format!("{} ({} KB)", r.name, remote.len() / 1024)),
                    );
                    let cached_file = std::fs::read(repo::meta_path(r, &cfg.cache_dir, file)).ok();
                    match cached_file {
                        Some(local) if local == remote => {
                            srow(&ok, "Updates", &ui::green(&format!("{} is up to date", r.name)))
                        }
                        Some(_) => {
                            srow(&warn, "Updates", &ui::yellow(&format!("{} has pending changes", r.name)));
                            pending = true;
                        }
                        None => srow(&warn, "Updates", &ui::yellow("not synced yet")),
                    }
                    srow(&info, "All repos", &ui::dim("run `slacker check-updates` for every repo"));
                }
                Err(e) => {
                    srow(&bad, "Reachable", &ui::red(&format!("{} unreachable: {e}", r.name)));
                    unreachable = true;
                }
            }
        }
    }

    // ---------- Verdict ----------
    // Recommended commands in the canonical post-install order:
    //   update gpg -> update -> install-new -> upgrade-all.
    let mut steps: Vec<&str> = Vec::new();
    if gpg_missing {
        steps.push("slacker update gpg");
    }
    // `update` comes first whenever the local cache is not in step with the
    // repos: either no metadata yet (fresh/unsynced) OR the online repo has
    // changes the cache has not pulled (pending). install-new/upgrade-all read
    // the cache, so it must be refreshed before them.
    if metadata_incomplete || pending {
        steps.push("slacker update");
    }
    // A fresh/unsynced system, or one with pending changes, installs the newly
    // added packages BEFORE upgrading — install-new always precedes upgrade-all,
    // because new packages may be dependencies of the upgrades.
    if metadata_incomplete || pending {
        steps.push("slacker install-new");
        steps.push("slacker upgrade-all");
    }
    // Advisories: real issues that are not a command to run in this sequence.
    let mut notes: Vec<String> = Vec::new();
    if !tampered.is_empty() {
        notes.push(format!(
            "BAD GPG signature for {} — possible tampering; do NOT install from it until resolved",
            tampered.join(", ")
        ));
    }
    if !unverified.is_empty() {
        notes.push(format!(
            "verification is OFF for {} — set VERIFY=all in slacker.conf or verify= per repo",
            unverified.join(", ")
        ));
    }
    if unreachable {
        notes.push(
            "the mirror did not respond — check your network or the active line in `mirrors`".into(),
        );
    }
    if security_problem {
        notes.push(
            "slacker's OWN files have unsafe permissions or unexpected symlinks (see above) — resolve before installing; the package and trust cache cannot be trusted until then".into(),
        );
    }

    println!();
    // A "fresh" system is one that is correctly configured and safe, just not
    // synced yet — the expected state on a first run, BEFORE any `update`. We
    // greet it as ready rather than listing problems.
    let fresh_setup = metadata_incomplete && !security_problem && tampered.is_empty();
    if steps.is_empty() && notes.is_empty() {
        println!("{}", ui::green("\u{2713} slacker is set up correctly."));
    } else if steps.is_empty() {
        println!("{}", ui::blue("slacker is set up, with notes:"));
        for n in &notes {
            println!("  {} {}", ui::yellow("!"), ui::dim(n));
        }
    } else {
        let header = if fresh_setup {
            "Setup looks correct \u{2014} you're ready. Start with your first update, in order:"
        } else {
            "slacker is configured. Recommended next steps, in order:"
        };
        println!("{}", ui::blue(header));
        for s in &steps {
            println!("  {} {}", ui::yellow("\u{2192}"), ui::white(s));
        }
        for n in &notes {
            println!("  {} {}", ui::yellow("!"), ui::dim(n));
        }
        if metadata_incomplete {
            println!(
                "  {} {}",
                ui::blue("\u{00B7}"),
                ui::dim("after that, re-run `slacker status` to verify the rest")
            );
        }
    }
    Ok(Outcome::Ok)
}

fn cmd_install(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (matched, misses) = collect(&db, patterns)?;
    report_pkg_misses(&db, &misses);
    // install = only packages that are not already installed and not blacklisted
    let mut frozen = Vec::new();
    let mut already = Vec::new();
    let todo: Vec<_> = matched
        .into_iter()
        .filter(|p| {
            if bl_frozen(cfg, &db, &installed, p) {
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
    let todo = select_packages(todo, "install", cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Install)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;
    // `already` is shown via the same blue "kept" line as priority skips — both
    // mean "installed, leaving it alone".
    show_plan(&plan, &frozen, &already);
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

fn cmd_upgrade(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (cands, protected, misses) =
        collect_installed_targets(&db, &installed, &cfg.tag_priorities, patterns)?;
    report_installed_misses(&db, &installed, &misses);
    let mut frozen = Vec::new();
    let todo: Vec<_> = cands
        .into_iter()
        .filter(|p| {
            if bl_frozen(cfg, &db, &installed, p) {
                frozen.push(p.id.name.clone());
                return false;
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
    let todo = select_packages(todo, "upgrade", cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Upgrade)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;
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

fn cmd_reinstall(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (cands, protected, misses) =
        collect_installed_targets(&db, &installed, &cfg.tag_priorities, patterns)?;
    report_installed_misses(&db, &installed, &misses);
    let mut frozen = Vec::new();
    let todo: Vec<_> = cands
        .into_iter()
        .filter(|p| {
            if bl_frozen(cfg, &db, &installed, p) {
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
    let todo = select_packages(todo, "reinstall", cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Reinstall)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;
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

fn cmd_remove(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
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

/// Download a cumulative-archive package and its detached `.asc`, then require a
/// GOOD GPG signature from the pinned official Slackware key before returning.
/// md5 is intentionally not used here: the cumulative CHECKSUMS does not list
/// superseded versions, so the per-package GPG signature alone authenticates. A
/// missing, bad, or unpinned-key signature is fatal — nothing is installed.
fn revert_fetch_and_gpg_verify(
    cfg: &Config,
    official: &config::Repo,
    txz_url: &str,
    dest: &std::path::Path,
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
    download::download_to(txz_url, dest)?;

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

/// Roll an official package back to a previous -current version (rollback). See
/// the `RevertPkg` command doc for the user-facing description. Guards: the
/// feature switch, a -current-only check (fail-closed), and a -current archive
/// URL. Then: list previous official versions from removed-packages, pick one,
/// locate it in the cumulative archive, GPG-verify, downgrade, offer to freeze.
fn cmd_revert_pkg(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
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

    // Locate it in the cumulative archive (series from the archive's PACKAGES.TXT).
    let pkgs_url = format!("{}/PACKAGES.TXT", cfg.cumulative_url.trim_end_matches('/'));
    println!("  {}", ui::dim(&format!("fetching {pkgs_url}")));
    let bytes =
        download::get_bytes(&pkgs_url).map_err(|e| format!("fetch cumulative PACKAGES.TXT: {e}"))?;
    let txt = String::from_utf8_lossy(&bytes);
    let locations = revert::parse_locations(&txt);
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
    revert_fetch_and_gpg_verify(cfg, official, &url, &dest)?;

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

fn cmd_download(
    cli: &Cli,
    cfg: &Config,
    patterns: &[String],
    output: Option<&str>,
) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let (matched, misses) = collect(&db, patterns)?;
    report_pkg_misses(&db, &misses);
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
    let matched = select_packages(matched, "download", cli.yes, cli.dry_run);
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

/// `upgrade-dist` — GATE ONLY for now.
/// Core packages that must be upgraded FIRST, serially, in this order: the C
/// runtime, then the very tools `upgradepkg` relies on to unpack and install the
/// rest. Doing the whole tree before these are in step is how a dist-upgrade
/// bricks a system. Both `glibc-solibs` spellings are covered (some releases ship
/// `aaa_glibc-solibs`).
const DIST_CRITICAL: &[&str] = &[
    "aaa_glibc-solibs",
    "glibc-solibs",
    "pkgtools",
    "tar",
    "xz",
    "gzip",
    "findutils",
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
fn dist_upgrade_sets(db: &PkgDb, installed: &[pkg::PkgId]) -> (Vec<PlanItem>, Vec<PlanItem>) {
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
    let at: Vec<String> = cfg
        .repos
        .iter()
        .filter(|r| r.official)
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
fn cmd_upgrade_dist(cli: &Cli, cfg: &Config, target_arg: &str) -> Result<Outcome, String> {
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
                 (glibc-solibs -> pkgtools/tar/xz/gzip/findutils), then install-new and the rest \
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
    cmd_new_config(cli)?;

    // ---- Phase 6: final report ----
    println!();
    cmd_status(&cfg.config_dir)?;

    // ---- Boot reminder ----
    println!();
    println!("{}", ui::green(&format!("Distribution upgrade to {target_seg} complete.")));
    println!("{}", ui::blue("Before rebooting:"));
    println!(
        "  {}",
        ui::white("rebuild the initrd (mkinitrd) and reinstall the bootloader — lilo, or eliloconfig on UEFI")
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
    out.push(format!("# --- upgrade-dist: local mirror (DISTRO_UPGRADE_MIRROR) ---"));
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

fn cmd_upgrade_all(cli: &Cli, cfg: &Config) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let mut ups = db.upgrades_for(&installed, &cfg.tag_priorities);
    // Drop frozen upgrades. The blacklist must be tested against the UPGRADE
    // CANDIDATE (`u.available`), not only the installed copy: a date/version
    // rule like `xf86-.*-202.*` only ever matches the incoming build, never the
    // older installed one, so an installed-only test (`bl_installed`) would let
    // the very build the user froze through. `bl_frozen` covers both the
    // candidate and any `@repo`-scoped rule on the installed source — the same
    // check `upgrade <pattern>`, `install`, `install-new` and `info` all use.
    let mut frozen = Vec::new();
    ups.retain(|u| {
        if bl_frozen(cfg, &db, &installed, u.available) {
            frozen.push(u.available.id.name.clone());
            return false;
        }
        true
    });
    frozen.sort();
    frozen.dedup();
    note_frozen_excluded(&frozen);
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
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;

    print_plan(&plan);
    report_pinned_in_plan(cfg, &plan);
    hint_freeze_pin();

    note_optional_suggests(&plan, resolve);
    let conflicts = detect_conflicts(&plan, &installed, resolve);
    report_conflicts(&conflicts);
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

fn cmd_install_new(cli: &Cli, cfg: &Config, repos: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;

    // Which repos to scan for newly-added packages:
    //   - no argument  -> official repo(s) only (slackpkg's behaviour: packages
    //     the Slackware distribution itself added)
    //   - repo name(s) -> exactly those, so the user can opt in to a third-party
    //     repo explicitly (e.g. `slacker install-new alienbob`)
    let selected: Vec<&config::Repo> = if repos.is_empty() {
        cfg.repos.iter().filter(|r| r.official).collect()
    } else {
        let mut out = Vec::new();
        for name in repos {
            match cfg.repos.iter().find(|r| &r.name == name) {
                Some(r) => out.push(r),
                None => return Err(format!("install-new: unknown repo '{name}'")),
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
            if bl_frozen(cfg, &db, &installed, p) {
                frozen.push(p.id.name.clone());
                return false;
            }
            !system::is_installed(&installed, &p.id.name)
        })
        .collect();
    if todo.is_empty() {
        if !frozen.is_empty() {
            show_plan(&[], &frozen, &[]);
        }
        println!("No new packages to install.");
        return Ok(Outcome::NothingFound);
    }
    // Let the user deselect before resolving deps (same select-before-resolve
    // step the other install paths use; skipped under --yes/--dry-run/single).
    note_frozen_excluded(&frozen);
    let todo = select_packages(todo, "install", cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Install)).collect();
    // Resolve dependencies up front so any extra packages pulled in are shown
    // before we ask to proceed (dry-run keeps installed versions, no prompts).
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;
    show_plan(&plan, &frozen, &[]);
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

fn cmd_clean_system(cli: &Cli, cfg: &Config) -> Result<Outcome, String> {
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
                    db.repo_for_tag(tag).map_or(false, |r| immutable.contains(r));
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
    if orphans.is_empty() {
        println!("No foreign packages found.");
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
fn cmd_clean_cache(cli: &Cli, cfg: &Config, repos: &[String]) -> Result<Outcome, String> {
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
                return Err(format!("clean-cache: unknown repo '{name}'"));
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

fn parse_selection(input: &str, max: usize) -> HashSet<usize> {
    let mut out = HashSet::new();
    for tok in input.split([' ', ',', '\t']).filter(|t| !t.is_empty()) {
        if let Some((a, b)) = tok.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                for n in a..=b {
                    if (1..=max).contains(&n) {
                        out.insert(n);
                    }
                }
            }
        } else if let Ok(n) = tok.parse::<usize>() {
            if (1..=max).contains(&n) {
                out.insert(n);
            }
        }
    }
    out
}

/// When a pattern matched more than one package, show a numbered list and let
/// the user pick which to act on: Enter = all, numbers/ranges (`1 3 5` or
/// `2-4`) = those only, `n` = cancel. A single match is returned unchanged.
/// With --yes (or in a dry-run preview) all matches are kept without asking.
fn select_packages<'a>(
    pkgs: Vec<&'a repo::AvailPkg>,
    verb: &str,
    assume_yes: bool,
    dry_run: bool,
) -> Vec<&'a repo::AvailPkg> {
    if pkgs.len() <= 1 || assume_yes || dry_run {
        return pkgs;
    }
    println!("{}", ui::blue(&format!("'{verb}' matched {} packages:", pkgs.len())));
    for (i, p) in pkgs.iter().enumerate() {
        println!(
            "  {}) {} {}{}",
            ui::dim(&format!("{:>3}", i + 1)),
            ui::cyan(&format!("[{}]", p.repo)),
            ui::white(&p.id.name),
            ui::dim(&format!("-{}-{}-{}", p.id.version, p.id.arch, p.id.build))
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

/// Byte-for-byte comparison. A missing/unreadable file counts as "not
/// identical" so the caller falls through to the interactive path.
fn files_identical(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Show a coloured unified diff between the installed file and the .new one,
/// shelling out to the system `diff` (as slacker already does for bzip2/gpg).
fn show_config_diff(old: &std::path::Path, new: &std::path::Path) {
    match std::process::Command::new("diff").arg("-u").arg(old).arg(new).output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.trim().is_empty() {
                println!("    {}", ui::dim("(no differences)"));
                return;
            }
            for line in text.lines() {
                let painted = if line.starts_with("+++") || line.starts_with("---") {
                    ui::dim(line)
                } else if line.starts_with("@@") {
                    ui::cyan(line)
                } else if line.starts_with('+') {
                    ui::green(line)
                } else if line.starts_with('-') {
                    ui::red(line)
                } else {
                    line.to_string()
                };
                println!("    {painted}");
            }
        }
        Err(_) => println!("    {}", ui::dim("(system `diff` not available)")),
    }
}

/// Open an external merge tool on (installed, .new). Honours $SLACKER_MERGE,
/// defaulting to vimdiff. Ok only if the tool ran successfully.
fn merge_config(old: &std::path::Path, new: &std::path::Path) -> Result<(), String> {
    let tool = std::env::var("SLACKER_MERGE").unwrap_or_else(|_| "vimdiff".to_string());
    match std::process::Command::new(&tool).arg(old).arg(new).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(format!("'{tool}' exited with an error")),
        Err(_) => Err(format!("merge tool '{tool}' not found (set $SLACKER_MERGE)")),
    }
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

/// Build an interactive choice prompt whose bracketed KEY letters are WHITE (so
/// they stand out) and whose surrounding text is blue. Each entry is
/// `(KEY, text-after-the-bracket)`; `default_key` is shown at the end as `? [X]`.
/// Centralised so every letter-choice prompt highlights its keys consistently.
fn choice_line(prefix: &str, entries: &[(&str, &str)], default_key: &str) -> String {
    let mut s = String::new();
    if !prefix.is_empty() {
        s.push_str(&ui::blue(prefix));
        s.push(' ');
    }
    for (i, (key, after)) in entries.iter().enumerate() {
        if i > 0 {
            s.push_str("  ");
        }
        s.push_str(&ui::white(&format!("[{key}]")));
        if !after.is_empty() {
            s.push_str(&ui::blue(after));
        }
    }
    s.push_str(&ui::blue(" ? "));
    s.push_str(&ui::white(&format!("[{default_key}]")));
    s
}

/// Colour an existing prompt/menu line: every bracketed key — `[s]`, `[a]ll`,
/// `[k/r/a/q]`, `keep-[a]ll`, `a[b]ort` … — is rendered in WHITE so it stands
/// out, while the surrounding text stays blue. Use this for the multi-line
/// option menus and selection prompts (choice_line is for one-line key rows).
/// With NO_COLOR / no TTY, ui::* are no-ops so the text comes through unchanged.
fn hilite_keys(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        match rest[open..].find(']') {
            Some(rel) => {
                let close = open + rel;
                out.push_str(&ui::blue(&rest[..open]));
                out.push_str(&ui::white(&rest[open..=close])); // the [..] in white
                rest = &rest[close + 1..];
            }
            None => break,
        }
    }
    out.push_str(&ui::blue(rest));
    out
}

/// Overwrite `target` with the `.new` file, first saving the existing `target`
/// as `<target>.orig` (slackpkg-style), so the previous config stays recoverable.
/// If a `.orig` already exists it is replaced (latest superseded config wins).
fn overwrite_with_orig(nc: &newconfig::NewConfig) -> Result<(), String> {
    if nc.target.exists() {
        let mut orig = nc.target.as_os_str().to_os_string();
        orig.push(".orig");
        let orig = std::path::PathBuf::from(orig);
        std::fs::copy(&nc.target, &orig)
            .map_err(|e| format!("back up {} -> {}: {e}", nc.target.display(), orig.display()))?;
    }
    std::fs::rename(&nc.new_file, &nc.target)
        .map_err(|e| format!("overwrite {}: {e}", nc.target.display()))?;
    Ok(())
}

/// Interactive per-file resolution for one differing config file: show the diff,
/// then [K]eep both / [O]verwrite (old saved as .orig) / [R]emove .new / [M]erge /
/// [D]iff. Used by the "(P)rompt one by one" path of `cmd_new_config`.
fn review_one_config(nc: &newconfig::NewConfig) -> Result<(), String> {
    println!("\n{}", ui::white(&nc.target.display().to_string()));
    show_config_diff(&nc.target, &nc.new_file);
    loop {
        print!(
            "  {} ",
            choice_line(
                "",
                &[
                    ("K", "eep both"),
                    ("O", "verwrite"),
                    ("R", "emove .new"),
                    ("M", "erge"),
                    ("D", "iff"),
                ],
                "K",
            )
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            break;
        }
        match line.trim().to_lowercase().as_str() {
            "" | "k" => {
                println!("    {}", ui::dim("kept both — decide later"));
                break;
            }
            "o" => {
                overwrite_with_orig(nc)?;
                println!("    {}", ui::dim("overwritten with the new file (old saved as .orig)"));
                break;
            }
            "r" => {
                std::fs::remove_file(&nc.new_file).map_err(|e| format!("remove: {e}"))?;
                println!("    {}", ui::dim("kept your current config — removed .new"));
                break;
            }
            "m" => {
                match merge_config(&nc.target, &nc.new_file) {
                    Ok(()) => {
                        if confirm("  merge done — remove the .new file?", false) {
                            std::fs::remove_file(&nc.new_file).map_err(|e| format!("remove: {e}"))?;
                            println!("    {}", ui::dim("merged, .new removed"));
                        } else {
                            println!("    {}", ui::dim("merged, .new left in place"));
                        }
                    }
                    Err(e) => println!("    {}", ui::red(&e)),
                }
                break;
            }
            "d" => {
                show_config_diff(&nc.target, &nc.new_file);
            }
            other => {
                println!("    {}", ui::dim(&format!("'{other}'? choose K, O, R, M or D")));
            }
        }
    }
    Ok(())
}

fn cmd_new_config(cli: &Cli) -> Result<Outcome, String> {
    let found = newconfig::find_new_configs(&newconfig::default_roots());
    if found.is_empty() {
        println!("No .new configuration files found.");
        return Ok(Outcome::Ok);
    }

    // Classify up front so we can show the whole picture before asking anything:
    //  * broken    — a .new with no installed counterpart (a broken package; we
    //                never touch it, just warn);
    //  * identical — a .new byte-identical to the installed file (redundant, drop);
    //  * conflicts — a .new that differs (the ones that actually need a decision).
    let mut broken: Vec<&newconfig::NewConfig> = Vec::new();
    let mut identical: Vec<&newconfig::NewConfig> = Vec::new();
    let mut conflicts: Vec<&newconfig::NewConfig> = Vec::new();
    for nc in &found {
        if !nc.target.exists() {
            broken.push(nc);
        } else if files_identical(&nc.target, &nc.new_file) {
            identical.push(nc);
        } else {
            conflicts.push(nc);
        }
    }

    // Broken packages: warn loudly, never touch.
    for nc in &broken {
        let bar = "=".repeat(66);
        println!("{}", ui::red(&format!("  {bar}")));
        println!("{}", ui::red("  !! WARNING: this package looks broken"));
        println!("{}", ui::red("  !! a .new config file was installed but no previous version exists:"));
        println!("{}{}", ui::red("  !!   "), ui::white(&nc.new_file.display().to_string()));
        println!("{}", ui::red("  !! slacker cannot diff or merge it. Please review it manually,"));
        println!("{}", ui::red("  !! at your own responsibility."));
        println!("{}", ui::red(&format!("  {bar}")));
    }

    // Redundant .new identical to the installed file: drop them (report in dry-run).
    if !identical.is_empty() {
        if cli.dry_run {
            println!(
                "{}",
                ui::dim(&format!(
                    "{} .new identical to the installed file (would remove)",
                    identical.len()
                ))
            );
        } else {
            let mut removed = 0usize;
            for nc in &identical {
                if std::fs::remove_file(&nc.new_file).is_ok() {
                    removed += 1;
                }
            }
            if removed > 0 {
                println!(
                    "{}",
                    ui::dim(&format!("removed {removed} redundant .new (identical to installed)"))
                );
            }
        }
    }

    if conflicts.is_empty() {
        println!("{}", ui::green("No config files need merging."));
        return Ok(Outcome::Ok);
    }

    // ---- Phase 1: show the whole list of differing files up front ----
    println!();
    println!(
        "{}",
        ui::blue(&format!("{} config file(s) differ from the new version:", conflicts.len()))
    );
    for (i, nc) in conflicts.iter().enumerate() {
        println!("  {:>3}) {}", i + 1, ui::white(&nc.target.display().to_string()));
    }

    if cli.dry_run {
        println!("{}", ui::dim("dry run — re-run without --dry-run to choose what to do"));
        return Ok(Outcome::Ok);
    }

    // --yes: non-interactive, keep the current configs (drop the .new) — the safe
    // default, and what the dist phase needs so it never blocks on a prompt.
    if cli.yes {
        let mut n = 0usize;
        for nc in &conflicts {
            if std::fs::remove_file(&nc.new_file).is_ok() {
                n += 1;
            }
        }
        println!("{}", ui::dim(&format!("--yes: kept current configs, removed {n} .new file(s)")));
        return Ok(Outcome::Ok);
    }

    // ---- Phase 2: one bulk choice for ALL files (slackpkg-style K/O/R/P) ----
    println!();
    loop {
        print!(
            "{} ",
            choice_line(
                &format!("ALL {} files:", conflicts.len()),
                &[
                    ("K", "eep current (.new left for later)"),
                    ("O", "verwrite all (old saved as .orig)"),
                    ("R", "emove all .new"),
                    ("P", "rompt one by one"),
                ],
                "P",
            )
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return Ok(Outcome::Ok);
        }
        match line.trim().to_lowercase().as_str() {
            "o" => {
                if !confirm(
                    &format!(
                        "Overwrite all {} files with the new versions (old saved as .orig)?",
                        conflicts.len()
                    ),
                    false,
                ) {
                    println!("{}", ui::blue("cancelled — nothing changed."));
                    return Ok(Outcome::Ok);
                }
                let mut n = 0usize;
                for nc in &conflicts {
                    overwrite_with_orig(nc)?;
                    n += 1;
                }
                println!(
                    "{}",
                    ui::green(&format!(
                        "overwrote {n} file(s) with the new versions (previous saved as .orig)."
                    ))
                );
                return Ok(Outcome::Ok);
            }
            "k" => {
                // slackpkg (K)eep: keep the current files, LEAVE the .new files in
                // place to deal with later. (Use R to discard them.)
                println!(
                    "{}",
                    ui::green(&format!(
                        "kept your current configs — {} .new file(s) left in place for later.",
                        conflicts.len()
                    ))
                );
                return Ok(Outcome::Ok);
            }
            "r" => {
                if !confirm(
                    &format!("Remove all {} .new file(s) and keep your current configs?", conflicts.len()),
                    false,
                ) {
                    println!("{}", ui::blue("cancelled — nothing changed."));
                    return Ok(Outcome::Ok);
                }
                let mut n = 0usize;
                for nc in &conflicts {
                    std::fs::remove_file(&nc.new_file)
                        .map_err(|e| format!("remove {}: {e}", nc.new_file.display()))?;
                    n += 1;
                }
                println!(
                    "{}",
                    ui::green(&format!("kept your current configs — removed {n} .new file(s)."))
                );
                return Ok(Outcome::Ok);
            }
            "" | "p" => {
                // ---- Phase 3: prompt for each one individually ----
                for nc in &conflicts {
                    review_one_config(nc)?;
                }
                return Ok(Outcome::Ok);
            }
            other => {
                println!("    {}", ui::dim(&format!("'{other}'? choose K, O, R or P")));
            }
        }
    }
}

fn cmd_check_updates(cfg: &Config) -> Result<Outcome, String> {
    if cfg.repos.is_empty() {
        return Err("no repos configured".into());
    }
    let width = cfg.repos.iter().map(|r| r.name.len()).max().unwrap_or(8);
    let mut any_pending = false;
    let mut any_unknown = false;
    for r in cfg.repos_by_priority() {
        let label = match changelog::check_repo_updates(r, &cfg.cache_dir) {
            changelog::UpdateStatus::UpToDate => ui::green("up-to-date"),
            changelog::UpdateStatus::Pending => {
                any_pending = true;
                ui::yellow("updates pending")
            }
            changelog::UpdateStatus::Unknown => {
                any_unknown = true;
                ui::dim("unknown (run update first)")
            }
        };
        println!("  {}  {label}", ui::white(&format!("{:<width$}", r.name)));
    }
    warn_unverified_repos(cfg);
    if any_pending {
        println!("\n{}", ui::blue("Run `slacker update` then `slacker upgrade-all`."));
        Ok(Outcome::Pending)
    } else if any_unknown {
        Ok(Outcome::Ok)
    } else {
        println!("\n{}", ui::green("Everything up-to-date."));
        Ok(Outcome::Ok)
    }
}

fn cmd_show_changelog(cfg: &Config, repo_name: Option<&str>) -> Result<Outcome, String> {
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
        Ok(t) => t,
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

fn cmd_history(
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
        let (sym, label, detail) = match &e.kind {
            EventKind::Installed { reinstall } => {
                let mut d = format!("{}-{}", e.pkg.version, e.pkg.build);
                if *reinstall {
                    d.push_str(&ui::yellow(" (reinstall)"));
                }
                (ui::green("+"), "installed", d)
            }
            EventKind::Removed => {
                (ui::red("\u{2212}"), "removed", format!("{}-{}", e.pkg.version, e.pkg.build))
            }
            EventKind::Upgraded { to } => match to {
                // upgradepkg over the same id is a rebuild / reinstall in place,
                // not a version change — show it as such, not "X -> X".
                Some(p) if p.tag() == e.pkg.tag() => (
                    ui::cyan("\u{21BB}"),
                    "reinstalled",
                    format!("{}-{}", e.pkg.version, e.pkg.build),
                ),
                Some(p) => (
                    ui::cyan("\u{2191}"),
                    "upgraded",
                    format!(
                        "{}-{} {} {}-{}",
                        e.pkg.version, e.pkg.build, ui::dim("\u{2192}"), p.version, p.build
                    ),
                ),
                None => (
                    ui::cyan("\u{2191}"),
                    "upgraded",
                    format!("{}-{} {} ?", e.pkg.version, e.pkg.build, ui::dim("\u{2192}")),
                ),
            },
        };
        out.push_str(&format!(
            "{date}  {sym} {label:<11}  {name}  {detail}  {src}\n",
            name = ui::white(&format!("{:<wn$}", e.pkg.name)),
            src = ui::dim(&format!("[{}]", source_of(cfg, db, &e.pkg))),
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

fn cmd_generate_template(cfg: &Config, name: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    // Snapshot installed packages that are known to a repo (skip orphans).
    let orphan_names: HashSet<&str> = db.orphans(&installed).into_iter().map(|p| p.name.as_str()).collect();
    let names: Vec<String> = installed
        .iter()
        .map(|p| p.name.clone())
        .filter(|n| !orphan_names.contains(n.as_str()))
        .collect();
    let path = template::generate(&cfg.config_dir, name, &names)?;
    println!("Wrote template with {} packages: {}", names.len(), path.display());
    Ok(Outcome::Ok)
}

fn cmd_install_template(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let names = template::load(&cfg.config_dir, name, true)?;
    let mut todo = Vec::new();
    for n in &names {
        if system::is_installed(&installed, n) {
            continue;
        }
        if let Some(p) = db.resolve(n) {
            if bl_avail(cfg, p) {
                continue;
            }
            todo.push(p);
        } else {
            eprintln!("template package not found in repos: {n}");
        }
    }
    if todo.is_empty() {
        println!("Nothing to install from template '{name}'.");
        return Ok(Outcome::NothingFound);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Install)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;
    print_plan(&plan);
    report_pinned_in_plan(cfg, &plan);
    hint_freeze_pin();
    note_optional_suggests(&plan, resolve);
    let conflicts = detect_conflicts(&plan, &installed, resolve);
    report_conflicts(&conflicts);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm_conflicts("Install template packages?", &conflicts, cli.yes)? {
        return Ok(Outcome::Ok);
    }
    execute_plan(cfg, &plan, cli.yes)?;
    Ok(Outcome::Ok)
}

fn cmd_remove_template(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    // Loaded so blacklist series/@repo rules can be evaluated against installed
    // packages (whose series/source aren't recorded locally).
    let db = PkgDb::load(cfg)?;
    let names = template::load(&cfg.config_dir, name, true)?;
    let todo: Vec<&String> = names
        .iter()
        .filter(|n| {
            system::installed_by_name(&installed, n).map_or(false, |i| !bl_installed(cfg, Some(&db), i))
        })
        .collect();
    if todo.is_empty() {
        println!("Nothing to remove from template '{name}'.");
        return Ok(Outcome::NothingFound);
    }
    let rows: Vec<PlanRow> = todo
        .iter()
        .map(|n| {
            let (version, repo) = match installed.iter().find(|i| &i.name == *n) {
                Some(p) => (
                    format!("{}-{}-{}", p.version, p.arch, p.build),
                    {
                        let t = p.build_tag();
                        if t.is_empty() { "-".to_string() } else { t.to_string() }
                    },
                ),
                None => ("-".to_string(), "-".to_string()),
            };
            PlanRow {
                action: "remove",
                color: ui::red,
                name: (*n).clone(),
                version,
                repo,
                note: String::new(),
            }
        })
        .collect();
    print_table(&rows);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    println!(
        "\nThis will UNINSTALL {} package(s) listed in template '{name}' from your system.",
        todo.len()
    );
    println!("(To delete just the template file, use `slacker delete-template {name}`.)");
    if !confirm("Uninstall these packages?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    for n in &todo {
        system::remove_package(n)?;
    }
    Ok(Outcome::Ok)
}

/// Delete a template file. Does not touch installed packages.
fn cmd_delete_template(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    if cli.dry_run {
        println!("(dry-run) would delete template file '{name}'");
        return Ok(Outcome::Ok);
    }
    let path = template::delete(&cfg.config_dir, name)?;
    println!("Deleted template file: {}", path.display());
    Ok(Outcome::Ok)
}

/// Add one or more package names to the blacklist ("freeze" them so update,
/// upgrade-all, reinstall, and clean-system leave them alone).
/// A soft warning for a `frozen` rule that parses but is almost certainly a
/// mistake: an `@repo` naming no active repo, or a plain regex whose pattern
/// contains whitespace — a package id (`name-version-arch-build`) never does,
/// so it usually means a forgotten `@` or a quoting slip. None if it looks ok.
fn frozen_warn(raw: &str, rule: &config::BlacklistRule, active: &[&str]) -> Option<String> {
    let q = format!("\"{raw}\"");
    if let Some(r) = rule.repo() {
        if !active.contains(&r) {
            return Some(format!("{q:<22} no active repo '{r}'"));
        }
    }
    if let Some(pat) = rule.pattern() {
        if pat.contains(char::is_whitespace) {
            let first = pat.split_whitespace().next().unwrap_or("");
            if active.contains(&first) {
                return Some(format!(
                    "{q:<22} looks like repo '{first}' without '@' — did you mean \"@{raw}\"?"
                ));
            }
            return Some(format!("{q:<22} pattern has a space; package names never do"));
        }
    }
    None
}

fn cmd_frozen(cli: &Cli, cfg: &Config, names: &[String]) -> Result<Outcome, String> {
    if names.is_empty() {
        // No argument: show the current freeze rules (pins are listed by `pin`).
        let text =
            std::fs::read_to_string(cfg.config_dir.join("blacklist")).unwrap_or_default();
        let rules: Vec<&str> = text
            .lines()
            .map(config::strip_comment)
            .filter(|r| !r.is_empty())
            .filter(|r| parse_pin_line(r).is_none())
            .collect();
        if rules.is_empty() {
            println!("No frozen rules set.");
        } else {
            println!("{}", ui::blue("Current frozen rules:"));
            for r in &rules {
                println!("  {}", ui::white(r));
            }
        }
        println!(
            "  {}",
            ui::dim(
                "to add a rule: `frozen <rule>`, e.g. `frozen vlc`, `frozen kde/`, `frozen \"@alienbob vlc\"`"
            )
        );
        return Ok(Outcome::Ok);
    }
    let active: Vec<&str> = cfg.repos.iter().map(|r| r.name.as_str()).collect();

    // Single pre-flight pass: parse every argument and collect *all* problems
    // (syntax errors and unknown-@repo typos) so they can be reported together,
    // before anything is written.
    let mut rules: Vec<(String, config::BlacklistRule)> = Vec::new();
    let mut syntax_errs: Vec<String> = Vec::new();
    let mut all_warns: Vec<String> = Vec::new();
    let mut repo_issue = false; // a message referred to an unknown/inactive repo
    for (idx, n) in names.iter().enumerate() {
        let raw = n.trim().to_string();
        match config::parse_blacklist_rule(n) {
            Ok(rule) => {
                if let Some(w) = frozen_warn(&raw, &rule, &active) {
                    all_warns.push(w);
                }
                rules.push((raw, rule));
            }
            Err(e) => {
                // Drop the leading "'raw': " the parser prepends so the batched
                // list stays aligned.
                let pfx = format!("'{raw}': ");
                let detail = e.strip_prefix(pfx.as_str()).unwrap_or(e.as_str()).to_string();
                // First check that the @repo (if any) is even an active repo: a
                // mistyped/inactive repo is the likelier mistake than a missing
                // pattern, so lead with it (with a typo hint), and remind that a
                // not-yet-enabled repo can still be declared with a full rule + --yes.
                let repo_tok = n
                    .trim()
                    .strip_prefix('@')
                    .and_then(|s| s.split(char::is_whitespace).next())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let detail = match repo_tok {
                    Some(r) if !active.contains(&r) => {
                        repo_issue = true;
                        let mut d = format!("no active repo '{r}'");
                        if let Some(s) = closest(r, active.iter().copied()) {
                            d.push_str(&format!(" — did you mean '@{s}'?"));
                        }
                        d.push_str(
                            "; a rule also needs a pattern, e.g. \"@REPO vlc\" \
                             (add --yes to declare one for a repo you will enable later)",
                        );
                        d
                    }
                    _ => detail,
                };
                let mut msg = format!("{:<22} {detail}", format!("\"{raw}\""));
                // Bare `@repo` followed by a separate argument => the user likely
                // forgot to quote them as one rule.
                if n.starts_with('@') && !n.contains(char::is_whitespace) && repo_tok.map_or(false, |r| active.contains(&r)) {
                    if let Some(next) = names.get(idx + 1) {
                        msg.push_str(&format!("  (did you mean \"{n} {next}\" ?)"));
                    }
                }
                syntax_errs.push(msg);
            }
        }
    }

    // Any syntax error is fatal: report every problem found (syntax + repo
    // typos) so the user can fix them all in one pass, and change nothing.
    if !syntax_errs.is_empty() {
        let total = syntax_errs.len() + all_warns.len();
        let mut out = format!("{total} problem(s), nothing changed:\n");
        for s in syntax_errs.iter().chain(all_warns.iter()) {
            out.push_str(&format!("  {s}\n"));
        }
        if !all_warns.is_empty() || repo_issue {
            out.push_str(&format!("  active repos: {}", active.join(", ")));
        }
        return Err(out.trim_end().to_string());
    }

    // Load the current blacklist and drop rules already present, so the
    // confirmation reflects exactly what will be added (not duplicates).
    let path = cfg.config_dir.join("blacklist");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let present: HashSet<String> = existing
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();

    let mut to_add: Vec<(String, config::BlacklistRule)> = Vec::new();
    let mut already: Vec<String> = Vec::new();
    for (raw, rule) in rules {
        if present.contains(&raw) {
            already.push(raw);
        } else {
            to_add.push((raw, rule));
        }
    }
    if !already.is_empty() {
        println!(
            "{}",
            ui::dim(&format!("already frozen, skipping: {}", already.join(", ")))
        );
    }
    if to_add.is_empty() {
        println!("Nothing new to add — every given rule is already frozen.");
        return Ok(Outcome::Ok);
    }

    // Rules that parse but look like mistakes (unknown @repo, or a regex with a
    // space that can never match) — considered only among the ones being added.
    let warns: Vec<String> = to_add
        .iter()
        .filter_map(|(raw, rule)| frozen_warn(raw, rule, &active))
        .collect();
    if !warns.is_empty() {
        println!(
            "{}",
            ui::purple(&format!("{} rule(s) look like a mistake:", warns.len()))
        );
        for s in &warns {
            println!("  {s}");
        }
        println!("  active repos: {}", active.join(", "));
        if !confirm("declare them anyway?", cli.yes) {
            println!("{}", ui::blue("aborted — nothing changed"));
            return Ok(Outcome::Ok);
        }
    }

    // Always confirm before writing: spell out exactly what will be frozen.
    println!("About to add {} blacklist rule(s):", to_add.len());
    for (i, (raw, rule)) in to_add.iter().enumerate() {
        println!(
            "  {}. {}  {}  {}",
            i + 1,
            ui::white(&format!("\"{raw}\"")),
            ui::dim("→"),
            rule.describe()
        );
    }
    if !confirm("Add these to the blacklist?", cli.yes) {
        println!("{}", ui::blue("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }

    // Append the new rules.
    let mut body = existing;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    for (raw, _) in &to_add {
        body.push_str(raw);
        body.push('\n');
    }
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    let added: Vec<&str> = to_add.iter().map(|(r, _)| r.as_str()).collect();
    println!("Frozen (added to blacklist): {}", added.join(", "));
    Ok(Outcome::Ok)
}

/// Remove blacklist rules from the file text by EXACT canonical match.
///
/// Each file line is canonicalised with the same `strip_comment` the parser
/// uses, then compared to the requested rules by literal string equality — a
/// rule is NEVER interpreted as a regex, so metacharacters (`.*`, `*`, `-`,
/// `[]`, `/`) are matched verbatim and a partial name can never match a longer
/// rule. Comment and blank lines, and any rule not requested, are preserved
/// exactly. Returns (new_text, removed_rules, not_found_rules). Pure: no I/O.
fn blacklist_remove(text: &str, wanted: &[&str]) -> (String, Vec<String>, Vec<String>) {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for line in text.lines() {
        let rule = config::strip_comment(line);
        if !rule.is_empty() && wanted.contains(&rule) {
            removed.push(rule.to_string());
        } else {
            kept.push(line); // comments, blanks and untouched rules kept verbatim
        }
    }
    let mut not_found: Vec<String> = Vec::new();
    for w in wanted {
        if !removed.iter().any(|r| r.as_str() == *w) {
            not_found.push((*w).to_string());
        }
    }
    let mut body = kept.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    (body, removed, not_found)
}

/// Remove ("unfreeze") one or more blacklist rules. The counterpart to `frozen`.
/// With no argument it lists the current rules so the exact text to remove is
/// visible. Matching is by exact literal rule text (see `blacklist_remove`).
fn cmd_unfrozen(_cli: &Cli, cfg: &Config, names: &[String]) -> Result<Outcome, String> {
    let path = cfg.config_dir.join("blacklist");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    // No argument: show the current rules (exact text) and stop.
    if names.is_empty() {
        let rules: Vec<&str> = text
            .lines()
            .map(config::strip_comment)
            .filter(|r| !r.is_empty())
            .collect();
        if rules.is_empty() {
            println!("The blacklist has no rules.");
            return Ok(Outcome::Ok);
        }
        println!("{}", ui::blue("Current blacklist rules:"));
        for r in &rules {
            println!("  {}", ui::white(r));
        }
        return Err(
            "unfrozen: give one or more rules to remove, EXACTLY as shown above \
             (quote rules with spaces or shell-special characters)"
                .into(),
        );
    }

    // Canonicalise the requested rules the same way a file line is canonicalised,
    // then remove by exact literal match.
    let wanted: Vec<&str> = names.iter().map(|s| config::strip_comment(s)).collect();
    let (new_text, removed, not_found) = blacklist_remove(&text, &wanted);

    if removed.is_empty() {
        return Err(format!(
            "no matching rule in {}: {}\n\
             (rules must match exactly — run `slacker unfrozen` with no argument to see them)",
            path.display(),
            not_found.join(", ")
        ));
    }

    std::fs::write(&path, new_text).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!(
        "Unfrozen (removed from blacklist): {}",
        removed.join(", ")
    );
    if !not_found.is_empty() {
        println!(
            "  {}",
            ui::dim(&format!("not found (unchanged): {}", not_found.join(", ")))
        );
    }
    Ok(Outcome::Ok)
}

/// If `rule` is a pin line `@repo 100% pkg`, return (repo, pkg). Mirrors the
/// parser in config::parse_blacklist_rule so the two never disagree.
fn parse_pin_line(rule: &str) -> Option<(&str, &str)> {
    let after = rule.strip_prefix('@')?;
    let mut it = after.splitn(2, char::is_whitespace);
    let repo = it.next()?.trim();
    let rest = it.next()?.trim();
    let pkg = rest.strip_prefix("100%")?.trim();
    if repo.is_empty() || pkg.is_empty() || pkg.contains(char::is_whitespace) {
        return None;
    }
    Some((repo, pkg))
}

/// Drop every pin line (`@repo 100% name`) whose package is in `names`, keeping
/// all other lines (freezes, comments, blanks) verbatim. Returns the new text
/// and the removed (package, repo) pairs.
fn blacklist_remove_pins(text: &str, names: &[&str]) -> (String, Vec<(String, String)>) {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let rule = config::strip_comment(line);
        if let Some((repo, pkg)) = parse_pin_line(rule) {
            if names.contains(&pkg) {
                removed.push((pkg.to_string(), repo.to_string()));
                continue;
            }
        }
        kept.push(line);
    }
    let mut body = kept.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    (body, removed)
}

/// Pin a package to one repo regardless of priority. Writes `@repo 100% package`
/// into the `blacklist` file; the counterpart is `unpin`. A package has at most
/// one pin, so pinning to a new repo replaces any existing pin for it.
fn cmd_pin(cli: &Cli, cfg: &Config, spec: Option<&str>) -> Result<Outcome, String> {
    // No argument: show the current pins (like `unpin`) and how to add one.
    let spec = match spec {
        Some(s) => s,
        None => {
            let pins = cfg.pins();
            if pins.is_empty() {
                println!("No pins set.");
            } else {
                println!("{}", ui::blue("Current pins (package -> repo):"));
                for (pkg, repo) in pins {
                    println!("  {} {} {}", ui::white(pkg), ui::dim("->"), ui::cyan(repo));
                }
            }
            println!(
                "  {}",
                ui::dim("to add a pin: `pin repo:package`, e.g. `pin alienbob:vlc`")
            );
            return Ok(Outcome::Ok);
        }
    };
    let (repo, name) = spec.split_once(':').ok_or_else(|| {
        format!("pin takes repo:package, e.g. \"pin alienbob:vlc\" (got \"{spec}\")")
    })?;
    let (repo, name) = (repo.trim(), name.trim());
    if repo.is_empty() || name.is_empty() {
        return Err(format!("pin takes repo:package, e.g. \"pin alienbob:vlc\" (got \"{spec}\")"));
    }
    if name.contains(char::is_whitespace) {
        return Err(format!("pin takes a single package name, not \"{name}\""));
    }
    if config::name_has_pattern_chars(name) {
        let is_glob = name.contains(|c| matches!(c, '*' | '?'));
        return Err(format!(
            "pin takes an EXACT package name — no {} (got \"{name}\"). \
             For pattern freezing use `slacker frozen`, which accepts both globs \
             (`*`, `?`) and regexes; e.g. `slacker frozen \"{name}\"`.",
            if is_glob { "shell wildcards" } else { "patterns" }
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        let fixed = name.trim_matches('-');
        return Err(format!(
            "\"{name}\" is not a valid package name (a name does not start or end with '-'){}",
            if fixed.is_empty() {
                ".".to_string()
            } else {
                format!(" — did you mean \"{fixed}\"?")
            }
        ));
    }

    // The target repo must be active (with a did-you-mean for a typo).
    let active: Vec<&str> = cfg.repos.iter().map(|r| r.name.as_str()).collect();
    if !active.contains(&repo) {
        let mut msg = format!("no active repo '{repo}'");
        if let Some(s) = closest(repo, active.iter().copied()) {
            msg.push_str(&format!(" — did you mean '{s}'?"));
        }
        msg.push_str(&format!("\n  active repos: {}", active.join(", ")));
        return Err(msg);
    }

    // Tolerant load: a pin may be set before `update` (or for a repo not yet
    // fetched). Missing metadata just means we cannot confirm the package yet.
    let (db, _missing) = PkgDb::load_available(cfg);

    // Warn (but allow) if the repo does not currently provide the package — repos
    // change, and a pin may be set ahead of time. Offer a name typo hint.
    if db.resolve(&format!("{repo}:{name}")).is_none() {
        println!(
            "{}",
            ui::purple(&format!(
                "note: '{repo}' does not currently provide '{name}' — pin recorded anyway; \
                 it takes effect once the repo offers it"
            ))
        );
        if let Some(s) = closest(name, db.available_names()) {
            println!("  {}", ui::dim(&format!("did you mean '{repo}:{s}'?")));
        }
    }

    // A freeze on the same package overrides the pin (frozen is absolute) — say so.
    let frozen_here = cfg.blacklist_hit(name, db.series_of(name), Some(repo));
    if frozen_here {
        println!(
            "{}",
            ui::purple(&format!(
                "warning: '{name}' is also frozen (blacklisted) — the freeze wins, so the pin \
                 will have no effect until you `unfrozen` it"
            ))
        );
    }

    let line = format!("@{repo} 100% {name}");
    let path = cfg.config_dir.join("blacklist");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    // Already pinned exactly here? Nothing to do.
    if existing.lines().any(|l| config::strip_comment(l) == line) {
        println!("Already pinned: {name} -> {repo}");
        return Ok(Outcome::Ok);
    }

    // Pinned to a DIFFERENT repo? A package has one pin, so replace it.
    let prior = cfg.pinned_repo(name).map(str::to_string);
    let (mut text, _) = blacklist_remove_pins(&existing, &[name]);
    if let Some(p) = &prior {
        println!("{}", ui::dim(&format!("replacing existing pin: {name} -> {p}")));
    }

    println!("About to pin (only source for '{name}', ignoring priority):");
    println!("  {}", ui::white(&line));
    if !confirm("write it to the blacklist?", cli.yes) {
        println!("{}", ui::blue("aborted — nothing changed"));
        return Ok(Outcome::Ok);
    }

    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{}", ui::green(&format!("Pinned: {name} -> {repo}")));
    Ok(Outcome::Ok)
}

/// Remove a package's pin, returning it to normal priority-based resolution. The
/// counterpart to `pin`. With no argument, lists the current pins.
fn cmd_unpin(_cli: &Cli, cfg: &Config, names: &[String]) -> Result<Outcome, String> {
    let path = cfg.config_dir.join("blacklist");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    if names.is_empty() {
        let pins = cfg.pins();
        if pins.is_empty() {
            println!("No pins set.");
            println!(
                "  {}",
                ui::dim("to add a pin: `pin repo:package`, e.g. `pin alienbob:vlc`")
            );
            return Ok(Outcome::Ok);
        }
        println!("{}", ui::blue("Current pins (package -> repo):"));
        for (pkg, repo) in pins {
            println!("  {} {} {}", ui::white(pkg), ui::dim("->"), ui::cyan(repo));
        }
        println!(
            "  {}",
            ui::dim("to remove a pin: `unpin package`, e.g. `unpin vlc`")
        );
        return Ok(Outcome::Ok);
    }

    let wanted: Vec<&str> = names.iter().map(|s| s.trim()).collect();
    let (new_text, removed) = blacklist_remove_pins(&text, &wanted);
    if removed.is_empty() {
        return Err(format!(
            "no pin found for: {}\n  (run `slacker unpin` with no argument to list pins)",
            wanted.join(", ")
        ));
    }
    std::fs::write(&path, new_text).map_err(|e| format!("write {}: {e}", path.display()))?;
    let summary: Vec<String> =
        removed.iter().map(|(p, r)| format!("{p} (was -> {r})")).collect();
    println!("{}", ui::green(&format!("Unpinned: {}", summary.join(", "))));
    Ok(Outcome::Ok)
}



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
const ADD_REPO_USAGE: &str = "usage: slacker add-repo PRIORITY NAME URL [official] [immutable] [subtree] [verify=gpg,md5]\n  \
     e.g.  slacker add-repo 60 alienbob https://slackware.nl/people/alien/sbrepos/current/x86_64\n  \
     e.g.  slacker add-repo 70 extras https://slackware.uk/slackware/slackware64-current/extra subtree\n  \
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
fn cmd_pri_repo(cli: &Cli, cfg: &Config, priority: &str, name: &str) -> Result<Outcome, String> {
    let pri = parse_priority(priority, PRI_REPO_USAGE)?;
    // 1) the repo must exist and be active.
    let target = match cfg.repos.iter().find(|r| r.name == name) {
        Some(r) => r,
        None => {
            let names: Vec<&str> = cfg.repos.iter().map(|r| r.name.as_str()).collect();
            let mut msg = format!("no active repo named '{name}'");
            match closest(name, names.iter().copied()) {
                Some(s) => msg.push_str(&format!(" — did you mean '{s}'?")),
                None if names.is_empty() => msg.push_str("\n  there are no active repos."),
                None => msg.push_str(&format!("\n  active repos: {}", names.join(", "))),
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

fn cmd_add_repo(
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

fn cmd_del_repo(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
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
            "no repository named '{name}' in {} (configured: {})",
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

fn cmd_add_tag(
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

fn cmd_del_tag(cli: &Cli, cfg: &Config, tag: &str) -> Result<Outcome, String> {
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

fn cmd_vet_repo(cfg: &Config, name: &str) -> Result<Outcome, String> {
    let r = cfg
        .repo_by_name(name)
        .ok_or_else(|| format!("no repo named '{name}' in {}", cfg.config_dir.join("repos").display()))?
        .clone();
    // Force a fresh verdict: drop any prior "trusted" mark so the checks run.
    repo::unmark_trusted(&cfg.state_dir, name);
    apply_vet(cfg, &r);
    Ok(Outcome::Ok)
}

fn cmd_trust_repo(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    // Allow trusting even if the name isn't currently in repos (e.g. a stale
    // marker), but prefer a clear error when the repo truly doesn't exist.
    if cfg.repo_by_name(name).is_none() {
        return Err(format!(
            "no repo named '{name}' in {}",
            cfg.config_dir.join("repos").display()
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

fn cmd_distrust_repo(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    let r = cfg
        .repo_by_name(name)
        .ok_or_else(|| format!("no repo named '{name}' in {}", cfg.config_dir.join("repos").display()))?
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
mod parallel_download_tests {
    use super::{summarize_outcomes, verify_suffix, DlOutcome};

    fn ok(idx: usize, name: &str, checks: &[&str]) -> DlOutcome {
        DlOutcome {
            idx,
            name: name.into(),
            result: Ok(checks.iter().map(|s| s.to_string()).collect()),
        }
    }
    fn err(idx: usize, name: &str, reason: &str) -> DlOutcome {
        DlOutcome {
            idx,
            name: name.into(),
            result: Err(reason.into()),
        }
    }

    #[test]
    fn verify_suffix_reflects_strength() {
        assert_eq!(verify_suffix(&["gpg (Someone)".into(), "md5".into()]), "");
        assert_eq!(verify_suffix(&["md5".into()]), "(integrity only) ");
        assert_eq!(verify_suffix(&[]), "(verify off) ");
    }

    #[test]
    fn summarize_marks_ready_and_collects_failures() {
        let outcomes = vec![
            ok(0, "alpha", &["gpg (X)", "md5"]),
            err(1, "bravo", "md5 mismatch for bravo"),
            ok(2, "charlie", &["md5"]),
        ];
        let (ready, failed) = summarize_outcomes(&outcomes, 3);
        assert_eq!(ready, vec![true, false, true]);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, "bravo");
        assert!(failed[0].1.contains("mismatch"));
    }

    #[test]
    fn summarize_ignores_out_of_range_index() {
        // a stray out-of-range Ok index must neither panic nor set anything
        let outcomes = vec![ok(5, "ghost", &["md5"])];
        let (ready, failed) = summarize_outcomes(&outcomes, 2);
        assert_eq!(ready, vec![false, false]);
        assert!(failed.is_empty());
    }
}

#[cfg(test)]
mod unfreeze_tests {
    use super::blacklist_remove;
    use super::{arch_compatible, blacklist_remove_pins, parse_pin_line};

    #[test]
    fn arch_family_compatibility() {
        // 32-bit x86 is one family: i586 base, i686 build, etc. interchangeable.
        assert!(arch_compatible("i586", "i686"));
        assert!(arch_compatible("i686", "i586"));
        assert!(arch_compatible("i586", "i486"));
        assert!(arch_compatible("x86_64", "x86_64"));
        // noarch always fits.
        assert!(arch_compatible("x86_64", "noarch"));
        assert!(arch_compatible("i686", "noarch"));
        // Cross-family is refused, both directions.
        assert!(!arch_compatible("x86_64", "i686"));
        assert!(!arch_compatible("i586", "x86_64"));
        assert!(!arch_compatible("x86_64", "aarch64"));
        // multilib compat32 packages carry an x86_64 arch field -> fine on x86_64.
        // (the `compat32` token lives in the name/build, not the arch field)
        assert!(arch_compatible("x86_64", "x86_64"));
    }

    #[test]
    fn parse_pin_line_recognizes_pins_only() {
        assert_eq!(parse_pin_line("@alienbob 100% vlc"), Some(("alienbob", "vlc")));
        // Not pins:
        assert_eq!(parse_pin_line("@alienbob vlc"), None); // freeze
        assert_eq!(parse_pin_line("vlc"), None); // bare freeze
        assert_eq!(parse_pin_line("@alienbob 100%"), None); // no package
        assert_eq!(parse_pin_line("@alienbob 100% a b"), None); // two names
    }

    #[test]
    fn remove_pins_drops_only_matching_pins() {
        // Freezes (plain + scoped) and comments must survive; only the named pins go.
        let text = "# head\nfirefox\n@alienbob 100% vlc\n@conraid 100% mpv\n@alienbob kde/\n";
        let (out, removed) = blacklist_remove_pins(text, &["vlc"]);
        assert_eq!(removed, vec![("vlc".to_string(), "alienbob".to_string())]);
        assert_eq!(out, "# head\nfirefox\n@conraid 100% mpv\n@alienbob kde/\n");
        // Removing a name with no pin removes nothing.
        let (out2, removed2) = blacklist_remove_pins(&out, &["firefox"]);
        assert!(removed2.is_empty());
        assert_eq!(out2, out);
    }

    #[test]
    fn removes_exact_rule_only_no_glob() {
        // 'fcitx5*' must remove only that line, never the longer 'fcitx5-qt'.
        let text = "# header\nsbopkg\nfcitx5*\nfcitx5-qt\nemacs\n";
        let (out, removed, nf) = blacklist_remove(text, &["fcitx5*"]);
        assert_eq!(removed, vec!["fcitx5*"]);
        assert!(nf.is_empty());
        assert_eq!(out, "# header\nsbopkg\nfcitx5-qt\nemacs\n");
    }

    #[test]
    fn regex_metachars_are_literal() {
        let text = "xf86-.*-202.*\nvlc\n";
        let (out, removed, _) = blacklist_remove(text, &["xf86-.*-202.*"]);
        assert_eq!(removed, vec!["xf86-.*-202.*"]);
        assert_eq!(out, "vlc\n");
    }

    #[test]
    fn partial_name_does_not_match() {
        // 'fcitx5' must NOT match the rule 'fcitx5*' — exact only.
        let text = "fcitx5*\n";
        let (out, removed, nf) = blacklist_remove(text, &["fcitx5"]);
        assert!(removed.is_empty());
        assert_eq!(nf, vec!["fcitx5"]);
        assert_eq!(out, "fcitx5*\n"); // unchanged
    }

    #[test]
    fn preserves_comments_and_reports_not_found() {
        // 'emacs # note' canonicalises to 'emacs' and matches; 'ghost' does not.
        let text = "# keep me\nemacs  # inline note\nsbopkg\n";
        let (out, removed, nf) = blacklist_remove(text, &["emacs", "ghost"]);
        assert_eq!(removed, vec!["emacs"]);
        assert_eq!(nf, vec!["ghost"]);
        assert_eq!(out, "# keep me\nsbopkg\n");
    }

    #[test]
    fn repo_scoped_rule_does_not_touch_bare_rule() {
        let text = "@alienbob vlc\nvlc\n";
        let (out, removed, _) = blacklist_remove(text, &["@alienbob vlc"]);
        assert_eq!(removed, vec!["@alienbob vlc"]);
        assert_eq!(out, "vlc\n");
    }

    #[test]
    fn removing_last_rule_leaves_header_intact() {
        let text = "# blacklist\nemacs\n";
        let (out, removed, _) = blacklist_remove(text, &["emacs"]);
        assert_eq!(removed, vec!["emacs"]);
        assert_eq!(out, "# blacklist\n");
    }
}

#[cfg(test)]
mod selection_tests {
    use super::parse_selection;

    #[test]
    fn edit_distance_basics() {
        assert_eq!(super::edit_distance("gnome", "gnome"), 0);
        assert_eq!(super::edit_distance("gnme", "gnome"), 1);
        assert_eq!(super::edit_distance("gnom", "gnome"), 1);
        assert_eq!(super::edit_distance("xyz", "gnome"), 5);
    }

    #[test]
    fn closest_suggests_within_two() {
        let cands = ["gnome", "conraid", "slackware"];
        assert_eq!(super::closest("gnme", cands.into_iter()), Some("gnome".into()));
        assert_eq!(super::closest("conrad", cands.into_iter()), Some("conraid".into()));
        assert_eq!(super::closest("zzzzzz", cands.into_iter()), None);
    }

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
    fn privilege_classification() {
        use super::{requires_privilege, Cmd};
        // read-only commands are free
        assert!(!requires_privilege(&Cmd::Search { pattern: "x".into() }));
        assert!(!requires_privilege(&Cmd::Info { name: "x".into() }));
        assert!(!requires_privilege(&Cmd::CheckUpdates));
        assert!(!requires_privilege(&Cmd::ShowChangelog { repo: None }));
        assert!(!requires_privilege(&Cmd::FileSearch { filename: "x".into() }));
        // mutating / cache-writing commands need root
        assert!(requires_privilege(&Cmd::Update { mode: None }));
        assert!(requires_privilege(&Cmd::UpgradeAll));
        assert!(requires_privilege(&Cmd::CleanCache { repos: vec![] }));
        assert!(requires_privilege(&Cmd::Frozen { names: vec![] }));
        assert!(requires_privilege(&Cmd::Download { patterns: vec![], output: None }));
    }

    #[test]
    fn parse_selection_works() {
        assert_eq!(parse_selection("1 3 5", 5), [1,3,5].into_iter().collect());
        assert_eq!(parse_selection("2-4", 5), [2,3,4].into_iter().collect());
        assert_eq!(parse_selection("1,3", 5), [1,3].into_iter().collect());
        assert_eq!(parse_selection("1 99 0", 5), [1].into_iter().collect()); // out-of-range dropped
        assert_eq!(parse_selection("1 3-5", 6), [1,3,4,5].into_iter().collect()); // list + range
        assert!(parse_selection("", 5).is_empty());
        assert!(parse_selection("xyz", 5).is_empty());
    }
}

#[cfg(test)]
mod attribution_tests {
    use super::attribute_tags;
    use crate::config::TagPriority;
    use crate::pkg::PkgId;

    fn ids(names: &[&str]) -> Vec<PkgId> {
        names.iter().map(|n| PkgId::parse(n).unwrap()).collect()
    }
    fn rule(name: &str, tag: &str) -> TagPriority {
        TagPriority { name: name.into(), tag: tag.into(), priority: 100 }
    }

    #[test]
    fn every_installed_package_has_a_source_never_untracked() {
        let installed = ids(&[
            "aaa-1.0-x86_64-1",          // empty tag   -> official repo
            "bbb-2.0-x86_64-1",          // empty tag   -> official repo
            "vim-9.1-x86_64-1_SBo",      // declared    -> rule "SBo"
            "mc-4.8-x86_64-1_SBo",       // declared    -> rule "SBo"
            "asio-1.36.0-x86_64-1cf",    // repo-served -> "conraid"
            "slacker-0.3-x86_64-1_FRG",  // other tag   -> "_FRG"
            "myradio-1.0-x86_64-1_wsr",  // other tag   -> "_wsr"
        ]);
        let rules = vec![rule("SBo", "_SBo"), rule("local", "_YOURTAG")];
        let (per_repo, per_rule, per_other) = attribute_tags(
            Some("slackware"),
            &rules,
            |t| (t == "cf").then(|| "conraid".to_string()),
            &installed,
        );

        assert_eq!(per_repo.get("slackware"), Some(&2));
        assert_eq!(per_repo.get("conraid"), Some(&1));
        assert_eq!(per_rule.get("SBo"), Some(&2));
        // A declared rule with no matching package is simply absent (count 0),
        // not an error and not "untracked".
        assert_eq!(per_rule.get("local"), None);
        assert_eq!(per_other.get("_FRG"), Some(&1));
        assert_eq!(per_other.get("_wsr"), Some(&1));

        // Nothing is dropped: every installed package is attributed exactly once.
        let total: usize =
            per_repo.values().chain(per_rule.values()).chain(per_other.values()).sum();
        assert_eq!(total, installed.len());
        // There is no "untracked" bucket; the only "other" keys are real tags.
        assert!(!per_other.keys().any(|k| k.contains("untracked")));
    }
}

#[cfg(test)]
mod collect_tests {
    use super::*;

    fn av(nv: &str, repo_: &str) -> repo::AvailPkg {
        repo::AvailPkg {
            id: pkg::PkgId::parse(nv).unwrap(),
            filename: format!("{nv}.txz"),
            location: "./x/".into(),
            series: "x".into(),
            size_k: None,
            size_uncompressed_k: None,
            summary: String::new(),
            description: String::new(),
            md5: None,
            sha: None,
            required: Vec::new(),
            conflicts: Vec::new(),
            suggests: String::new(),
            repo: repo_.into(),
        }
    }

    #[test]
    fn detect_conflicts_finds_installed_declared_conflicts() {
        // `foo` declares a conflict with `bar`; flagged only when bar is installed.
        let mut foo = av("foo-1.0-x86_64-1", "alienbob");
        foo.conflicts = vec!["bar".into(), "foo".into()]; // self-name must be ignored
        let plan = vec![PlanItem {
            pkg: foo,
            action: InstallAction::Install,
            dep_for: None,
            from: None,
        }];
        let bar_installed = vec![pkg::PkgId::parse("bar-2.0-x86_64-1.txz").unwrap()];

        // bar installed -> exactly one conflict (foo vs bar); self-conflict dropped.
        let c = detect_conflicts(&plan, &bar_installed, true);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].installing, "foo");
        assert_eq!(c[0].installed, "bar");
        // resolve disabled -> nothing.
        assert!(detect_conflicts(&plan, &bar_installed, false).is_empty());
        // bar not installed -> nothing.
        assert!(detect_conflicts(&plan, &[], true).is_empty());
    }

    #[test]
    fn shell_expansion_gate_blocks_floods_but_allows_deliberate_terms() {
        // Deliberate use must stay open: no misses (e.g. `reinstall emacs` ->
        // emacs, emacspeak), or a single ordinary miss/typo, never blocks.
        assert!(guard_shell_expansion(&[]).is_ok());
        assert!(guard_shell_expansion(&pats(&["emacss"])).is_ok());
        // A flood of unmatched args is a shell-expanded glob -> refuse the command.
        let flood = pats(&["1.html", "me.jpg", "a", "b", "c", "d", "e", "f"]); // 8
        assert!(guard_shell_expansion(&flood).is_err());
        // Two whitespace-bearing tokens can never be package names -> refuse even
        // below the flood threshold (a glob expanded onto files with spaces).
        let spaced = pats(&["VirtualBox VMs", "anagnoristiko starlink"]);
        assert!(guard_shell_expansion(&spaced).is_err());
        // ...but a single space-bearing token alone is not enough to refuse.
        assert!(guard_shell_expansion(&pats(&["one weird name"])).is_ok());
    }

    // slackware (priority 100) and alienbob (60) both provide `vlc`; each also
    // has a package only it ships. Shared fixture for the integration tests.
    fn fixture() -> PkgDb {
        PkgDb::for_test(
            vec![
                av("vlc-3.0.20-x86_64-1", "slackware"),
                av("vlc-3.0.21-x86_64-1alien", "alienbob"),
                av("aaa-1.0-x86_64-1", "slackware"),
                av("bbb-1.0-x86_64-1alien", "alienbob"),
            ],
            &[("slackware", 100), ("alienbob", 60)],
            Some(100),
        )
    }

    fn repo_of(pkgs: &[&repo::AvailPkg], name: &str) -> Option<String> {
        pkgs.iter().find(|p| p.id.name == name).map(|p| p.repo.clone())
    }

    fn pats(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn search_display_prefers_installed_source_over_priority_winner() {
        // conraid (the priority winner) and alienbob both ship flatpak.
        let conraid = av("flatpak-1.18.0-x86_64-2cf", "conraid");
        let alien = av("flatpak-1.18.0-x86_64-1alien", "alienbob");
        let cands = [&conraid, &alien]; // candidates(): winner first

        // installed from alienbob (1alien) -> show alienbob, NOT the winner.
        let inst_alien = pkg::PkgId::parse("flatpak-1.18.0-x86_64-1alien").unwrap();
        assert_eq!(search_display(&conraid, Some(&inst_alien), &cands).repo.as_str(), "alienbob");

        // installed from conraid (2cf) -> show conraid.
        let inst_cf = pkg::PkgId::parse("flatpak-1.18.0-x86_64-2cf").unwrap();
        assert_eq!(search_display(&conraid, Some(&inst_cf), &cands).repo.as_str(), "conraid");

        // not installed -> the priority winner.
        assert_eq!(search_display(&conraid, None, &cands).repo.as_str(), "conraid");

        // installed from a source with no matching candidate (local/_SBo) ->
        // fall back to the winner.
        let inst_sbo = pkg::PkgId::parse("flatpak-1.18.0-x86_64-1_SBo").unwrap();
        assert_eq!(search_display(&conraid, Some(&inst_sbo), &cands).repo.as_str(), "conraid");
    }

    #[test]
    fn dist_upgrade_sets_bypass_priority_and_order_critical() {
        // Target repos after the transform: slackware(100) and patches(200).
        let db = PkgDb::for_test(
            vec![
                av("glibc-solibs-2.41-x86_64-1", "slackware"),
                av("xz-5.6.2-x86_64-1", "slackware"),
                av("vlc-3.0.21-x86_64-1", "slackware"),
                av("kernel-generic-6.6.30-x86_64-1", "patches"),
                av("kernel-generic-6.6.10-x86_64-1", "slackware"),
            ],
            &[("slackware", 100), ("patches", 200)],
            Some(100),
        );
        // Installed: old core (glibc-solibs, xz), a vlc that came from a now-disabled
        // third-party repo (its build tag would outrank slackware), and an old kernel.
        let installed = vec![
            pkg::PkgId::parse("glibc-solibs-2.33-x86_64-1").unwrap(),
            pkg::PkgId::parse("xz-5.2.5-x86_64-1").unwrap(),
            pkg::PkgId::parse("vlc-3.0.18-x86_64-1alien").unwrap(),
            pkg::PkgId::parse("kernel-generic-6.6.5-x86_64-1").unwrap(),
        ];
        let (critical, rest) = dist_upgrade_sets(&db, &installed);

        // Critical set is ordered per DIST_CRITICAL: glibc-solibs before xz; the
        // absent ones (pkgtools/tar/gzip/findutils) are simply skipped.
        let cnames: Vec<&str> = critical.iter().map(|p| p.pkg.id.name.as_str()).collect();
        assert_eq!(cnames, vec!["glibc-solibs", "xz"]);

        // vlc is upgraded to slackware's version even though the installed copy
        // came from a higher-priority third-party source — the dist bypass.
        let vlc = rest.iter().find(|p| p.pkg.id.name == "vlc").unwrap();
        assert_eq!(vlc.pkg.repo, "slackware");
        assert_eq!(vlc.pkg.id.version, "3.0.21");

        // kernel-generic is taken from the priority winner (patches), proving the
        // set is built from db.resolve (highest-priority candidate).
        let kernel = rest.iter().find(|p| p.pkg.id.name == "kernel-generic").unwrap();
        assert_eq!(kernel.pkg.repo, "patches");
    }

    #[test]
    fn dist_defers_gpg_chain_to_the_end() {
        // A normal package plus gnupg + a gpg lib, all upgradeable; the GnuPG
        // verification chain (DIST_GPG_LAST) must come LAST in the rest order so
        // the working gpg keeps verifying everything else first.
        let db = PkgDb::for_test(
            vec![
                av("zlib-1.3-x86_64-1", "slackware"),
                av("gnupg-2.4-x86_64-1", "slackware"),
                av("libassuan-3.0-x86_64-1", "slackware"),
                av("aaa_base-16.0-x86_64-1", "slackware"),
            ],
            &[("slackware", 100)],
            Some(100),
        );
        let installed: Vec<pkg::PkgId> = ["zlib", "gnupg", "libassuan", "aaa_base"]
            .iter()
            .map(|n| pkg::PkgId::parse(&format!("{n}-0.1-x86_64-1")).unwrap())
            .collect();
        let (_critical, rest) = dist_upgrade_sets(&db, &installed);
        let order: Vec<&str> = rest.iter().map(|it| it.pkg.id.name.as_str()).collect();
        let gnupg = order.iter().position(|n| *n == "gnupg").unwrap();
        let libassuan = order.iter().position(|n| *n == "libassuan").unwrap();
        let zlib = order.iter().position(|n| *n == "zlib").unwrap();
        assert!(
            gnupg > zlib && libassuan > zlib,
            "gpg chain must come after non-gpg pkgs: {order:?}"
        );
    }

    #[test]
    fn kernel_packages_are_recognised() {
        assert!(is_kernel_pkg("kernel-generic"));
        assert!(is_kernel_pkg("kernel-huge"));
        assert!(is_kernel_pkg("kernel-modules"));
        assert!(is_kernel_pkg("kernel-modules-6.6.5"));
        // not boot-critical kernel packages
        assert!(!is_kernel_pkg("kernel-firmware"));
        assert!(!is_kernel_pkg("kernel-headers"));
        assert!(!is_kernel_pkg("kernel-source"));
        assert!(!is_kernel_pkg("bash"));
    }

    #[test]
    fn tool_in_dirs_detects_executables() {
        let dir = std::env::temp_dir().join("slacker_tool_in_dirs_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let exe = dir.join("faketool");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let dirs = vec![dir.clone()];
        assert!(tool_in_dirs("faketool", &dirs)); // present and executable
        assert!(!tool_in_dirs("absent", &dirs)); // not there at all

        // On unix a present-but-non-executable file must NOT count as a tool.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let plain = dir.join("plain");
            std::fs::write(&plain, b"x").unwrap();
            std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!tool_in_dirs("plain", &dirs));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_state_moves_trust_anchors_once_and_never_clobbers() {
        let base = std::env::temp_dir().join("slacker_migrate_state_test");
        let _ = std::fs::remove_dir_all(&base);
        let cache = base.join("cache");
        let state = base.join("state");
        // Old layout: trust anchors live under the (disposable) cache dir.
        std::fs::create_dir_all(cache.join("gpg")).unwrap();
        std::fs::write(cache.join("gpg").join("alienbob.fpr"), "ABC\n").unwrap();
        std::fs::create_dir_all(cache.join("quarantine")).unwrap();
        std::fs::write(cache.join("quarantine").join("badrepo"), "hard\nreason").unwrap();
        std::fs::create_dir_all(cache.join("trusted")).unwrap();
        std::fs::write(cache.join("trusted").join("slackware"), "").unwrap();
        // Re-downloadable cache data must stay put.
        std::fs::create_dir_all(cache.join("packages")).unwrap();

        migrate_state_dirs(&cache, &state);

        // Anchors moved to state, content preserved.
        assert_eq!(
            std::fs::read_to_string(state.join("gpg").join("alienbob.fpr")).unwrap(),
            "ABC\n"
        );
        assert!(state.join("quarantine").join("badrepo").exists());
        assert!(state.join("trusted").join("slackware").exists());
        // ...and gone from cache.
        assert!(!cache.join("gpg").exists());
        assert!(!cache.join("quarantine").exists());
        assert!(!cache.join("trusted").exists());
        // Cache-only data untouched.
        assert!(cache.join("packages").is_dir());

        // Established-install safety: if state already holds the anchors, a
        // stale cache copy must NEVER clobber them (that would re-pin = the
        // first-contact event the whole change exists to avoid).
        std::fs::create_dir_all(cache.join("gpg")).unwrap();
        std::fs::write(cache.join("gpg").join("evil.fpr"), "EVIL\n").unwrap();
        migrate_state_dirs(&cache, &state);
        assert!(state.join("gpg").join("alienbob.fpr").exists());
        assert!(!state.join("gpg").join("evil.fpr").exists());
        // The stale cache dir is left as-is (skipped because state existed).
        assert!(cache.join("gpg").join("evil.fpr").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn audit_flags_writable_and_symlinks_without_following() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join("slacker_audit_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();

        let normal = root.join("normal");
        std::fs::write(&normal, b"x").unwrap();
        std::fs::set_permissions(&normal, std::fs::Permissions::from_mode(0o644)).unwrap();

        let ww = root.join("sub").join("ww");
        std::fs::write(&ww, b"x").unwrap();
        std::fs::set_permissions(&ww, std::fs::Permissions::from_mode(0o666)).unwrap();

        // A symlink must be COUNTED but never followed (so its target is not
        // re-audited, and a link pointing outside the tree cannot lure us out).
        let link = root.join("link");
        std::os::unix::fs::symlink(&normal, &link).unwrap();

        let a = audit_owned_paths(&[root.as_path()]);
        assert!(a.world_writable >= 1, "world-writable file detected");
        assert!(a.symlinks >= 1, "stray symlink detected");
        assert!(a.severe(), "world-writable + symlink is a severe finding");
        assert!(!a.clean());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prefers_precedence_is_pin_then_priority_then_first() {
        // 1) a pin beats a non-pin, regardless of priority either way
        assert!(collect_prefers(true, 10, false, 100)); // low-prio pin beats high-prio non-pin
        assert!(!collect_prefers(false, 100, true, 10)); // high-prio non-pin can't displace a pin
        // 2) both non-pinned: higher priority wins
        assert!(collect_prefers(false, 100, false, 80));
        assert!(!collect_prefers(false, 80, false, 100));
        // 3) ties keep the incumbent (first seen)
        assert!(!collect_prefers(false, 100, false, 100)); // equal priority, non-pins
        assert!(!collect_prefers(true, 60, true, 100)); // two pins: first listed stays...
        assert!(!collect_prefers(true, 100, true, 60)); // ...whatever their priorities
    }

    #[test]
    fn single_pattern_is_unchanged() {
        let d = fixture();
        let (got, miss) = collect(&d, &pats(&["@slackware"])).unwrap();
        assert!(miss.is_empty());
        let mut names: Vec<&str> = got.iter().map(|p| p.id.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["aaa", "vlc"]); // one entry per name
        assert_eq!(repo_of(&got, "vlc").unwrap(), "slackware");
    }

    #[test]
    fn two_repos_higher_priority_wins_either_order() {
        let d = fixture();
        // alienbob listed first, but slackware (100) must win vlc over alienbob (60)
        let (a, _) = collect(&d, &pats(&["@alienbob", "@slackware"])).unwrap();
        assert_eq!(repo_of(&a, "vlc").unwrap(), "slackware");
        // reversed order: priority, not listing order, decides
        let (b, _) = collect(&d, &pats(&["@slackware", "@alienbob"])).unwrap();
        assert_eq!(repo_of(&b, "vlc").unwrap(), "slackware");
        // each repo's unique package is still present
        assert!(repo_of(&a, "aaa").is_some() && repo_of(&a, "bbb").is_some());
    }

    #[test]
    fn pin_beats_higher_priority_repo_either_order() {
        let d = fixture();
        // pin to alienbob wins vlc even though @slackware (100) is higher priority
        let (a, _) = collect(&d, &pats(&["alienbob:vlc", "@slackware"])).unwrap();
        assert_eq!(repo_of(&a, "vlc").unwrap(), "alienbob");
        // reversed: the pin still wins regardless of listing order
        let (b, _) = collect(&d, &pats(&["@slackware", "alienbob:vlc"])).unwrap();
        assert_eq!(repo_of(&b, "vlc").unwrap(), "alienbob");
    }

    #[test]
    fn two_pins_same_name_keep_first_listed() {
        let d = fixture();
        let (a, _) = collect(&d, &pats(&["slackware:vlc", "alienbob:vlc"])).unwrap();
        assert_eq!(repo_of(&a, "vlc").unwrap(), "slackware"); // first listed
        let (b, _) = collect(&d, &pats(&["alienbob:vlc", "slackware:vlc"])).unwrap();
        assert_eq!(repo_of(&b, "vlc").unwrap(), "alienbob"); // first listed
    }

    #[test]
    fn different_name_pins_both_kept() {
        let d = fixture();
        let (got, miss) = collect(&d, &pats(&["alienbob:bbb", "slackware:aaa"])).unwrap();
        assert!(miss.is_empty());
        assert_eq!(repo_of(&got, "bbb").unwrap(), "alienbob");
        assert_eq!(repo_of(&got, "aaa").unwrap(), "slackware");
    }

    #[test]
    fn misses_reported_for_non_matching_patterns() {
        let d = fixture();
        // a bare name that matches nothing, and a pin to a non-existent repo
        let (got, miss) = collect(&d, &pats(&["nonexistent", "zzz:vlc"])).unwrap();
        assert!(got.is_empty());
        assert!(miss.contains(&"nonexistent".to_string()));
        assert!(miss.contains(&"zzz:vlc".to_string()));
    }

    // #5: read-only alternatives note. The plan's choice is untouched; we only
    // surface that another repo also offers the same name.
    #[test]
    fn alternatives_surface_other_repo_for_collided_name() {
        let db = fixture();
        let plan = vec![PlanItem {
            pkg: av("vlc-3.0.20-x86_64-1", "slackware"),
            action: InstallAction::Install,
            dep_for: None,
            from: None,
        }];
        let alts = plan_alternatives(&db, &plan);
        assert_eq!(alts.len(), 1);
        let (name, chosen, others) = &alts[0];
        assert_eq!(name, "vlc");
        assert!(chosen.contains("slackware"));
        assert_eq!(others.len(), 1);
        assert!(others[0].contains("alienbob"));
    }

    #[test]
    fn no_alternatives_when_single_repo_offers_name() {
        let db = fixture();
        let plan = vec![PlanItem {
            pkg: av("aaa-1.0-x86_64-1", "slackware"),
            action: InstallAction::Install,
            dep_for: None,
            from: None,
        }];
        assert!(plan_alternatives(&db, &plan).is_empty());
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
}

#[cfg(test)]
mod freshness_tests {
    use super::*;

    #[test]
    fn parses_canonical_header() {
        let line = "PACKAGES.TXT;  Wed Jun 24 22:11:34 UTC 2026";
        assert_eq!(
            parse_packages_date(line),
            Some(crate::history::to_naive_epoch((2026, 6, 24, 22, 11, 34)))
        );
    }

    #[test]
    fn parses_space_padded_day() {
        // date's %e left-pads a single-digit day with a space -> double space.
        let line = "PACKAGES.TXT;  Tue Jun  4 09:05:06 UTC 2026";
        assert_eq!(
            parse_packages_date(line),
            Some(crate::history::to_naive_epoch((2026, 6, 4, 9, 5, 6)))
        );
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(parse_packages_date("").is_none());
        assert!(parse_packages_date("no semicolon at all").is_none());
        // wrong timezone
        assert!(parse_packages_date("PACKAGES.TXT;  Wed Jun 24 22:11:34 PST 2026").is_none());
        // unknown month
        assert!(parse_packages_date("PACKAGES.TXT;  Wed Xxx 24 22:11:34 UTC 2026").is_none());
        // missing seconds
        assert!(parse_packages_date("PACKAGES.TXT;  Wed Jun 24 22:11 UTC 2026").is_none());
        // non-numeric day
        assert!(parse_packages_date("PACKAGES.TXT;  Wed Jun XX 22:11:34 UTC 2026").is_none());
    }

    #[test]
    fn staleness_threshold_is_48h() {
        let base = crate::history::to_naive_epoch((2026, 6, 24, 0, 0, 0));
        assert!(!mirror_is_stale(base + 48 * 3600, base)); // exactly 48h -> fresh
        assert!(mirror_is_stale(base + 48 * 3600 + 1, base)); // 48h + 1s -> stale
        assert!(!mirror_is_stale(base, base)); // identical -> fresh
        assert!(!mirror_is_stale(base, base + 10_000)); // mirror ahead -> fresh
    }

    #[test]
    fn humanize_lag_is_readable() {
        assert_eq!(humanize_lag(9 * 86400), "9d");
        assert_eq!(humanize_lag(3 * 86400 + 5 * 3600), "3d 5h");
        assert_eq!(humanize_lag(5 * 3600), "5h");
        assert_eq!(humanize_lag(40 * 60), "40m");
    }

    #[test]
    fn slackware_dir_release_and_arch() {
        // -current is the codename, NOT a version suffix; a real -current
        // reports VERSION_ID=15.0 with no '+'.
        assert_eq!(
            slackware_dir_parts("x86_64", Some("current"), Some("15.0")).as_deref(),
            Some("slackware64-current")
        );
        // stable VM with a spurious '+' in a hand-made os-release: strip it.
        assert_eq!(
            slackware_dir_parts("x86_64", Some("stable"), Some("15.0+")).as_deref(),
            Some("slackware64-15.0")
        );
        // proper stable.
        assert_eq!(
            slackware_dir_parts("x86_64", Some("stable"), Some("15.0")).as_deref(),
            Some("slackware64-15.0")
        );
        // 32-bit -> slackware (no 64).
        assert_eq!(
            slackware_dir_parts("i586", Some("stable"), Some("15.0")).as_deref(),
            Some("slackware-15.0")
        );
        // quoted VERSION_ID is unwrapped.
        assert_eq!(
            slackware_dir_parts("x86_64", None, Some("\"15.0\"")).as_deref(),
            Some("slackware64-15.0")
        );
        // not current and no usable VERSION_ID -> None (caller fails open).
        assert_eq!(slackware_dir_parts("x86_64", None, None), None);
        assert_eq!(slackware_dir_parts("x86_64", None, Some("")), None);
        assert_eq!(slackware_dir_parts("x86_64", None, Some("+")), None);
    }

    #[test]
    fn upstream_url_uses_release_dir() {
        let dir = slackware_dir_parts("x86_64", None, Some("15.0")).unwrap();
        assert_eq!(
            format!("http://ftp.osuosl.org/pub/slackware/{dir}/PACKAGES.TXT"),
            "http://ftp.osuosl.org/pub/slackware/slackware64-15.0/PACKAGES.TXT"
        );
    }

    #[test]
    fn freshness_url_is_patches_on_stable_root_on_current() {
        // -current: the main-tree root moves, so read the release root.
        assert_eq!(
            osuosl_freshness_url("slackware64-current", true),
            "http://ftp.osuosl.org/pub/slackware/slackware64-current/PACKAGES.TXT"
        );
        // stable: the root is frozen, so read patches/ (where updates land).
        assert_eq!(
            osuosl_freshness_url("slackware64-15.0", false),
            "http://ftp.osuosl.org/pub/slackware/slackware64-15.0/patches/PACKAGES.TXT"
        );
    }

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

    #[test]
    fn repo_release_token_detects_official_and_thirdparty() {
        use crate::dist::Release;
        // official / conraid: the slackware{arch}-<suffix> form.
        assert_eq!(
            repo_release_token("https://mirror/slackware64-current/"),
            Some(Release::Current)
        );
        assert_eq!(
            repo_release_token("https://slackers.it/repository/slackware64-15.0"),
            Some(Release::Stable("15.0".into()))
        );
        // alienbob: a bare `current` or `15.0` path segment.
        assert_eq!(
            repo_release_token("https://slackware.nl/people/alien/sbrepos/current/x86_64"),
            Some(Release::Current)
        );
        assert_eq!(
            repo_release_token("https://slackware.nl/people/alien/sbrepos/15.0/x86_64"),
            Some(Release::Stable("15.0".into()))
        );
        // release-agnostic / no token → None (never flagged).
        assert_eq!(repo_release_token("https://slackbuilds.org/repository"), None);
        // a package-version-looking segment (3 dotted parts) is NOT a release.
        assert_eq!(repo_release_token("https://x/pkgs/3.0.23/here"), None);
    }

    #[test]
    fn release_version_segment_is_x_dot_y_only() {
        assert!(is_release_version_segment("15.0"));
        assert!(is_release_version_segment("14.2"));
        assert!(is_release_version_segment("16.0"));
        assert!(!is_release_version_segment("15")); // bare integer
        assert!(!is_release_version_segment("3.0.23")); // package version
        assert!(!is_release_version_segment("current"));
        assert!(!is_release_version_segment("x86_64"));
        assert!(!is_release_version_segment(""));
    }
}

