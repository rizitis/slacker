# slacker: code structure, rules and refactor history

Last updated: 29 Oct 2026. Covers `slacker-cli` after the refactor (steps 1–14). Maintainer: Ioannis Anagnostakis (r-tz, `rizitis`).

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
- **MSRV:** `Cargo.toml` says `1.85.1`, but the `icu_*` crates (pulled in via ureq -> url -> idna) require **1.88**, and have since icu 2.3. The correct value is `rust-version = "1.88"`.
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

1. **Commands that run before the config is loaded:** `status`, `find-mirror`, `new-config`. They must work even when the config is broken: `status` is what diagnoses it, and `new-config` is what repairs it. `new-config` still checks privileges and takes the lock.
2. `Config::load_dir()`. Strict: an error in `repos`/`mirrors` stops here.
3. `migrate_state()`: moves old trust anchors from the cache to STATE_DIR, once, never overwriting existing ones.
4. `download::set_auth(credentials::build_registry(...))`: HTTP Basic credentials, sent over https only.
5. For commands that modify the system or the cache (`requires_privilege`): root is required (membership in `wheel` only changes the error message), plus the exclusive lock. Read-only commands also run as a normal user.
6. Dispatch: one `match` that calls the command's `cmd_*`.

**Exit codes** (`enum Outcome`): `0` Ok, `20` NothingFound, `50` SelfUpgrade (slacker upgraded itself, run it again), `100` Pending (check-updates found updates), `1` error. `slacker-gui` and scripts may depend on these: **they do not change**.

Global flags (all commands): `--config-dir`, `-y/--yes`, `--dry-run`, `--no-deps`.

---

## 5. Code map (`slacker-cli/src/`)

### 5.1 Entry point and CLI definition

| file | lines | contents |
|---|---|---|
| `main.rs` | 3,922 | entry point (`main`, `run`, `Outcome`, lock, privilege checks, `migrate_state`) **and the shared core** (see 5.3), with its tests |
| `cli.rs` | 255 | `Cli`, `Cmd` (every command, clap derive), `HELP_STYLES`. The first paragraph of each doc comment is the short help line; the whole doc comment is `<cmd> --help`. |
| `commands.rs` | 15 | declares the `commands/` modules |

### 5.2 Commands (`src/commands/`)

Each file holds the `pub(crate) fn cmd_*` of one group of commands, plus the helpers used **only** by that group.

| file | commands | notes |
|---|---|---|
| `templates.rs` | generate/install/remove/delete-template | |
| `repos.rs` | add-repo, del-repo, pri-repo, add-tag, del-tag, vet-repo, trust-repo, distrust-repo | the vetting itself (`vet_repo`, `apply_vet`, `reload_repo`) is in the core |
| `blacklist.rs` | frozen, unfrozen, pin, unpin | honour `--dry-run` (fixed in 0.12.5) |
| `logs.rs` | history, show-changelog | not named `history.rs`, which already exists |
| `query.rs` | search, file-search, info | read-only |
| `find_mirror.rs` | find-mirror | not named `mirrors.rs`, which already exists |
| `revert_pkg.rs` | revert-pkg | dangerous: GPG verify -> `upgradepkg --reinstall` -> offer to freeze |
| `clean.rs` | clean-system, clean-cache | dangerous: removes packages |
| `new_config.rs` | new-config | writes into `/etc`. `PROTECTED_CONFIGS` (passwd, shadow, group, rc.inet1.conf, rc.local, rc.local_shutdown) are never replaced. |
| `update.rs` | update, update gpg | the per-repo processing (`update_one_repo`) is in the core |
| `status.rs` | status, list-repos, check-updates | `status` is the setup doctor |
| `upgrade_dist.rs` | upgrade-dist | the most dangerous one. Phases: escape kit -> transform -> update -> core (serial, with a package-database check) -> install-new -> the rest in batches -> clean-system -> second pass -> new-config -> status. |
| `install.rs` | install, upgrade, reinstall, remove, download, upgrade-all, install-new | the resolver they use is in the core |

### 5.3 The shared core (stays in `main.rs`, on purpose)

Used by several groups at once (package commands, upgrade-dist, install-template, vet, update), so it belongs to no single command file. **It was decided not to move it** (see 8.4).

