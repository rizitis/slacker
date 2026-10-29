//! Command-line definition: the top-level `Cli` and every subcommand.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    after_help = "Run `slacker <command> --help` for the full description and examples.",
    styles = HELP_STYLES
)]
pub(crate) struct Cli {
    /// Directory holding the plain-text config files.
    #[arg(long, global = true, default_value = "/etc/slacker")]
    pub(crate) config_dir: PathBuf,

    /// Assume "yes" to confirmation prompts.
    #[arg(short = 'y', long, global = true)]
    pub(crate) yes: bool,

    /// Show what would happen without changing the system.
    #[arg(long, global = true)]
    pub(crate) dry_run: bool,

    /// Do not read .dep files / pull in dependencies for this run.
    #[arg(long, global = true)]
    pub(crate) no_deps: bool,

    #[command(subcommand)]
    pub(crate) command: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
    /// Refresh metadata from every repo. `update gpg` imports repo GPG keys.
    Update { mode: Option<String> },
    /// Find a package by its exact name (case-insensitive).
    Search { pattern: String },
    /// Find which package ships a file (uses MANIFEST).
    FileSearch { filename: String },
    /// Show details and per-repo candidates for a package.
    Info { name: String },
    /// List configured repositories and their priorities.
    ///
    /// Shows each repo's priority, verify policy, and how many installed
    /// packages came from it.
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
    /// Download package files without installing them.
    ///
    /// Saved to the cache by default, or to a directory given with -o/--output.
    Download {
        patterns: Vec<String>,
        /// Directory to save into (default: the slacker package cache).
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Upgrade every installed package that has a newer revision.
    UpgradeAll,
    /// Distribution upgrade: 15.0 to -current or next stable.
    ///
    /// Migrate a Slackware 15.0 system to -current (or to the next stable).
    /// UNLIKE every other command it deliberately ignores source priority and
    /// the blacklist and takes the target distribution's version of every
    /// package.
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
    /// Install the repos' packages that aren't installed yet.
    ///
    /// The "fill what's missing" counterpart to `install @repo`: catches
    /// packages new to the distribution and ones you removed, correct across any
    /// number of updates. Honours source/tag priority and the blacklist (frozen
    /// packages are skipped); a newer build/version of an installed package is an
    /// upgrade (use `upgrade-all`), not new. Default: official repo(s) only; name
    /// repos to use those instead.
    InstallNew { repos: Vec<String> },
    /// Remove installed packages no longer in the official baseline.
    ///
    /// The baseline is the official repo plus any `immutable` repo.
    /// slackpkg-style: a package the distribution dropped is removed even if a
    /// third-party repo still ships the name. Kept by the blacklist, an
    /// `IGNORE_TAGS` build tag, or an immutable repo.
    CleanSystem,
    /// Delete downloaded package files from the cache.
    ///
    /// Repo metadata and GPG keys are never touched. Optionally limit to named
    /// repos.
    CleanCache { repos: Vec<String> },
    /// Handle leftover *.new configuration files.
    NewConfig,
    /// Check every configured repo for pending updates (exit 100 if any).
    CheckUpdates,
    /// Print a repo's cached ChangeLog.
    ///
    /// With no argument, the official (tracked) repo; name a repo to fetch and
    /// show that one instead.
    ShowChangelog { repo: Option<String> },
    /// Show a chronological log of package changes, newest first.
    ///
    /// Installed, upgraded and removed, and when. Derived from the pkgtools
    /// admin directories, so it also reflects changes made outside slacker.
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
    /// Find the fastest up-to-date Slackware mirror.
    ///
    /// Probes the official mirror list over HTTP and ranks reachable, fresh
    /// mirrors by speed, lists the 7 fastest and proposes the 3 fastest. Works
    /// on -current and stable. Does not change your configuration.
    FindMirror,
    /// Snapshot installed packages into a template.
    GenerateTemplate { name: String },
    /// Install all packages listed in a template.
    InstallTemplate { name: String },
    /// Remove all packages listed in a template.
    RemoveTemplate { name: String },
    /// Delete a template file (does not touch installed packages).
    DeleteTemplate { name: String },
    /// Add blacklist rules ("freeze"); no arg lists them.
    ///
    /// Each argument is one rule: a glob (`*`, `?`) or a regex, a `series/`, or
    /// `@repo PATTERN` (quote rules with spaces). Run with no argument to list
    /// the current frozen rules.
    Frozen { names: Vec<String> },
    /// Remove blacklist rules ("unfreeze"); no arg lists them.
    ///
    /// Each argument must match an existing rule EXACTLY, as a literal string:
    /// special characters like `.*`, `*`, `-` or `/` are compared verbatim,
    /// never as a pattern. Run with no argument to list the current rules. Quote
    /// rules with spaces.
    Unfrozen { names: Vec<String> },
    /// Pin a package to ONE repo, ignoring priority.
    ///
    /// `pin repo:package` (e.g. `pin alienbob:vlc`). From then on every command
    /// (incl. upgrade-all) sources that package only from that repo. Stored in
    /// the `blacklist` file as `@repo 100% package`. A freeze on the same
    /// package always wins. Run with no argument to list the current pins.
    Pin { spec: Option<String> },
    /// Remove a pin; no arg lists current pins.
    ///
    /// `unpin package` (e.g. `unpin vlc`). The package returns to normal
    /// priority-based resolution. Run with no argument to list the current pins.
    Unpin { names: Vec<String> },
    /// Add a binary repository to the `repos` file.
    ///
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
    /// Change a repo's priority in the `repos` file.
    ///
    /// `pri-repo PRIORITY NAME`. Refuses if NAME is not an active repo
    /// (suggesting the closest match), or if PRIORITY is already used by another
    /// repo (priorities must be distinct).
    PriRepo { priority: String, name: String },
    /// Add a build-tag priority line to the `repos` file.
    ///
    /// `add-tag PRIORITY NAME TAG` (e.g. `add-tag 100 SBo _SBo`; no quotes).
    AddTag { priority: String, name: String, tag: String },
    /// Remove a build-tag priority line (by its TAG) from the `repos` file.
    DelTag { tag: String },
    /// Re-run the safety vetting on a repo on demand.
    ///
    /// Fetches metadata only. Quarantines it if it fails, or clears a prior
    /// quarantine if it now passes.
    VetRepo { name: String },
    /// Trust a quarantined repo, lifting its freeze.
    ///
    /// Lets it be used again. This overrides slacker's safety verdict — at your
    /// own responsibility.
    TrustRepo { name: String },
    /// Manually quarantine a repo so it provides no packages.
    ///
    /// Freezes the repo until you `trust-repo` it again.
    DistrustRepo { name: String },
}
