# slacker - project structure

`slacker` = slackpkg + slackpkg+ in one minimal Rust tool. Full slackpkg action
parity, plus slackpkg+ multi-repo priority resolution and .dep dependency
handling.

Build needs Rust 1.85.1+ (the effective MSRV; current Slackware ships 1.96). The
crate itself is edition 2021 for broad compatibility, but a dependency (clap_lex
1.1.0) is written in edition 2024 — that is what sets the 1.85.1 floor, not any
slacker code. Direct deps: clap, ureq, native-tls, md-5, regex. Heavy lifting
(bzip2 for MANIFEST, GPG) shells out to the system tools Slackware already ships -
no extra Rust deps.

### Source_Tree
```
slacker/
├── Cargo.toml                      <- build manifest (deps: clap, ureq, native-tls, md-5, regex; Apache-2.0)
├── README.md
├── slacker.8                       <- man page (section 8)
├── examples/etc-slacker/           <- config templates for /etc/slacker/
│   ├── slacker.conf                <- globals (ARCH auto-detect, ADM_DIR, CACHE_DIR, PKG_DB_DIR, RESOLVE_DEPS, IGNORE_TAGS, VERIFY)
│   ├── mirrors                     <- catalogue of official mirrors - uncomment ONE (none by default)
│   ├── repos                       <- binary repos + tag-priority lines
│   └── blacklist                   <- blacklist rules: [@repo] REGEX | [@repo] series/
└── src/                            <- 14 modules
    ├── main.rs        CLI + commands (31 actions, exit codes, prompts, dry-run, dep resolution, @-selectors, multi-match selection, repo/tag management, quarantine, history)
    ├── config.rs      plain-text config + arch auto-detect + ADM_DIR/PKG_DB_DIR + tag-priorities + VerifyPolicy/Check + blacklist rules (regex/@repo/series) + repo flags (official/immutable/subtree) + subtree download base + mirror/<subpath> URLs
    ├── pkg.rs         Slackware package-name splitting (name-version-arch-build) + build_tag()
    ├── repo.rs        PACKAGES.TXT/CHECKSUMS(.md5/.sha256) parsing (UTF-8-lossy), metadata fetch, series, arch filter, lazy MANIFEST, .dep fetch, quarantine/trust markers
    ├── pkgdb.rs       unified DB, priority, pattern/series/@-matching, upgrade resolution, newly-added, orphans, baseline names (clean-system), blacklist source lookups
    ├── download.rs    https/http (ureq+native-tls) + file:// + md5 + sha256 (sha256sum)
    ├── system.rs      installed DB (PKG_DB_DIR) + pkgtools wrappers (install/upgrade/reinstall/remove) + cached_pkg_path
    ├── history.rs     package-change timeline reconstructed from the pkgtools admin dirs (ADM_DIR: packages/ + removed_packages/), local-time calibration, upgrade/reinstall inference
    ├── manifest.rs    file-search (decompressed MANIFEST)
    ├── changelog.rs   check-updates / show-changelog (pager when on a TTY)
    ├── gpg.rs         GPG import + TOFU key pinning + verify (captured output, fail-closed)
    ├── template.rs    templates (generate/load/delete, includes)
    ├── newconfig.rs   .new config file handling
    └── ui.rs           minimal ANSI colouring (TTY + NO_COLOR aware), plan tables
```

### Config_Model

- **slacker.conf** - `KEY=value`. ARCH is auto-detected from the installed
  `aaa_base` package (override only for cross). ADM_DIR (default `/var/adm`) is
  the Slackware pkgtools admin root (holds `packages/`, `removed_packages/`,
  `scripts/`, `setup/`); `history` reads it. CACHE_DIR (default
  `/var/cache/slacker`). PKG_DB_DIR defaults to `ADM_DIR/packages`; set it
  explicitly only to override that (kept for back-compat). RESOLVE_DEPS (default
  yes), VERIFY (default all), IGNORE_TAGS (build tags that `clean-system` treats
  as non-foreign, e.g. `_SBo cf alien`).
