//! slacker — a minimal Slackware package manager with full slackpkg parity,
//! combined with slackpkg+ multi-repo priority resolution.

mod changelog;
mod config;
mod download;
mod gpg;
mod manifest;
mod newconfig;
mod pkg;
mod pkgdb;
mod repo;
mod system;
mod template;
mod ui;

use clap::{Parser, Subcommand};
use config::Config;
use pkgdb::PkgDb;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

/// slackpkg-compatible exit statuses.
enum Outcome {
    Ok,            // 0
    NothingFound,  // 20
    SelfUpgrade,   // 50
    Pending,       // 100
}

#[derive(Parser)]
#[command(name = "slacker", version, about = "slackpkg + slackpkg+ in one, minimal Rust tool")]
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
    /// Install new packages (refuses already-installed ones).
    Install { patterns: Vec<String> },
    /// Upgrade installed packages to the newest available revision.
    Upgrade { patterns: Vec<String> },
    /// Reinstall the currently installed version.
    Reinstall { patterns: Vec<String> },
    /// Remove installed packages.
    Remove { patterns: Vec<String> },
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
    /// Install packages newly added since the last update. By default only the
    /// official repo(s); name one or more repos to use those instead.
    InstallNew { repos: Vec<String> },
    /// Remove installed packages that exist in no configured repo.
    CleanSystem,
    /// Delete downloaded package files from the cache. Repo metadata and GPG
    /// keys are never touched. Optionally limit to named repos.
    CleanCache { repos: Vec<String> },
    /// Handle leftover *.new configuration files.
    NewConfig,
    /// Check every configured repo for pending updates (exit 100 if any).
    CheckUpdates,
    /// Print the cached ChangeLog.
    ShowChangelog,
    /// Snapshot installed packages into a template.
    GenerateTemplate { name: String },
    /// Install all packages listed in a template.
    InstallTemplate { name: String },
    /// Remove all packages listed in a template.
    RemoveTemplate { name: String },
    /// Delete a template file (does not touch installed packages).
    DeleteTemplate { name: String },
    /// Add one or more packages to the blacklist ("freeze" them).
    Frozen { names: Vec<String> },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(Outcome::Ok) => ExitCode::SUCCESS,
        Ok(Outcome::NothingFound) => ExitCode::from(20),
        Ok(Outcome::SelfUpgrade) => ExitCode::from(50),
        Ok(Outcome::Pending) => ExitCode::from(100),
        Err(e) => {
            eprintln!("slacker: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<Outcome, String> {
    let cfg = Config::load_dir(&cli.config_dir)?;
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
        Cmd::Install { patterns } => cmd_install(cli, &cfg, patterns),
        Cmd::Upgrade { patterns } => cmd_upgrade(cli, &cfg, patterns),
        Cmd::Reinstall { patterns } => cmd_reinstall(cli, &cfg, patterns),
        Cmd::Remove { patterns } => cmd_remove(cli, &cfg, patterns),
        Cmd::Download { patterns, output } => cmd_download(cli, &cfg, patterns, output.as_deref()),
        Cmd::UpgradeAll => cmd_upgrade_all(cli, &cfg),
        Cmd::InstallNew { repos } => cmd_install_new(cli, &cfg, repos),
        Cmd::CleanSystem => cmd_clean_system(cli, &cfg),
        Cmd::CleanCache { repos } => cmd_clean_cache(cli, &cfg, repos),
        Cmd::NewConfig => cmd_new_config(cli),
        Cmd::CheckUpdates => cmd_check_updates(&cfg),
        Cmd::ShowChangelog => cmd_show_changelog(&cfg),
        Cmd::GenerateTemplate { name } => cmd_generate_template(&cfg, name),
        Cmd::InstallTemplate { name } => cmd_install_template(cli, &cfg, name),
        Cmd::RemoveTemplate { name } => cmd_remove_template(cli, &cfg, name),
        Cmd::DeleteTemplate { name } => cmd_delete_template(cli, &cfg, name),
        Cmd::Frozen { names } => cmd_frozen(&cfg, names),
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
        | Cmd::FileSearch { .. }
        | Cmd::CheckUpdates
        | Cmd::ShowChangelog => false,
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
        Cmd::Install { .. } => "install",
        Cmd::Upgrade { .. } => "upgrade",
        Cmd::Reinstall { .. } => "reinstall",
        Cmd::Remove { .. } => "remove",
        Cmd::Download { .. } => "download",
        Cmd::UpgradeAll => "upgrade-all",
        Cmd::InstallNew { .. } => "install-new",
        Cmd::CleanSystem => "clean-system",
        Cmd::CleanCache { .. } => "clean-cache",
        Cmd::NewConfig => "new-config",
        Cmd::CheckUpdates => "check-updates",
        Cmd::ShowChangelog => "show-changelog",
        Cmd::GenerateTemplate { .. } => "generate-template",
        Cmd::InstallTemplate { .. } => "install-template",
        Cmd::RemoveTemplate { .. } => "remove-template",
        Cmd::DeleteTemplate { .. } => "delete-template",
        Cmd::Frozen { .. } => "frozen",
    }
}

