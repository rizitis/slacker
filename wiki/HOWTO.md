# slacker HOWTO

A practical, example-driven guide to `slacker`, a binary package manager for
Slackware that combines `slackpkg` and `slackpkg+` in one tool.

Every example below is a real command. Mutating actions (install, upgrade,
remove, ...) need root; queries (search, info, file-search, ...) do not.

---

## Table of contents

1. First-time setup
2. Keeping metadata fresh
3. Searching, inspecting, listing repos, and package history
4. Installing
5. Upgrading
6. Reinstalling and removing
7. Whole repos and build tags (`@` selectors)
8. Managing repositories (add, remove, tags, trust)
9. Downloading without installing
10. Freezing packages (blacklist)
11. Templates
12. Cleaning up
13. Global flags and exit codes
14. Common workflows
15. Package verification

---

## 1. First-time setup

On Slackware you install the **binary package** (built from the SlackBuild). It
puts everything in place — the `slacker` binary, the man page, and the config
files under `/etc/slacker/` (override the directory with `--config-dir`). You do
**not** copy anything. There are only two files to edit before first use: pick a
mirror in `mirrors`, and set your repo priorities in `repos`.

Pick exactly one mirror in `/etc/slacker/mirrors` (none is active by default;
two or more active lines is an error):

```
# /etc/slacker/mirrors  - uncomment ONE line
https://slackware.uk/slackware/slackware64-current/
```

Declare your repos in `/etc/slacker/repos`. The URL field is either a literal
URL, the keyword `mirror` (the active mirror, for the official repo), or
`mirror/<subpath>` (the active mirror with a subpath appended, e.g.
`mirror/extra` — it tracks a distribution subtree on whichever mirror you picked,
so you never hardcode the host). Binary repos must have distinct priorities;
higher priority wins:

```
# priority  name        url|mirror|mirror/<subpath>                          [flags]
100         slackware   mirror                                                official
90          extras      mirror/extra                                          subtree immutable
80          conraid     https://slackers.it/repository/slackware64-current
60          alienbob    https://slackware.nl/people/alien/sbrepos/current/x86_64
```

Flags (any order) go after the URL: `official` (the tracked repo), `subtree` (a
Slackware distribution subtree — see below), `immutable` (a repo whose packages
`clean-system` never removes), and `verify=...` (a per-repo verification
override).

The four Slackware distribution subtrees — **`extra`, `patches`, `testing`,
`pasture`** — **must always carry the `subtree` flag** (anywhere after the URL).
Their `PACKAGES.TXT` lists package locations relative to the distribution root,
so without `subtree` their packages fail to download (a doubled path segment);
with it, packages and `GPG-KEY` are fetched from the parent (root) URL while the
metadata still comes from the repo URL itself. This is not optional for these
four repos.

Optionally add tag-priority lines so source/local packages are never migrated
or downgraded by `upgrade-all`:

```
100         SBo         _SBo
100         local       _rtz
```

Import the repo GPG keys once, then refresh and check the setup:

```
slacker update gpg
slacker update
slacker status                  # confirms the setup is healthy and flags anything to fix
```

`update gpg` pins each repo's key (trust on first use). For a `subtree` repo it
pins the same Slackware key from the root, which is what lets per-package GPG
verification succeed for `extra/`/`testing/`/`patches/` packages at install.

---

## 2. Keeping metadata fresh

```
slacker update                  # refresh PACKAGES.TXT/CHECKSUMS, verify GPG
slacker update gpg              # (re)import repo GPG keys, then refresh
slacker check-updates           # check every repo; exit 100 if any has updates pending
slacker show-changelog          # view the cached ChangeLog (paged on a TTY)
slacker show-changelog conraid  # a named repo's ChangeLog (fetched on demand)
```

---

## 3. Searching, inspecting, and listing repos