- **mirrors** - slackpkg-style catalogue; uncomment exactly ONE (none by
  default; 2+ -> error). Holds the official mirror URL (current/15.0 × 64/32,
  http/https/file://).
- **repos** - two kinds of line:
  - binary repo: `priority  name  url|mirror|mirror/<subpath>  [official]
    [immutable] [subtree] [verify=...]`. The url field may be a literal URL, the
    keyword `mirror` (filled from the active line in `mirrors`), or
    `mirror/<subpath>` (the active mirror with a subpath appended, e.g.
    `mirror/extra`, `mirror/testing` - so a distribution subtree tracks whichever
    mirror you picked without hardcoding the host). Higher priority wins;
    `name:package` pins a repo. Binary-repo priorities must be **distinct**
    (duplicate -> fail-fast error). Flags: `official` (tracked repo: ChangeLog +
    install-new default), `immutable` (its packages never treated as foreign by
    clean-system), `subtree` (a Slackware distribution subtree - extra/, patches/,
    testing/, pasture/ - whose PACKAGES.TXT locations are root-relative, so
    packages and GPG-KEY are fetched from the parent/root URL while metadata comes
    from the repo URL).
  - tag-priority: `priority  name  tag` (e.g. `100 SBo _SBo`). Gives packages
    carrying a build tag a priority on the same scale, so SBo/local/source
    packages are never silently migrated to another repo or downgraded by
    `upgrade-all`. Tag-priority lines may share priority values.
- **blacklist** - one rule per line: `[@repo] PATTERN`. `PATTERN` is a
  Slackware series when it ends in `/` (e.g. `kde/`), otherwise an unanchored
  **regex** matched against the full package id `name-version-arch-build`
  (slackpkg-style, so `xf86-.*-202.*` works; anchor with `^...$` for exact). An
  optional `@repo` scopes the rule to one repo (for an available package its
  candidate repo, for an installed one its source). An installed match is
  **frozen** (never installed/upgraded/reinstalled/removed, and never listed by
  `clean-system`); an uninstalled match is **hidden** from `install-new`,
  upgrades and `check-updates`, but still shown by `search`/`info` marked
  `[blacklisted]`. The `frozen` command validates and appends rules to this
  file.

### Build-tag priority model

A package's *build tag* is its build field minus the leading digits
(`1_SBo`->`_SBo`, `7cf`->`cf`, `1`->``). `upgrade-all` decides each installed
package's "owning" priority by: (1) a user tag-priority line, else (2) the
highest-priority binary repo whose packages carry that tag (cf->conraid,
alien->alienbob auto-detected), else (3) official priority for an empty tag, else
(4) protect (never touch). A candidate is proposed only from a repo of higher or
equal priority - no cross-repo migration, no downgrade. Giving a repo a
distinct, high priority therefore *locks* its packages: nothing from a lower
repo can replace them, even with a newer version.

### Selectors and matching

A PATTERN may be:
- an exact package name or a name substring (`python` -> all `python-*`),
- a Slackware series (`a`, `ap`, `kde`, `y`, ...) - matches that series only,
  never substrings,
- a `repo:name` pin forcing one repo regardless of priority,
- a set selector (the `@` is required, so a bare word is never a repo):
  - `@repo` - every package in that repo (`install @gnome`, `remove @gnome`),
  - `@_tag` - every package with that build tag (`remove @_SBo`).
An unknown `@repo`/`@tag` gives a helpful error with a "did you mean" suggestion
(edit-distance) and lists the available repos and tags. When a pattern matches
more than one package, install/upgrade/reinstall/remove show a numbered list
(Enter = all, numbers/ranges like `1 3 5` or `2-4`, `n` = cancel).

### Actions (31; slackpkg parity + extras)

```
update [gpg]    search        file-search   info        list-repos
status          install       upgrade       reinstall    remove
download        clean-cache   upgrade-all   install-new  clean-system
frozen          new-config    check-updates show-changelog  history
add-repo        del-repo      add-tag       del-tag
vet-repo        trust-repo    distrust-repo
generate-template  install-template  remove-template  delete-template
```
- `list-repos` / `status` - inspect repos (priority, verify, flags, installed
  counts) and health-check the whole setup. An installed package's build tag is
  treated as a legitimate source, never "untracked": `list-repos` shows an `Inst`
  count per repo and per declared tag-priority rule, flags a declared rule with
  zero installed packages as `(declared, no installed package)`, and groups any
  remaining tags under "Installed under other build tags". `status`'s by-source
  view lists repos plus declared tag-rules plus remaining raw tags, with no
  "untracked" bucket.
- `history [NAME]` - a newest-first timeline of every package change on the box
  (install / upgrade / reinstall / remove), reconstructed from the pkgtools admin
  dirs under ADM_DIR (`packages/` + `removed_packages/`), so it captures changes
  made by any tool (slacker, slackpkg, sbopkg, installpkg, ...), not only
  slacker's own. Each row shows the local date, the change, the package, the
  version (upgrades read `old -> new`), and the attributed source repo/tag. When
  an upgrade target's record was lost to a `removed_packages` name collision, the
  version is inferred from the next known tenure of that package. Filters:
  `--installed` (only currently-installed, with install date), `--removed`
  (left the system), `--upgraded` (upgrade/reinstall events), `--last N`,
  `--since YYYY-MM-DD`. Paged on a TTY like `show-changelog`.
- `add-repo`/`del-repo`/`add-tag`/`del-tag` - edit the `repos` file (validated,
  with confirmation). `add-repo` flags: `official`, `immutable`, `subtree`,
  `verify=...`.
- `vet-repo`/`trust-repo`/`distrust-repo` - the quarantine model: re-vet a repo,
  lift a quarantine (override the verdict), or freeze a repo yourself.
- `install-new [REPO...]` - official repos only by default; name repos to opt in.
- `download [-o DIR] PATTERN...` - saves to CACHE_DIR/packages/<repo>/ by default,
  or to DIR; confirms before bulk (>10) downloads; refuses to write through a
  pre-existing symlink.
- `clean-cache [REPO...]` - deletes downloaded *.txz only; never touches repo
  metadata or GPG keys under CACHE_DIR/repos.
- `remove-template` uninstalls a template's packages (slackpkg behaviour);
  `delete-template` removes only the template file.
- `frozen RULE...` - add one or more blacklist rules. Each argument is one
  rule (quote rules with spaces, e.g. `"@alienbob vlc"`); slacker validates them,
  warns about a likely typo (unknown `@repo`, or a regex with a space — package
  ids never contain spaces / a forgotten `@`), shows what each rule freezes, and
  asks for confirmation before writing (`--yes` skips the prompts).
- `show-changelog [REPO]` - print a ChangeLog: the official repo by default, or a
  named repo (fetched on demand if not cached).
- `search` matches an **exact** package name (case-insensitive); use `info` or
  `file-search` for broader lookups.

Global flags: `-y/--yes`, `--dry-run`, `--no-deps`, `--config-dir`.
Exit codes: 0 ok ; 1 error ; 20 nothing found ; 50 self-upgrade ; 100 pending.

### Verification

Packages are verified before install, governed by `VERIFY` (slacker.conf, global)
and `verify=` (repos, per-repo override). Policy types live in config.rs
(`VerifyPolicy` = All | Required(list) | None; `Check` = Gpg | Md5 | Sha).

- GPG is verified at `update` (the repo's CHECKSUMS file is signed); a bad
  signature is always fatal, a missing one is skipped under `all`.
- Per-package integrity is verified at install in `fetch_and_verify`. Slackware
  ships a per-package `.txz.asc`, so under `all` slacker also GPG-verifies the
  package itself (best-effort: a missing `.asc` falls back to md5); under an
  explicit `gpg` policy the package signature is required. At least one of
  gpg/md5/sha must pass; if none is available the install stops. `sha` uses
  CHECKSUMS.sha256 if a repo ships it (none do today), via `sha256sum`. On
  success slacker prints which checks passed (e.g. `verified: gpg (signer) + md5`).
- A repo whose effective policy does no checks at all triggers a visible WARNING
  after `update` and in `check-updates`.
- A `Required(list)` policy (e.g. `gpg,md5,sha`) fails if a listed method is
  absent, with a message pointing at where to relax it. The official repo gets
  no exemption.
- **Key pinning (TOFU):** the first GPG-KEY import pins the repo's fingerprint;
  a later key change is refused (possible key-substitution attack), fail-closed.
  A `subtree` repo fetches GPG-KEY from the root, so extra/testing/patches pin
  the same Slackware key as the official repo.
- **Quarantine model:** a repo that fails vetting (unreachable / malformed /
  hostile metadata) is auto-quarantined and provides no packages. New/untrusted
  repos are light-vetted on `update`; `add-repo`/`vet-repo` vet thoroughly.
  `trust-repo` lifts a quarantine (override), `distrust-repo` freezes one,
  `vet-repo` re-checks. Markers in cache: `quarantine/<name>`, `trusted/<name>`.

### Dependencies (.dep)

If a package has a `.dep` file beside it in the repo, slacker pulls in missing
deps from the *same* repo, recursively, before installing. A dep already
satisfied by that repo's build is left alone. A dep installed but differing from
the repo's version is handled by source priority: if its source is of LOWER
priority it prompts skip / replace / skip-all / abort; if of HIGHER-or-equal
priority it is kept by the priority rule but still surfaced in a table with a
keep / replace / keep-all choice (`--yes` keeps the installed one in both cases).
New deps are shown up front (before the confirm) in the same coloured plan table
as everything else — a `new dep` row tagged `for <parent>`. On by default; off via
`RESOLVE_DEPS=no` or per-run `--no-deps`. Applies to install, upgrade,
reinstall, upgrade-all, install-new, install-template.

### Build_and_Tests

> NO root needed for build & tests (only the mutating actions need root).
> 70 unit tests (+1 ignored), all passing; `cargo build` is warning-clean.

```
cargo build --release
cargo test

mkdir -p /tmp/slk && cp examples/etc-slacker/* /tmp/slk/
sed -i 's|^CACHE_DIR=.*|CACHE_DIR=/tmp/slk/cache|' /tmp/slk/slacker.conf
# REQUIRED: pick a mirror - nothing is uncommented by default.
# Edit /tmp/slk/mirrors and uncomment exactly one line.

./target/release/slacker --config-dir /tmp/slk update gpg     # once: import repo keys
./target/release/slacker --config-dir /tmp/slk update         # verifies GPG, fast
./target/release/slacker --config-dir /tmp/slk search firefox
./target/release/slacker --config-dir /tmp/slk info bash
./target/release/slacker --config-dir /tmp/slk file-search bin/bash   # lazy MANIFEST (first time large)
./target/release/slacker --config-dir /tmp/slk check-updates ; echo "exit=$?"
```

### Notes

- TLS via native-tls (system OpenSSL). file:// reads the filesystem directly.
- PACKAGES.TXT/CHECKSUMS/MANIFEST/ChangeLog are read UTF-8-lossy (some repos,
  e.g. extras, contain non-UTF-8 bytes).
- Arch filtering keeps native arch + noarch + fw (firmware) + x86 (32-bit
  headers), matching slackpkg, so e.g. kernel-headers-x86 isn't flagged foreign.
- MANIFEST.bz2 (~35 MB for official) is fetched lazily on first `file-search`,
  not on every `update`, and is invalidated on each `update`.
- GPG is fail-closed: a bad signature or missing key stops the update.
- Dep resolution does one small `.dep` request per package at install time
  (404 = no deps, proceeds normally) - same as slackpkg+.
- `clean-system` lists packages **no longer in the official baseline** (the
  official repo's PACKAGES.TXT plus any `immutable` repo) in a numbered table;
  pick which to keep (Enter removes all). slackpkg-style: a package the distro
  dropped is removed even if a third-party repo still ships the name. Kept when
  any of three holds: a `blacklist` match; build tag in `IGNORE_TAGS`; or
  attributed to an `immutable` repo. Refuses to run if a baseline repo has no
  metadata loaded (safety guard against mass removal).
- `clean-cache` frees disk by deleting cached package files; metadata and GPG
  keys are safe. `download -o DIR` and the symlink guard make explicit
  downloads (e.g. into /tmp) safe.
- ADM_DIR defaults to `/var/adm` rather than `/var/lib/pkgtools` on purpose: on a
  real box `/var/adm/packages`, `scripts/`, `setup/` symlink up into
  `/var/lib/pkgtools`, but `removed_packages/` (and `removed_scripts/`,
  `removed_uninstall_scripts/`) live under `/var/adm/pkgtools/...` and are NOT
  exposed by name from `/var/lib/pkgtools`. Only `/var/adm` exposes the whole set,
  which `history` needs. `removed_packages` is lossy: plain `removepkg` records
  collide on the package id (a later removal overwrites an earlier one), while
  `-upgraded-<timestamp>` records are unique — hence the upgrade-target inference.
- Long output (`history`, `show-changelog`) is paged through `$PAGER` (or
  `less -FRX`) when stdout is a TTY: it opens at the top, short output prints
  inline and the pager exits immediately, and `q` quits cleanly. The pager is fed
  from a scoped thread so a large body cannot deadlock the quit.
- If a mirror shows stale versions, switch to another in `mirrors`.