- **Dependency resolver:** `expand_with_deps`, `add_with_deps`, `resolve_protected_deps`, `ask_dep_conflict`, `ask_protected_dep`, `quarantine_deep_repo`, **`MAX_DEP_DEPTH = 512`**, `DEEP_CHAIN_ERR`. The DoS fix lives here (see 7).
- **Plans and tables:** `PlanItem`, `InstallAction`, `PlanRow`, `print_table`, `show_plan`, `print_plan`, `report_pinned_in_plan`, `hint_freeze_pin`, `note_optional_suggests`, conflicts (`detect_conflicts`, `report_conflicts`, `confirm_conflicts`).
- **Execution:** `execute_plan`, `parallel_fetch`, `fetch_and_verify`, `package_dest`, `DlItem`, `summarize_outcomes`, `report_batch_failures`, `enforce_release_match`, `enforce_arch_match`, kernel and perl reminders.
- **Package selection:** `collect`, `collect_prefers`, `validate_selector`, `reject_mixed_selectors`, `guard_shell_expansion`, `closest`, `edit_distance`, `at_prefix_hint`, `parse_selection`, `bl_installed`, `priority_display`, `installed_source_label`.
- **Repos:** `update_one_repo`, `UpdateOutcomes`, `malicious_filename_count`, `vet_repo`, `apply_vet`, `reload_repo`, `warn_unverified_repos`, `unverified_repo_names`.
- **Release/arch/mirrors:** `slackware_dir`, `is_current_codename`, `repo_release_token`, `system_release`, `arch_family`, `arch_compatible`, `is_x86_arch`, `osuosl_tree_prefix`, `parse_packages_date`, `mirror_is_stale`, `humanize_lag`, `FRESHNESS_*`.
- **Misc:** `confirm`, `hilite_keys`, `is_foundational`, `in_graphical_session`, `migrate_state`.

### 5.4 Helper modules (`src/*.rs`, other than `main`/`cli`/`commands`)

Mostly pure logic and system integration. They know nothing about commands.

| file | contents |
|---|---|
| `config.rs` | loading/validating `slacker.conf`, `repos`, `mirrors`, `blacklist`, `distro-upgrade.conf`; blacklist rules and pins (`@repo 100% name`) |
| `pkgdb.rs` | unified database of available packages, priority resolution, freeze/pin, matching |
| `repo.rs` | fetch/parse PACKAGES.TXT, CHECKSUMS, `.dep`; per-repo quarantine/trust; safe-filename checks |
| `pkg.rs` | package identity (`name-version-arch-build`), build tags |
| `system.rs` | the live system: installed packages, os-release, pkgtools calls (argv only, never `sh -c`) |
| `download.rs` | downloads (http/https/file), size limits, md5/sha256, HTTP auth |
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

- **pkgtools exactly as Patrick ships them.** We do not modify them or parse their output. The safety net is feeding them only verified packages and, in upgrade-dist, checking the result (the package-database record) for the core packages.
- **Dependency depth cap `MAX_DEP_DEPTH = 512`.** A repo that exceeds it is quarantined and the run (e.g. upgrade-all) carries on with the rest. Under `--dry-run` it only warns. Dependency cycles are cut via `visiting`. Confirmed in real use.
- **GPG TOFU:** every update re-fetches GPG-KEY and pins every fingerprint (subkeys included). A changed key -> hard quarantine. Deliberately strict.
- **Trust anchors in STATE_DIR**, never in the cache.
- **Path traversal:** `is_safe_filename` / `is_safe_location`. A repo advertising a suspicious package name -> hard quarantine.
- **Size limits:** metadata 512 MiB, package 8 GiB.
- **Never `sh -c`.** Every external call uses argv.
- **Never write through a symlink** for config files and download destinations.
- **Credentials over https only**, files root:0600 only.

### Semantics (settled, do not reopen)