```
slacker search firefox          # find a package by its exact name (case-insensitive)
slacker info bash               # per-repo candidates + installed version
slacker file-search bin/bash    # which package ships this file (uses MANIFEST)
slacker list-repos              # repos: priority, installed counts, verify, flags
slacker status                  # health-check the whole setup; says what to fix next
```

`info` shows which repo wins by priority. For example, if `ffmpeg` exists in
several repos, the highest-priority one is the candidate; pin another with
`repo:name` (see below).

### Package history

`slacker history` prints a newest-first log of every package change on the box —
installed, upgraded, reinstalled, removed — with the local date of each change
and the source repo or build tag. It is reconstructed from the pkgtools admin
directories under `ADM_DIR` (`packages/` + `removed_packages/`), so it also
reflects changes made by other tools (slackpkg, sbopkg, plain
installpkg/upgradepkg/removepkg), not only slacker.

```
slacker history                 # everything, newest first
slacker history emacs           # just one package (exact name)
slacker history --installed     # only what is installed now, with install dates
slacker history --removed       # only what left the system (removed or upgraded away)
slacker history --upgraded      # only upgrade / reinstall events
slacker history --last 20       # the 20 most recent events
slacker history --since 2026-06-01   # events on or after a date
```

An upgrade reads as `old → new`. Filters combine (e.g. `history --installed
--since 2026-06-01`). Output is paged on a terminal (press `q` to quit). Note:
because plain `removepkg` records share one filename per package id in
`removed_packages`, a later removal overwrites an earlier one; when an upgrade's
target record was lost that way, the new version is inferred from that package's
next known entry rather than shown as `?`.

---

## 4. Installing

```
slacker install vlc                     # one package (+ its .dep chain)
slacker install vlc mpv obs-studio      # several at once
slacker install conraid:ffmpeg          # force the conraid build (pin)
slacker --dry-run install vlc           # preview only, change nothing
slacker --no-deps install vlc           # skip dependency resolution
slacker -y install vlc                  # assume yes to all prompts
```

If a pattern matches more than one package, slacker prints a numbered list:

```
slacker install python
# 'install' matched 12 packages:
#   1) [slackware] python-build-...
#   2) [slackware] python-cffi-...
#   ...
# Enter numbers to install (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel:
```

Already-installed packages are refused by `install` (use `upgrade` or
`reinstall` instead).

**Dependencies are a third-party feature, not an official one.** Slackware's own
repositories — `slackware` and the `extra`/`patches`/`testing`/`pasture`
subtrees — neither ship nor expect dependency information: a complete install of
**all** Slackware package sets is the official prerequisite, so every dependency
is assumed already present, and slacker performs no dependency resolution for
them (none is needed). Independent repos such as `alienbob` and `conraid` do ship
per-package `.dep` files; slacker reads them **only for the repos that provide
them**, pulling in any missing dependencies from that same repo. So an
`install`/`upgrade` from the official tree resolves nothing, while one from a
third-party repo with `.dep` files does. Turn it off anywhere with `--no-deps`
(per run) or `RESOLVE_DEPS=no` (in `slacker.conf`).

---

## 5. Upgrading

```
slacker upgrade vlc             # upgrade specific package(s)
slacker upgrade-all             # upgrade everything with a newer revision
slacker --dry-run upgrade-all   # preview the whole upgrade plan first
slacker -y upgrade-all          # no prompts
```

`upgrade-all` respects priority and build tags: a package is only replaced by a
candidate from a higher- or equal-priority repo, so SBo/local/source packages
are never silently migrated to another repo or downgraded. New dependencies it
needs are shown before the confirmation as `new-dep: [repo] pkg (for parent)`.

Install packages newly added to the distribution since your last update:

```
slacker install-new             # official repos only (default)
slacker install-new conraid     # only newly-added conraid packages
slacker install-new slackware extras
```

---

## 6. Reinstalling and removing

