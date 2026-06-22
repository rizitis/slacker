# slacker - slackpkg + slackpkg+ in one

A Slackware package manager in Rust with full **slackpkg action parity**, plus
**slackpkg+ multi-repo priority** resolution.

- slackpkg: official mirror, update/install/upgrade/remove/clean-system, file-
  search, templates, ChangeLog tracking, GPG, .new config handling.
- slackpkg+: many repos in one priority-ordered model; the official mirror is
  just a repo whose priority you choose, so it can sit in any position.

## Philosophy

- Thin layer over the native pkgtools - never reimplements
  installpkg/upgradepkg/removepkg, just calls them.
- Dependencies come only from a package's own `.dep` file (opt-in, like
  slackpkg+); no dependency *guessing* - Slackware tradition.
- Synchronous; heavy lifting (bzip2 for MANIFEST, GPG) shells out to the
  system tools Slackware already ships, so no extra Rust deps.
- Everything a user edits is plain text.

## Build

Needs current Slackware's Rust (1.96+):

    cargo build --release
    install -m0755 target/release/slacker /usr/sbin/slacker
    install -m0644 slacker.8 /usr/man/man8/slacker.8

See `slacker.8` (`man slacker`) for the full manual.

## Configuration  (/etc/slacker/)

    slacker.conf   KEY=value globals (ARCH, CACHE_DIR, PKG_DB_DIR, RESOLVE_DEPS, IGNORE_TAGS, VERIFY)
    mirrors        catalogue of official mirrors - uncomment exactly ONE
    repos          repo priorities/names + external repos
    blacklist      one package name per line
    templates/     generated/created templates

PKG_DB_DIR defaults to `/var/lib/pkgtools/packages`.

### mirrors

A slackpkg-style catalogue. Uncomment exactly one line for your architecture
and release (current vs 15.0; slackware64 for x86_64, slackware for 32-bit).
slacker
errors out if two are active. Change your default mirror by changing which line
is uncommented.

### repos

    # priority  name        url       [official]
    100         slackware   mirror    official
    80          ktown       https://slackware.nl/people/alien/ktown/current/x86_64
    60          alienbob    https://slackware.nl/people/alien/sbrepos/current/x86_64

Higher priority wins. Pin a repo with `name:package`. The official line's URL
is the keyword **`mirror`**, filled in from the active line in `mirrors` - URL
lives in `mirrors`, priority/placement live here. The `official` tag marks it
for ChangeLog tracking; placement is by priority only.

**URL schemes:** `https://`, `http://`, and `file://` are supported (the last
for a local clone, NFS mount, or mounted media - three slashes for an absolute
path). A URL points at the repo root containing PACKAGES.TXT; for official
mirrors, MANIFEST.bz2 lives in a per-arch subdir which slacker finds
automatically.

## Actions (slackpkg-compatible)

    slacker update [gpg]          refresh metadata; `update gpg` imports repo keys
    slacker check-updates         per-repo update check; exit 100 if any pending
    slacker show-changelog        print the cached ChangeLog
    slacker search PATTERN        search names + descriptions
    slacker file-search FILE      which package ships FILE (MANIFEST)
    slacker info PACKAGE          per-repo candidates + installed version
    slacker install PATTERN...    install new packages (refuses installed ones)
    slacker upgrade PATTERN...    upgrade installed packages
    slacker reinstall PATTERN...  reinstall current version
    slacker remove PATTERN...     remove installed packages
    slacker download PATTERN...   download to CACHE_DIR/packages/<repo>/ (or -o DIR), don't install
    slacker clean-cache [REPO...] delete downloaded *.txz (keeps metadata + GPG keys)
    slacker upgrade-all           upgrade everything with a newer revision
    slacker install-new [REPO...] install newly-added packages (official only by default)
    slacker clean-system          remove packages in no configured repo
    slacker frozen PKG...         freeze package(s): add to blacklist
    slacker new-config            handle leftover *.new config files
    slacker generate-template N   snapshot installed packages to template N
    slacker install-template N    install everything in template N
    slacker remove-template N     uninstall every package in template N
    slacker delete-template N     delete the template file (keeps packages)