fn confirm(prompt: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true;
    }
    print!("{} ", ui::blue(&format!("{prompt} [y/N/a]")));
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    match line.trim() {
        "y" | "Y" | "yes" => true,
        "a" | "A" | "abort" => {
            println!("{}", ui::blue("aborted — nothing changed"));
            false
        }
        // anything else (including the default empty Enter and 'n') cancels
        _ => false,
    }
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
}

enum DepChoice {
    Skip,
    Replace,
    SkipAll,
    Abort,
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
    println!("    {}", ui::blue("[s]kip      keep the installed version (default)"));
    println!(
        "    {}",
        ui::blue(&format!("[r]eplace   install the {}'s version instead", offered.repo))
    );
    println!("    {}", ui::blue("skip-[a]ll  keep installed for this and all later conflicts"));
    println!("    {}", ui::blue("a[b]ort     cancel the whole operation, change nothing more"));
    print!("  {} ", ui::blue("Choice [s/r/a/b]:"));
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return DepChoice::Skip;
    }
    match line.trim() {
        "r" | "R" => DepChoice::Replace,
        "a" | "A" => DepChoice::SkipAll,
        "b" | "B" => DepChoice::Abort,
        _ => DepChoice::Skip,
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
    // Names already scheduled as roots (e.g. every package upgrade-all will
    // upgrade). A dependency whose name is here will be satisfied by its own
    // root entry, so we must not prompt about it as a "conflict".
    let root_names: HashSet<String> =
        roots.iter().map(|(p, _)| p.id.name.clone()).collect();
    for (pkg, action) in roots {
        add_with_deps(
            cfg, db, installed, pkg, action, None, resolve, assume_yes, &root_names,
            &mut plan, &mut planned, &mut visiting, &mut skip_all,
        )?;
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
                if cfg.is_blacklisted(&dep) {
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
                            // it is protected: keep it, do not prompt or replace.
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
    if planned.insert(name) {
        plan.push(PlanItem { pkg, action, dep_for });
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
    PlanRow {
        action,
        color,
        name: it.pkg.id.name.clone(),
        version: format!("{}-{}-{}", it.pkg.id.version, it.pkg.id.arch, it.pkg.id.build),
        repo: it.pkg.repo.clone(),
        note: it.dep_for.as_ref().map(|p| format!("for {p}")).unwrap_or_default(),
    }
}

/// Print a resolved plan as a coloured table. `frozen` are blacklisted names
/// left untouched (purple); `protected` are names kept because an installed
/// source of higher-or-equal priority already owns them (blue). Version is
/// never compared — only source priority decides.
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

/// Print just the action part of a plan (no skip categories). Used by commands
/// that don't compute frozen/priority skips themselves.
fn print_plan(plan: &[PlanItem]) {
    show_plan(plan, &[], &[]);
}

/// Download, verify and install/upgrade/reinstall every item in a plan.
fn execute_plan(cfg: &Config, plan: &[PlanItem]) -> Result<(), String> {
    for it in plan {
        let r = cfg.repo_by_name(&it.pkg.repo).ok_or("internal repo lookup failed")?;
        let dest = system::cached_pkg_path(&cfg.cache_dir, &it.pkg.repo, &it.pkg.filename);
        fetch_and_verify(cfg, r, &it.pkg, &dest)?;
        match it.action {
            InstallAction::Install => system::install(&dest)?,
            InstallAction::Upgrade => system::upgrade_only(&dest)?,
            InstallAction::Reinstall => system::reinstall(&dest)?,
        }
    }
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
) -> Result<(), String> {
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
        println!("  fetching {url}");
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
        match gpg::verify_detached(repo, &cfg.cache_dir, dest, &asc) {
            Ok(gpg::Verify::Good(signer)) => checks.push(format!("gpg ({signer})")),
            Ok(gpg::Verify::NoSignature) => {
                if policy.requires(config::Check::Gpg) {
                    return Err(verify_unavailable_error(
                        &p.repo,
                        config::Check::Gpg,
                        &cfg.config_dir,
                    ));
                }
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

    if checks.is_empty() {
        println!("  {}", ui::dim("(verification is disabled for this repo)"));
    } else {
        println!("  {}", ui::green(&format!("verified: {}", checks.join(" + "))));
    }
    Ok(())
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
    let mut seen = HashSet::new();
    let mut pkgs = Vec::new();
    let mut misses = Vec::new();
    for pat in patterns {
        let matched = db.match_pattern(pat);
        if matched.is_empty() {
            misses.push(pat.clone());
        }
        for p in matched {
            if seen.insert(p.id.name.clone()) {
                pkgs.push(p);
            }
        }
    }
    Ok((pkgs, misses))
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
) -> Result<(Vec<&'a repo::AvailPkg>, Vec<String>), String> {
    for pat in patterns {
        validate_selector(db, pat)?;
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut protected = Vec::new(); // names kept because their source has priority
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
            for p in db.match_pattern(pat) {
                let Some(inst) = installed.iter().find(|i| i.name == p.id.name) else {
                    continue;
                };
                if !seen.insert(p.id.name.clone()) {
                    continue;
                }
                if !pinned && !db.upgrade_respects_priority(inst, p, tag_prios) {
                    protected.push(kept_detail(db, inst, p, tag_prios));
                    continue;
                }
                out.push(p);
            }
        }
    }
    Ok((out, protected))
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

// ---- commands ------------------------------------------------------------

/// Warn about active repos whose effective verify policy performs NO checks at
/// all — either global `VERIFY=none` with no per-repo override, or an explicit
/// `verify=none` on the repo line. Shown after `update` and in `check-updates`.
fn warn_unverified_repos(cfg: &Config) {
    let bare: Vec<&str> = cfg
        .repos
        .iter()
        .filter(|r| {
            let p = r.verify_policy(&cfg.verify);
            !p.wants(config::Check::Gpg)
                && !p.wants(config::Check::Md5)
                && !p.wants(config::Check::Sha)
        })
        .map(|r| r.name.as_str())
        .collect();
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
fn update_one_repo(
    cfg: &Config,
    r: &config::Repo,
    track_changelog: bool,
    failed: &mut Vec<String>,
) {
    println!("Updating '{}' (priority {}):", r.name, r.priority);
    if let Err(e) = repo::update_repo(r, &cfg.cache_dir, track_changelog) {
        println!("  FAILED: {e}");
        return;
    }
    let policy = r.verify_policy(&cfg.verify);
    if policy.wants(config::Check::Gpg) {
        match gpg::verify_checksums(r, &cfg.cache_dir) {
            Ok(gpg::Verify::Good(signer)) => println!("  GPG: good signature ({signer})"),
            Ok(gpg::Verify::NoSignature) => {
                if policy.requires(config::Check::Gpg) {
                    println!(
                        "{}",
                        ui::red("  GPG: required signature is missing — this repo will NOT be used.")
                    );
                    repo::invalidate_metadata(r, &cfg.cache_dir);
                    failed.push(r.name.clone());
                } else {
                    println!("  GPG: no signature provided (skipped)");
                }
            }
            Err(e) => {
                println!("{}", ui::red(&format!("  GPG: {e}")));
                println!(
                    "{}",
                    ui::red("  this repo's metadata was discarded and will NOT be used.")
                );
                repo::invalidate_metadata(r, &cfg.cache_dir);
                failed.push(r.name.clone());
            }
        }
    } else {
        println!("  GPG: skipped (verify policy)");
    }
}

fn cmd_update(cfg: &Config, mode: Option<&str>) -> Result<Outcome, String> {
    if mode == Some("gpg") {
        for r in cfg.repos_by_priority() {
            print!("Importing GPG key for '{}' ... ", r.name);
            std::io::stdout().flush().ok();
            match gpg::import_key(r, &cfg.cache_dir) {
                Ok(()) => println!("ok"),
                Err(e) => println!("skipped ({e})"),
            }
        }
        return Ok(Outcome::Ok);
    }

    // ---- check phase: see which repos actually changed, without touching the
    // cache (so unchanged repos keep their metadata, including the MANIFEST). ----
    let repos: Vec<&config::Repo> = cfg.repos_by_priority();
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
        ui::blue(&format!(
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
    let mut failed_verify: Vec<String> = Vec::new();
    println!();
    for r in &chosen {
        let track = changelog_repo.as_deref() == Some(r.name.as_str());
        update_one_repo(cfg, *r, track, &mut failed_verify);
    }

    if !failed_verify.is_empty() {
        println!(
            "\n{}",
            ui::red(&format!(
                "{} repo(s) failed verification and were skipped: {}.",
                failed_verify.len(),
                failed_verify.join(", ")
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
    warn_unverified_repos(cfg);
    Ok(Outcome::Ok)
}

fn cmd_search(cfg: &Config, term: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let results = db.search(term);
    if results.is_empty() {
        println!("No packages match '{term}'.");
        return Ok(Outcome::NothingFound);
    }
    for p in results {
        let mark = if system::is_installed(&installed, &p.id.name) {
            ui::green(&format!("{:<11}", "installed"))
        } else {
            ui::red(&format!("{:<11}", "uninstalled"))
        };
        println!(
            "{} {} {}{}  {}",
            ui::cyan(&format!("[{}]", p.repo)),
            mark,
            ui::white(&p.id.name),
            ui::dim(&format!("-{}", p.id.version)),
            p.summary
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
    for h in hits {
        let pkgname = pkg::PkgId::parse(&h.package).map(|p| p.name).unwrap_or_else(|| h.package.clone());
        let mark = if system::is_installed(&installed, &pkgname) { "installed" } else { "uninstalled" };
        println!("[{}] {:<11} {}: {}", h.repo, mark, h.package, h.path);
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
        }
        return Ok(Outcome::NothingFound);
    }
    Ok(Outcome::Ok)
}

fn cmd_info(cfg: &Config, name: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let candidates = db.candidates(name);
    if candidates.is_empty() {
        println!("No package named '{name}' in any repo.");
        return Ok(Outcome::NothingFound);
    }
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    match system::installed_by_name(&installed, name) {
        Some(inst) => println!("Installed: {}", inst.tag()),
        None => println!("Installed: (none)"),
    }
    println!("Available candidates (highest priority first):");
    for p in candidates {
        let csize = p.size_k.map(|k| format!("{k} K")).unwrap_or_else(|| "?".into());
        let usize_ = p.size_uncompressed_k.map(|k| format!("{k} K")).unwrap_or_else(|| "?".into());
        let md5 = if p.md5.is_some() { "md5 ok" } else { "no md5" };
        println!("  [{}] {}", p.repo, p.id.tag());
        println!("        series: {}   compressed: {csize}   uncompressed: {usize_}   {md5}", p.series);
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

fn cmd_install(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (matched, misses) = collect(&db, patterns)?;
    for m in &misses {
        eprintln!("no match for '{m}'");
    }
    // install = only packages that are not already installed and not blacklisted
    let mut frozen = Vec::new();
    let mut already = Vec::new();
    let todo: Vec<_> = matched
        .into_iter()
        .filter(|p| {
            if cfg.is_blacklisted(&p.id.name) {
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
        println!("Nothing to install.");
        return Ok(Outcome::NothingFound);
    }
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
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("Proceed with installation?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    let before_cfgs: HashSet<PathBuf> = newconfig::find_new_configs(&newconfig::default_roots())
        .into_iter()
        .map(|nc| nc.new_file)
        .collect();
    execute_plan(cfg, &plan)?;
    report_pending_configs(&before_cfgs);
    Ok(Outcome::Ok)
}

fn cmd_upgrade(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (cands, protected) =
        collect_installed_targets(&db, &installed, &cfg.tag_priorities, patterns)?;
    let mut frozen = Vec::new();
    let todo: Vec<_> = cands
        .into_iter()
        .filter(|p| {
            if cfg.is_blacklisted(&p.id.name) {
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
    let todo = select_packages(todo, "upgrade", cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Upgrade)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;
    show_plan(&plan, &frozen, &protected);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("Proceed with upgrade?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    let before_cfgs: HashSet<PathBuf> = newconfig::find_new_configs(&newconfig::default_roots())
        .into_iter()
        .map(|nc| nc.new_file)
        .collect();
    execute_plan(cfg, &plan)?;
    report_pending_configs(&before_cfgs);
    Ok(Outcome::Ok)
}

fn cmd_reinstall(cli: &Cli, cfg: &Config, patterns: &[String]) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (cands, protected) =
        collect_installed_targets(&db, &installed, &cfg.tag_priorities, patterns)?;
    let mut frozen = Vec::new();
    let todo: Vec<_> = cands
        .into_iter()
        .filter(|p| {
            if cfg.is_blacklisted(&p.id.name) {
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
    let todo = select_packages(todo, "reinstall", cli.yes, cli.dry_run);
    if todo.is_empty() {
        println!("Nothing selected.");
        return Ok(Outcome::Ok);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Reinstall)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;
    show_plan(&plan, &frozen, &protected);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("Proceed with reinstall?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    execute_plan(cfg, &plan)?;
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
                    if cfg.is_blacklisted(&inst.name) {
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
        for inst in &installed {
            if (inst.name == term || inst.name.contains(term)) && seen.insert(inst.name.clone()) {
                if cfg.is_blacklisted(&inst.name) {
                    frozen.push(inst.name.clone());
                    continue;
                }
                todo.push(inst);
            }
        }
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

fn cmd_download(
    cli: &Cli,
    cfg: &Config,
    patterns: &[String],
    output: Option<&str>,
) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let (matched, misses) = collect(&db, patterns)?;
    for m in &misses {
        eprintln!("no match for '{m}'");
    }
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

    // Selecting a whole repo/tag can be hundreds of packages; confirm first.
    const BULK: usize = 10;
    if matched.len() > BULK && !cli.yes && !cli.dry_run {
        println!("This will download {} packages into {dest_label}.", matched.len());
        if !confirm("Proceed with download?", false) {
            return Ok(Outcome::Ok);
        }
    } else {
        println!("Downloading {} package(s) into {dest_label}.", matched.len());
    }
    if cli.dry_run {
        for p in &matched {
            println!("would download: [{}] {}", p.repo, p.filename);
        }
        println!("(dry-run: nothing downloaded)");
        return Ok(Outcome::Ok);
    }

    for p in &matched {
        let r = cfg.repo_by_name(&p.repo).ok_or("internal repo lookup failed")?;
        let dest = match &out_dir {
            Some(d) => d.join(&p.filename),
            None => system::cached_pkg_path(&cfg.cache_dir, &p.repo, &p.filename),
        };
        fetch_and_verify(cfg, r, p, &dest)?;
        println!("downloaded: {}", dest.display());
    }
    Ok(Outcome::Ok)
}

fn cmd_upgrade_all(cli: &Cli, cfg: &Config) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let mut ups = db.upgrades_for(&installed, &cfg.tag_priorities);
    ups.retain(|u| !cfg.is_blacklisted(&u.installed.name));
    if ups.is_empty() {
        println!("Everything is up to date.");
        return Ok(Outcome::Ok);
    }
    let self_upgrade = ups.iter().any(|u| u.installed.name == "slacker");
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots: Vec<_> =
        ups.iter().map(|u| (u.available.clone(), InstallAction::Upgrade)).collect();

    // Resolve dependencies up front, so the complete plan — including any new
    // packages pulled in as dependencies — is shown *before* we ask to proceed.
    // In a dry-run we keep installed versions for conflicts (no prompts).
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;

    println!("{}", ui::blue("The following packages will be upgraded:"));
    for u in &ups {
        println!(
            "{} {}",
            ui::green("  upgrade"),
            ui::white(&format!(
                "{} -> {}  [{}]",
                u.installed.tag(),
                u.available.id.tag(),
                u.available.repo
            ))
        );
    }
    // Any new packages that will be installed to satisfy dependencies.
    let new_deps: Vec<&PlanItem> = plan.iter().filter(|it| it.dep_for.is_some()).collect();
    if !new_deps.is_empty() {
        println!("\nThe following new packages will be installed as dependencies:");
        for it in &new_deps {
            let parent = it.dep_for.as_deref().unwrap_or("?");
            println!("  new-dep: [{}] {}  (for {})", it.pkg.repo, it.pkg.id.tag(), parent);
        }
    }

    if cli.dry_run {
        println!("\n(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("\nProceed with upgrade-all?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    let before_cfgs: HashSet<PathBuf> = newconfig::find_new_configs(&newconfig::default_roots())
        .into_iter()
        .map(|nc| nc.new_file)
        .collect();
    execute_plan(cfg, &plan)?;
    report_pending_configs(&before_cfgs);
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

    // Build map repo -> package *names* newly present since the last update. A
    // new build/version of an existing package is not "new" here (its name was
    // already present) — that is an upgrade, handled by upgrade-all.
    let mut new_by_repo: HashMap<String, HashSet<String>> = HashMap::new();
    for r in selected {
        if let Some(prev) = repo::previous_names(r, &cfg.cache_dir) {
            let cur = repo::load_repo(r, &cfg.cache_dir, &cfg.arch)?;
            let added: HashSet<String> = cur
                .iter()
                .map(|p| p.id.name.clone())
                .filter(|n| !prev.contains(n))
                .collect();
            if !added.is_empty() {
                new_by_repo.insert(r.name.clone(), added);
            }
        }
    }
    let news = db.newly_added(&new_by_repo, &installed);
    let todo: Vec<_> = news.into_iter().filter(|p| !cfg.is_blacklisted(&p.id.name)).collect();
    if todo.is_empty() {
        println!("No new packages to install.");
        return Ok(Outcome::NothingFound);
    }
    println!("{}", ui::blue("New packages:"));
    for p in &todo {
        println!(
            "{}{}",
            ui::green("  install   "),
            ui::white(&format!("[{}] {}", p.repo, p.id.tag()))
        );
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Install)).collect();
    // Resolve dependencies up front so any extra packages pulled in are shown
    // before we ask to proceed (dry-run keeps installed versions, no prompts).
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes)?;
    let new_deps: Vec<&PlanItem> = plan.iter().filter(|it| it.dep_for.is_some()).collect();
    if !new_deps.is_empty() {
        println!("\nThe following new packages will be installed as dependencies:");
        for it in &new_deps {
            let parent = it.dep_for.as_deref().unwrap_or("?");
            println!("  new-dep: [{}] {}  (for {})", it.pkg.repo, it.pkg.id.tag(), parent);
        }
    }
    if cli.dry_run {
        println!("\n(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("\nInstall new packages?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    execute_plan(cfg, &plan)?;
    Ok(Outcome::Ok)
}

fn cmd_clean_system(cli: &Cli, cfg: &Config) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    // Foreign = installed but in no configured repo. Two kinds are always kept:
    // blacklisted packages (by name), and packages whose build tag is in
    // IGNORE_TAGS (e.g. _SBo, cf, alien) — these come from sources slacker
    // doesn't manage as binary repos, so they must never be treated as foreign.
    let orphans: Vec<_> = db
        .orphans(&installed)
        .into_iter()
        .filter(|p| !cfg.is_blacklisted(&p.name) && !cfg.is_ignored_tag(p.build_tag()))
        .collect();
    if orphans.is_empty() {
        println!("No foreign packages found.");
        return Ok(Outcome::Ok);
    }

    println!("The following installed packages belong to no configured repo:");
    println!();
    let width = orphans.len().to_string().len();
    for (i, p) in orphans.iter().enumerate() {
        println!("  {:>width$}) {}", i + 1, p.tag(), width = width);
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
        println!("Enter numbers to KEEP (e.g. 1 3 5 or 2-4), 'n' to keep all/cancel,");
        print!("or press Enter to remove all listed: ");
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
    println!("'{verb}' matched {} packages:", pkgs.len());
    for (i, p) in pkgs.iter().enumerate() {
        println!("  {:>3}) {}", i + 1, p.tag());
    }
    print!("Enter numbers to {verb} (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel: ");
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
    println!("'{verb}' matched {} packages:", pkgs.len());
    for (i, p) in pkgs.iter().enumerate() {
        println!("  {:>3}) [{}] {}", i + 1, p.repo, p.id.tag());
    }
    print!("Enter numbers to {verb} (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel: ");
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

fn cmd_new_config(cli: &Cli) -> Result<Outcome, String> {
    let found = newconfig::find_new_configs(&newconfig::default_roots());
    if found.is_empty() {
        println!("No .new configuration files found.");
        return Ok(Outcome::Ok);
    }
    for nc in &found {
        println!("\n{}", ui::white(&nc.new_file.display().to_string()));

        // A .new only reaches us when the installed file exists and differs:
        // the package's own doinst.sh moves a .new into place when there is no
        // previous file, and removes one identical to it. So a .new with no
        // installed counterpart should never happen; if it does, the package is
        // most likely broken. We cannot diff or merge it, so warn loudly and
        // leave it untouched for the user to deal with.
        if !nc.target.exists() {
            let bar = "=".repeat(66);
            println!("{}", ui::red(&format!("  {bar}")));
            println!("{}", ui::red("  !! WARNING: this package looks broken"));
            println!("{}", ui::red("  !! a .new config file was installed but no previous version exists:"));
            println!("{}{}", ui::red("  !!   "), ui::white(&nc.new_file.display().to_string()));
            println!("{}", ui::red("  !! slacker cannot diff or merge it. Please review it manually,"));
            println!("{}", ui::red("  !! at your own responsibility."));
            println!("{}", ui::red(&format!("  {bar}")));
            continue;
        }

        // Identical to the installed file: the .new is redundant, drop it.
        if files_identical(&nc.target, &nc.new_file) {
            if cli.dry_run {
                println!("    {}", ui::dim("identical to the installed file (would remove .new)"));
                continue;
            }
            std::fs::remove_file(&nc.new_file).map_err(|e| format!("remove: {e}"))?;
            println!("    {}", ui::dim("identical to the installed file — removed redundant .new"));
            continue;
        }

        if cli.dry_run {
            println!("    {}", ui::dim(&format!("differs from {}", nc.target.display())));
            continue;
        }

        show_config_diff(&nc.target, &nc.new_file);
        loop {
            print!(
                "  {} ",
                ui::blue("[K]eep both  [O]verwrite  [R]emove .new  [M]erge  [D]iff ? [K]")
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
                    std::fs::rename(&nc.new_file, &nc.target).map_err(|e| format!("rename: {e}"))?;
                    println!("    {}", ui::dim("overwritten with the new file"));
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
                                std::fs::remove_file(&nc.new_file)
                                    .map_err(|e| format!("remove: {e}"))?;
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
    }
    Ok(Outcome::Ok)
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
            changelog::UpdateStatus::UpToDate => "up-to-date",
            changelog::UpdateStatus::Pending => {
                any_pending = true;
                "updates pending"
            }
            changelog::UpdateStatus::Unknown => {
                any_unknown = true;
                "unknown (run update first)"
            }
        };
        println!("  {:<width$}  {label}", r.name, width = width);
    }
    warn_unverified_repos(cfg);
    if any_pending {
        println!("\nRun `slacker update` then `slacker upgrade-all`.");
        Ok(Outcome::Pending)
    } else if any_unknown {
        Ok(Outcome::Ok)
    } else {
        println!("\nEverything up-to-date.");
        Ok(Outcome::Ok)
    }
}

fn cmd_show_changelog(cfg: &Config) -> Result<Outcome, String> {
    let Some(r) = changelog::changelog_repo(&cfg.repos) else {
        return Err("no repo configured".into());
    };
    match changelog::cached_changelog(r, &cfg.cache_dir) {
        Some(text) => {
            page_output(&text);
            Ok(Outcome::Ok)
        }
        None => {
            println!("No cached ChangeLog. Run `slacker update` first.");
            Ok(Outcome::NothingFound)
        }
    }
}

/// Print text through a pager when stdout is a terminal, so long output (the
/// ChangeLog) opens at the top — newest first — and is scrollable/quittable
/// like slackpkg. Falls back to a plain print when not a TTY (piped/redirected)
/// or when no pager is available.
fn page_output(text: &str) {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
        let mut parts = pager.split_whitespace();
        if let Some(cmd) = parts.next() {
            let args: Vec<&str> = parts.collect();
            let spawned = std::process::Command::new(cmd)
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .spawn();
            if let Ok(mut child) = spawned {
                if let Some(stdin) = child.stdin.take() {
                    let mut stdin = stdin;
                    let _ = stdin.write_all(text.as_bytes());
                    // drop stdin to signal EOF before waiting
                }
                let _ = child.wait();
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
        if system::is_installed(&installed, n) || cfg.is_blacklisted(n) {
            continue;
        }
        if let Some(p) = db.resolve(n) {
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
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm("Install template packages?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    execute_plan(cfg, &plan)?;
    Ok(Outcome::Ok)
}

fn cmd_remove_template(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let names = template::load(&cfg.config_dir, name, true)?;
    let todo: Vec<&String> = names
        .iter()
        .filter(|n| system::is_installed(&installed, n) && !cfg.is_blacklisted(n))
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
fn cmd_frozen(cfg: &Config, names: &[String]) -> Result<Outcome, String> {
    if names.is_empty() {
        return Err("frozen: give one or more package names".into());
    }
    let path = cfg.config_dir.join("blacklist");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut present: HashSet<String> = existing
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();

    let mut added = Vec::new();
    for n in names {
        if present.insert(n.clone()) {
            added.push(n.clone());
        }
    }
    if added.is_empty() {
        println!("Already frozen: {}", names.join(", "));
        return Ok(Outcome::Ok);
    }
    let mut body = existing;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    for n in &added {
        body.push_str(n);
        body.push('\n');
    }
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("Frozen (added to blacklist): {}", added.join(", "));
    Ok(Outcome::Ok)
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
        assert!(!requires_privilege(&Cmd::ShowChangelog));
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