```
slacker reinstall bash          # reinstall the current version
slacker reinstall y             # reinstall a whole series (here: games)
slacker reinstall ap            # the 'ap' series
slacker remove libfoo           # remove installed package(s)
slacker --dry-run remove libfoo # preview
```

Series names (`a`, `ap`, `d`, `k`, `kde`, `l`, `n`, `t`, `x`, `xap`, `xfce`,
`y`, ...) match exactly that series, not every package whose name contains those
letters. A multi-match still shows the numbered selection list.

---

## 7. Whole repos and build tags (`@` selectors)

The `@` prefix is an explicit set selector. It is required - a bare word is
never treated as a repo.

```
slacker install @gnome          # install every package in the gnome repo
slacker remove  @gnome          # remove the installed packages from that repo
slacker remove  @_SBo           # remove all installed SlackBuilds.org packages
slacker download @alienbob      # download every package in the alienbob repo
```

`@repo` means "every package in that repo"; `@_tag` means "every package with
that build tag". A typo gives a helpful error:

```
slacker install @gnme
# error: unknown repo or tag '@gnme'; did you mean '@gnome'?
#   available repos: conraid, gnome, slackware
#   available tags:  _gnome, cf
```

Typical use: put a desktop repo (e.g. gnome) at a distinct, high priority such
as 101 and install it as a set. Its `_gnome`-tagged packages are then locked -
no lower repo can replace or "upgrade" them, even with a newer version:

```
# /etc/slacker/repos
101  gnome  https://your-gnome-repo/...
```
```
slacker update
slacker install @gnome
slacker upgrade-all             # leaves the gnome packages untouched
```

---

## 8. Managing repositories (add, remove, tags, trust)

You can edit `/etc/slacker/repos` by hand, or let slacker do it for you
(validated, with a confirmation prompt):

```
slacker add-repo 70 extras https://.../slackware64-current/extra subtree
slacker add-repo 80 conraid https://slackers.it/repository/slackware64-current
slacker del-repo conraid
slacker add-tag 100 SBo _SBo            # a build-tag priority line
slacker del-tag _SBo
```

`add-repo` flags (any order): `official`, `immutable`, `subtree`, `verify=...`.
A Slackware **subtree** (`extra/`, `patches/`, `testing/`, `pasture/`) must get
`subtree` or its packages fail to download; `immutable` keeps a repo's packages
out of `clean-system` (see §12).

Inspect and health-check at any time:

```
slacker list-repos              # priority, installed counts, verify policy, flags
slacker status                  # whole-setup health check + what to fix next
```

`list-repos` shows a table and marks `(official)`, `(immutable)`, `(subtree)`
and any quarantine. `status` groups its findings (Setup / Installed / Online)
with ✓/!/✗ markers and ends with a plain-language verdict and next steps.

### Repository safety: quarantine and trust

slacker vets repos and **quarantines** any that are unreachable or serve
malformed/hostile metadata; a quarantined repo provides no packages until you
act. New or as-yet-untrusted repos are light-vetted on every `update`;
`add-repo` and `vet-repo` vet thoroughly.

```
slacker vet-repo conraid        # re-check on demand (quarantine on fail, clear on pass)
slacker trust-repo conraid      # lift a quarantine you judge a false positive (override)
slacker distrust-repo conraid   # freeze a repo yourself
```

GPG keys are pinned on first import (trust on first use): if a repo's key ever
changes, slacker refuses it as a possible key-substitution attack rather than
trusting the new key silently. `list-repos` and `status` show the state.

---

## 9. Downloading without installing

Files are saved to `CACHE_DIR/packages/<repo>/` by default, the same place
`install` looks, so a later install reuses them.

```
slacker download pandoc-bin             # into the cache
slacker download -o /tmp pandoc-bin     # into /tmp instead
slacker download -o . pandoc-bin        # into the current directory
slacker download @alienbob              # whole repo (asks to confirm if >10)
```

