# slacker: code structure, rules and refactor history

Last updated: 10 Nov 2026. Covers `slacker-cli` 0.14.0, the refactored code (steps 1–14) and the changes made on it since (see 8.5). Maintainer: Ioannis Anagnostakis (r-tz, `rizitis`).

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
- Dependencies: `clap` (derive), `ureq` 2 (rustls, gzip), `md-5`, `regex`, `rusqlite` (bundled).
- **MSRV:** `rust-version = "1.88"`. The `icu_*` crates (pulled in via ureq -> url -> idna) require it since icu 2.3.
- Updating dependencies: targeted `cargo update -p <crate>` when `cargo audit` reports something; a full `cargo update` at the start of a development cycle, not on release day. Always follow with `cargo build --release --locked && cargo test --locked && cargo audit`.

---

## 3. Files on the system (runtime)

| location | contents | written by |
|---|---|---|
| `/etc/slacker/` (or `--config-dir`) | `slacker.conf`, `repos`, `mirrors`, `blacklist`, `distro-upgrade.conf`, `credentials.d/`, `credentials.cat`, `templates/`, `dist-backup-*/` | the user, and the commands that edit configuration (add-repo, frozen, pin, upgrade-dist, ...) |
| `CACHE_DIR` (default `/var/cache/slacker`) | `repos/<name>/` metadata (PACKAGES.TXT, CHECKSUMS.md5(.asc), ChangeLog.txt, MANIFEST), `packages/<repo>/` downloaded packages | slacker. Disposable; `clean-cache` deletes only the packages. |
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

**Exit codes** (`enum Outcome`): `0` Ok, `20` NothingFound, `50` SelfUpgrade (slacker upgraded itself, run it again), `100` Pending (check-updates found updates), `1` error. `slacker-gui` and scripts may depend on these: **they do not change**.

Global flags (all commands): `--config-dir`, `-y/--yes`, `--dry-run`, `--no-deps`.

---

## 5. Code map (`slacker-cli/src/`)

### 5.1 Entry point and CLI definition

| file | lines | contents |
|---|---|---|
| `main.rs` | 4,338 | entry point (`main`, `run`, `Outcome`, lock, privilege checks, `migrate_state`) **and the shared core** (see 5.3), with its tests |
| `cli.rs` | 271 | `Cli`, `Cmd` (every command, clap derive), `HELP_STYLES`. The first paragraph of each doc comment is the short help line; the whole doc comment is `<cmd> --help`. |
| `commands.rs` | 15 | declares the `commands/` modules |

### 5.2 Commands (`src/commands/`)

Each file holds the `pub(crate) fn cmd_*` of one group of commands, plus the helpers used **only** by that group.

| file | commands | notes |
|---|---|---|
| `templates.rs` | generate/install/remove/delete-template | |
| `repos.rs` | add-repo, del-repo, pri-repo, add-tag, del-tag, vet-repo, trust-repo, distrust-repo | the vetting itself (`vet_repo`, `apply_vet`, `reload_repo`) is in the core |
| `blacklist.rs` | frozen, unfrozen, pin, unpin, nodep, undep | honour `--dry-run` (fixed in 0.12.5) |
| `logs.rs` | history, show-changelog | not named `history.rs`, which already exists |
| `query.rs` | search, file-search, info | read-only |
| `find_mirror.rs` | find-mirror | not named `mirrors.rs`, which already exists |
| `revert_pkg.rs` | revert-pkg | dangerous: GPG verify -> `upgradepkg --reinstall` -> offer to freeze |
| `clean.rs` | clean-system, clean-cache | dangerous: removes packages |
| `new_config.rs` | new-config | writes into `/etc`. `PROTECTED_CONFIGS` (passwd, shadow, group, rc.inet1.conf, rc.local, rc.local_shutdown) are never replaced by the menu or by a glob. The `NEWCONFIG_*` rules (`preset_for`, `apply_presets`, `glob_match`) run before the menu; `preset_key` names the rule for a file, for the list the package commands print. |
| `update.rs` | update, update gpg | the per-repo processing (`update_one_repo`) is in the core |
| `status.rs` | status, list-repos, check-updates | `status` is the setup doctor |
| `upgrade_dist.rs` | upgrade-dist | the most dangerous one. Phases: escape kit -> transform -> update -> core (serial, with a package-database check) -> install-new -> the rest in batches -> clean-system -> second pass -> new-config -> status. |
| `install.rs` | install, upgrade, reinstall, remove, download, upgrade-all, install-new | the resolver they use is in the core. `report_pending_configs` (also called by install-template and revert-pkg) lists the `.new` files a run left and, with `NEWCONFIG_AFTER`, asks to run new-config or runs it (`after_configs`). `pending_configs` takes the `.new` files and their stamps before the packages are applied; `is_fresh_config` tells which ones this run wrote. |

