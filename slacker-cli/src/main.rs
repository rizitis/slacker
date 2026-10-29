//! slacker — a minimal Slackware package manager with full slackpkg parity,
//! combined with slackpkg+ multi-repo priority resolution.

mod banner;
mod banner2;
mod changelog;
mod cli;
mod commands;
mod stockdb;
mod config;
mod credentials;
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

use clap::Parser;
use cli::{Cli, Cmd};
use commands::blacklist::{cmd_frozen, cmd_pin, cmd_unfrozen, cmd_unpin};
use commands::clean::{cmd_clean_cache, cmd_clean_system};
use commands::find_mirror::cmd_find_mirror;
use commands::install::{
    cmd_download, cmd_install, cmd_install_new, cmd_reinstall, cmd_remove, cmd_upgrade, cmd_upgrade_all,
};
use commands::logs::{cmd_history, cmd_show_changelog};
use commands::new_config::cmd_new_config;
use commands::query::{cmd_file_search, cmd_info, cmd_search};
use commands::repos::{
    cmd_add_repo, cmd_add_tag, cmd_del_repo, cmd_del_tag, cmd_distrust_repo, cmd_pri_repo,
    cmd_trust_repo, cmd_vet_repo,
};
use commands::revert_pkg::cmd_revert_pkg;
use commands::status::{cmd_check_updates, cmd_list_repos, cmd_status};
use commands::templates::{
    cmd_delete_template, cmd_generate_template, cmd_install_template, cmd_remove_template,
};
use commands::update::cmd_update;
use commands::upgrade_dist::cmd_upgrade_dist;
use config::Config;
use pkgdb::PkgDb;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

