# slacker - project structure

`slacker` = slackpkg + slackpkg+ in one minimal Rust tool. Full slackpkg action
parity, plus slackpkg+ multi-repo priority resolution and .dep dependency
handling.

Build needs current Slackware's Rust (1.96+; edition 2021 for broad
compatibility). Direct deps: clap, ureq, native-tls, md-5. Heavy lifting (bzip2
for MANIFEST, GPG) shells out to the system tools Slackware already ships - no
extra Rust deps.

### Source_Tree
```
slacker/
├── Cargo.toml                      <- build manifest (deps: clap, ureq, native-tls, md-5; Apache-2.0)
├── README.md
├── slacker.8                       <- man page (section 8)
├── examples/etc-slacker/           <- config templates for /etc/slacker/
│   ├── slacker.conf                <- globals (ARCH auto-detect, CACHE_DIR, PKG_DB_DIR, RESOLVE_DEPS, IGNORE_TAGS)
│   ├── mirrors                     <- catalogue of official mirrors - uncomment ONE (none by default)
│   ├── repos                       <- binary repos + tag-priority lines
│   └── blacklist                   <- one package_name per line
└── src/                            <- 12 modules
    ├── main.rs        CLI + commands (21 actions, exit codes, prompts, dry-run, dep resolution, @-selectors, multi-match selection)
    ├── config.rs      plain-text config (slacker.conf, mirrors, repos, blacklist) + arch auto-detect + tag-priorities
    ├── pkg.rs         Slackware package-name splitting (name-version-arch-build) + build_tag()
    ├── repo.rs        PACKAGES.TXT/CHECKSUMS parsing (UTF-8-lossy), metadata fetch, series, arch filter, lazy MANIFEST, .dep fetch
    ├── pkgdb.rs       unified DB, priority, pattern/series/@-matching, upgrade resolution, newly-added, orphans
    ├── download.rs    https/http (ureq+native-tls) + file:// + md5
    ├── system.rs      installed DB (PKG_DB_DIR) + pkgtools wrappers (install/upgrade/reinstall/remove) + cached_pkg_path
    ├── manifest.rs    file-search (decompressed MANIFEST)
    ├── changelog.rs   check-updates / show-changelog (pager when on a TTY)
    ├── gpg.rs         GPG import + verify (captured output, fail-closed)
    ├── template.rs    templates (generate/load/delete, includes)
    └── newconfig.rs   .new config file handling
```

### Config_Model

- **slacker.conf** - `KEY=value`. ARCH is auto-detected from the installed
  `aaa_base` package (override only for cross). CACHE_DIR (default
  `/var/cache/slacker`), PKG_DB_DIR (default `/var/lib/pkgtools/packages`),
  RESOLVE_DEPS (default yes), IGNORE_TAGS (build tags that `clean-system` treats
  as non-foreign, e.g. `_SBo cf alien`).
- **mirrors** - slackpkg-style catalogue; uncomment exactly ONE (none by
  default; 2+ -> error). Holds the official mirror URL (current/15.0 × 64/32,
  http/https/file://).
- **repos** - two kinds of line:
  - binary repo: `priority  name  url|mirror  [official]`. The official line's
    url is the keyword `mirror` (filled from `mirrors`). Higher priority wins;
    `name:package` pins a repo. Binary-repo priorities must be **distinct**
    (duplicate -> fail-fast error).
  - tag-priority: `priority  name  tag` (e.g. `100 SBo _SBo`). Gives packages
    carrying a build tag a priority on the same scale, so SBo/local/source
    packages are never silently migrated to another repo or downgraded by
    `upgrade-all`. Tag-priority lines may share priority values.
- **blacklist** - packages never installed/upgraded/reinstalled/removed;
  honoured by every mutating command, and hidden from `clean-system`. The
  `frozen` command appends to this file.

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

### Actions (21; slackpkg parity + extras)

```
update [gpg]    search        file-search   info        install
upgrade         reinstall     remove        download     clean-cache
upgrade-all     install-new   clean-system  frozen       new-config
check-updates   show-changelog
generate-template  install-template  remove-template  delete-template
```
- `install-new [REPO...]` - official repos only by default; name repos to opt in.
- `download [-o DIR] PATTERN...` - saves to CACHE_DIR/packages/<repo>/ by default,
  or to DIR; confirms before bulk (>10) downloads; refuses to write through a
  pre-existing symlink.
- `clean-cache [REPO...]` - deletes downloaded *.txz only; never touches repo
  metadata or GPG keys under CACHE_DIR/repos.
- `remove-template` uninstalls a template's packages (slackpkg behaviour);
  `delete-template` removes only the template file.
- `frozen PKG...` - add one or more packages to the blacklist.

Global flags: `-y/--yes`, `--dry-run`, `--no-deps`, `--config-dir`.
Exit codes: 0 ok ; 1 error ; 20 nothing found ; 50 self-upgrade ; 100 pending.

### Dependencies (.dep)

If a package has a `.dep` file beside it in the repo, slacker pulls in missing
deps from the *same* repo, recursively, before installing. A dep already
satisfied by that repo's build is left alone; one installed but differing from
the repo's version (e.g. another source) prompts: skip / replace / skip-all /
abort (`--yes` keeps the installed one). New deps are shown up front (before the
confirm) as `new-dep: [repo] pkg (for parent)`. On by default; off via
`RESOLVE_DEPS=no` or per-run `--no-deps`. Applies to install, upgrade,
reinstall, upgrade-all, install-new, install-template.

### Build_and_Tests

> NO root needed for build & tests (only the mutating actions need root).
> 49 unit tests (+1 ignored), all passing; `cargo build` is warning-clean.

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
- `clean-system` lists foreign packages (absent from every configured repo) in
  a numbered table; pick which to keep (Enter removes all). Blacklisted packages
  and packages whose build tag is in IGNORE_TAGS never appear. Third-party
  +repo packages are NOT foreign - only truly unknown ones are.
- `clean-cache` frees disk by deleting cached package files; metadata and GPG
  keys are safe. `download -o DIR` and the symlink guard make explicit
  downloads (e.g. into /tmp) safe.
- If a mirror shows stale versions, switch to another in `mirrors`.