### 5.3 The shared core (stays in `main.rs`, on purpose)

Used by several groups at once (package commands, upgrade-dist, install-template, vet, update), so it belongs to no single command file. **It was decided not to move it** (see 8.4).

- **Dependency resolver:** `expand_with_deps`, `add_with_deps`, `resolve_protected_deps`, `ask_dep_conflict`, `ask_protected_dep`, `quarantine_deep_repo`, **`MAX_DEP_DEPTH = 512`**, `DEEP_CHAIN_ERR`. The DoS fix lives here (see 7).
- **Install order:** `upgrade_order` (called at the end of `expand_with_deps`, so the plan table shows the order packages are installed in), `UPGRADE_FIRST`, `upgrade_last` (also used by upgrade-dist).
- **Plans and tables:** `PlanItem`, `InstallAction`, `PlanRow`, `print_table`, `show_plan`, `print_plan`, `report_pinned_in_plan`, `hint_freeze_pin`, `note_optional_suggests`, conflicts (`detect_conflicts`, `report_conflicts`, `confirm_conflicts`).
- **Execution:** `execute_plan`, `install_counter` (`[ 37/245 ] upgrading ...`), `parallel_fetch` (with the live download line: `live_line`, `DlLive`, `term_width`, `DL_TICK`, `DL_STALL`, `DL_ANNOUNCE`), `fetch_and_verify`/`fetch_and_verify_metered`, `package_dest`, `DlItem`, `summarize_outcomes`, `report_batch_failures`, `enforce_release_match`, `enforce_arch_match`, kernel and perl reminders.
- **Package selection:** `collect`, `collect_prefers`, `validate_selector`, `reject_mixed_selectors`, `guard_shell_expansion`, `closest`, `edit_distance`, `at_prefix_hint`, `parse_selection`, `bl_installed`, `priority_display`, `installed_source_label`.
- **Repos:** `update_one_repo`, `UpdateOutcomes`, `malicious_filename_count`, `vet_repo`, `apply_vet`, `reload_repo`, `warn_unverified_repos`, `unverified_repo_names`.
- **Release/arch/mirrors:** `slackware_dir`, `is_current_codename`, `repo_release_token`, `system_release`, `arch_family`, `arch_compatible`, `is_x86_arch`, `osuosl_tree_prefix`, `parse_packages_date`, `mirror_is_stale`, `humanize_lag`, `FRESHNESS_*`.
- **Misc:** `confirm`, `hilite_keys`, `is_foundational`, `in_graphical_session`, `migrate_state`.

### 5.4 Helper modules (`src/*.rs`, other than `main`/`cli`/`commands`)

Mostly pure logic and system integration. They know nothing about commands.