slacker refuses to write through a pre-existing symlink, so downloading into a
shared directory like `/tmp` is safe.

---

## 10. Freezing packages (blacklist)

Freeze a package so update, upgrade-all, reinstall, and clean-system leave it
alone (it is added to `/etc/slacker/blacklist`):

```
slacker frozen pandoc-bin               # freeze one
slacker frozen firefox chromium vlc     # freeze several
slacker frozen "@alienbob vlc-*"        # scope to a repo + a pattern (quotes required)
```

Quote any rule that has a space (an `@repo` rule) or a shell glob character
(`*`, `?`, `[`, `]`, ...). `"@alienbob vlc-*"` scopes the rule to the `alienbob`
repo, and `vlc-*` is an **unanchored regex** against the full package id — in a
regex `-*` is "zero or more hyphens", not "anything", so it freezes any installed
`alienbob` package whose id contains `vlc` (e.g. `vlc`, `vlc-plugin-qt`), just as a
bare `vlc` would. To freeze only the `vlc` package, anchor it: `"@alienbob
^vlc-[0-9]"`.

Use the exact package name (not the full version-tag). To unfreeze, remove the
line from `/etc/slacker/blacklist`.

The blacklist freezes individual **packages**. To act on a whole **repository**
there is a separate mechanism — *quarantine*: `distrust-repo` freezes a repo,
`vet-repo` re-checks it, `trust-repo` lifts it (§8).

Blacklisting is the per-package way to keep something out of `clean-system`. To
protect a whole group at once, prefer `IGNORE_TAGS` (by build tag, e.g.
`_SBo cf alien`) or marking a repo `immutable` (§8, §12) instead of freezing
each package by hand.

---

## 11. Templates

A template is a snapshot of installed package names that you can replay on
another machine or after a reinstall.

```
slacker generate-template mybox         # snapshot current packages -> mybox.template
slacker install-template mybox          # install everything the template lists
slacker remove-template mybox           # UNINSTALL every package the template lists
slacker delete-template mybox           # delete only the template file (keeps packages)
```

Note the distinction: `remove-template` removes the *packages*;
`delete-template` removes only the *file*.

---

## 12. Cleaning up

```
slacker clean-system            # list packages no longer in the official baseline, choose what to remove
slacker --dry-run clean-system  # preview first — always do this
slacker clean-cache             # delete downloaded *.txz from the cache
slacker clean-cache alienbob    # only that repo's cached files
slacker --dry-run clean-cache   # show what would be freed
slacker new-config              # handle leftover *.new config files
```

`clean-cache` never touches repo metadata or GPG keys (those live under
`CACHE_DIR/repos`), so it is always safe to run.

`clean-system` is slackpkg-style: it removes packages that are **no longer part
of the official baseline** — the official repo's `PACKAGES.TXT` plus any repo
marked `immutable`. So a package the distribution itself dropped is removed even
if a third-party repo still ships the name. A package is kept (never listed)
when any of three things is true:

- it matches a **blacklist** rule (`slacker frozen NAME`);
- its **build tag** is in `IGNORE_TAGS` (`slacker.conf`), e.g. `_SBo cf alien`;
- it is attributed to an **`immutable`** repo (the repo that owns its build tag,
  or for a tagless package any immutable repo that provides its name).

So before your first `clean-system`, set `IGNORE_TAGS` for your SBo/local/source
tags and/or mark `extra/`/`testing/`/`patches/` repos `immutable` — otherwise
those packages will be listed as foreign. As a safety guard `clean-system`
refuses to run if a baseline repo has no metadata loaded (run `update` first),
and `--dry-run` shows exactly what it would remove without touching anything.

---

## 13. Global flags and exit codes

Read-only commands (`search`, `info`, `file-search`, `check-updates`,
`show-changelog`) run as any user. Everything that changes the system, cache, or
config must be run as root (or via sudo by a wheel member); a non-root attempt
stops immediately with a clear message.

