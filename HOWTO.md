# slacker HOWTO

A practical, example-driven guide to `slacker`, a binary package manager for
Slackware that combines `slackpkg` and `slackpkg+` in one tool.

Every example below is a real command. Mutating actions (install, upgrade,
remove, ...) need root; queries (search, info, file-search, ...) do not.

---

## Table of contents

1. First-time setup
2. Keeping metadata fresh
3. Searching and inspecting
4. Installing
5. Upgrading
6. Reinstalling and removing
7. Whole repos and build tags (`@` selectors)
8. Downloading without installing
9. Freezing packages (blacklist)
10. Templates
11. Cleaning up
12. Global flags and exit codes
13. Common workflows
14. Package verification

---

## 1. First-time setup

Configuration lives in `/etc/slacker/` (override with `--config-dir`). Copy the
shipped templates and edit them:

```
mkdir -p /etc/slacker
cp examples/etc-slacker/* /etc/slacker/
```

Pick exactly one mirror in `/etc/slacker/mirrors` (none is active by default;
two or more active lines is an error):

```
# /etc/slacker/mirrors  - uncomment ONE line
https://slackware.uk/slackware/slackware64-current/
```

Declare your repos in `/etc/slacker/repos`. Binary repos take a URL (or the
keyword `mirror` for the official one) and must have distinct priorities;
higher priority wins:

```
# priority  name        url|mirror                                            [official]
100         slackware   mirror                                                official
90          extras      https://mirror.nl.leaseweb.net/slackware/slackware64-current/extra
80          conraid     https://slackers.it/repository/slackware64-current
60          alienbob    https://slackware.nl/people/alien/sbrepos/current/x86_64
```

Optionally add tag-priority lines so source/local packages are never migrated
or downgraded by `upgrade-all`:

```
100         SBo         _SBo
100         local       _rtz
```

Import the repo GPG keys once, then refresh:

```
slacker update gpg
slacker update
```

---

## 2. Keeping metadata fresh

```
slacker update                  # refresh PACKAGES.TXT/CHECKSUMS, verify GPG
slacker update gpg              # (re)import repo GPG keys, then refresh
slacker check-updates           # exit 100 if the official ChangeLog changed
slacker show-changelog          # view the cached ChangeLog (paged on a TTY)
```

---

## 3. Searching and inspecting

```
slacker search firefox          # match names and descriptions
slacker info bash               # per-repo candidates + installed version
slacker file-search bin/bash    # which package ships this file (uses MANIFEST)
```

`info` shows which repo wins by priority. For example, if `ffmpeg` exists in
several repos, the highest-priority one is the candidate; pin another with
`repo:name` (see below).

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

## 8. Downloading without installing

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

## 9. Freezing packages (blacklist)

Freeze a package so update, upgrade-all, reinstall, and clean-system leave it
alone (it is added to `/etc/slacker/blacklist`):

```
slacker frozen pandoc-bin               # freeze one
slacker frozen firefox chromium vlc     # freeze several
```

Use the exact package name (not the full version-tag). To unfreeze, remove the
line from `/etc/slacker/blacklist`.

---

## 10. Templates

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

## 11. Cleaning up

```
slacker clean-system            # list packages in no configured repo, choose what to remove
slacker clean-cache             # delete downloaded *.txz from the cache
slacker clean-cache alienbob    # only that repo's cached files
slacker --dry-run clean-cache   # show what would be freed
slacker new-config              # handle leftover *.new config files
```

`clean-cache` never touches repo metadata or GPG keys (those live under
`CACHE_DIR/repos`), so it is always safe to run. `clean-system` never lists
blacklisted packages or packages whose build tag is in `IGNORE_TAGS`.

---

## 12. Global flags and exit codes

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

## 13. Common workflows

Routine system update:

```
slacker update
slacker upgrade-all
slacker clean-system
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

## 14. Package verification

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
