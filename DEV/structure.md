# slacker — project structure

`slacker` = slackpkg + slackpkg+ in one minimal Rust tool. Full slackpkg action
parity, plus slackpkg+ multi-repo priority resolution and .dep dependency
handling.

Build needs current Slackware's Rust (1.96+). Direct deps: clap, ureq,
native-tls, md-5. Heavy lifting (bzip2 for MANIFEST, GPG) shells out to the
system tools Slackware already ships — no extra Rust deps.

### Source_Tree
```
slacker/
├── Cargo.toml                      <- build manifest (deps: clap, ureq, native-tls, md-5)
├── README.md
├── slacker.8                       <- man page (section 8)
├── examples/etc-slacker/           <- config templates for /etc/slacker/
│   ├── slacker.conf                <- globals (ARCH auto-detect, CACHE_DIR, PKG_DB_DIR, RESOLVE_DEPS)
│   ├── mirrors                     <- catalogue official mirrors — uncomment ΕΝΑ (none by default)
│   ├── repos                       <- priorities/names + external (+repos)
│   └── blacklist                   <- one package_name per line
└── src/                            <- 12 modules
    ├── main.rs        CLI + commands (19 actions, exit codes, prompts, dry-run, dep resolution)
    ├── config.rs      plain-text config (slacker.conf, mirrors, repos, blacklist) + arch auto-detect
    ├── pkg.rs         Slackware package-name splitting (name-version-arch-build)
    ├── repo.rs        PACKAGES.TXT/CHECKSUMS parsing, metadata fetch, series, lazy MANIFEST, .dep fetch
    ├── pkgdb.rs       unified DB, priority, pattern/series matching, newly-added, orphans
    ├── download.rs    https/http (ureq+native-tls) + file:// + md5
    ├── system.rs      installed DB (PKG_DB_DIR) + pkgtools wrappers (install/upgrade/reinstall/remove)
    ├── manifest.rs    file-search (decompressed MANIFEST)
    ├── changelog.rs   check-updates / show-changelog
    ├── gpg.rs         GPG import + verify (captured output, fail-closed)
    ├── template.rs    templates (generate/install/remove, includes)
    └── newconfig.rs   .new config file handling
```

### Config_Model

- **slacker.conf** - `KEY=value`. ARCH is auto-detected from the installed
  `aaa_base` package (override only for cross). CACHE_DIR, PKG_DB_DIR
  (default `/var/lib/pkgtools/packages`), RESOLVE_DEPS (default yes).
- **mirrors** - slackpkg-style catalogue; uncomment exactly ONE (none by
  default; 2+ -> error). Holds the official mirror URL (current/15.0 × 64/32,
  http/https/file://).
- **repos** - `priority  name  url  [official]`. The official line's url is the
  keyword `mirror` (filled from `mirrors`). Higher priority wins; `name:package`
  pins a repo. External (+repos) listed here with their own priority.
- **blacklist** - packages never installed/upgraded/reinstalled/removed;
  honoured by every mutating command, and hidden from `clean-system`.

### Actions (slackpkg parity)

```
update [gpg]    search        file-search   info        install
upgrade         reinstall     remove        download    upgrade-all
install-new     clean-system  new-config    check-updates
show-changelog  generate-template  install-template  remove-template
```
Global flags: `-y/--yes`, `--dry-run`, `--no-deps`, `--config-dir`.
Exit codes: 0 ok ; 1 error ; 20 nothing found ; 50 self-upgrade ; 100 pending.

### Dependencies (.dep)

If a package has a `.dep` file beside it in the repo, 
slacker pulls in missing deps from the *same* repo, recursively,
before installing. A dep already satisfied by that repo's build is left alone;
one installed but differing from the repo's version (e.g. another source)
prompts: skip / replace / skip-all / abort (`--yes` keeps the installed one).
On by default; off via `RESOLVE_DEPS=no` or per-run `--no-deps`. Applies to
install, upgrade, reinstall, upgrade-all, install-new, install-template.

### Build_and_Tests

> NO root needed for build & tests (only the mutating actions need root).

```
cargo build --release
cargo test

mkdir -p /tmp/slk && cp examples/etc-slacker/* /tmp/slk/
sed -i 's|^CACHE_DIR=.*|CACHE_DIR=/tmp/slk/cache|' /tmp/slk/slacker.conf
# REQUIRED: pick a mirror — nothing is uncommented by default.
# Edit /tmp/slk/mirrors and uncomment exactly one line.

./target/release/slacker --config-dir /tmp/slk update gpg     # once: import repo keys
./target/release/slacker --config-dir /tmp/slk update         # verifies GPG, fast
./target/release/slacker --config-dir /tmp/slk search firefox
./target/release/slacker --config-dir /tmp/slk info bash
./target/release/slacker --config-dir /tmp/slk file-search bin/bash   # lazy MANIFEST (first time large)
./target/release/slacker --config-dir /tmp/slk info emacs
./target/release/slacker --config-dir /tmp/slk file-search emacs
./target/release/slacker --config-dir /tmp/slk search emacs
./target/release/slacker --config-dir /tmp/slk check-updates ; echo "exit=$?"
```

### Notes

- TLS via native-tls (system OpenSSL). file:// reads the filesystem directly.
- MANIFEST.bz2 (~35 MB for official) is fetched lazily on first `file-search`,
  not on every `update`, and is invalidated on each `update`.
- GPG is fail-closed: a bad signature or missing key stops the update.
- Dep resolution does one small `.dep` request per package at install time
  (404 = no deps, proceeds normally) - same as slackpkg+.
- `clean-system` lists foreign packages (absent from every configured repo) in
  a numbered table; pick which to keep (Enter removes all). Blacklisted packages
  never appear. Third-party +repo packages are NOT foreign - only truly unknown
  ones are.
- If a mirror shows stale versions, switch to another in `mirrors`.
