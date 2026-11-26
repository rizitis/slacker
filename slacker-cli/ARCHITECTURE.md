# slacker: code structure, rules and refactor history

Last updated: 25 Nov 2026. Covers `slacker-cli` 0.14.3, the refactored code (steps 1–14) and the changes made on it since (see 8.5). Maintainer: Ioannis Anagnostakis (r-tz, `rizitis`).

This document is for anyone, human or AI, opening the code for the first time. It says **what lives where, why it lives there, and what must not change**. If anything here disagrees with the code, the code wins and this document gets fixed.

---

## 1. What slacker is

A package manager for Slackware, written in Rust. It combines in one tool:

- what `slackpkg` does (update, install, upgrade, upgrade-all, install-new, clean-system, new-config, etc.)
- the multi-repo priority resolution of `slackpkg+`.

slacker **never installs packages by itself**. It downloads, verifies (GPG, md5/sha256), decides what to do, and calls the Slackware pkgtools (`installpkg`, `upgradepkg`, `removepkg`) exactly as Patrick ships them. It reads the installed-package database (`/var/adm/packages`) and never writes it.

Slackware -current is called the **development branch** (not "rolling").

---

## 2. The repository

```
slacker/
├── slacker-cli/          the command-line tool (the subject of this document)
│   ├── Cargo.toml
│   ├── Cargo.lock
│   └── src/
├── slacker-gui/          separate GTK4/Rust graphical front-end (its own crate)
├── slacker.SlackBuild    Slackware packaging
├── slack-desc, doinst.sh, slacker.nfo, slacker-banner.svg
├── README.md, NEWS, TODO, LICENSE (Apache-2.0)
```

### Building

- `cargo build --release` inside `slacker-cli/`. The release profile uses `lto = true`, `strip = true`, `opt-level = 2`.
- Dependencies: `clap` (derive, wrap_help), `ureq` 2 (rustls, gzip), `md-5`, `regex`, `rusqlite` (bundled). `wrap_help` (25 Nov 2026) makes `--help` break lines between words at the terminal width; it brings `terminal_size`, `rustix`, `errno` and `linux-raw-sys`, pure Rust, no new library linked (`readelf -d` still shows only libc, libm, libgcc_s).
- **MSRV:** `rust-version = "1.89"`, set by `rusqlite` 0.42 (taken in 0.14.1 for the SQLite fix in RUSTSEC-2026-0459). The `icu_*` crates (via ureq -> url -> idna) need 1.88 and `clap` 1.85, both below it.
- Updating dependencies: targeted `cargo update -p <crate>` when `cargo audit` reports something; a full `cargo update` at the start of a development cycle, not on release day. Always follow with `cargo build --release --locked && cargo test --locked && cargo audit`.

---

## 3. Files on the system (runtime)

| location | contents | written by |
|---|---|---|
| `/etc/slacker/` (or `--config-dir`) | `slacker.conf`, `repos`, `mirrors`, `blacklist`, `distro-upgrade.conf`, `credentials.d/`, `credentials.cat`, `templates/`, `dist-backup-*/` | the user, and the commands that edit configuration (add-repo, frozen, pin, upgrade-dist, ...) |
| `CACHE_DIR` (default `/var/cache/slacker`) | `repos/<name>/` metadata (PACKAGES.TXT, CHECKSUMS.md5(.asc), ChangeLog.txt, MANIFEST and MANIFEST.bz2.md5, the md5 of the MANIFEST.bz2 it came from), `packages/<repo>/` downloaded packages | slacker. Disposable; `clean-cache` deletes only the packages. |
| `STATE_DIR` (default `/var/lib/slacker`) | `trusted/`, `quarantine/` (per-repo trust state), `gpg/` (private keyring and pinned fingerprints), stock-db | slacker. **Trust anchors live here, never in the cache.** |
| `ADM_DIR`/`PKG_DB_DIR` (default `/var/adm`) | the Slackware package database | **pkgtools only**. slacker only reads it. |
| `/run/slacker.lock` | exclusive lock (flock) | slacker, for commands that change something |

---

## 4. Execution flow

`main()` -> `Cli::try_parse()` -> `run(&cli)` -> exit code.

Order inside `run()` (in `main.rs`):

0. **Proxy settings**, before anything can download (`status` and `find-mirror` included): `PROXY` and `NO_PROXY` are read leniently from slacker.conf (`config::proxy_settings`) and handed to `download::set_proxy`.
1. **The labyrinth header** (`banner2::show()`), on a terminal only, unless `BANNER=off` in slacker.conf. That one key is read on its own (`config::banner_enabled`, like `system_arch`) because the header prints before the config is loaded and must not depend on it being valid. The same key hides the SLACKWARE art (`banner::show()`) after an upgrade-all that touched the official tree and at upgrade-dist.
2. **Commands that run before the config is loaded:** `status`, `find-mirror`, `new-config`. They must work even when the config is broken: `status` is what diagnoses it, and `new-config` is what repairs it. `new-config` still checks privileges and takes the lock. It reads only its own `NEWCONFIG_*` keys from slacker.conf, leniently (`config::newconfig_rules`), like `BANNER`: a missing or broken slacker.conf just means no rules.
3. `Config::load_dir()`. Strict: an error in `repos`/`mirrors` stops here.
4. `migrate_state()`: moves old trust anchors from the cache to STATE_DIR, once, never overwriting existing ones.
5. `download::set_auth(credentials::build_registry(...))`: HTTP Basic credentials, sent over https only.
6. For commands that modify the system or the cache (`requires_privilege`): root is required (membership in `wheel` only changes the error message), plus the exclusive lock. Read-only commands also run as a normal user.
7. Dispatch: one `match` that calls the command's `cmd_*`.

**Exit codes** (`enum Outcome`): `0` Ok, `20` NothingFound, `50` SelfUpgrade (slacker upgraded itself, run it again), `100` Pending (check-updates found updates), `1` error. `2` comes from clap (unknown command or option) and from the shell-glob refusal in `clap_error_exit`. `slacker-gui` and scripts may depend on these: **they do not change**.

Global flags (all commands): `--config-dir`, `-y/--yes`, `--dry-run`, `--no-deps`.

---

## 5. Code map (`slacker-cli/src/`)

### 5.1 Entry point and CLI definition

| file | lines | contents |
|---|---|---|
| `main.rs` | 4,469 | entry point (`main`, `run`, `Outcome`, lock, privilege checks, `migrate_state`) **and the shared core** (see 5.3), with its tests |
| `cli.rs` | 305 | `Cli`, `Cmd` (every command, clap derive), `HELP_STYLES`. The first paragraph of each doc comment is the short help line; the whole doc comment is `<cmd> --help`. |
| `commands.rs` | 17 | declares the `commands/` modules |

### 5.2 Commands (`src/commands/`)

Each file holds the `pub(crate) fn cmd_*` of one group of commands, plus the helpers used **only** by that group.