Those commands also take an exclusive lock (`/run/slacker.lock`) so two cannot
run at once; a second invocation exits immediately reporting the running PID.
The lock is released automatically if slacker exits or is killed, so a crash
never locks you out. Queries take no lock.


Flags (work with any command):

```
--config-dir <DIR>    use a different config directory (default /etc/slacker)
-y, --yes             assume "yes" to all prompts
--dry-run             show what would happen, change nothing
--no-deps             do not read .dep files for this run
```

Exit codes:

```
0     success
1     error
20    nothing found / nothing to do
50    a self-upgrade of slacker is available
100   pending updates (from check-updates)
```

Example scripted check:

```
slacker check-updates ; [ $? -eq 100 ] && slacker -y upgrade-all
```

---

## 14. Common workflows

Routine system update:

```
slacker update
slacker upgrade-all
slacker --dry-run clean-system   # review first (it removes anything off the baseline)
slacker clean-system             # then run it once IGNORE_TAGS/immutable are set (see §12)
```

First sync after editing repos, with key import:

```
slacker update gpg
slacker update
slacker check-updates ; echo "exit=$?"
```

Preview everything before committing:

```
slacker --dry-run upgrade-all
slacker --dry-run install @gnome
slacker --dry-run clean-cache
```

Move a machine's package set to another box:

```
# on the source machine
slacker generate-template snapshot
# copy /etc/slacker/templates/snapshot.template to the target, then:
slacker update
slacker install-template snapshot
```

Free disk without risking metadata or keys:

```
slacker clean-cache
```

---

## 15. Package verification

slacker verifies packages before installing them. The policy is set globally
with `VERIFY` in `slacker.conf` and can be overridden per repo with a `verify=`
flag on the repos line.

Default (`VERIFY=all`):

```
# slacker.conf
VERIFY=all
```

With `all`, for each package: the GPG signature is checked when the repo
provides one (a bad signature always fails; a missing one is skipped), and at
least one integrity checksum (md5 or sha) must be present and match. If neither
md5 nor sha is available, installation stops - the repo's checksum file is
missing or broken.

Slackware ships a per-package `.txz.asc` next to each package, so under `all`
slacker GPG-verifies the package itself and prints, e.g.,
`verified: gpg (signer) + md5`. For this you must have pinned the repo's key
with `slacker update gpg`; until then you get `integrity only: md5` (the package
is still md5-checked against the GPG-signed `CHECKSUMS.md5`, just not
authenticated per package). For a `subtree` repo the key is fetched from the
root, where Slackware keeps the one key that signs the whole tree — so
`extra/`/`testing/`/`patches/` pin the same fingerprint as the official repo.

**Key pinning (trust on first use):** the first import pins the repo's
fingerprint; if it ever changes, slacker refuses the repo as a possible
key-substitution attack rather than trusting the new key silently. See §8 for
the quarantine/trust commands.

Require specific methods (stops if one is missing, telling you how to relax it):

```
VERIFY=gpg,md5,sha
VERIFY=gpg,md5
VERIFY=md5
```

Disable entirely (not recommended):

```
VERIFY=none
```

Per-repo override - useful when one repo ships a broken or missing checksum or
signature, so you can relax just that repo instead of weakening everything:

```
# repos
100  slackware  mirror                       official
80   conraid    https://slackers.it/...      verify=gpg,md5
60   alienbob   https://slackware.nl/...      verify=md5
```

The same rules apply to every repo, including the official one - there is no
exemption. The `official` flag only affects `install-new` scope and ChangeLog
tracking, not verification.

If a download fails verification you will see a clear message, for example:

```
md5 mismatch for foo-1.0-x86_64-1cf.txz: expected ..., got ...
no usable checksum (md5 or sha) for foo-...: the repo's checksum file may be
  missing or broken. ... relax verification for it with a `verify=` flag ...
```