/// slackpkg-compatible exit statuses.
enum Outcome {
    Ok,            // 0
    NothingFound,  // 20
    SelfUpgrade,   // 50
    Pending,       // 100
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
    // new-config reconciles only the *.new files under /etc; it reads no repos
    // and no mirror. It must therefore run even when the mirror config is broken
    // — which is exactly the situation right after you replace the `mirrors`
    // file THROUGH new-config and then want to run new-config again. Blocking it
    // on the strict config load would lock you out of the one tool that fixes the
    // mirror. It still writes under /etc, so it keeps the root check and the
    // exclusive lock; it only skips the config load it never needed.
    if matches!(cli.command, Cmd::NewConfig) {
        ensure_privileged(&cli.command)?;
        let _lock = acquire_lock()?;
        return cmd_new_config(cli, false);
    }
    let cfg = Config::load_dir(&cli.config_dir)?;
    migrate_state(&cfg);
    // Install repo credentials (HTTP Basic) before any download. Secrets come
    // from credentials.d/<name> and credentials.cat, never the repos file.
    download::set_auth(credentials::build_registry(&cfg));
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
        Cmd::Update { mode } => cmd_update(cli, &cfg, mode.as_deref()),
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
        Cmd::NewConfig => unreachable!("new-config is dispatched before config load"),
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
        Cmd::Frozen { names } => cmd_frozen(cli, &cfg, names),
        Cmd::Unfrozen { names } => cmd_unfrozen(cli, &cfg, names),
        Cmd::Pin { spec } => cmd_pin(cli, &cfg, spec.as_deref()),
        Cmd::Unpin { names } => cmd_unpin(cli, &cfg, names),
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
        // Deliberately NOT truncating at open: if another instance holds the
        // flock, its PID must survive in the file so we can report it. We
        // truncate manually (set_len) only after the lock is acquired.
        .truncate(false)
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
        format!("run it as root: slacker {name} ...")
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
        format!(
            "{} ({})",
            installed_source_label(&p.installed),
            priority_display(db.installed_priority(&p.installed, tag_prios))
        )
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
/// Quarantine a repo whose dependency graph blew past the depth cap, and tell the
/// user what happened and how to undo it. On a dry run nothing is written, only a
/// warning. Best-effort: a failed write just means the warning stands for this run.
fn quarantine_deep_repo(cfg: &Config, repo_name: &str, reason: &str, dry_run: bool) {
    if dry_run {
        eprintln!("{}", ui::yellow(&format!(
            "repo '{repo_name}' declares an abnormally deep dependency chain — would quarantine it (dry-run)."
        )));
        eprintln!("  {}", ui::dim(reason));
        return;
    }
    if let Some(r) = cfg.repo_by_name(repo_name) {
        let _ = repo::quarantine(r, &cfg.cache_dir, &cfg.state_dir, repo::QuarantineKind::Hard, reason);
    }
    eprintln!("{}", ui::red(&format!(
        "repo '{repo_name}' declares an abnormally deep dependency chain — quarantined."
    )));
    eprintln!("  {}", ui::yellow(reason));
    eprintln!("  {}", ui::red("While quarantined it provides NO packages."));
    eprintln!("  {}", ui::dim(&format!(
        "Investigate it, then `slacker trust-repo {repo_name}` to lift, or `del-repo {repo_name}` to remove."
    )));
}

fn expand_with_deps(
    cfg: &Config,
    db: &PkgDb,
    installed: &[pkg::PkgId],
    roots: Vec<(repo::AvailPkg, InstallAction)>,
    resolve: bool,
    assume_yes: bool,
    dry_run: bool,
) -> Result<Vec<PlanItem>, String> {
    if cfg.resolve_stock && !cfg.stock_db_path().exists() {
        eprintln!(
            "  {} {}",
            ui::yellow("!"),
            ui::yellow("stock-db not found \u{2014} resolve-stock is on but has no database.")
        );
        eprintln!("    run `slacker update` first. continuing without stock deps.");
    }
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
    let mut quarantined_now: HashSet<String> = HashSet::new();
    for (pkg, action) in roots {
        let repo_name = pkg.repo.clone();
        // a repo quarantined earlier in this run provides nothing more.
        if quarantined_now.contains(&repo_name) {
            continue;
        }
        let protected_before = protected.len();
        match add_with_deps(
            cfg, db, installed, pkg, action, None, resolve, assume_yes, &root_names,
            &mut plan, &mut planned, &mut visiting, &mut skip_all, &mut protected,
        ) {
            Ok(()) => {}
            // An abnormally deep chain means a hostile or broken repo, not a reason
            // to abort the whole run: quarantine that repo and carry on with the rest.
            Err(e) if e.starts_with(DEEP_CHAIN_ERR) => {
                visiting.clear(); // the aborted descent left its path behind
                protected.truncate(protected_before);
                quarantine_deep_repo(cfg, &repo_name, &e, dry_run);
                quarantined_now.insert(repo_name);
            }
            Err(e) => return Err(e),
        }
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

const MAX_DEP_DEPTH: usize = 512;
const DEEP_CHAIN_ERR: &str = "dependency chain too deep";

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
    // cap the chain depth so a hostile repo can't recurse us into a stack overflow
    if visiting.len() > MAX_DEP_DEPTH {
        visiting.remove(&name);
        return Err(format!("{DEEP_CHAIN_ERR} (>{MAX_DEP_DEPTH}) at '{name}' — refusing"));
    }

    if resolve {
        if let Some(repo) = cfg.repo_by_name(&pkg.repo) {
            let mut deps = repo::fetch_dep(repo, &pkg);
            // resolve-stock: in a container/minimal system, also pull the
            // stock deps this OFFICIAL package needs. The stock-db's `closure`
            // table is PRECOMPUTED and COMPLETE (level-1 + library-loader
            // chains), so it is consulted ONLY for the package the user asked
            // for (dep_for is None). Deps added from it must NOT consult it
            // again: each dep's own closure would re-add that package's
            // tool-only deps (e.g. sasl plugins -> mariadb) and cascade into
            // pulling half the distribution. Off by default -> no change.
            if cfg.resolve_stock && repo.official && dep_for.is_none() {
                for e in stockdb::deps_for(&cfg.stock_db_path(), &name) {
                    if !deps.contains(&e) {
                        deps.push(e);
                    }
                }
            }
            for dep in deps {
                // A blacklisted dependency is never pulled in — whether it would
                // come fresh from this repo, or is already installed/frozen.
                let bl = db
                    .resolve(&format!("{}:{}", pkg.repo, dep))
                    .is_some_and(|o| o.frozen)
                    || system::installed_by_name(installed, &dep)
                        .is_some_and(|i| bl_installed(cfg, Some(db), i));
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

/// The system's foundational shared libraries: a major-version bump of one of
/// these is best applied outside a running graphical session. Matched by exact
/// name plus any `aaa*` package (aaa_base, aaa_glibc-solibs, aaa_libraries, ...).
const FOUNDATIONAL_LIBS: &[&str] = &[
    "glibc",
    "aaa_glibc-solibs",
    "glibc-i18n",
    "icu4c",
    "openssl",
    "openssl-solibs",
    "lmdb",
    "ncurses",
    "readline",
    "zlib",
    "libxml2",
];

fn is_foundational(name: &str) -> bool {
    name.starts_with("aaa") || FOUNDATIONAL_LIBS.contains(&name)
}

/// The kernel's table of unix sockets, and the flag it sets on the ones a
/// process is listening on (SO_ACCEPTCON).
const PROC_NET_UNIX: &str = "/proc/net/unix";
const SO_ACCEPTCON: u32 = 0x1_0000;

/// Whether a display server is running on this machine.
///
/// NOT "does this process have DISPLAY set". That is whatever the caller
/// happened to hand us, and the ways slacker is normally started to change the
/// system take it away: pkexec, which slacker-gui uses, resets the environment
/// to a minimal safe set on purpose and drops DISPLAY and XAUTHORITY; `su -`
/// drops everything; sudo keeps DISPLAY but not WAYLAND_DISPLAY. Asked that
/// way, slacker told people upgrading from the GUI, in a window in front of
/// them, that they were on a console.
///
/// So ask the kernel instead. A display server is a process listening on a
/// well-known socket: X11 on `/tmp/.X11-unix/X<n>`, Wayland on
/// `<runtime dir>/wayland-<n>`. Those come from the protocols themselves, not
/// from a list of compositor names, so nothing here goes stale when a new
/// compositor appears. `/proc/net/unix` names every bound socket and flags the
/// ones something is listening on, which also tells a live server apart from
/// the socket file a crashed one leaves behind. Reading a file changes nothing
/// and cannot block, unlike connecting to the socket to see who answers.
///
/// The environment is still asked first: it is right whenever slacker does
/// inherit the session, and it can only make this answer MORE cautious, never
/// less.
fn in_graphical_session() -> bool {
    if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return true;
    }
    std::fs::read_to_string(PROC_NET_UNIX)
        .map(|table| any_display_server_listening(&table))
        .unwrap_or(false)
}

/// Looks through a `/proc/net/unix` table for a display server's socket. Kept
/// apart from reading the file so it can be tested against captured tables.
fn any_display_server_listening(table: &str) -> bool {
    table.lines().any(|line| {
        // Num RefCount Protocol Flags Type St Inode Path
        let f: Vec<&str> = line.split_whitespace().collect();
        let (Some(flags), Some(path)) = (f.get(3), f.get(7)) else {
            return false;
        };
        let listening =
            u32::from_str_radix(flags, 16).is_ok_and(|flags| flags & SO_ACCEPTCON != 0);
        listening && is_display_socket(path)
    })
}

/// True for the socket an X server or a Wayland compositor listens on. A
/// leading `@` is ignored: an X server listens on both the filesystem socket
/// and the abstract one, and either proves it is there. The runtime directory
/// is not pinned to /run/user, because a system without elogind may put it
/// elsewhere; the socket's own name is what identifies it.
fn is_display_socket(path: &str) -> bool {
    let path = path.strip_prefix('@').unwrap_or(path);
    let numbered = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(display) = path.strip_prefix("/tmp/.X11-unix/X") {
        return numbered(display);
    }
    path.rsplit('/')
        .next()
        .and_then(|name| name.strip_prefix("wayland-"))
        .is_some_and(numbered)
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
///
/// Slackware ARM names its kernel `kernel_<platform>` (e.g. `kernel_armv8` on
/// aarch64) — an underscore, not a dash, and no separate `-generic`/`-huge`
/// flavour — so that prefix is matched too. No x86 package starts with
/// `kernel_` (kernel-headers, kernel-source and kernel-firmware all use a dash),
/// so this adds nothing on x86.
fn is_kernel_pkg(name: &str) -> bool {
    name.starts_with("kernel-generic")
        || name.starts_with("kernel-huge")
        || name == "kernel-modules"
        || name.starts_with("kernel-modules-")
        || name.starts_with("kernel_")
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
    let x86 = plan
        .iter()
        .filter(|it| is_kernel_pkg(&it.pkg.id.name))
        .any(|it| is_x86_arch(&it.pkg.id.arch));
    println!("  {}", ui::dim(kernel_bootloader_hint(x86)));
}

/// The hint line under the kernel reminder. LILO, ELILO and GRUB are x86
/// bootloaders; they do not exist on Slackware ARM, whose kernel package
/// (kernel_armv8) runs os-initrd-mgr from its own install script, so there only
/// the closing sentence is shown.
fn kernel_bootloader_hint(x86: bool) -> &'static str {
    if x86 {
        "LILO: run `lilo`  |  ELILO/UEFI: `eliloconfig`  |  GRUB: `update-grub`  \
         |  This message is a reminder; generally speaking, you should not need to act on it."
    } else {
        "This message is a reminder; generally speaking, you should not need to act on it."
    }
}

/// The version out of a `version-arch-build` triple, the shape `PlanItem::from`
/// holds. Split from the RIGHT: a Slackware version never contains a dash, but
/// reading from the left would still break on the day one does.
fn version_of_triple(triple: &str) -> Option<&str> {
    let mut parts = triple.rsplitn(3, '-');
    let _build = parts.next()?;
    let _arch = parts.next()?;
    parts.next().filter(|v| !v.is_empty())
}

/// `(old, new)` when a plan moves `perl` to a DIFFERENT version.
///
/// Perl looks for modules in directories named after its own version
/// (`/usr/lib64/perl5/site_perl/5.40.2/…`), so a version change leaves
/// everything built against the old one off the path. A rebuild of the same
/// version (a build bump) moves no directory and needs no rebuilds, so it is
/// not reported: a reminder that fires when nothing is wrong teaches people to
/// ignore reminders.
fn perl_upgrade_versions(plan: &[PlanItem]) -> Option<(String, String)> {
    let it = plan.iter().find(|it| it.pkg.id.name == "perl")?;
    let old = version_of_triple(it.from.as_deref()?)?;
    let new = it.pkg.id.version.as_str();
    (old != new).then(|| (old.to_string(), new.to_string()))
}

/// True for a line of a package's file list that names a file inside perl's
/// directory for `version`: the core, vendor and site trees all carry the
/// version as a path component under `perl5/`.
fn is_perl_version_file(line: &str, version: &str) -> bool {
    line.contains("perl5/") && line.contains(&format!("/{version}/"))
}

/// Installed packages that still own files under perl's OLD version directory
/// — that is, exactly the ones this perl will no longer find. Returns at most
/// `limit` names plus the total, so the caller can say "and N more".
///
/// The file lists in /var/log/packages are the only source needed: no network,
/// no repo metadata, and it works for packages built by hand as well as for
/// anything installed from a repo.
fn packages_with_perl_version_files(
    pkg_db_dir: &std::path::Path,
    version: &str,
    limit: usize,
) -> (Vec<String>, usize) {
    use std::io::BufRead;
    let mut names: Vec<String> = Vec::new();
    let mut total = 0usize;
    let Ok(rd) = std::fs::read_dir(pkg_db_dir) else {
        return (names, total);
    };
    for ent in rd.flatten() {
        if !ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Ok(file) = std::fs::File::open(ent.path()) else {
            continue;
        };
        let hit = std::io::BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .any(|l| is_perl_version_file(&l, version));
        if hit {
            total += 1;
            if names.len() < limit {
                names.push(ent.file_name().to_string_lossy().into_owned());
            }
        }
    }
    names.sort();
    (names, total)
}

/// How many affected packages the reminder names before summarising the rest.
const PERL_REBUILD_SHOW: usize = 15;

/// After a plan that moved perl to a new version, say which installed packages
/// were left behind in the old version's directory.
///
/// Slackware upgrades its own perl modules in the same run, so what remains is
/// almost always what the admin built: SlackBuilds, CPAN, anything local. No-op
/// when perl did not change version.
fn perl_rebuild_reminder(pkg_db_dir: &std::path::Path, plan: &[PlanItem]) {
    let Some((old, new)) = perl_upgrade_versions(plan) else {
        return;
    };
    println!();
    println!(
        "{}",
        ui::yellow(&format!("/// !!!! --- Perl {old} was upgraded to {new}. ---- !!!! ///"))
    );
    println!(
        "  {}",
        ui::white(
            "Perl only looks for modules in directories named after its own version, so anything \
             built against the old one is no longer on its path."
        )
    );
    let (names, total) = packages_with_perl_version_files(pkg_db_dir, &old, PERL_REBUILD_SHOW);
    if total == 0 {
        println!(
            "  {}",
            ui::dim("Nothing installed still has files under the old version, so there is nothing to rebuild.")
        );
        return;
    }
    println!(
        "  {}",
        ui::dim(&format!(
            "{total} installed package(s) still have files under perl {old}:"
        ))
    );
    for n in &names {
        println!("    {}", ui::white(n));
    }
    if total > names.len() {
        println!("    {}", ui::dim(&format!("... and {} more", total - names.len())));
    }
    println!(
        "  {}",
        ui::dim("Rebuild and reinstall the ones you use (their SlackBuild, sbopkg, cpan); slacker does not build packages.")
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

/// True for x86_64 and every 32-bit x86 token: the architectures the official
/// Slackware tree (and its mirror list) carries. Slackware ARM (aarch64, arm) is
/// a separate port with its own trees and mirrors.
fn is_x86_arch(a: &str) -> bool {
    a == "x86_64" || arch_family(a) == "x86"
}

/// Is a package built for `pkg_arch` safe to install on a `sys_arch` system?
/// `noarch` always is; so are `fw` (firmware) and `x86` (kernel-headers), which
/// ship with those arch fields on EVERY Slackware architecture — the same rule
/// the available-list filter applies (repo.rs), so whatever the picker offers,
/// this guard accepts. Otherwise the families must match. (Note: multilib does
/// NOT need an exception — compat32 packages carry an `x86_64` arch field, with
/// `compat32` only in the name/build, so they pass trivially on x86_64.)
fn arch_compatible(sys_arch: &str, pkg_arch: &str) -> bool {
    pkg_arch == "noarch"
        || pkg_arch == "fw"
        || pkg_arch == "x86"
        || arch_family(sys_arch) == arch_family(pkg_arch)
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
        perl_rebuild_reminder(&cfg.pkg_db_dir, plan);
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
    // Separate each package's pkgtools output with a blank line so a multi-package
    // run is readable (the installpkg blocks otherwise butt up against each other).
    let mut printed_any = false;
    for (i, it) in items.iter().enumerate() {
        if !ready[i] {
            continue;
        }
        if printed_any {
            println!();
        }
        printed_any = true;
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
    perl_rebuild_reminder(&cfg.pkg_db_dir, plan);
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
        if d <= 2 && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
            best = Some((d, c.to_string()));
        }
    }
    best.map(|(_, s)| s)
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

/// Suffix for "unknown repo" errors when the name was written as `@repo`.
///
/// `@name` marks a repo in the commands that take PACKAGES (install, upgrade,
/// remove, download), where a bare word would otherwise be a package name.
/// Commands that take only repo names have nothing to tell apart, so they want
/// the plain name — but the habit carries over. When the `@` is the only thing
/// wrong, say so instead of reporting the whole token as an unknown repo.
///
/// Returns an empty string in every other case, so the caller's message is
/// unchanged for a genuine typo.
fn at_prefix_hint(cfg: &Config, name: &str) -> String {
    match name.strip_prefix('@') {
        Some(rest) if cfg.repo_by_name(rest).is_some() => {
            format!(" — did you mean '{rest}'? (this command takes plain repo names, no '@')")
        }
        _ => String::new(),
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

/// Reject a pattern list that mixes a package selector with an `@repo` / `@tag`
/// selector.
///
/// `@repo` means "every package in that repo", so `install fontforge @conraid`
/// asks for fontforge AND all 1259 conraid packages. That is almost never what
/// was meant — `conraid:fontforge` is — and the numbered prompt that follows
/// defaults to "all" on a bare Enter, which makes the slip expensive. Each form
/// on its own stays fully supported: `@repo` alone still takes a whole repo, and
/// `repo:name` still takes one package from one repo.
///
/// A `repo:name` pin is NOT a bare selector: it already names its repo, cannot
/// expand, and so may appear alongside `@repo`.
fn reject_mixed_selectors(db: &PkgDb, patterns: &[String]) -> Result<(), String> {
    let ats: Vec<&str> = patterns.iter().filter(|p| p.starts_with('@')).map(String::as_str).collect();
    if ats.is_empty() {
        return Ok(());
    }
    let bare: Vec<&str> = patterns
        .iter()
        .filter(|p| !p.starts_with('@') && !p.contains(':'))
        .map(String::as_str)
        .collect();
    let Some(first) = bare.first() else {
        return Ok(());
    };

    let mut msg = format!(
        "cannot mix a package name with {}: '{}' selects that package AND every \
         package in {}",
        ats.join(" "),
        patterns.join(" "),
        if ats.len() == 1 { ats[0] } else { "those repos" },
    );
    // Point at the pin that was almost certainly meant, when the named repo
    // really does ship the package.
    let hint = ats.iter().find_map(|at| {
        let repo_ = at.trim_start_matches('@');
        db.is_repo(repo_)
            .then(|| db.candidates(first).iter().any(|c| c.repo == repo_).then(|| format!("{repo_}:{first}")))
            .flatten()
    });
    if let Some(pin) = hint {
        msg.push_str(&format!("\n  for that package from that repo: {pin}"));
    }
    msg.push_str(&format!("\n  for the whole repo, use it on its own: {}", ats.join(" ")));
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
    reject_mixed_selectors(db, patterns)?;
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

/// Render an installed package's source priority for display. An unknown build
/// tag — one with no priority line in `repos` and shipped by no configured repo
/// — resolves to `i32::MAX` so that nothing can auto-replace it. Printing that
/// raw number tells the user nothing, so name the situation instead; the caller
/// adds a hint line explaining how to resolve it.
fn priority_display(prio: i32) -> String {
    if prio == i32::MAX {
        "no priority set".to_string()
    } else {
        prio.to_string()
    }
}

// ---- blacklist helpers ---------------------------------------------------

// Candidate freezes are precomputed once on load (AvailPkg::frozen, set by
// PkgDb::mark_frozen) and read directly as `p.frozen`, so a frozen candidate is
// treated as absent during resolution. The only freeze check that still needs a
// helper is for INSTALLED packages (no AvailPkg to carry a flag), used by
// `remove` / `clean-system` / `remove-template` — operations on installed
// packages, where there is no candidate to fall through to.

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

    // 1. Transport: plaintext http is tamperable in flight — unless the user
    //    explicitly accepted that risk for this repo with the `insecure` flag
    //    (in which case freezing it for the very thing they opted into would be
    //    contradictory; GPG/checksum verification still applies below).
    if r.url.to_ascii_lowercase().starts_with("http://") && !r.insecure {
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
    // The repos file just changed (e.g. add-repo added this repo with a
    // credentials= flag). Rebuild the auth registry from the fresh config so
    // that vetting the new repo authenticates with its credentials, instead of
    // the stale registry built before the write.
    download::set_auth(credentials::build_registry(&fresh));
    fresh
        .repo_by_name(name)
        .cloned()
        .ok_or_else(|| format!("internal: repo '{name}' not found after writing config"))
}

/// The source label for an INSTALLED build: its build tag, or `official` for a
/// tagless (-current) build. Shared by `search` and the "kept (priority)"
/// detail lines so both name a source the same way.
fn installed_source_label(inst: &pkg::PkgId) -> &str {
    let tag = inst.build_tag();
    if tag.is_empty() {
        "official"
    } else {
        tag
    }
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
/// own version substituted.
///
/// x86 only: Slackware ARM names its trees `slackwareaarch64-*` and lives on its
/// own mirrors, which this does not model. A wrong path does NOT reliably fail
/// open (on aarch64 it would name the real 32-bit x86 tree), so callers that
/// reach an official reference tree check [`osuosl_tree_prefix`] first
/// (find-mirror, the status freshness row). upgrade-dist, the other caller,
/// never gets here on aarch64: Slackware ARM aarch64 exists only as -current,
/// and the route gate refuses -current before this is read.
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

/// Directory prefix of the official tree for `arch` on the osuosl reference
/// mirror (`slackware64` on x86_64, `slackware` on 32-bit x86), or None for an
/// architecture osuosl does not carry: Slackware ARM (aarch64, arm) lives on its
/// own mirrors. Callers skip the reference check on None instead of comparing
/// against another architecture's tree.
fn osuosl_tree_prefix(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("slackware64"),
        a if is_x86_arch(a) => Some("slackware"),
        _ => None,
    }
}

/// -current is marked by `VERSION_CODENAME=current` in /etc/os-release; that
/// codename is the reliable signal. A real -current reports VERSION_ID=15.0 with
/// no suffix, while stable releases carry a different codename (e.g. "stable").
pub(crate) fn is_current_codename(codename: Option<&str>) -> bool {
    codename
        .map(|c| c.trim().eq_ignore_ascii_case("current"))
        .unwrap_or(false)
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
mod arch_tests {
    use super::arch_compatible;

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
        // `x86` (kernel-headers) and `fw` (firmware) ship on EVERY Slackware
        // arch — the guard must accept them, exactly like the available-list
        // filter (repo.rs) does. Regression for the live refusal of
        // kernel-headers-6.18.38-x86-1 on an x86_64 upgrade-all.
        assert!(arch_compatible("x86_64", "x86"));
        assert!(arch_compatible("i586", "x86"));
        assert!(arch_compatible("x86_64", "fw"));
        assert!(arch_compatible("i686", "fw"));
        // Cross-family is refused, both directions.
        assert!(!arch_compatible("x86_64", "i686"));
        assert!(!arch_compatible("i586", "x86_64"));
        assert!(!arch_compatible("x86_64", "aarch64"));
        // multilib compat32 packages carry an x86_64 arch field -> fine on x86_64.
        // (the `compat32` token lives in the name/build, not the arch field)
        assert!(arch_compatible("x86_64", "x86_64"));
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
mod collect_tests {
    use super::*;
    use crate::commands::query::search_display;
    use crate::commands::install::{abort_on_misses, plan_alternatives, selector_miss_hint};
    use crate::commands::upgrade_dist::dist_upgrade_sets;

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
            frozen: false,
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
        assert_eq!(
            search_display(Some(&inst_alien), &cands).map(|c| c.repo.as_str()),
            Some("alienbob")
        );

        // installed from conraid (2cf) -> show conraid.
        let inst_cf = pkg::PkgId::parse("flatpak-1.18.0-x86_64-2cf").unwrap();
        assert_eq!(
            search_display(Some(&inst_cf), &cands).map(|c| c.repo.as_str()),
            Some("conraid")
        );

        // not installed -> nothing to match; the caller shows the winner.
        assert_eq!(search_display(None, &cands).map(|c| c.repo.as_str()), None);

        // installed from a source that is not a configured repo (_SBo) -> no
        // candidate. The caller must label the line from the installed build
        // tag, NOT misattribute the package to the priority winner.
        let inst_sbo = pkg::PkgId::parse("flatpak-1.18.0-x86_64-1_SBo").unwrap();
        assert_eq!(search_display(Some(&inst_sbo), &cands).map(|c| c.repo.as_str()), None);
        assert_eq!(installed_source_label(&inst_sbo), "_SBo");

        // a tagless (-current) installed build is official Slackware.
        let inst_official = pkg::PkgId::parse("flatpak-1.18.0-x86_64-1").unwrap();
        assert_eq!(installed_source_label(&inst_official), "official");
    }

    #[test]
    fn dist_upgrade_sets_bypass_priority_and_order_critical() {
        // Target repos after the transform: slackware(100) and patches(200).
        let db = PkgDb::for_test(
            vec![
                av("aaa_glibc-solibs-2.41-x86_64-1", "slackware"),
                av("xz-5.6.2-x86_64-1", "slackware"),
                av("vlc-3.0.21-x86_64-1", "slackware"),
                av("kernel-generic-6.6.30-x86_64-1", "patches"),
                av("kernel-generic-6.6.10-x86_64-1", "slackware"),
            ],
            &[("slackware", 100), ("patches", 200)],
            Some(100),
        );
        // Installed: old core (aaa_glibc-solibs, xz), a vlc that came from a now-disabled
        // third-party repo (its build tag would outrank slackware), and an old kernel.
        let installed = vec![
            pkg::PkgId::parse("aaa_glibc-solibs-2.33-x86_64-1").unwrap(),
            pkg::PkgId::parse("xz-5.2.5-x86_64-1").unwrap(),
            pkg::PkgId::parse("vlc-3.0.18-x86_64-1alien").unwrap(),
            pkg::PkgId::parse("kernel-generic-6.6.5-x86_64-1").unwrap(),
        ];
        let (critical, rest) = dist_upgrade_sets(&db, &installed);

        // Critical set is ordered per DIST_CRITICAL: glibc-solibs before xz; the
        // absent ones (pkgtools/tar/gzip/findutils) are simply skipped.
        let cnames: Vec<&str> = critical.iter().map(|p| p.pkg.id.name.as_str()).collect();
        assert_eq!(cnames, vec!["aaa_glibc-solibs", "xz"]);

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
        // Slackware ARM: kernel_<platform>, underscore, no flavour suffix.
        assert!(is_kernel_pkg("kernel_armv8"));
    }

    #[test]
    fn perl_files_are_recognised_by_their_version_directory() {
        assert!(is_perl_version_file(
            "usr/lib64/perl5/site_perl/5.40.2/x86_64-linux-thread-multi/DBI.pm",
            "5.40.2"
        ));
        assert!(is_perl_version_file("usr/lib64/perl5/5.40.2/CORE/perl.h", "5.40.2"));
        assert!(is_perl_version_file(
            "usr/lib/perl5/vendor_perl/5.40.2/Try/Tiny.pm",
            "5.40.2"
        ));
        // The new version's files are not what we are looking for.
        assert!(!is_perl_version_file(
            "usr/lib64/perl5/site_perl/5.42.0/x86_64-linux-thread-multi/DBI.pm",
            "5.40.2"
        ));
        // A version-shaped directory outside perl is not perl's.
        assert!(!is_perl_version_file("usr/share/foo/5.40.2/data", "5.40.2"));
        assert!(!is_perl_version_file("usr/bin/perl", "5.40.2"));
    }

    #[test]
    fn the_version_comes_out_of_a_version_arch_build_triple() {
        assert_eq!(version_of_triple("5.40.2-x86_64-1"), Some("5.40.2"));
        assert_eq!(version_of_triple("1.2.3-x86_64-2_SBo"), Some("1.2.3"));
        // A dash inside the version survives, because the split is from the right.
        assert_eq!(version_of_triple("2.4-r1-x86_64-1"), Some("2.4-r1"));
        assert_eq!(version_of_triple("nonsense"), None);
        assert_eq!(version_of_triple("-x86_64-1"), None);
    }

    #[test]
    fn only_a_change_of_perl_version_is_reported() {
        let item = |nv: &str, from: Option<&str>| PlanItem {
            pkg: av(nv, "slackware"),
            action: InstallAction::Upgrade,
            dep_for: None,
            from: from.map(str::to_string),
        };
        // A new version: both sides are reported, old first.
        let plan = vec![item("perl-5.42.0-x86_64-1", Some("5.40.2-x86_64-1"))];
        assert_eq!(perl_upgrade_versions(&plan), Some(("5.40.2".into(), "5.42.0".into())));
        // A rebuild of the same version moves no directory: nothing to say.
        let plan = vec![item("perl-5.42.0-x86_64-2", Some("5.42.0-x86_64-1"))];
        assert_eq!(perl_upgrade_versions(&plan), None);
        // A first install has nothing to have been built against.
        let plan = vec![item("perl-5.42.0-x86_64-1", None)];
        assert_eq!(perl_upgrade_versions(&plan), None);
        // Another package's version change is not perl's.
        let plan = vec![item("python3-3.13.1-x86_64-1", Some("3.12.7-x86_64-1"))];
        assert_eq!(perl_upgrade_versions(&plan), None);
    }

    #[test]
    fn only_packages_left_in_the_old_perl_tree_are_named() {
        // A unique directory per process: package records are also written by
        // root during a build, and a fixed /tmp path would collide.
        let dir = std::env::temp_dir().join(format!("slacker_perl_db_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let record = |name: &str, files: &str| {
            std::fs::write(dir.join(name), format!("PACKAGE NAME: {name}\nFILE LIST:\n{files}"))
                .unwrap()
        };
        record("perl-DBI-1.645-x86_64-1_SBo", "usr/lib64/perl5/site_perl/5.40.2/DBI.pm\n");
        record("perl-Try-Tiny-0.32-x86_64-1_SBo", "usr/lib64/perl5/site_perl/5.40.2/Try/Tiny.pm\n");
        record("perl-5.42.0-x86_64-1", "usr/lib64/perl5/5.42.0/CORE/perl.h\n");
        record("bash-5.2.37-x86_64-1", "bin/bash\n");

        let (names, total) = packages_with_perl_version_files(&dir, "5.40.2", 15);
        assert_eq!(total, 2, "{names:?}");
        assert_eq!(
            names,
            vec![
                "perl-DBI-1.645-x86_64-1_SBo".to_string(),
                "perl-Try-Tiny-0.32-x86_64-1_SBo".to_string()
            ]
        );

        // The cap reports the total and returns only as many names as asked.
        let (few, total) = packages_with_perl_version_files(&dir, "5.40.2", 1);
        assert_eq!((few.len(), total), (1, 2));

        // A version nothing owns finds nothing at all.
        assert_eq!(packages_with_perl_version_files(&dir, "5.36.0", 15).1, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kernel_hint_names_x86_bootloaders_only_on_x86() {
        assert!(kernel_bootloader_hint(true).contains("lilo"));
        let arm = kernel_bootloader_hint(false);
        assert!(!arm.contains("lilo") && !arm.contains("grub") && !arm.contains("eliloconfig"));
        assert!(arm.contains("reminder"));
    }

    #[test]
    fn osuosl_reference_only_for_x86() {
        assert_eq!(osuosl_tree_prefix("x86_64"), Some("slackware64"));
        assert_eq!(osuosl_tree_prefix("i586"), Some("slackware"));
        assert_eq!(osuosl_tree_prefix("i686"), Some("slackware"));
        // Slackware ARM: no tree on osuosl, so no reference (never the 32-bit
        // x86 `slackware-*` tree, which is what the old fallback produced).
        assert_eq!(osuosl_tree_prefix("aarch64"), None);
        assert_eq!(osuosl_tree_prefix("arm"), None);
        assert!(is_x86_arch("x86_64") && is_x86_arch("i486"));
        assert!(!is_x86_arch("aarch64") && !is_x86_arch("noarch"));
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
    fn a_miss_that_names_a_repo_or_tag_says_so_instead_of_guessing_a_package() {
        let d = fixture();
        // A repo name typed as a package (a dropped ':' in `download fontforge
        // ponce`). Suggesting the closest PACKAGE name here is misleading, so
        // name what the word actually is and give both selector forms.
        let h = selector_miss_hint(&d, "slackware").expect("repo must be recognised");
        assert!(h.contains("is a repo"), "{h}");
        assert!(h.contains("slackware:<package>"), "{h}");
        assert!(h.contains("@slackware"), "{h}");

        // An ordinary typo is left to the existing closest-name help.
        assert!(selector_miss_hint(&d, "vlcc").is_none());
        // Already-punctuated selectors are handled elsewhere.
        assert!(selector_miss_hint(&d, "@slackware").is_none());
        assert!(selector_miss_hint(&d, "slackware:vlc").is_none());
    }

    #[test]
    fn any_miss_refuses_the_whole_command() {
        // No miss: nothing to guard against.
        assert!(abort_on_misses(&[]).is_ok());

        // One miss names it and says nothing was done, so a partial result can
        // never masquerade as a complete one.
        let e = abort_on_misses(&pats(&["ponce"])).unwrap_err();
        assert!(e.contains("nothing was done"), "{e}");
        assert!(e.contains("ponce"), "{e}");
        assert!(e.contains("1 argument matched no package"), "{e}");

        // Several misses are all named, in the order given.
        let e = abort_on_misses(&pats(&["ponce", "fontforg"])).unwrap_err();
        assert!(e.contains("2 arguments matched no package"), "{e}");
        assert!(e.contains("ponce, fontforg"), "{e}");

        // The guard lives at the command level, NOT inside collect: an
        // @repo-only expansion that matches nothing (an empty patches tree on
        // -current) is a miss, and the internal callers that ignore misses must
        // keep working. Prove collect still returns such a miss rather than
        // failing.
        let d = fixture();
        let (got, miss) = collect(&d, &pats(&["nonexistent"])).unwrap();
        assert!(got.is_empty());
        assert_eq!(miss, vec!["nonexistent".to_string()]);
    }

    #[test]
    fn bare_name_with_at_repo_is_refused_but_each_form_alone_still_works() {
        let d = fixture();
        // The hazard: `install vlc @slackware` reads as "vlc AND every package
        // in slackware", and the prompt that follows takes everything on a bare
        // Enter. Refuse it and name the pin that was meant.
        let e = collect(&d, &pats(&["vlc", "@slackware"])).unwrap_err();
        assert!(e.contains("cannot mix"), "{e}");
        assert!(e.contains("slackware:vlc"), "{e}");
        // order must not matter
        assert!(collect(&d, &pats(&["@slackware", "vlc"])).is_err());
        // Everything that was legal stays legal:
        assert!(collect(&d, &pats(&["@slackware"])).is_ok()); // whole repo
        assert!(collect(&d, &pats(&["@alienbob", "@slackware"])).is_ok()); // several repos
        assert!(collect(&d, &pats(&["slackware:vlc"])).is_ok()); // one package, one repo
        assert!(collect(&d, &pats(&["vlc", "aaa"])).is_ok()); // several names
        // A `repo:name` pin already names its repo and cannot expand, so it is
        // not a bare selector and may sit next to @repo.
        assert!(collect(&d, &pats(&["alienbob:vlc", "@slackware"])).is_ok());
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


#[cfg(test)]
mod foundational_tests {
    use super::*;

    #[test]
    fn foundational_matches_aaa_prefix_and_list() {
        assert!(is_foundational("aaa_base"));
        assert!(is_foundational("aaa_glibc-solibs"));
        assert!(is_foundational("icu4c"));
        assert!(is_foundational("openssl"));
        assert!(!is_foundational("vlc"));
        assert!(!is_foundational("firefox"));
        // Not a false-positive on a name that merely contains "aaa".
        assert!(!is_foundational("libraaa"));
    }

    /// Captured from a real /proc/net/unix, one socket per line:
    /// Num RefCount Protocol Flags Type St Inode Path
    #[test]
    fn a_running_display_server_is_seen_in_the_kernel_socket_table() {
        const HEAD: &str = "Num       RefCount Protocol Flags    Type St Inode Path\n";

        // An X server: it listens on the filesystem socket and on the
        // abstract one with the same name.
        let x11 = format!(
            "{HEAD}\
             0000000059ef6a19: 00000002 00000000 00010000 0001 01   478 /tmp/.X11-unix/X0\n\
             000000007144de2c: 00000002 00000000 00010000 0001 01   477 @/tmp/.X11-unix/X0\n"
        );
        assert!(any_display_server_listening(&x11));

        let wayland = format!(
            "{HEAD}\
             0000000012ab34cd: 00000002 00000000 00010000 0001 01  9120 /run/user/1000/wayland-0\n"
        );
        assert!(any_display_server_listening(&wayland));

        // A console: plenty of sockets, none of them a display server's. The
        // cursor-shared file and the lock sit right next to a real one and
        // must not be taken for it.
        let console = format!(
            "{HEAD}\
             0000000000aa11bb: 00000003 00000000 00010000 0001 01   120 /run/dbus/system_bus_socket\n\
             0000000000aa22bb: 00000002 00000000 00010000 0001 01   121 /run/user/1000/pipewire-0\n\
             0000000000aa33bb: 00000002 00000000 00010000 0001 01   122 /run/user/1000/wayland-cursor-shared-Ab12Cd\n\
             0000000000aa44bb: 00000002 00000000 00010000 0001 01   123 /run/user/1000/wayland-0.lock\n"
        );
        assert!(!any_display_server_listening(&console));

        // What a crashed server leaves behind: the path is still bound, but
        // nothing is listening on it (no SO_ACCEPTCON in the flags).
        let stale = format!(
            "{HEAD}\
             0000000000cc11dd: 00000002 00000000 00000000 0001 01   130 /tmp/.X11-unix/X0\n"
        );
        assert!(!any_display_server_listening(&stale));

        // The header alone, an empty file, and a short line are all "no".
        assert!(!any_display_server_listening(HEAD));
        assert!(!any_display_server_listening(""));
        assert!(!any_display_server_listening("garbage\n"));
    }

    #[test]
    fn display_sockets_are_told_apart_from_every_other_socket() {
        for path in [
            "/tmp/.X11-unix/X0",
            "/tmp/.X11-unix/X11",
            "@/tmp/.X11-unix/X0",
            "/run/user/1000/wayland-0",
            // A runtime directory somewhere else (no elogind) still counts.
            "/tmp/runtime-alice/wayland-1",
        ] {
            assert!(is_display_socket(path), "should be a display socket: {path}");
        }
        for path in [
            "/run/user/1000/wayland-0.lock",
            "/run/user/1000/wayland-cursor-shared-Ab12Cd",
            "/run/user/1000/pipewire-0",
            "/run/user/1000/bus",
            "/tmp/.X11-unix/Xorg",
            "/run/dbus/system_bus_socket",
            "",
        ] {
            assert!(!is_display_socket(path), "should not be a display socket: {path}");
        }
    }
}