| file | commands | notes |
|---|---|---|
| `templates.rs` | generate/install/remove/delete-template | generate-template asks `Overwrite it? [y/N]` when the template exists and writes nothing under `--dry-run`; delete-template asks `Delete this template file? [y/N]`; `--yes` skips both (22 Nov 2026) |
| `repos.rs` | add-repo, del-repo, pri-repo, add-tag, del-tag, vet-repo, trust-repo, distrust-repo | the vetting itself (`vet_repo`, `apply_vet`, `reload_repo`) is in the core |
| `blacklist.rs` | frozen, unfrozen, pin, unpin, nodep, undep | honour `--dry-run` (fixed in 0.12.5) |
| `logs.rs` | history, show-changelog | not named `history.rs`, which already exists |
| `query.rs` | search, file-search, info | read-only. An installed package's `search` line shows the INSTALLED version, labelled by the repo of its exact build, else of the same build tag (`search_display`; official builds are tagless in every subtree, so the tag alone named the first of them); the repos' other builds, the label's own newer one included, go under `(also: ...)` (`also_entries`). Before 24 Nov 2026 it printed the matched repo's version next to "installed", so a pending update read as installed (and slacker-gui showed the same). `search` looks at package names only: exact by default, every name containing the word with `--partial` or `SEARCH=partial` (`PkgDb::search`, `search_rows`). Without them the match and the exit code are as before; the hint printed when no such name exists now also names `search --partial`. |
| `find_mirror.rs` | find-mirror | not named `mirrors.rs`, which already exists |
| `revert_pkg.rs` | revert-pkg | dangerous: GPG verify -> `upgradepkg --reinstall` -> offer to freeze |
| `clean.rs` | clean-system, clean-cache | dangerous: removes packages |
| `new_config.rs` | new-config | writes into `/etc`. `PROTECTED_CONFIGS` (passwd, shadow, group, gshadow, rc.inet1.conf, rc.local, rc.local_shutdown; gshadow added 24 Nov 2026) are never replaced by the menu or by a glob. The `NEWCONFIG_*` rules (`preset_for`, `apply_presets`, `glob_match`) run before the menu; `preset_key` names the rule for a file, for the list the package commands print. |
| `update.rs` | update, update gpg | the per-repo processing (`update_one_repo`) is in the core |
| `status.rs` | status, list-repos, check-updates | `list-repos` prints three tables: the repos, the build-tag priorities, and **Repository contents** (24 Nov 2026), read from the cache only: packages offered (`PkgDb::packages_of`), upgrades counted from `upgrades_for` (the very function upgrade-all uses) under the repo each upgrade comes from, the summed `PACKAGE SIZE` fields (`?` if one is missing), and the date on the first line of PACKAGES.TXT (`repo::cached_list_date`, `?` if none); a repo with no metadata or quarantined is all `?`. The first two tables are unchanged on purpose: the GUI parses them by column count. `status` is the setup doctor. A `Leftovers` row (Installed) names the `-upgraded-` records `upgradepkg` left in the package DB, and an `Incomplete` row lists what a cut-off install leaves (a record with no file list, a record whose install script did not run, `installpkg`'s own `installpkg-<cookie>` directory); a `Duplicates` row lists package names with more than one record, kernel packages apart in an information-only `Kernels` row; with any of the three (Kernels not counted), `slacker fix-upgrade` is a step of the verdict (after `update`, before `install-new`); with Duplicates the verdict also has a short note naming up to three of them and asking to use one package manager. Pending `.new` files (the `Configs` row) make `slacker new-config` the last step, as upgrades leave them behind (24 Nov 2026; before that the row had no step). A `Slackpkg` row, with a note in the verdict, is shown only when `KEEP_N_LAST_PKG_METADATA` is active in `ROOT/etc/slackpkg/slackpkg.conf` (`slackpkg_keep_n_last`). A `CheckRestart` row (Installed): as root, shown only when running programs still use replaced files, with a note in the verdict; as a normal user, "not checked (needs root)". |
| `check_restart.rs` | check-restart | prints the scan of `stale.rs` (see 7). Read-only, but needs root (`requires_privilege`), so it also takes the lock. |
| `fix_upgrade.rs` | fix-upgrade | finishes upgrades `upgradepkg` did not finish, and installs cut off before their file list, and removes the extra records of a package with more than one (`fix_duplicates`, see 7). Reads the pkgtools records (file lists, install scripts) to tell a record that only has to go (`record_only`), and calls only the pkgtools. |
| `upgrade_dist.rs` | upgrade-dist | the most dangerous one. Phases: escape kit -> transform -> update -> core (serial, with a package-database check) -> install-new -> the rest in batches -> clean-system -> second pass -> new-config -> status. The official repo and its patches subtree must refresh (`update_one_repo` returns whether the list is fresh); any stop before the first install puts `mirrors`, `repos`, `blacklist` back from the escape kit (`dist_restore`; `dist_execute` sets `started` before the first install). |
| `install.rs` | install, upgrade, reinstall, remove, download, upgrade-all, install-new | the resolver they use is in the core. `report_pending_configs` (also called by install-template and revert-pkg) lists the `.new` files a run left and, with `NEWCONFIG_AFTER`, asks to run new-config or runs it (`after_configs`). `pending_configs` takes the `.new` files and their stamps before the packages are applied; `is_fresh_config` tells which ones this run wrote. |

### 5.3 The shared core (stays in `main.rs`, on purpose)

Used by several groups at once (package commands, upgrade-dist, install-template, vet, update), so it belongs to no single command file. **It was decided not to move it** (see 8.4).

- **Dependency resolver:** `expand_with_deps`, `add_with_deps`, `resolve_protected_deps`, `ask_dep_conflict`, `ask_protected_dep`, `quarantine_deep_repo`, **`MAX_DEP_DEPTH = 512`**, `DEEP_CHAIN_ERR`. The DoS fix lives here (see 7).
- **Install order:** `upgrade_order` (called at the end of `expand_with_deps`, so the plan table shows the order packages are installed in), `UPGRADE_FIRST`, `upgrade_last` (also used by upgrade-dist).
- **Plans and tables:** `PlanItem`, `InstallAction`, `PlanRow`, `print_table`, `show_plan`, `print_plan`, `report_pinned_in_plan`, `hint_freeze_pin`, `note_optional_suggests`, conflicts (`detect_conflicts`, `report_conflicts`, `confirm_conflicts`).
- **Execution:** `execute_plan`, `install_counter` (`[ 37/245 ] upgrading ...`), `parallel_fetch` (with the live download line: `live_line`, `DlLive`, `term_width`, `DL_TICK`, `DL_STALL`, `DL_ANNOUNCE`), `fetch_and_verify`/`fetch_and_verify_metered`, `package_dest`, `DlItem`, `summarize_outcomes`, `report_batch_failures`, `enforce_release_match`, `enforce_arch_match`, kernel and perl reminders. The kernel reminder (`kernel_reboot_reminder`, `kernel_bootloader_hint`) only prints text: on x86 it says geninitrd rebuilds the initrd, runs update-grub and reinstalls LILO (its post-install `lilo-reinstall.sh` runs `lilo` when LILO boots `/boot/vmlinuz` or `/boot/vmlinuz-generic`), own steps go in `/etc/geninitrd.d/`; on ARM only the closing sentence (kernel_armv8 runs os-initrd-mgr itself). slacker never runs a bootloader or initrd tool.
- **Package selection:** `collect`, `collect_prefers`, `validate_selector`, `reject_mixed_selectors`, `guard_shell_expansion`, `closest`, `edit_distance`, `at_prefix_hint`, `parse_selection`, `bl_installed` (a series only for an official build: `PkgDb::series_of_installed`), `priority_display`, `installed_source_label`.
- **Repos:** `update_one_repo`, `UpdateOutcomes`, `malicious_filename_count`, `vet_repo`, `apply_vet`, `reload_repo`, `warn_unverified_repos`, `unverified_repo_names`.
- **Release/arch/mirrors:** `slackware_dir`, `is_current_codename`, `repo_release_token`, `system_release`, `arch_family`, `arch_compatible`, `is_x86_arch`, `osuosl_tree_prefix`, `parse_packages_date`, `mirror_is_stale`, `humanize_lag`, `FRESHNESS_*`.
- **Misc:** `confirm`, `hilite_keys`, `is_foundational`, `in_graphical_session`, `migrate_state`, `refuse_through_symlink` (every config write).

### 5.4 Helper modules (`src/*.rs`, other than `main`/`cli`/`commands`)

Mostly pure logic and system integration. They know nothing about commands.

| file | contents |
|---|---|
| `config.rs` | loading/validating `slacker.conf`, `repos`, `mirrors`, `blacklist`, `distro-upgrade.conf`; blacklist rules, pins (`@repo 100% name`) and dependency exclusions (`!package:dependency`); the lenient readers `banner_enabled`, `system_arch`, `newconfig_rules`, `proxy_settings`; `NEWCONFIG_AFTER` (`parse_newconfig_after`), `SEARCH` (`parse_search_partial`) |
| `pkgdb.rs` | unified database of available packages, priority resolution, freeze/pin, matching |
| `repo.rs` | fetch/parse PACKAGES.TXT (a list dated before the cached one goes to the caller's `accept_older` before anything is written), CHECKSUMS, MANIFEST (`refresh_manifest`: checked against CHECKSUMS.md5, fetched again only when its md5 there changes), `.dep` (read once per run and cached; `prefetch_deps` reads many in parallel before the resolver); per-repo quarantine/trust; safe-filename checks |
| `pkg.rs` | package identity (`name-version-arch-build`), build tags |
| `system.rs` | the live system: installed packages (skipping `-upgraded-` records, `is_removed_record`), the leftover records themselves (`leftover_upgrade_records`), records with no file list (`incomplete_records`, `record_lacks_file_list`), package names with more than one record (`duplicate_records`), an active `KEEP_N_LAST_PKG_METADATA` in slackpkg.conf (`slackpkg_keep_n_last`), installpkg's own leftover directories and the install scripts that did not run (`installpkg_tmp_dirs`, `scripts_not_run`, the root from ADM_DIR in `system_root`), running pkgtools (`pkgtools_running`, from `/proc`), os-release, pkgtools calls (argv only, never `sh -c`; `--terse` first when `TERSE=on`) |
| `download.rs` | downloads (http/https/file), size limits, md5/sha256, HTTP auth; `Meter`, the byte count of a running download that `parallel_fetch` shows; the proxy (`set_proxy`, `proxy_var`, `proxy_for`, `no_proxy_matches`, `describe_proxy` for `status`) |
| `gpg.rs` | private keyring, TOFU pinning of every fingerprint, signature checks |
| `credentials.rs` | HTTP Basic (zypper-style): `credentials.d/`, `credentials.cat`, root:0600 only, https only. The prefix match is in `download.rs` (`prefix_covers`: whole path segments only) |
| `changelog.rs` | ChangeLog.txt, check-updates |
| `history.rs` | the package change log (pure) |
| `stale.rs` | running programs that still use replaced files: reads `/proc` (maps, exe, map_files, ns/mnt) and the pkgtools records, for check-restart and status |
| `revert.rs` | pure helpers for revert-pkg |
| `newconfig.rs` | finding `*.new` under `/etc` and `/usr/share/vim` |
| `mirrors.rs` | mirror discovery and probing |
| `dist.rs` | upgrade-dist direction gate (fail-closed whitelist: only 15.0 -> -current and 15.0 -> a newer stable). The arch gate (x86 only) is `dist_arch_supported` in `commands/upgrade_dist.rs`. |
| `manifest.rs` | file-search via the cached MANIFEST (only the lines the text occurs in are parsed), and the listings of given packages (`entries_of`) for `space.rs` |
| `space.rs` | the disk-space estimate and question before a plan runs (`confirm_space`; see 7) |
| `template.rs` | templates of installed packages |
| `stockdb.rs` | `resolve-stock`: SQLite dependency database for stock -current |
| `ui.rs` | ANSI colours; honours `NO_COLOR` and non-terminal output |
| `banner.rs`, `banner2.rs` | banners |

---

## 6. Where new code goes

- **New command:** a variant in `Cmd` (`cli.rs`), with a doc comment on every argument (it is its line under `Arguments:` in `--help`; 25 Nov 2026, issue #26), a `pub(crate) fn cmd_x` in the right `commands/<area>.rs` (or a new file declared in `commands.rs`), a line in the `run()` dispatch, and in `command_name` (the compiler requires it). If the command modifies the system or the cache, also in `requires_privilege`.
- **Helper used by one command group only:** in that group's file.
- **Helper used by several groups:** in the core in `main.rs`. `commands/` modules reach it with `use crate::name` (private items of the crate root are visible to its descendant modules).
- **Pure logic with no command I/O:** in a helper module (`src/*.rs`), with its unit tests there.
- **File names:** must not collide with the helper modules. `history.rs`, `revert.rs`, `newconfig.rs`, `mirrors.rs`, `dist.rs`, `template.rs` already exist, which is why the commands live in `logs.rs`, `revert_pkg.rs`, `new_config.rs`, `find_mirror.rs`, `upgrade_dist.rs`, `templates.rs`.
- **Where options go, in --help** (issue #26, 25 Nov 2026): the global options are accepted anywhere on the line (`global = true`), a command's own options only after the command; this is by design and stays. `slacker --help` says it in its `after_help`, and every command's own `--help` ends with `cli::OPTIONS_PLACE`, added in `main()` with `mut_subcommands` when the command line is read, so no command needs its own copy.
- **Visibility:** `pub(crate)` only, and only where needed (the `cmd_*`, and helpers a test in another module needs).
- **Messages and errors** (24 Nov 2026, r-tz's request). A message that says something and then explains it is not one long line: a short first line in its colour (white, yellow, red …), then the explanation on indented lines in `ui::dim`, broken near 80 columns, words unchanged. An error (`Err(String)` that reaches `main()`) keeps what happened on its first line and puts the rest after a `\n`; `print_error` prints the first line after `slacker: error:` and every further line indented and dim. Put a `\n` only in an error that goes straight to `main()`: one printed inside other output (the per-repo lines of `update`, the failure list of a plan, a `status` row, anything reached from upgrade-dist) stays one line, and so does the part of an error that code matches with `starts_with` (`OLDER_PREFIX`, `MISMATCH_PREFIX`, `DEEP_CHAIN_ERR`). upgrade-dist's own messages are left as they are (r-tz's decision).

---

## 7. Invariants (do not change without an explicit decision by the maintainer)

### Principles (every change is checked against these first)

- **slacker keeps no database or state of its own** about packages. What is installed comes only from the pkgtools records; what is available only from the repos' files in the cache. Nothing is remembered between runs except trust state (STATE_DIR) and the cache.
- **slacker does not guess; the admin (the local system administrator) decides.** Where a choice has to be made, slacker shows the options with the facts it has and the user picks; where nothing can be done automatically, it prints the exact commands to run by hand (fix-upgrade, the installpkg directory). It never infers why something is installed or what something needs beyond what a repo itself declares.
- **pkgtools do the work** (see Security below). slacker never replaces what they do and never deletes or changes a file that belongs to an installed package. What it writes itself is its own config, state and cache, and the `.new` files new-config handles as the user chooses.
- **A guard warns, the user decides.** Every safety check stops without `--yes` and tells the user exactly what is wrong; with `--yes` the user takes the responsibility, after the same message. Some checks only ask and never refuse (the disk space). The package-list check has no override (see 0.14.1).
- **A new setting keeps the old behaviour by default.** A new slacker.conf key that is unset behaves as before it existed.
- **Nothing runs automatically that the user did not ask for** (no restarting services, no reboots). One read-only exception, r-tz's decision of 25 Nov 2026: after the commands that apply packages, check-restart's scan runs quietly and only says how many programs to restart and points at `slacker check-restart`; it lists nothing and restarts nothing.
- **Security comes before features.**

### How changes are made

- An idea that changes behaviour is **proposed first and decided with the maintainer** before any code. Never change something without saying so: every delivered change lists all its behaviour changes, especially the ones not asked for (new keys, files, messages, exceptions, limits).
- Anything that touches pkgtools or the installed system is **tried first in a sandbox** with the real pkgtools (cut points, fake roots, small filesystems) and only then written.
- Delivered files go one by one, with their path in the tree and md5. The docs (man page, wiki `Commands.md`, this file, completions when a command or option changes) change in the same delivery.
- Version numbers, NEWS and packaging are the maintainer's.

### Security

- **pkgtools exactly as Patrick ships them.** We do not modify them or parse their output. The options slacker passes are their own documented ones: what each command needs (`--reinstall`, `--install-new`), `--terse` when the user sets `TERSE=on`, and, in fix-upgrade only, `upgradepkg --install-new --reinstall` and `removepkg --skip-douninst` for a leftover record. The safety net is feeding them only verified packages and checking the result in the package database: the core packages in upgrade-dist, and in fix-upgrade the new record before `removepkg` and the leftover record after it (`removepkg` exits 0 even when it removed nothing), and, after a reinstall of a record with no file list, that the record now has one.
- **Dependency depth cap `MAX_DEP_DEPTH = 512`.** A repo that exceeds it is quarantined and the run (e.g. upgrade-all) carries on with the rest. Under `--dry-run` it only warns. Dependency cycles are cut via `visiting`. Confirmed in real use.
- **GPG TOFU:** every update re-fetches GPG-KEY and pins every fingerprint (subkeys included). A changed key -> hard quarantine. Deliberately strict.
- **Trust anchors in STATE_DIR**, never in the cache.
- **Path traversal:** `is_safe_filename` / `is_safe_location`. A repo advertising a suspicious package name -> hard quarantine.
- **Size limits:** metadata 512 MiB, package 8 GiB.
- **Never `sh -c`.** Every external call uses argv.
- **Never write through a symlink** for config files and download destinations (`refuse_through_symlink` before every config write).
- **Credentials over https only**, files root:0600 only.
- **Proxy, off by default.** Unset or `PROXY=off`: direct, and the environment's proxy variables are ignored, so an environment kept through sudo cannot route root's downloads. `PROXY=http://...` uses that proxy for every http and https download; `PROXY=env` takes `https_proxy`/`http_proxy`/`all_proxy`/`no_proxy` from the environment (lowercase, then uppercase, as wget and curl). `NO_PROXY` replaces the environment's `no_proxy`. Only http:// proxies: https goes through them as a CONNECT tunnel, so TLS and the certificate check still run end to end with the mirror. A proxy of another kind is an error, never a silent direct connection. ureq's own `try_proxy_from_env` is not used: one variable for every scheme and no `no_proxy`.
- **The proxy password goes only to the proxy**, in the CONNECT that ureq sends for an https URL. **Never add a `Proxy-Authorization` header to a request:** ureq keeps custom headers across redirects (it drops only `Authorization`, `Cookie` and `Content-Length`), so a mirror, or anyone on a plain-http path, could redirect to a host of its choosing and receive the password inside the tunnel. An earlier draft did this for http mirrors; it was removed before release. Consequence: an http mirror behind a proxy that asks for a password fails (407); use https. Passwords never appear in messages or in `status` (`hide_password`).

### Semantics (settled, do not reopen)

- **Priority above all:** never silently migrate or downgrade. Only an explicit `repo:name` or a pin overrides it.
- **Dependencies come from the parent's repo.** `add_with_deps` resolves a dependency only as `<parent's repo>:<dep>`, so a dependency that is not installed is taken from the same repo as the package that needs it, not from the priority winner. This is intended. An installed dependency from a source of equal or higher priority becomes a protected dependency (k/r/a/q prompt); a missing one only warns.
- **Official Slackware packages are never declared as dependencies.** Slackware assumes a full install. Do not invent scenarios where a third-party package lists an official one as a dependency.
- **upgrade-all may move an installed package to a higher-priority source** (`upgrades_for` in `pkgdb.rs`), even at the same version. This is correct, and it was checked against a real report (ghostty, Nov 2026). Do not change it.
- **The install plan says why each build was chosen.** In `show_plan_alternatives` (`commands/install.rs`) the chosen line reads `installing (dependency of <parent>, from its repo | pinned | priority winner | your choice)`. An earlier version labelled every chosen build "priority winner", which was wrong for dependencies and for a "replace" at a protected dependency.
- **Where an installed package came from** (`repo_for_installed` in `pkgdb.rs`): the repo shipping that exact build; else the same name with the same build tag; else, by tag only, the repo shipping the most packages with that tag. When several repos match, a pin on the name picks its repo if it is one of them, otherwise the higher priority wins. A tagless package is the official repo's. list-repos, status, history, `@repo` blacklist rules on installed packages and clean-system all use this. A build tag does not belong to one repo: Slackware's extra/ ships alienbob's `slackpkg+` with the `alien` tag (issue #11, Nov 2026, where every alien package was counted under extras).
- **`remove @repo` removes exactly what list-repos counts for that repo.** Never "every installed package carrying a tag the repo ships": that took every alien package for `@extras`, and every official package for any repo that ships a tagless build. `upgrade @repo` and `reinstall @repo` still select by the repo's tags, but they only act where that repo offers the package itself, and the priority check applies.
- **Dependency exclusion** (`!package:dependency`, `!*:dependency` for any package, in the `blacklist`; commands `nodep`/`undep`): `add_with_deps` does not pull that dependency in for that package; any other package that requires it still does, and it can be installed by name. One dependency per line, exact names, no spaces (anything else is rejected, never guessed); the commands take several at once and check them all before writing. `info` prints each candidate's `requires:` line, read as the resolver reads it (`.dep`, else `PACKAGE REQUIRED`), marking what is not pulled in. A rule acts only on what the package requires itself: `nodep` warns (and still writes the rule) when it does not, naming the dependency one level down that does (`nodep_reach_note` in `commands/blacklist.rs`). Freezing the dependency is not a substitute: a freeze applies to every package, silently, and blocks installing it by name (feature request by gegechris99, Nov 2026).
- **A dependency left out is always said.** When a frozen or excluded dependency is not installed, not asked for and not planned for another package, the plan prints `note: not pulling in '<dep>' for '<package>' (<rule or frozen>)`, so nothing is installed without one of its dependencies unnoticed.
- **Install order follows UPGRADE.TXT.** Every plan (`expand_with_deps` -> `upgrade_order`) puts `aaa_glibc-solibs`, `pkgtools`, `tar`, `xz`, `findutils`, `mkinitrd` first, in that order, and `kernel-generic` (or `kernel_armvN`) last; everything else keeps the resolver's order (alphabetical, each dependency before what needs it). The kernel is last because its doinst runs `geninitrd` (`/usr/sbin/geninitrd`, settings in `/etc/default/geninitrd`), and everything geninitrd runs must already be upgraded: first the `pre-install/*.sh` hooks in `/etc/geninitrd.d/`, where the `dkms-*` ones build the DKMS modules for the new kernel (only when DKMS modules are registered and `/lib/modules/$KV/build` exists), then `setup.01.mkinitrd` (or dracut, or an override) to build the initrd from kmod, udev, lvm2 and the rest, then `POST_INSTALL_SCRIPT` and the `post-install/*.sh` hooks: `lilo-reinstall.sh` runs `lilo` itself when the machine booted with LILO, `/etc/lilo.conf` has `image=/boot/vmlinuz` or `image=/boot/vmlinuz-generic` and the boot sector holds LILO; the optional `kexec.sh`; `dkms-*` ones, under the same conditions as before the initrd; and the admin's own. And `update-grub` runs after the initrd is generated when `AUTO_UPDATE_GRUB=true`, the default in `/etc/default/geninitrd`. That is also why `mkinitrd` is in the first list, as in step 3 of UPGRADE.TXT: the initrd of the new kernel is built with the new mkinitrd; slackpkg 16.0.1 also puts the kernel last (its own "first" list differs: pkgtools, aaa_glibc-solibs, glibc-solibs, aaa_libraries, aaa_elflibs, readline, sed; checked in its source, 24 Nov 2026). `kernel-source` is **not** moved: installed before `kernel-generic`, its tree is there when DKMS builds for the new kernel, so the build succeeds the first time (checked on a real upgrade with NVIDIA, Nov 2026).
- **The official tree and its subtrees are not asked for `.dep` files.** They ship none, and asking meant one 404 per package. `PACKAGE REQUIRED` from PACKAGES.TXT is still read for every repo. Third-party repos are asked as before.
- **Freeze:** installed + rule -> FROZEN. Not installed + rule -> hidden from install-new/upgrades/check-updates, visible in search/info. A frozen candidate is treated as absent and resolution continues with the remaining candidates.
- **Pin:** `@repo 100% name`, exact name, never freezes. A real freeze on the same name wins.
- **install-new:** `cmd_install_new` and `dist_install_new` are intentionally different (normal: official + subtrees ranked above official; dist: official + patches only, regardless of priority).
- **upgrade-dist:** x86 only (x86_64 and 32-bit x86): on Slackware ARM (aarch64) it refuses before reading or changing anything (`dist_arch_supported`, a final decision, Nov 2026). It requires an explicit target; deliberately bypasses the blacklist and priority; empties the blacklist and keeps `blacklist.bak` (never overwritten on a re-run); disables third-party repos; installs `kernel-generic` after everything else and the GnuPG chain last of all; clean-system stays in Phase 3, after the upgrade. **Which settings apply in it** (checked in the code, 24 Nov 2026): `VERIFY`, `PROXY`/`NO_PROXY`, `MAX_PARALLEL`, `TERSE`, `BANNER`, `ARCH`, the directories, `DISTRO_UPGRADE_MIRROR`, `IGNORE_TAGS` (Phase 3 is the normal `clean-system`) and `NEWCONFIG_KEEP` apply; the protected configs are never replaced. Not applied, by design: the blacklist (frozen rules, pins, nodep: it is emptied), repo priorities (third-party repos are disabled), `NEWCONFIG_OVERWRITE`/`NEWCONFIG_REMOVE` (those files are shown like any other), `RESOLVE_DEPS`/`RESOLVE_STOCK`, `NEWCONFIG_AFTER` (new-config always runs as a phase). Consequence: in Phase 3 a package the blacklist or a pin used to keep is listed by clean-system like any other foreign package; only `IGNORE_TAGS`, the `make-kernel-backup` kernel and slacker itself are never listed, and with `--yes` everything listed is removed. After a successful run the blacklist stays empty; the old rules are in `blacklist.bak`, to put back by hand.
- **clean-system:** pinned packages installed from the pin's repo get a separate R/K/P prompt (default Keep). With `-y` they are kept. It never proposes removing slacker itself, nor a fallback kernel saved by Slackware's `/boot/make-kernel-backup`: that tool records the kernel and its modules as a package `kernel-<BACKUP_NAME>-<version>` (`kernel-backup` by default) that no repository ships, so it looked foreign and was removed on Enter or `-y`. Since BACKUP_NAME can be anything, the backup is recognised by the two description lines the script writes in its pkgtools record, whatever its name (`is_kernel_backup` in `commands/clean.rs`, Nov 2026).
- **new-config:** protected files are never replaced, whatever the user answers in the menu. Only a manual merge, or the user naming the exact path in `NEWCONFIG_OVERWRITE`; a glob there never matches them.
- **new-config rules** (`NEWCONFIG_OVERWRITE` / `_REMOVE` / `_KEEP` in slacker.conf, paths or globs of the installed file, `*` and `?` not crossing `/`): applied before the menu, with or without `--yes`. In upgrade-dist **only `NEWCONFIG_KEEP`** applies, so a file the user keeps is never replaced there, not even by "[A]ll new"; OVERWRITE and REMOVE are not applied there: as with the blacklist, upgrade-dist shows every other file (it prints a note when such rules exist). These keys add up when repeated (every other key keeps its last line). When several match, the safer wins: KEEP over REMOVE over OVERWRITE. OVERWRITE keeps the old file as `.orig`; KEEP renames the `.new` to `.new.kept`, so nothing is left pending and nothing is lost (issue by gegechris99, Nov 2026).
- **new-config after a package command** (`NEWCONFIG_AFTER`, default `off`): after install, reinstall, upgrade, upgrade-all, install-new, install-template and revert-pkg, the `.new` files are listed (fresh ones apart from older pending ones, each with its `NEWCONFIG_*` key if a rule covers it) and the reminder is printed, as before. `ask` asks "Run new-config now? [Y/n]" and `run` runs it, but only when stdin and stdout are both a terminal, without `--yes` or `--dry-run`, and only if this run left new `.new` files; otherwise the reminder. A `.new` is this run's when it was not there before, or was and has been written again (a reinstall, or an upgrade shipping a newer `.new` over a pending one): slacker compares each file's inode, change time, size and modification time before and after. The modification time and size alone are not enough, because tar gives a `.new` the modification time stored in the package, so a reinstall rewrites it with the same one; `installpkg` unpacks with `tar --unlink-first`, so the inode and change time do move. A stamp that cannot be read never makes a file new. All of this is held in memory for the one command: **slacker keeps no state of its own between runs** and reads package state only from pkgtools (issue #19, Nov 2026). End of input at the question means no. cron, scripts, slacker-gui and `docker build` therefore never have `.new` files touched. The `NEWCONFIG_*` rules still apply first (request by gegechris99, Nov 2026).
- **Download progress:** on a terminal, `parallel_fetch` redraws one line with every running download and says `no data for Ns` after 15 s without a byte; the width is read from the terminal (TIOCGWINSZ) on every redraw. Off a terminal (pipe, log, slacker-gui) there is no `\r`: only a plain `↓ name (size)` line for a download still running after 1 s, so logs do not depend on timing for small packages (issue #14). A plan of **one** package goes through `parallel_fetch` too (issue #27, 26 Nov 2026), under the heading `Downloading 1 package...`, so it gets the same live line and `✓ name [1/1]`; before, it used `fetch_and_verify` with only `fetching URL` and nothing while a large package downloaded. A failed download still stops the command with the error and installs nothing, as before. `download` of one package does the same (its heading and its `downloaded: PATH` line are unchanged). fix-upgrade and revert-pkg fetch their one file on their own path and are unchanged.
- **Output:** exit codes and output format do not change without a reason. `slacker-gui` and scripts may depend on them.

- **fix-upgrade** (21 Nov 2026). `upgradepkg` renames the old record to `<pkg>-upgraded-<date,time>`, installs the new package, `removepkg`s the renamed record and installs the new package again; a record still in the package DB means it stopped in between. fix-upgrade runs the missing steps with the pkgtools, in this order: `upgradepkg --install-new --reinstall FILE`, `removepkg RECORD`, `upgradepkg --reinstall FILE`. The reinstall comes **first**: `installpkg` writes the record before it unpacks and the file list after, so an install cut off halfway leaves a record with no list, and a `removepkg` run before the reinstall would delete files the package needs (seen in the sandbox test). **slacker does not pick the package:** it lists every non-frozen candidate with its priority, whether it is exactly the installed build and whether it moves the package to a lower-priority source; the user picks one or skips (the default). With `--yes` only the exact installed build is taken; otherwise the package is skipped. A package no repo has, or one skipped, gets the three commands printed with the file name it needs. When the installed record is complete and lists every file and symlink of the leftover one (a reinstall cut off after its install pass), `removepkg` would delete nothing: only `removepkg --skip-douninst RECORD` runs, no package needed; `--skip-douninst` because removepkg would otherwise run the uninstall script of the package that stays installed, with no second install pass after it. It refuses while `installpkg`, `upgradepkg` or `removepkg` runs. Tested by cutting real pkgtools at four points (after the rename, mid-unpack, before removepkg, a corrupt package) and on the maintainer's system (an `_SBo` reinstall cut in July 2026).

  **Records with no file list** (22 Nov 2026). `installpkg` writes the record header, then `FILE LIST:`, and appends the list only after `tar` has extracted everything; a fresh install cut off in between leaves a record with no `FILE LIST:` line, or with no entry after it (a complete record always lists at least `./`). `removepkg` cannot remove what such a record does not list. `incomplete_records` finds them (reading each record only up to the first list entry), leaving out `-upgraded-` records and every record of a package that has one (that is an unfinished upgrade, fixed as above). `status` shows them as `Incomplete`. fix-upgrade fixes each with one `upgradepkg --reinstall FILE`, the package chosen in the same way (with `--yes` the exact build only); a candidate that is not the same build is marked, because files only the cut-off install put on disk stay. Two installed records of a kernel package are not touched (which one upgradepkg would replace is not for slacker to guess); the step is printed instead. Two records of any other package are fixed first, see Duplicate records. Tested by cutting a fresh `installpkg` before `FILE LIST:` and mid-unpack: after fix-upgrade the files and the record are the same as after a clean install. **installpkg's own directory** (22 Nov 2026). `installpkg` unpacks the package's `install/` into `ROOT/installpkg-<mcookie>` (`-<PID>` without mcookie), runs `doinst.sh` from there, copies it to `scripts/` and removes the directory; a cut after the unpack leaves it. A cut after the file list but before `doinst.sh` leaves a **complete** record, no script in `scripts/` and the symlinks of the script missing (seen in the sandbox), and the directory is the only sign of it. `installpkg_tmp_dirs` finds the directories in the root (read from ADM_DIR: the part before `var/adm`, `var/log` or `var/lib/pkgtools`, else `/`), names the package from their `slack-desc`, and `scripts_not_run` reports a package whose directory holds a `doinst.sh` that no record of it has in `scripts/`. That package is fixed like a record with no file list, with one `upgradepkg --reinstall`, which runs the script. **slacker does not remove the directory** (r-tz's decision: no pkgtool removes it): fix-upgrade prints the two commands `installpkg` would have run, `rm -f DIR/do*inst.sh DIR/slack-*` and `rmdir DIR`, to run after the fixes, and says so when the directory holds other files. Tested by cutting a fresh `installpkg` before `FILE LIST:`, mid-unpack and after the list, and an upgrade mid-unpack: after fix-upgrade and the printed commands, the files, symlinks and record are the same as after a clean install.
  **Duplicate records** (24 Nov 2026, r-tz's decisions). Two records of one package name in the package DB come from slackpkg's `sanity_check` (it renames a leftover `-upgraded-` record back to its old name, see "What other tools do") or from `installpkg` on a package already installed. Before this, every command saw one of them at random (`installed_by_name` takes the first in directory order): in the sandbox `info`/`search` showed the new build, `upgrade-all` planned an upgrade from the old one, `upgrade NAME` said nothing to upgrade, `remove` would have left the old record, and `status`/`fix-upgrade` said nothing. Now `duplicate_records` finds them (`-upgraded-` records not counted). **Kernel packages** (`is_kernel_pkg`) are left out everywhere but `status`, which shows them in the information-only `Kernels` row: keeping an older kernel with `installpkg` is a normal choice. `status` shows the rest as `Duplicates` with fix-upgrade as a step. Every command that installs, upgrades or removes packages (`changes_packages`: install, upgrade, reinstall, remove, revert-pkg, upgrade-all, install-new, clean-system, install-template, remove-template) prints them first and asks `Continue anyway? [y/N]` (`duplicate_guard`); `--yes` continues (the user takes responsibility), `--dry-run` only prints. upgrade-dist is not touched (r-tz's decision). The message names both causes rather than guessing which, and ends "Please use one package manager on this system." `fix-upgrade` handles them first (`fix_duplicates`): it lists the records, the user types the one to keep (Enter skips, the default), and every other record goes with `removepkg --skip-douninst RECORD`, which removes only the files no other record lists; the uninstall script is skipped because the package of that name stays installed. With `--yes` nothing is picked and the command is printed. Tested in the sandbox (foo-1 installed, upgrade to foo-2 cut after the new install, then slackpkg's rename): keeping foo-2 removed only the file foo-1 alone had, the shared files kept foo-2's content, and afterwards `upgrade-all` and `fix-upgrade` found nothing to do.

- **check-restart** (22 Nov 2026). Lists running programs that still use files an upgrade replaced (`installpkg` unpacks over the old files, so a running process keeps what it loaded, and `/proc` shows it as deleted). A process is listed only when: an **executable** mapping (`x`) or its program is deleted; it is in our mount namespace (other namespaces are counted, not checked: a container has its own files under the same paths); the path is in a pkgtools FILE LIST (installed, else a removed_packages record, shown as a removed package); and what is loaded differs from the file there now, read through `/proc/PID/map_files` (so a reinstall of the same build is not listed). Paths in no record (memfd, /tmp files) and non-executable mappings (data files such as `locale-archive`) are never listed. It **changes nothing and decides nothing**: it does not restart anything and guesses nothing about scripts (python, perl, shell), which it says it does not see. `status` runs the scan as root and shows a `CheckRestart` row and a note only when something is found. **After applying packages** (25 Nov 2026, r-tz's decision, changing the one of 22 Nov that it never runs automatically): `report_pending_configs`, called by install, upgrade, reinstall, upgrade-all, install-new, install-template and revert-pkg after the packages are in (never on `--dry-run`, never when nothing was applied), prints the `.new` files and then runs the same scan quietly (`report_stale_programs`): nothing when there are none, else "N running program(s) still use files an upgrade replaced." and a dim line "Run `slacker check-restart` to see them, then restart them or reboot." The count includes programs left by earlier upgrades. Cost: the scan's time, about 1.5 s measured on r-tz's desktop. Not after remove, fix-upgrade or upgrade-dist. Tested with a real running program whose binary an upgrade replaced (counted) and with none (silent). Root only, even though read-only. Exit 100 when something is found. The table shows packages, not files (22 Nov 2026, after the Qt 6.12 upgrade under a running Plasma listed over 300 files for plasmashell on one line): the program's own package, then the removed packages (marked `removed`, the version that is no longer on the system, so the real reason to restart is never cut off), then the others, each with the number of its files the process uses, most first; the full names of the removed packages are listed once under the table; cut to the terminal width with "… +N packages" (`packages_summary`), whole in a pipe. Compared before writing it with zypper ps, dnf needs-restarting, needrestart and the FreeBSD/OpenBSD checkrestart: the same base (deleted executable mappings); the pkgtools file list and the content check are slacker's own. Tested with the real pkgtools on: an upgraded program and library (listed), memfd, a deleted /tmp file and a read-only data mapping (not listed), a same-build reinstall (not listed), a container namespace (counted only), a removed package (listed as removed).
- **Disk space** (22 Nov 2026). Before `install`, `upgrade`, `reinstall`, `upgrade-all`, `install-new` and `install-template` (the six callers of `execute_plan`, right before their dry-run return), `space::confirm_space` estimates the space per filesystem and compares it with `df`'s free space. It **never refuses** (r-tz's decision): when it may not fit it shows the short filesystems and asks `Continue anyway? [y/N]` (Enter stops, nothing changed); `--yes` prints the warning and continues; `--dry-run` prints it and asks nothing; when it fits nothing is printed. upgrade-dist keeps its own checks. The estimate is the **whole new package**, file by file from the cached MANIFEST (each file rounded up to the block of its filesystem, one block per directory that does not exist yet, the record twice), plus 10%; packages not yet in the cache count compressed on `CACHE_DIR`'s filesystem; the root is `system_root(ADM_DIR)`, paths are resolved through symlinks and matched to the longest mount point of `/proc/self/mounts`. Why the whole new package: measured with real `upgradepkg` runs on ext4 (xz, freetype, python3, boost, breeze-icons, rust, old builds from the cumulative archive), the peak is that when running programs keep the old files open (their blocks are freed only when they exit), and up to 5% more for many small files, hence the 10%; without open files the peak is much lower, so the question can come when the plan would fit. A package in no cached MANIFEST counts as **twice** its PACKAGES.TXT uncompressed size (that figure was as low as 0.48 of the real one), and the warning says how many. Cost: about 0.3 s for a 275-package plan against the official MANIFEST (89 MB), nothing for a repo with none. For this, `update` now fetches MANIFEST.bz2 (before 0.14.2 only file-search did, lazily, unchecked, and `update` deleted it): after CHECKSUMS.md5 and its signature, checked against the md5 listed there (`checks_packages_txt` policy; not listed = kept, as PACKAGES.TXT), fetched again only when that md5 changes (`MANIFEST.bz2.md5`), decompressed capped and renamed into place whole; on any failure the old one is removed. An up-to-date repo with no MANIFEST in the cache gets it on the next `update`. `file-search` downloads nothing any more, so it works as any user, and names repos with no cached MANIFEST.

- **An older package list is not taken by default** (24 Nov 2026). An old snapshot passes every check (its CHECKSUMS.md5 and signature are genuine, only old; PACKAGES.TXT matches its md5), and upgrade-all takes the repo's version even when it is older (Pat sometimes downgrades on purpose), so a mirror that is behind, or a switch to an older one, would bring old builds back as "upgrades". `update_repo` (`repo.rs`) compares the date in the first line of the fetched PACKAGES.TXT (`PACKAGES.TXT;  <date>`, `parse_packages_date`) with the cached one (`list_went_back`), after the consistency check and before writing anything; strictly older: the caller decides (`accept_older`). `update_one_repo` takes an `OlderList`: `update` asks "Use the older list anyway? [y/N]", and with `--yes` keeps the cached list without asking (r-tz's choice: a scheduled `update --yes` must never take it; like the package-list check, no `--yes` override); `upgrade-dist` takes the target's lists (`OlderList::Take`); `vet_repo` keeps the cached list and vets it, and the older list is no reason to freeze the repo. Refused, nothing of the repo changes (`OLDER_PREFIX`, listed under "NOT updated"). No date on either side, or no cache: taken as before. `status` still shows the official mirror's lag against upstream; this check also covers third-party repos and runs on every update. Tested with file:// mirrors: older with `--yes`, n, y; newer; undated; vet-repo.

### Rejected (do not propose again)

- A mandb freshness check in `status`.
- Moving clean-system before the upgrade in upgrade-dist.
- Reverse-dependency tracking (ELF `NEEDED` scanning, "who needs X") and a `clean-deps` command: Slackware has no dependency system by design, pkgtools do not record why a package was installed, and slacker keeps no state, so only the local admin knows. slacker shows only the dependencies a repo itself declares (`PACKAGE REQUIRED:`, `.dep`).
- `check-conflicts`: reporting files written by two packages, either before an install (from the MANIFEST) or afterwards (from the records). Rejected as risky (22 Nov 2026): package splits overlap by design (qt6 and qt6-webengine, 21 Nov 2026), and removepkg already keeps the files another record lists.
- `removepkg --warn` in the dry-run of remove, clean-system and remove-template (22 Nov 2026). Tested with the real pkgtools: for one package it is exact, but for several it judges each against all other installed records, so a file shared only by the selected packages is reported as "Skipping" while the real removal, one package after another, deletes it. It also needs root (it writes its temporary lists to `$ROOT/var/lib/pkgtools/setup/tmp`, mode 700). Computing the list in slacker would reimplement removepkg's logic. Not worth it.
- Hooks of any kind (24 Nov 2026): running admin scripts after a command (as slackpkg's `functions.d` does, or slackpkg+'s plugins and its `notifymsg.conf`, whose messages go through `eval`), and text-only notifications per package. The distribution already has the place for this: when a kernel is installed, `geninitrd` (`/etc/default/geninitrd`) rebuilds the initrd, runs `update-grub` and runs every executable script in `/etc/geninitrd.d/pre-install/` (DKMS rebuilds: `dkms-nvidia.sh`, `dkms-bcachefs.sh`) and `post-install/` (the admin's own steps, e.g. copying to the EFI partition). It works the same under slacker, slackpkg or plain `upgradepkg`. Notes the admin needs after an upgrade are in Pat's ChangeLog (`show-changelog`). Anything else is chained in the shell: `slacker upgrade-all && my-script`.
- Backing up the protected configs (passwd, shadow, group, gshadow, rc.inet1.conf, rc.local, rc.local_shutdown) in the upgrade-dist escape kit (24 Nov 2026). Neither slacker nor pkgtools ever write them: slacker never replaces them, the packages ship them as `.new` and `config()` leaves the `.new` beside an existing file, and an interrupted upgrade does not touch them. Only the admin's own action can change them (`M` in new-config, or naming the exact path in `NEWCONFIG_OVERWRITE`), and slacker warns before every choice. A copy of shadow/gshadow in the kit would put the password hashes in a second place, a risk added to guard against something slacker does not do. The shadow tools already keep `/etc/passwd-`, `shadow-`, `group-`, `gshadow-`.
- Printing the check-restart table after a command (only the one-line count is printed, see check-restart in Semantics), restarting services or rc scripts for the user, a "Reboot now?" prompt, scanning `/proc/*/fd`.
- The disk-space check refusing a plan; slacker removing `installpkg`'s leftover `/installpkg-<cookie>` directory itself (it prints the commands).
- slacker-gui writing slacker.conf (it writes only the blacklist and the repos, through slacker commands).
- Flatpak applications in `slacker search` or slacker-gui (that is slacker-flatpak).
- Rust edition 2024 (staying on 2021).
- Several mirrors with fallback: left as it is for now.

### What other tools do to the package DB (checked in the slackpkg 16.0.1 source, 24 Nov 2026)

slacker reads the pkgtools records and changes none of them itself. Two things slackpkg (Slackware's `ap/slackpkg`, not slackpkg+) does can change what slacker sees, on a system where both are used:

- **`sanity_check`** (`core-functions.sh`), run at the start of slackpkg's `upgrade` and `upgrade-all`, renames every `<pkg>-upgraded-<date>` record in the package DB (and its script) back to the plain name with `mv`, instead of finishing the upgrade. After that slacker's `status` shows no `Leftovers` row for it; if the new build was already installed there are two records of the same name, which slackpkg then asks to remove one of. slacker shows them as `Duplicates` and `fix-upgrade` removes the ones the user does not keep (see Duplicate records in 7).
- **`KEEP_N_LAST_PKG_METADATA`** (`lookoldpkgfiles` in `post-functions.sh`), commented out in the shipped `slackpkg.conf`, run by slackpkg's post-install step after every command except check-updates, remove, search, file-search, update, info, blacklist, clean-system, download, generate-template and remove-template: when set, it deletes all but the last N `-upgraded-` records of each package from `removed_packages` and `removed_scripts`. `history`, `revert-pkg` (its list of earlier versions) and the removed-package lookup of `check-restart` read those records, so they see less history.

Neither is a slacker bug; do not "fix" them in slacker.

### Not bugs (do not raise)

- `update` exits 0 when one repo fails: the rest of the update finishes; a non-zero exit would stop scripts that chain it.
- `history --since` compares dates as text: pkgtools write YYYY-MM-DD.
- The exit codes (2 from clap, 1 for a name that matches nothing, 20 nothing to do, 100 updates pending / programs to restart / mismatched cached list, 0 from check-updates when a repo cannot be checked) stay as they are.
- `remove NAME` matching every installed name that contains the word: the list is shown and the user picks.
- A package from a repo built for another release goes in only with an explicit `--yes`, after the red release-mismatch list; that is the user's responsibility.
- The two `arch_family` functions disagreeing on a hand-written `ARCH=` value.
- The `cmd_install_new` and `dist_install_new` source rules being opposite: intended (upgrade-dist must not pull `testing`).
- Slackware never declares official packages as dependencies (a full install is assumed); do not build scenarios on that.

---

## 8. The `main.rs` refactor (Oct 2026)

### 8.1 Why and how

`main.rs` was **12,245 lines** with every command in one file. It was split into modules **without any change in behaviour**. After the refactor it is **3,922 lines** (entry point, shared core, and their tests).

Rules followed at every step:

1. **Verbatim moves.** Each item's text (code, comments, doc comments) moves as is. The only changes: `pub(crate)` where needed, and `use` lines as the compiler requires them.
2. **No logic changes**, not even "small improvements". Anything found was noted and done separately (e.g. the dry-run fix).
3. **Tests:** bodies never change. A test moves with the code it tests. Tests that use the shared fixtures of `collect_tests` (`av()`, `fixture()`) stay in `main.rs` and only get an import. The count stays **219 passed + 1 ignored**.
4. **No rustfmt.** Upstream is not formatted already (e.g. `banner2.rs`), and formatting would change text and break the line-by-line comparison.
5. **Helper modules were not touched.**

### 8.2 Steps

| # | file | contents |
|---|---|---|
| 1 | `cli.rs` | `Cli`, `Cmd`, `HELP_STYLES` |
| 2 | `commands/templates.rs` | template commands |
| 3 | `commands/repos.rs` | repo commands, with the `repos_text_set_priority` test |
| 4 | `commands/blacklist.rs` | frozen/unfrozen/pin/unpin, with 7 tests |
| 5 | `commands/logs.rs` | history, show-changelog |
| 6 | `commands/query.rs` | search, file-search, info. `search_display` is `pub(crate)` for `collect_tests`. |
| 7 | `commands/find_mirror.rs` | find-mirror |
| 8 | `commands/revert_pkg.rs` | revert-pkg |
| 9 | `commands/clean.rs` | clean-system, clean-cache, with `is_pinned_from_its_repo` and its test |
| 10 | `commands/new_config.rs` | new-config, with `PROTECTED_CONFIGS` and its test |
| 11 | `commands/update.rs` | update, `refresh_stock_db` |
| 12 | `commands/status.rs` | status, list-repos, check-updates, with `attribution_tests`, `locate_db_tests` and 6 more tests |
| 13 | `commands/upgrade_dist.rs` | upgrade-dist and its 19 own items, with 4 tests (5 since 0.14.1). `dist_upgrade_sets` is `pub(crate)` for 2 tests in `collect_tests`. |
| 14 | `commands/install.rs` | the 7 package commands and their 20 own helpers, with 4 tests. `abort_on_misses`, `selector_miss_hint`, `plan_alternatives` are `pub(crate)` for tests in `collect_tests`. |

### 8.3 How it was verified

Every step: build with no warnings, 219/219 tests, the 79 help outputs identical (`--help`, plus `-h` and `help <cmd>` for every command). Each command group was run against a fake system with the binary from before and after the step, and the results were compared.

For the dangerous commands, additionally:
- a line-by-line text comparison with the original
- proof that `main.rs` is exactly the previous version minus the removed pieces (plus only `use` lines)
- a check that every name in the new file resolves to the same item as before

The fake systems run in a private mount (and net) namespace: a copy of `/etc` with a fake os-release, local file:// mirrors with a real GPG key, and fake `installpkg`/`upgradepkg`/`removepkg` that keep a fake package database and log every call. The real system is never touched.

| group | runs |
|---|---|
| templates, repos, blacklist, history, query, find-mirror | 8, 18, 22, 13, 24, 5 |
| revert-pkg (plus the full GPG -> reinstall -> freeze path) | 13 + 11 |
| clean-system / clean-cache | 36 |
| new-config (plus dist mode) | 17 (+22) |
| update | 25 |
| status / list-repos / check-updates | 29 |
| upgrade-dist | 31 |
| package commands and the DoS fix | 105 |

**Final check:** the deliverable was built on a clean checkout of the repo. All of the above came out **identical to the binary of the normal branch** (0.12.4 with cap+quarantine, help, dry-run fix). In other words, the refactor behaves exactly like the code running in production.

The scripts were written for this refactor and ran only in the environment where it was done. Besides root, `unshare`, `gpg` and `python3`, they depend on fixture directories and fixed setup binaries that are not in `slacker-refactor-harness.tar.gz`, and their paths are hard-coded. r-tz keeps the tarball as a record; it is not part of the repo.

### 8.4 What was deliberately not done

- **The shared core was not moved** (the planned step 15: `plan.rs`, `execute.rs`, `select.rs`, `prompt.rs`). The resolver with `MAX_DEP_DEPTH`, execution, fetch/verify and package selection stay where they were tested on a real system. The only gain would have been organisation. The same goes for `collect_tests` (about 550 lines with shared fixtures).
- **The entry point stays in `main.rs`:** `main`, `run`, `Outcome`, lock, privilege checks.

### 8.5 After the refactor

The 0.12.x line is no longer maintained, so there is no normal branch to keep in sync. Changes go straight into this code. The "identical to the normal branch" result in 8.3 is historical: it held for the refactor as delivered.

The first change after the refactor was the plan label (see 7). The second was issue #11 (source attribution and `remove @repo`, see 7), with 2 tests. The third was `BANNER` (issue #12, see 4), with 1 test. The fourth was the dependency exclusion (see 7), with 4 tests. The fifth was the download progress line (issue #14, see 7), with 2 tests. These five are 0.13.0.

0.14.0 added, each with its tests: "Preparing the plan" with the parallel, cached `.dep` reading, and the official tree no longer asked for `.dep`; `TERSE` and the install counter; the install order of UPGRADE.TXT with the kernel last, in every plan and in upgrade-dist; the `NEWCONFIG_*` rules of new-config; `NEWCONFIG_AFTER`, and `NEWCONFIG_KEEP` in upgrade-dist; the proxy (`PROXY`/`NO_PROXY`, off by default, with a `Proxy` line in `status`); `search --partial` and `SEARCH` (issue #22); and clean-system keeping the `make-kernel-backup` kernel (all see 7). 0.14.1: update checks PACKAGES.TXT against CHECKSUMS.md5 and keeps the old metadata on a mismatch, and every plan is refused while a cached list does not match (`enforce_package_list`, no `--yes` override); the `status` Freshness fix; upgrade-dist refused on aarch64. Then (16 Nov 2026): `remove series/` and `remove` honouring `series/` and `@repo` freezes (`PkgDb::has_series`, `series_of_installed`, `BlacklistRule::needs_package_lists`); config writes refuse a symlink (`refuse_through_symlink`, moved to `main.rs`); credential prefixes match whole path segments only (`prefix_covers`); `unfrozen` with no argument lists the rules and exits 0; upgrade-dist stops when the official repo or its patches subtree cannot be refreshed, and any stop before the first install restores `mirrors`, `repos`, `blacklist` from the escape kit (`dist_restore`). 0.14.2 (22 Nov 2026): the disk-space question before every install, upgrade and reinstall plan (`space.rs`); `update` keeps every repo's MANIFEST, checked against CHECKSUMS.md5, and `file-search` reads only the cache and parses only the matching lines; `check-restart` and the `CheckRestart` row of `status`; fix-upgrade also fixes records with no file list and install scripts that did not run, and prints the commands to remove `installpkg`'s leftover directory, all shown by `status` as `Incomplete` (both see 7). Also (22 Nov 2026): generate-template asks before overwriting an existing template and honours `--dry-run`, delete-template asks before deleting, and `check-yes-noninteractive.sh` covers both (13 commands). Before that (21 Nov 2026): `status` reports `-upgraded-` records left by an unfinished `upgradepkg` (`Leftovers`) and lists `slacker fix-upgrade` as a step; the new command `fix-upgrade` (see 7), with its completions and man page. Every new key defaults to the behaviour from before it existed. After any change: `cargo build --release --locked` with no warnings, `cargo test --release --locked`, and the checks the SlackBuild runs (`contrib/check-completion-drift.sh`, `contrib/gen-completions.py --check`, `contrib/check-yes-noninteractive.sh`, `cargo audit`). Before a release: a run on a real system by the maintainer.

---

## 9. Tests

- `cargo test`: **283 passed, 1 ignored** (284 in total).
- Per file: `main.rs` 52, `config.rs` 39, `pkgdb.rs` 36, `repo.rs` 17, `revert.rs` 11, `commands/blacklist.rs` 11, `pkg.rs` 10, `dist.rs` 10, `commands/status.rs` 10, `stockdb.rs` 9, `download.rs` 9 (one ignored: a live TLS handshake), `mirrors.rs` 7, `history.rs` 6, `credentials.rs` 5, `stale.rs` 3, `commands/install.rs` 7, `system.rs` 11, `commands/upgrade_dist.rs` 5, `commands/new_config.rs` 4, `commands/fix_upgrade.rs` 3, `commands/check_restart.rs` 2, `template.rs` 3, `manifest.rs` 4, `space.rs` 3, `ui.rs` 2, `commands/clean.rs` 2, and one each in `newconfig.rs`, `commands/query.rs`, `commands/repos.rs`.
- Test modules in `main.rs`: `parallel_download_tests`, `arch_tests`, `selection_tests`, `collect_tests` (with the `av()` and `fixture()` fixtures), `freshness_tests`, `foundational_tests`, `symlink_guard_tests`.
