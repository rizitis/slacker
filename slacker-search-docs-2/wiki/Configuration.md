# Configuration

Everything slacker reads is plain text under **`/etc/slacker/`** (override the
directory with `--config-dir`). After installing the package these files are
already in place.

```
/etc/slacker/
├── slacker.conf   KEY=value global settings
├── mirrors        catalogue of official mirrors - uncomment exactly ONE
├── repos          repo priorities/names + external repos + tag-priority lines
├── blacklist      freeze rules, pins (@repo 100% name) and dependency exclusions (!package:dependency)
├── distro-upgrade.conf   optional: DISTRO_UPGRADE_MIRROR for upgrade-dist
└── templates/     generated and user-created templates
```

---

## slacker.conf

`KEY=value`, one per line, `#` for comments.

| Key | Default | Meaning |
|---|---|---|
| `ARCH` | auto | Architecture, auto-detected from the installed `aaa_base`. Set only to force a specific arch (e.g. cross). |
| `ADM_DIR` | `/var/adm` | Slackware pkgtools administrative root - holds `packages/`, `removed_packages/`, `scripts/`, `setup/`. The [[Package History]] command reads it. |
| `CACHE_DIR` | `/var/cache/slacker` | Downloaded metadata and package files - all re-downloadable; safe to clear. |
| `STATE_DIR` | `/var/lib/slacker` | Persistent security state: the GPG keyring, the TOFU fingerprint pins (`*.fpr`), and the repo quarantine/trust markers. Kept separate from the disposable `CACHE_DIR` so a cache wipe can't reset the trust anchors; existing state is migrated here automatically on first run. |
| `PKG_DB_DIR` | `ADM_DIR/packages` | The installed-package database. Set explicitly only to override the derived location. |
| `RESOLVE_DEPS` | `yes` | Whether `.dep` files are read to pull in dependencies. See [[Dependencies]]. |
| `IGNORE_TAGS` | *(empty)* | Space-separated build tags (e.g. `_SBo cf alien`) that `clean-system` must not treat as foreign. |
| `VERIFY` | `all` | Package verification policy. See [[Security]]. |
| `MAX_PARALLEL` | `4` | How many packages download concurrently during install/upgrade/download (clamped to 1-16; `1` = serial). A failed download or verification is skipped and reported, never aborting the rest; a package that fails GPG/checksum is never installed. |
| `REVERT` | `on` | Enables the `revert-pkg` rollback command. Set to `off` to disable it. |
| `CUMULATIVE_URL` | `https://slackware.uk/cumulative/slackware64-current` | The cumulative -current archive `revert-pkg` fetches superseded official packages from - a **revert-only** source, never consulted by `update`/`upgrade`/`install`/`check-updates`, so its historical packages never enter the priority model. |
| `TERSE` | `off` | `on` passes `--terse` to `installpkg` and `upgradepkg`, so each package prints a single line - handy for following a mass upgrade. `removepkg` is terse already. |
| `SEARCH` | `exact` | How `slacker search NAME` matches: `exact` finds the package with exactly that name, `partial` lists every package whose name contains the word. Only package names are searched, never descriptions or files. `slacker search --partial NAME` and `--exact NAME` override it for one run. |
| `BANNER` | `on` | The labyrinth header at the top of every command and the SLACKWARE art after an upgrade of the official tree and at `upgrade-dist`. Printed only on a terminal, never into a pipe or a file; `off` hides both. |
| `NEWCONFIG_OVERWRITE` | *(empty)* | Paths or globs of config files whose `.new` `new-config` installs without asking (your file is saved as `.orig`). See [new-config rules](#new-config-rules) below. |
| `NEWCONFIG_REMOVE` | *(empty)* | Paths or globs whose `.new` `new-config` deletes without asking, keeping your file. |
| `NEWCONFIG_KEEP` | *(empty)* | Paths or globs whose `.new` `new-config` renames to `.new.kept` without asking, keeping your file - out of the way, not lost. |
| `NEWCONFIG_AFTER` | `off` | What a package command does when it leaves new `.new` files: `off` prints the reminder to run `slacker new-config`, `ask` asks to run it now, `run` runs it. Only at a terminal, never with `--yes`. See [new-config after a package command](#new-config-after-a-package-command). |
| `PROXY` | `off` | A proxy for downloads: `http://host:port`, or `env` to use the environment's proxy variables. Unset or `off`: direct. See [Proxy](#proxy). |
| `NO_PROXY` | *(empty)* | Hosts reached directly, not through the proxy. Replaces the environment's `no_proxy` when set. |
| `RESOLVE_STOCK` | `no` | Only for a container or minimal install: also pull the stock dependencies an official package needs, from a local dependency database. |
| `STOCK_DB_URL` | *(empty)* | Where `update` fetches that database from (used only with `RESOLVE_STOCK=yes`). |

> `ADM_DIR` defaults to `/var/adm` rather than `/var/lib/pkgtools` on purpose: on
> a real box `removed_packages/` lives under `/var/adm/pkgtools/...` and is not
> exposed by name from `/var/lib/pkgtools`. Only `/var/adm` exposes the whole set
> that `history` needs.

### new-config rules

The three `NEWCONFIG_*` keys let `slacker new-config` handle some `.new` files on
its own, so it only asks about the rest. Each takes paths or globs of the
installed file, **without** the `.new` suffix (`*` and `?` do not cross a `/`).
A key may be repeated on as many lines as you need; its lines add up (every
other key keeps only its last line).

```
# take every new rc script except rc.modules.local,
# and never bother about the new ntp.conf or sshd_config
NEWCONFIG_OVERWRITE=/etc/rc.d/rc.*
NEWCONFIG_REMOVE=/etc/ntp.conf
NEWCONFIG_KEEP=/etc/rc.d/rc.modules.local
NEWCONFIG_KEEP=/etc/ssh/sshd_config
```

| Key | What happens | Left behind |
|---|---|---|
| `NEWCONFIG_OVERWRITE` | the `.new` replaces your file | your old file as `.orig` |
| `NEWCONFIG_REMOVE` | the `.new` is deleted | your file, unchanged |
| `NEWCONFIG_KEEP` | the `.new` is renamed | your file, unchanged, and the new version as `.new.kept` |

- When a file matches more than one key, the **safer** one wins: KEEP over
  REMOVE over OVERWRITE.
- A **glob** in `NEWCONFIG_OVERWRITE` never replaces `/etc/passwd`, `shadow`,
  `group`, `rc.inet1.conf`, `rc.local` or `rc.local_shutdown`; write such a file's
  exact path if you really want it overwritten.
- The rules apply with or without `--yes`; `--dry-run` shows what they would do.
- In `upgrade-dist` only `NEWCONFIG_KEEP` applies, so a file you keep is never
  replaced there, not even by "[A]ll new". `NEWCONFIG_OVERWRITE` and
  `NEWCONFIG_REMOVE` are not applied there: like the blacklist, a release jump
  shows every other file.
- `new-config` reads these keys even when the rest of the configuration is
  broken, like `new-config` itself.
- A `.new.kept` is no longer a pending `.new`: compare it by hand when you want,
  e.g. `diff /etc/ssh/sshd_config /etc/ssh/sshd_config.new.kept`.

### new-config after a package command

`install`, `reinstall`, `upgrade`, `upgrade-all`, `install-new`,
`install-template` and `revert-pkg` list the `.new` files they leave behind
(new ones apart from those already pending) and remind you to run
`slacker new-config`. A file a `NEWCONFIG_*` rule covers is marked with its key:

```
New configuration files were installed (your current ones were kept):
  /etc/ssh/sshd_config.new  (NEWCONFIG_KEEP)
  /etc/ntp.conf.new
```

`NEWCONFIG_AFTER` decides what comes after that list:

| Value | What happens |
|---|---|
| `off` *(default)* | only the reminder, as before |
| `ask` | `Run new-config now? [Y/n]` - Enter or `y` runs it, `n` leaves the reminder |
| `run` | new-config runs right away |

`ask` and `run` act only when someone is there to answer: input and output both
a terminal, no `--yes`, no `--dry-run`, and only when this run left new `.new`
files (older pending ones do not bring the menu back after every command).
Otherwise you get the reminder, so cron jobs, scripts, slacker-gui, a Dockerfile
`RUN ... -y`, `docker run -t` without `-i` and `ssh` without `-t` never have
`.new` files touched behind their back. Ctrl-D at the question means no. The
`NEWCONFIG_*` rules above are applied first, as always.

---

## Proxy

slacker connects directly by default and ignores the proxy variables of the
environment. To download through a proxy, set it in `slacker.conf`:

```
PROXY=http://proxy.lan:3128
NO_PROXY=localhost,.lan,192.168.0.0/16
```

| `PROXY` | Downloads go |
|---|---|
| unset or `off` *(default)* | direct |
| `http://host:port` | through that proxy, for http and https mirrors alike, however slacker is started (sudo, cron and slacker-gui clear the environment, so this is the way for them) |
| `env` | through `https_proxy`, `http_proxy` or `all_proxy` from the environment, with its `no_proxy`, as wget and curl read them (lowercase first, then uppercase) |

- Only `http://` proxies. https mirrors go through them as a CONNECT tunnel, so
  TLS and the certificate check still run end to end with the mirror; the proxy
  sees only the host name. A `socks5://` or `https://` proxy is an error, never
  a silent direct connection.
- A password goes in the URL, %-encoded (`%40` for `@`, `%23` for `#`):
  `PROXY=http://bob:p%40ss@proxy.lan:3128`. It is sent **only to the proxy**, for
  https mirrors. An http mirror behind a proxy that asks for a password fails
  (407); use https. `slacker.conf` is readable by every user, so they can read a
  password written there. See [[Security]].
- `NO_PROXY` takes names (each also matching its subdomains; a leading `.` is
  the same), IP addresses and CIDR ranges, each with an optional `:port`,
  separated by commas, or `*` for every host. Set in `slacker.conf`, it replaces
  the environment's `no_proxy`.
- `file://` sources never use a proxy.
- `slacker status` shows the proxy in use and where it comes from:
  `· Proxy      http://bob:***@proxy.lan:3128 (slacker.conf), not for: localhost,.lan,192.168.0.0/16`

---

## mirrors

A `slackpkg`-style catalogue. **Uncomment exactly one** line. None is active by
default; two or more active lines is an error. `http://`, `https://`, and
`file://` are all supported (the last for a local clone, NFS mount, or mounted
media); `ftp://` is not. The **shipped file already carries the right directory
for your arch and release** - you only choose which mirror.

The directory segment follows arch + release:

| Arch / release | Directory segment | Example active line |
|---|---|---|
| x86_64 · -current | `slackware64-current` | `https://mirrors.slackware.com/slackware/slackware64-current/` |
| 32-bit · -current | `slackware-current` | `https://mirrors.slackware.com/slackware/slackware-current/` |
| x86_64 · 15.0 | `slackware64-15.0` | `https://mirrors.slackware.com/slackware/slackware64-15.0/` |
| 32-bit · 15.0 | `slackware-15.0` | `https://mirrors.slackware.com/slackware/slackware-15.0/` |

```
# /etc/slacker/mirrors  - uncomment ONE line (the redirector is a good default)
https://mirrors.slackware.com/slackware/slackware64-current/
```

Not sure which to pick? `slacker find-mirror` probes the official list, drops
stale mirrors, and prints the fastest fresh one to paste here - it never edits
the file. Change your mirror by changing which line is uncommented.

---

## repos

Two kinds of line: **binary repositories** and **tag-priority** lines.

### What ships active by default

The installed `repos` comes with the official tree plus the three distribution
subtrees already enabled - note `patches` at the **highest** priority (200), so a
15.0 security/bugfix package always wins:

```
# [priority]  [name]      [url]              [flags]
100         slackware   mirror              official
200         patches     mirror/patches      subtree immutable
90          extras      mirror/extra        subtree immutable
85          testing     mirror/testing      subtree immutable
```

Third-party repos ship **commented out** below that block, ready to uncomment and
retune - e.g. `alienbob`, `conraid`, `restricted`, `multilib`, `csb` on -current/x86_64;
`mate`, `cbs` on 15.0/x86_64; a smaller set on 32-bit. Their URLs are already
matched to the build's arch and release (see the variants table below).

### Binary repository

```
priority  name  url|mirror|mirror/<subpath>  [official] [immutable] [subtree] [verify=...]
```

- **priority** - higher wins; must be **distinct** across binary repos (a
  duplicate is a fail-fast error).
- **url** - a literal URL, the keyword `mirror` (the active mirror, for the
  official repo), or `mirror/<subpath>` (the active mirror with a subpath
  appended, e.g. `mirror/extra` - tracks a subtree on whichever mirror you
  picked, without hardcoding the host).
- **flags** (any order): `official`, `immutable`, `subtree`, `verify=...`.

The full semantics of the flags, the priority model, and pinning are in
[[Repositories and Priority]].

### Tag-priority

```
priority  name  tag        # e.g.  100  SBo  _SBo
```

Gives packages carrying a build tag a priority on the same scale as repos, so
SBo/local/source packages are never silently migrated or downgraded by
`upgrade-all`. Tag-priority lines *may* share a priority.

Edit this file by hand, or let `add-repo` / `del-repo` / `add-tag` / `del-tag` do
it for you (validated, with a confirmation prompt).

### The shipped configs are arch- and release-specific

slacker ships **four** ready-made `/etc/slacker` sets; the build for your platform
embeds the matching one, so the directory segments and example URLs already fit
your system. The differences are only in the URL/segment text:

| | x86_64 · -current | 32-bit · -current | x86_64 · 15.0 | 32-bit · 15.0 |
|---|---|---|---|---|
| mirror dir | `slackware64-current` | `slackware-current` | `slackware64-15.0` | `slackware-15.0` |
| `alienbob` example | `.../current/x86_64` | `.../current/x86` | `.../15.0/x86_64` | `.../15.0/x86` |
| `CUMULATIVE_URL` | `.../slackware64-current` | `.../slackware-current` | *(revert is -current-only)* | *(revert is -current-only)* |
| `ARCH` hint | `x86_64` | `x86` | `x86_64` | `x86` |

The active default block (official + `patches`/`extras`/`testing`) is identical
across all four. You normally never touch this - it is chosen at install time.

---

## blacklist

One rule per line. Three kinds share the file:

| Rule | Written as | Managed with |
|---|---|---|
| freeze | `[@repo] PATTERN` | `frozen` / `unfrozen` |
| pin | `@repo 100% name` | `pin` / `unpin` |
| dependency exclusion | `!package:dependency` (or `!*:dependency`) | `nodep` / `undep` |

For a freeze, `PATTERN` is a Slackware series when it ends in `/`, otherwise a
**glob** *or* an unanchored **regex** matched against the full package id
`name-version-arch-build` (with no regex syntax it is a glob, so `*` is any run
of characters and a literal `.` stays literal). Full reference: [[Blacklist]].

---

## distro-upgrade.conf

Optional, and only consulted by `upgrade-dist` (see [[Distribution Upgrade]]). A
single key points a distribution upgrade at a **local** copy of the target
release instead of the network:

| Key | Meaning |
|---|---|
| `DISTRO_UPGRADE_MIRROR` | A local source for `upgrade-dist`: a local release tree (`file://` or `http://`), an NFS clone, or a mounted install ISO. When set, the upgrade re-points to this base and validates it (release segment matches the target, `PACKAGES.TXT` reachable) before the point of no return. Absent or empty, `upgrade-dist` uses the configured mirrors. |

```
# /etc/slacker/distro-upgrade.conf
DISTRO_UPGRADE_MIRROR=file:///mnt/iso/
```

> The local-source path is **implemented but not yet tested end-to-end** - treat
> it as experimental and keep backups. The network path is the tested one.

---

## Environment variables

| Variable | Effect |
|---|---|
| `NO_COLOR` | Disables all coloured output (also auto-disabled when stdout is not a terminal). |
| `SLACKER_MERGE` | External merge tool for `new-config`'s (M)erge action (default `vimdiff`). |
| `PAGER` | Pager for long output (`history`, `show-changelog`); defaults to `less -FRX`. |
| `https_proxy`, `http_proxy`, `all_proxy`, `no_proxy` | Read only with `PROXY=env` in `slacker.conf`; otherwise ignored. See [Proxy](#proxy). |