| file | contents |
|---|---|
| `config.rs` | loading/validating `slacker.conf`, `repos`, `mirrors`, `blacklist`, `distro-upgrade.conf`; blacklist rules, pins (`@repo 100% name`) and dependency exclusions (`!package:dependency`); the lenient readers `banner_enabled`, `system_arch`, `newconfig_rules`, `proxy_settings`; `NEWCONFIG_AFTER` (`parse_newconfig_after`) |
| `pkgdb.rs` | unified database of available packages, priority resolution, freeze/pin, matching |
| `repo.rs` | fetch/parse PACKAGES.TXT, CHECKSUMS, `.dep` (read once per run and cached; `prefetch_deps` reads many in parallel before the resolver); per-repo quarantine/trust; safe-filename checks |
| `pkg.rs` | package identity (`name-version-arch-build`), build tags |
| `system.rs` | the live system: installed packages, os-release, pkgtools calls (argv only, never `sh -c`; `--terse` first when `TERSE=on`) |
| `download.rs` | downloads (http/https/file), size limits, md5/sha256, HTTP auth; `Meter`, the byte count of a running download that `parallel_fetch` shows; the proxy (`set_proxy`, `proxy_var`, `proxy_for`, `no_proxy_matches`, `describe_proxy` for `status`) |
| `gpg.rs` | private keyring, TOFU pinning of every fingerprint, signature checks |
| `credentials.rs` | HTTP Basic (zypper-style): `credentials.d/`, `credentials.cat`, root:0600 only, https only |
| `changelog.rs` | ChangeLog.txt, check-updates |
| `history.rs` | the package change log (pure) |
| `revert.rs` | pure helpers for revert-pkg |
| `newconfig.rs` | finding `*.new` under `/etc` and `/usr/share/vim` |
| `mirrors.rs` | mirror discovery and probing |
| `dist.rs` | upgrade-dist direction gate (fail-closed whitelist: only 15.0 -> -current and 15.0 -> a newer stable) |
| `manifest.rs` | file-search via MANIFEST |
| `template.rs` | templates of installed packages |
| `stockdb.rs` | `resolve-stock`: SQLite dependency database for stock -current |
| `ui.rs` | ANSI colours; honours `NO_COLOR` and non-terminal output |
| `banner.rs`, `banner2.rs` | banners |

---

## 6. Where new code goes

- **New command:** a variant in `Cmd` (`cli.rs`), a `pub(crate) fn cmd_x` in the right `commands/<area>.rs` (or a new file declared in `commands.rs`), a line in the `run()` dispatch, and in `command_name` (the compiler requires it). If the command modifies the system or the cache, also in `requires_privilege`.
- **Helper used by one command group only:** in that group's file.
- **Helper used by several groups:** in the core in `main.rs`. `commands/` modules reach it with `use crate::name` (private items of the crate root are visible to its descendant modules).
- **Pure logic with no command I/O:** in a helper module (`src/*.rs`), with its unit tests there.
- **File names:** must not collide with the helper modules. `history.rs`, `revert.rs`, `newconfig.rs`, `mirrors.rs`, `dist.rs`, `template.rs` already exist, which is why the commands live in `logs.rs`, `revert_pkg.rs`, `new_config.rs`, `find_mirror.rs`, `upgrade_dist.rs`, `templates.rs`.
- **Visibility:** `pub(crate)` only, and only where needed (the `cmd_*`, and helpers a test in another module needs).

---

## 7. Invariants (do not change without an explicit decision by the maintainer)

### Security