PATTERN is a package name, a name substring, a series (a, ap, n, kde, xap, ...),
a `repo:name` pin, or a set selector `@repo` / `@_tag`. Global flags: `-y/--yes`,
`--dry-run`, `--no-deps`, `--config-dir`.

## Exit status (matches slackpkg)

    0    success
    1    error
    20   nothing found to act on
    50   slacker upgraded itself; re-run
    100  pending updates (check-updates)

## Security note

GPG: `update gpg` imports each repo's GPG-KEY into a private keyring under the
cache dir; subsequent `update` verifies CHECKSUMS.md5 against
CHECKSUMS.md5.asc. Per-package integrity is md5 from the (signature-verified)
CHECKSUMS. Run `slacker update gpg` once before trusting a mirror.

## Notes / limits

- Pattern matching is substring + series + exact, not full regex.
- The `repos` file also accepts **build-tag priority** lines (`priority name tag`,
  e.g. `100 SBo _SBo`) that give SlackBuilds.org/local packages a priority on the
  same scale as repos. `upgrade-all` then only replaces a tagged package with a
  candidate from a higher- or equal-priority repo, so SBo/local packages are
  never silently migrated to another repo or downgraded.
- Two explicit set selectors (the `@` is required, so a bare word is never a
  repo):
  - `@repo` means "every package in that repo" - e.g. `install @gnome` installs
    the whole gnome repo, `remove @gnome` removes the installed packages that
    came from it.
  - `@_tag` means "every package with that build tag" - e.g. `remove @_SBo`
    removes all installed SlackBuilds.org packages.
  Give a repo a distinct, high priority and its packages (carrying their own
  build tag, e.g. `_gnome`) are protected: no other repo can replace or
  "upgrade" them, even with a newer version, because priority wins.
- When an `install`/`upgrade`/`reinstall` pattern matches more than one package,
  slacker shows a numbered list and lets you choose which to act on (Enter for
  all, numbers/ranges like `1 3 5` or `2-4`, `n` to cancel). A series name
  (`a`, `ap`, `kde`, `y`, ...) matches exactly that series, not every package
  whose name happens to contain those letters.
- `install-new` installs packages newly added since the last update. By default
  it considers only the **official** repo(s); name one or more repos to opt in
  explicitly (e.g. `slacker install-new alienbob`).
- `frozen <pkg>...` adds one or more packages to the `blacklist` so update,
  upgrade-all, reinstall, and clean-system leave them alone (freeze a version).
- `remove-template <name>` uninstalls the packages a template lists (slackpkg
  behaviour); `delete-template <name>` removes only the template file.
- `clean-system` lists installed packages absent from all configured repos and
  lets you pick which to remove (numbers to keep, Enter to remove all). Packages
  whose build tag is in `IGNORE_TAGS` (e.g. `_SBo cf alien`) are never treated
  as foreign - essential when you have many SBo/source/custom packages that no
  binary repo manages. Add individual packages to `blacklist` too if needed.
- The `blacklist` is honoured by every mutating command, including `reinstall`.
- ChangeLog is fetched (on `update`) only for the official repo, and powers
  `show-changelog`. `check-updates` covers every repo: official via ChangeLog,
  external repos by comparing PACKAGES.TXT to the cached copy.

## Dependencies (.dep files)

If a package has a `.dep` file next to it in the repository (one dependency
package name per line), slacker reads it and pulls in missing dependencies from
the same repository, recursively, before installing. Dependencies already
satisfied by that repo's build are left alone; one that is installed but differs
from what the repo offers (e.g. from another source) prompts: skip / replace /
skip-all / abort (with `--yes`, the installed version is kept).

On by default; disable with `RESOLVE_DEPS=no` in `slacker.conf`, or per-run with
`--no-deps`. Applies to install, upgrade, reinstall, upgrade-all, install-new
and install-template.

---

## NOTE

**slacker** status is `developing mode` and that means:
1. It not building for slackware(64)-15.0 and it will never be.
2. Build and run fine for slackware64-current **only for testers**... 
3. It is not ready even for testing for slackware-current (32bit) but will be soon.