- **Priority above all:** never silently migrate or downgrade. Only an explicit `repo:name` or a pin overrides it.
- **Freeze:** installed + rule -> FROZEN. Not installed + rule -> hidden from install-new/upgrades/check-updates, visible in search/info. A frozen candidate is treated as absent and resolution continues with the remaining candidates.
- **Pin:** `@repo 100% name`, exact name, never freezes. A real freeze on the same name wins.
- **install-new:** `cmd_install_new` and `dist_install_new` are intentionally different (normal: official + subtrees ranked above official; dist: official + patches only, regardless of priority).
- **upgrade-dist:** requires an explicit target; deliberately bypasses the blacklist and priority; empties the blacklist and keeps `blacklist.bak` (never overwritten on a re-run); disables third-party repos; installs the GnuPG chain last; clean-system stays in Phase 3, after the upgrade.
- **clean-system:** pinned packages installed from the pin's repo get a separate R/K/P prompt (default Keep). With `-y` they are kept. It never proposes removing slacker itself.
- **new-config:** protected files are never replaced, whatever the user answers. Only a manual merge.
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

The scripts are in `slacker-refactor-harness.tar.gz` (kept by r-tz, not part of the repo). They need root, `unshare`, `gpg` and `python3`. Their directory is hard-coded in the scripts (the `SP` variable), so it has to be changed on another machine. `check_all.sh` runs from `slacker-cli/` and does everything in one go.

### 8.4 What was deliberately not done

- **The shared core was not moved** (the planned step 15: `plan.rs`, `execute.rs`, `select.rs`, `prompt.rs`). The resolver with `MAX_DEP_DEPTH`, execution, fetch/verify and package selection stay where they were tested on a real system. The only gain would have been organisation. The same goes for `collect_tests` (about 550 lines with shared fixtures).
- **The entry point stays in `main.rs`:** `main`, `run`, `Outcome`, lock, privilege checks.

### 8.5 Sync rule

Until the refactor is released, **every change in the normal branch must also go into the refactor** (as was done with the dry-run fix). Otherwise the "refactor == normal" proof no longer holds. After any such change, `check_all.sh` and the comparison with the normal binary run again. Before a release: a run on a real system by the maintainer.

---

## 9. Tests

- `cargo test`: **219 passed, 1 ignored** (220 in total).
- Per file: `main.rs` 45, `config.rs` 30, `pkgdb.rs` 29, `repo.rs` 12, `revert.rs` 11, `pkg.rs` 10, `dist.rs` 10, `commands/status.rs` 10, `stockdb.rs` 9, `commands/blacklist.rs` 8, `mirrors.rs` 7, `history.rs` 6, `credentials.rs` 5, `download.rs` 4, `system.rs` 4, `commands/install.rs` 4, `commands/upgrade_dist.rs` 4, `template.rs` 3, `manifest.rs` 2, `ui.rs` 2, and one each in `newconfig.rs`, `commands/clean.rs`, `commands/new_config.rs`, `commands/query.rs`, `commands/repos.rs`.
- Test modules in `main.rs`: `parallel_download_tests`, `arch_tests`, `selection_tests`, `collect_tests` (with the `av()` and `fixture()` fixtures), `freshness_tests`, `foundational_tests`.

---

## 10. Open issues (outside the refactor)

- **Two `arch_compatible` functions with opposite argument order:** `main.rs` `(sys_arch, pkg_arch)` and `repo.rs` `(pkg_arch, arch)`. They do the same thing. Not a bug today, but a trap. Worth unifying as a separate change.
- **`history` dates after a restore (NOT fixed, on purpose):** the timezone is derived by comparing the time in the `-upgraded-<T>` name with the file's ctime. ctime is lost when restoring from a backup, and then the dates shift. A simple "ignore unrealistic timezones" does NOT fix it. Display only, and only on restored systems. Needs its own design if it is ever done.
- **MSRV:** see 2 (`rust-version = "1.88"`).
- **Misplaced doc comments** (rustdoc only, no functional effect): a paragraph belonging to `warn_unverified_repos` above `unverified_repo_names` (`main.rs`); the `cmd_status` doc above `MLOCATE_DB` (`commands/status.rs`); a "Parse a keep-selection" above `select_packages_pkgid` (`commands/install.rs`); a "GATE ONLY for now" above `DIST_CRITICAL` (`commands/upgrade_dist.rs`). Left as they were to keep the "verbatim text" proof. Fix them as a separate cosmetic change.
- **Package `.asc` files stay in the cache** after install/upgrade-dist (the package is deleted, the signature is not). Seen in testing. They are small and `clean-cache` removes them. Whether this should change has not been evaluated.

---

## 11. Patches to Slackware/Patrick

Anything going upstream to Slackware must be: plain `git diff`, no long commit messages, short comments in the style of the existing file.