- **pkgtools exactly as Patrick ships them.** We do not modify them or parse their output. The only option slacker ever adds of its own is `--terse`, when the user sets `TERSE=on`. The safety net is feeding them only verified packages and, in upgrade-dist, checking the result (the package-database record) for the core packages.
- **Dependency depth cap `MAX_DEP_DEPTH = 512`.** A repo that exceeds it is quarantined and the run (e.g. upgrade-all) carries on with the rest. Under `--dry-run` it only warns. Dependency cycles are cut via `visiting`. Confirmed in real use.
- **GPG TOFU:** every update re-fetches GPG-KEY and pins every fingerprint (subkeys included). A changed key -> hard quarantine. Deliberately strict.
- **Trust anchors in STATE_DIR**, never in the cache.
- **Path traversal:** `is_safe_filename` / `is_safe_location`. A repo advertising a suspicious package name -> hard quarantine.
- **Size limits:** metadata 512 MiB, package 8 GiB.
- **Never `sh -c`.** Every external call uses argv.
- **Never write through a symlink** for config files and download destinations.
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
- **Install order follows UPGRADE.TXT.** Every plan (`expand_with_deps` -> `upgrade_order`) puts `aaa_glibc-solibs`, `pkgtools`, `tar`, `xz`, `findutils`, `mkinitrd` first, in that order, and `kernel-generic` (or `kernel_armvN`) last; everything else keeps the resolver's order (alphabetical, each dependency before what needs it). The kernel is last because its doinst runs `geninitrd` (mkinitrd or dracut) and DKMS builds its modules then, so kmod, udev, lvm2 and the rest must already be upgraded; slackpkg 16.0.1 does the same. `kernel-source` is **not** moved: installed before `kernel-generic`, its tree is there when DKMS builds for the new kernel, so the build succeeds the first time (checked on a real upgrade with NVIDIA, Nov 2026).
- **The official tree and its subtrees are not asked for `.dep` files.** They ship none, and asking meant one 404 per package. `PACKAGE REQUIRED` from PACKAGES.TXT is still read for every repo. Third-party repos are asked as before.
- **Freeze:** installed + rule -> FROZEN. Not installed + rule -> hidden from install-new/upgrades/check-updates, visible in search/info. A frozen candidate is treated as absent and resolution continues with the remaining candidates.
- **Pin:** `@repo 100% name`, exact name, never freezes. A real freeze on the same name wins.
- **install-new:** `cmd_install_new` and `dist_install_new` are intentionally different (normal: official + subtrees ranked above official; dist: official + patches only, regardless of priority).
- **upgrade-dist:** requires an explicit target; deliberately bypasses the blacklist and priority; empties the blacklist and keeps `blacklist.bak` (never overwritten on a re-run); disables third-party repos; installs `kernel-generic` after everything else and the GnuPG chain last of all; clean-system stays in Phase 3, after the upgrade.
- **clean-system:** pinned packages installed from the pin's repo get a separate R/K/P prompt (default Keep). With `-y` they are kept. It never proposes removing slacker itself.
- **new-config:** protected files are never replaced, whatever the user answers in the menu. Only a manual merge, or the user naming the exact path in `NEWCONFIG_OVERWRITE`; a glob there never matches them.
- **new-config rules** (`NEWCONFIG_OVERWRITE` / `_REMOVE` / `_KEEP` in slacker.conf, paths or globs of the installed file, `*` and `?` not crossing `/`): applied before the menu, with or without `--yes`. In upgrade-dist **only `NEWCONFIG_KEEP`** applies, so a file the user keeps is never replaced there, not even by "[A]ll new"; OVERWRITE and REMOVE are not applied there: as with the blacklist, upgrade-dist shows every other file (it prints a note when such rules exist). These keys add up when repeated (every other key keeps its last line). When several match, the safer wins: KEEP over REMOVE over OVERWRITE. OVERWRITE keeps the old file as `.orig`; KEEP renames the `.new` to `.new.kept`, so nothing is left pending and nothing is lost (issue by gegechris99, Nov 2026).
- **new-config after a package command** (`NEWCONFIG_AFTER`, default `off`): after install, reinstall, upgrade, upgrade-all, install-new, install-template and revert-pkg, the `.new` files are listed (fresh ones apart from older pending ones, each with its `NEWCONFIG_*` key if a rule covers it) and the reminder is printed, as before. `ask` asks "Run new-config now? [Y/n]" and `run` runs it, but only when stdin and stdout are both a terminal, without `--yes` or `--dry-run`, and only if this run left new `.new` files; otherwise the reminder. A `.new` is this run's when it was not there before, or was and has been written again (a reinstall, or an upgrade shipping a newer `.new` over a pending one): slacker compares each file's inode, change time, size and modification time before and after. The modification time and size alone are not enough, because tar gives a `.new` the modification time stored in the package, so a reinstall rewrites it with the same one; `installpkg` unpacks with `tar --unlink-first`, so the inode and change time do move. A stamp that cannot be read never makes a file new. All of this is held in memory for the one command: **slacker keeps no state of its own between runs** and reads package state only from pkgtools (issue #19, Nov 2026). End of input at the question means no. cron, scripts, slacker-gui and `docker build` therefore never have `.new` files touched. The `NEWCONFIG_*` rules still apply first (request by gegechris99, Nov 2026).
- **Download progress:** on a terminal, `parallel_fetch` redraws one line with every running download and says `no data for Ns` after 15 s without a byte; the width is read from the terminal (TIOCGWINSZ) on every redraw. Off a terminal (pipe, log, slacker-gui) there is no `\r`: only a plain `↓ name (size)` line for a download still running after 1 s, so logs do not depend on timing for small packages (issue #14).
- **Output:** exit codes and output format do not change without a reason. `slacker-gui` and scripts may depend on them.

### Rejected (do not propose again)

- A mandb freshness check in `status`.
- Moving clean-system before the upgrade in upgrade-dist.

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
| 13 | `commands/upgrade_dist.rs` | upgrade-dist and its 19 own items, with 4 tests. `dist_upgrade_sets` is `pub(crate)` for 2 tests in `collect_tests`. |
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

0.14.0 added, each with its tests: "Preparing the plan" with the parallel, cached `.dep` reading, and the official tree no longer asked for `.dep`; `TERSE` and the install counter; the install order of UPGRADE.TXT with the kernel last, in every plan and in upgrade-dist; the `NEWCONFIG_*` rules of new-config; `NEWCONFIG_AFTER`, and `NEWCONFIG_KEEP` in upgrade-dist; and the proxy (`PROXY`/`NO_PROXY`, off by default, with a `Proxy` line in `status`) (all see 7). Every new key defaults to the behaviour from before it existed. After any change: `cargo build --release --locked` with no warnings, `cargo test --release --locked`, and the checks the SlackBuild runs (`contrib/check-completion-drift.sh`, `contrib/gen-completions.py --check`, `contrib/check-yes-noninteractive.sh`, `cargo audit`). Before a release: a run on a real system by the maintainer.

---

## 9. Tests

- `cargo test`: **249 passed, 1 ignored** (250 in total).
- Per file: `main.rs` 50, `config.rs` 37, `pkgdb.rs` 34, `repo.rs` 12, `revert.rs` 11, `commands/blacklist.rs` 11, `pkg.rs` 10, `dist.rs` 10, `commands/status.rs` 10, `stockdb.rs` 9, `download.rs` 8 (one ignored: a live TLS handshake), `mirrors.rs` 7, `history.rs` 6, `credentials.rs` 5, `commands/install.rs` 6, `system.rs` 5, `commands/upgrade_dist.rs` 4, `commands/new_config.rs` 4, `template.rs` 3, `manifest.rs` 2, `ui.rs` 2, and one each in `newconfig.rs`, `commands/clean.rs`, `commands/query.rs`, `commands/repos.rs`.
- Test modules in `main.rs`: `parallel_download_tests`, `arch_tests`, `selection_tests`, `collect_tests` (with the `av()` and `fixture()` fixtures), `freshness_tests`, `foundational_tests`.

---

## 10. Open issues (outside the refactor)

- **Two `arch_compatible` functions with opposite argument order:** `main.rs` `(sys_arch, pkg_arch)` and `repo.rs` `(pkg_arch, arch)`. They do the same thing. Not a bug today, but a trap. Worth unifying as a separate change.
- **`history` dates after a restore (NOT fixed, on purpose):** the timezone is derived by comparing the time in the `-upgraded-<T>` name with the file's ctime. ctime is lost when restoring from a backup, and then the dates shift. A simple "ignore unrealistic timezones" does NOT fix it. Display only, and only on restored systems. Needs its own design if it is ever done.
- **Misplaced doc comments** (rustdoc only, no functional effect): a paragraph belonging to `warn_unverified_repos` above `unverified_repo_names` (`main.rs`); the `cmd_status` doc above `MLOCATE_DB` (`commands/status.rs`); a "Parse a keep-selection" above `select_packages_pkgid` (`commands/install.rs`); a "GATE ONLY for now" above `DIST_CRITICAL` (`commands/upgrade_dist.rs`). Left as they were to keep the "verbatim text" proof. Fix them as a separate cosmetic change.
- **The proxy and redirects:** `no_proxy` is checked for the URL slacker asks for, not again for a redirect target; a redirect takes the same route (proxy or direct). Nothing leaks; at worst the redirect fails.
- **slacker-gui and the proxy:** pkexec clears the environment, so with `PROXY=env` the GUI downloads directly. A URL in `PROXY` works there.
- **Package `.asc` files stay in the cache** after install/upgrade-dist (the package is deleted, the signature is not). Seen in testing. They are small and `clean-cache` removes them. Whether this should change has not been evaluated.

---

## 11. Patches to Slackware/Patrick

Anything going upstream to Slackware must be: plain `git diff`, no long commit messages, short comments in the style of the existing file.
