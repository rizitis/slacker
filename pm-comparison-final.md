# Package manager comparison — slacker vs APT / DNF / Pacman / Zypper / APK / XBPS

*Honest assessment, July 2026, based on a full read of the actual 0.10.1+ source (all 20 modules), the real Cargo.toml (4 direct deps, rustls, MSRV 1.85.1), the SlackBuild, and production configs. Where slacker loses, we say so; where it does things nobody else does, we show them.*

---

## First: what slacker does NOT do — **on purpose, because pkgtools already do it**

This is the single most misunderstood point in any comparison. slacker is a **thin layer over Slackware's native pkgtools**, which have been battle-tested for over three decades. Several "missing features" are not gaps — they are a deliberate division of labor:

| "Missing" in slacker | Who actually does it | Since |
|---|---|---|
| Transaction engine (unpack, install, upgrade, remove, package DB records, doinst.sh execution) | `installpkg` / `upgradepkg` / `removepkg`. slacker **never reimplements or parses them** — it feeds them only verified packages, via argv (no `sh -c`), and reads the *result* (the DB record) where safety demands it | ~1993 |
| First-line `.new` config handling | `doinst.sh config()` already moves a fresh `.new` into place when no old file exists and removes a `.new` identical to the old one — **before slacker ever runs**. slacker's `new-config` handles only the genuine conflicts that remain | decades |
| Package change records (the raw data behind `history`) | The pkgtools admin DB (`packages/`, `removed_packages/`, `-upgraded-` records) records every install/upgrade/removal — including ones made by hand or by other tools. slacker keeps **no log of its own**; it reads the system's | decades |
| Package building | `SlackBuild` + `makepkg` — slacker installs, it does not build | decades |
| Dependency solver for the official tree | **Slackware's full-install design**: the official tree ships no dependency metadata *on purpose*. A solver would have nothing to solve. slacker resolves deps only where a repo actually declares them (`.dep` files, or `PACKAGE REQUIRED:` in PACKAGES.TXT) | design |

Judge slacker as `slacker + pkgtools`, the way you judge apt as `apt + dpkg`. Reimplementing any of the above would violate its founding invariant: *"ο Pat ξέρει"* — never second-guess pkgtools.

---

## The big table

| Criterion | **slacker** | **XBPS** (Void) | APT (Debian) | DNF (Fedora) | Pacman (Arch) | Zypper (openSUSE) | APK (Alpine) |
|---|---|---|---|---|---|---|---|
| Language / size | Rust, **~17.8k LOC, 1 binary** | C, tens of kLOC, ~15 binaries | C++, huge | Python+C (libdnf5) | C, medium | C++ (libzypp), large | C, small |
| PM's own dependencies | **4 crates** (clap, ureq/rustls, md-5, regex); gpg/bzip2/sha256sum shelled out | zlib, OpenSSL/LibreSSL, **libarchive** | many | many | libarchive, gpgme, curl | many | minimal |
| Dependency solver | **No** — names from `.dep` / `PACKAGE REQUIRED` only, no version constraints; official tree = no auto-deps (see division-of-labor above) | Full: version constraints, virtual packages, **automatic SONAME/shlib deps** verified per transaction | Full SAT-ish | Full SAT (libsolv) | Full (versioned, provides) | Full SAT (libsolv) | Full |
| Multi-repo & priority | **Numeric per-repo priority + build-tag "virtual sources"** (SBo, local tags join the same priority model) | Declaration order (first repo providing the name wins) | apt pinning (powerful but arcane) | repo priority + modularity | Order in pacman.conf | repo priority + vendor stickiness | order + tagged repos |
| Never-migrate-down invariant | **Yes — nowhere else**: an installed package is NEVER silently migrated to a lower-priority source; SBo/local builds are protected via tag priorities | Partial: `repolock` pins a package to its install repo (manual, per package) | Via pinning (manual, fragile) | Not natively | No | Vendor-change protection (closest) | No |
| Pin a package to a repo | `pin repo:name` (persistent, in the blacklist file, bypasses priority) + transient `repo:name` | `repolock` (only to the repo it came from) | per-origin pinning | `--repo`, versionlock plugin | Not natively | vendor lock | pin to tagged repo |
| Freeze / hold | **Scoped freeze-fallback**: `@repo PATTERN` freezes ONLY that repo/tag's candidate; resolution falls through to the next non-frozen candidate by priority, **never below the installed source's priority**; held only when EVERY candidate is frozen; globs + regex + series | `hold` (total, per package), `ignorepkg` | `apt-mark hold` (total) | versionlock (total) | IgnorePkg (total) | lock (total) | hold (total) |
| Signatures / transport | GPG on CHECKSUMS **+ per-package `.asc`** verified at install; rustls built in (no system libssl) | Signed repodata + **per-package RSA** signatures | InRelease GPG (per-repo) | repo GPG + per-rpm | per-pkg GPG (web-of-trust keyring) | repo GPG + per-rpm | signed index |
| **Repo credentials** | **Zypper-modeled, done properly**: per-repo HTTP Basic via `credentials=<name>` referencing `credentials.d/<name>` — **secrets never in the repos file, never in a URL, never in logs or output**; longest-prefix registry; **https-only by default** (plaintext http refused unless an explicit per-repo `insecure` flag); `add-repo` refuses http+credentials; `status` audits credential transport | None native (user:pass in URL at best — leaks into logs/ps) | auth.conf.d (proper) | username/password inline in `.repo` files (plaintext in config) | None native (XferCommand workaround) | credentials.d (the original model) | URL embedding |
| **Honest identity on the wire** | **Full zypper-style UA**: `slacker/0.10.1 (rustls) Slackware-current-x86_64` — names the tool, version, TLS backend, distro, release, arch, so mirror operators see a legitimate package manager and can whitelist it. **Never browser-spoofing, never anonymous** | libfetch UA (basic) | `Debian APT-HTTP/x.y` (basic) | libdnf UA (basic) | curl default-ish | **Full**: `ZYpp x.y (curl) openSUSE-Leap-15.x-x86_64` — the model slacker adopted | basic |
| TOFU / key pinning | **Pins ALL fingerprints** on first contact, re-checks on EVERY update; KeyChanged → **automatic hard quarantine with a recorded reason** | **Closest rival**: fingerprint-accept on first sync, refusal on mismatch — but no quarantine/reason/override workflow | No (keys pre-installed) | No | keyring package | No | No |
| Repo vetting / quarantine | **Unique**: add-repo/vet-repo run 5 checks (http-without-insecure, fetch, **PACKAGES.TXT scan for path-traversal filenames → "MALICIOUS"**, loadable pkgs, GPG); soft (unreachable, auto-retried) vs hard (needs `trust-repo`) markers with recorded reasons | No | No | No | No | No | No |
| Verify installed files | **No** (verification at download only; the install result is pkgtools' domain, and core installs ARE record-checked in dist) | **Yes** — `xbps-pkgdb -a` | debsums (extra) | `rpm -V` | `pacman -Qk` | `rpm -V` | `apk audit` |
| Rollback / downgrade | `revert-pkg`: previous official versions from the cumulative archive, **mandatory GPG** against the pinned key, danger banner on foundational pkgs — narrow scope (official, -current only) | Only from cached .xbps (xdowngrade); repos keep latest only | Only if still in the pool | **dnf history undo/rollback** | cache/ALA (manual) | **zypper + snapper/btrfs = full snapshots** (the king here) | limited |
| Dist-upgrade | `upgrade-dist`: **escape kit** (config backup + installed-set template), typed point-of-no-return, disk gates BEFORE any change, core-first order (glibc-solibs→pkgtools→tar/xz/gzip/findutils), **GnuPG chain upgraded last** (gpg keeps verifying throughout), batches download→install→delete (cache never holds the whole tree), idempotent re-run, clean-system skipped on partial failure | Rolling — N/A | do-release-upgrade | system-upgrade plugin | Rolling | `zypper dup` (+snapper net) | Rolling/edge |
| History | **No log of its own** — derived from the pkgtools DB, so it captures changes made by installpkg/sbopkg/slackpkg/by hand too; tenure pairing, reinstall detection, self-calibrating local clock | None | history.log (misses non-apt changes) | dnf history (same) | pacman.log (same) | zypper history (same) | None |
| .new / config merge | **Built in**: broken/identical/conflicts triage, bulk K/O/R/P, per-file K/O/R/M/D with colored diff + `$SLACKER_MERGE` (vimdiff); runs even with a broken mirror. (First-line .new handling is doinst.sh's job — see above) | keeps yours + `.new-<version>`; merge via external xtools | conffile prompt | .rpmnew/.rpmsave (manual) | .pacnew + pacdiff (external) | .rpmnew | .apk-new |
| Doctor / setup health | **`status`**: environment/tools, config validation, mirror **freshness vs upstream osuosl**, per-repo release & arch mismatch, empirical GPG verification, **permissions/symlink audit of its OWN trust files**, credentials transport audit, pins/blacklist sanity, ordered next-steps recipe | `xbps-pkgdb` (DB integrity only) | No single tool | No single tool | No | No single tool | No |
| Mirror tooling | `find-mirror`: latency + freshness **in one request per mirror**, parallel probe, propose-only, works before any mirror is configured | **xmirror** (official, no freshness ranking) | netselect-apt (extra) | metalink **automatic** (best UX) | reflector (extra) | mirrorbrain automatic | manual |
| Parallel downloads | Yes (MAX_PARALLEL 1–16, default 4) + **per-batch verification summary** (how many GPG / integrity-only / unverified) | **No** (serial) | Yes | Yes | Yes (ParallelDownloads) | Yes | Yes |
| Implementation safety | Memory-safe Rust; **all shell-outs argv (no `sh -c`)**; double path-traversal barrier; 512MiB/8GiB size caps + anti-decompression-bomb; symlink refusal on every write; flock with PID | C; libarchive on the extraction path (CVE history) | mature C++, many eyes | C/Python | C, lean | C++ | C, tiny attack surface |
| **Documentation** | man page covering all 38 commands (groff-lint-clean); **wiki in 11 languages** (EN, EL, FR, ES, IT, RU, ZH, AR, HI, PT, DE) kept claim-accurate against the code; `structure.md` + `CODE_MAP` for contributors; Quick-Start; all written by the author **in lockstep with the code** | Void Handbook (good) | Debian docs (vast) | Fedora docs | **ArchWiki — the gold standard, full stop** | openSUSE docs | Alpine wiki |
| Bus factor / audit / maturity | **1 person**, 2026, no external audit, 189 unit tests | small Void team, ~15 years in production | all of Debian, decades | Red Hat | Arch team | SUSE | Alpine team |
| Ecosystem | Slackware-current (+15.0 via the patches-repo model) | Void | half of Linux | Fedora/RHEL | Arch + derivatives | SUSE | Alpine/containers |

---

## Head-to-head with XBPS — the closest rival

xbps deserves its own section: a small-team, single-distro PM that takes cryptographic verification seriously. Honestly:

**Where xbps leads:**
- **SONAME/shlib dependency tracking**: automatically generated shared-library dependencies, verified per transaction — a broken-soname upgrade is refused. Its biggest technical edge; slacker has nothing comparable (and deliberately won't, for the official tree — see division-of-labor).
- **Real solver** with version constraints and virtual packages.
- **`xbps-pkgdb -a`**: checksum verification of installed files.
- ~15 years in production, team > 1.

**Where slacker leads:**
- **Freeze-fallback with scoped rules and a priority floor** — xbps has only a total `hold`. "Freeze only the testing candidate and let the official update flow, never downgrading the source" cannot be expressed in xbps.
- **Repo quarantine with malicious-metadata detection**: a repo advertising path-traversal filenames is auto-frozen as MALICIOUS, with a recorded reason and an explicit override. xbps refuses a bad signature but has no concept of "this source is quarantined, here's why, here's how you lift it".
- **Proper credentials**: xbps has no native private-repo authentication; slacker has the full zypper model with https-only transport enforcement.
- **Doctor + freshness**: `status` audits even the permissions/symlinks of its own trust anchors and how far your mirror lags upstream. xbps has nothing unified.
- **History from the system DB** (captures out-of-band changes); xbps has no history at all.
- **Built-in config merge** with diff/merge tooling; on Void you reach for xtools.
- **Parallel downloads** with a batch verification summary; xbps fetches serially.
- Audit surface: 1 binary / 4 crates / rustls, vs C + libarchive + openssl.
- **Documentation breadth**: an 11-language wiki from a one-person project vs the (good, English) Void Handbook.

**A draw (both serious):** first-contact repo trust. xbps asks you to accept the key fingerprint on first sync and refuses a mismatch; slacker pins ALL fingerprints, re-checks on every update, and turns a mismatch into a hard quarantine with a reason. Same spirit — slacker's is the more institutionalized version.

---

## What slacker has that **none** of the six have

1. **Scoped freeze-fallback + priority floor** — freezing per repo/tag/pattern/series that falls through in a controlled way, never downgrading the source.
2. **Build-tag virtual sources** in the unified priority model: your `_SBo`/local builds are first-class citizens with their own priority, automatically protected by the never-migrate-down invariant.
3. **Repo vetting/quarantine with malicious-metadata detection** (path-traversal scan) and soft/hard semantics with recorded reasons.
4. **A `status` doctor that audits itself** (permissions/symlinks of its trust anchors) and your mirror's freshness against upstream.
5. **Log-less history** — complete even if you worked with installpkg/sbopkg in between.
6. **upgrade-dist with an escape kit** and batched download→install→delete so the cache never needs the whole distribution.
7. **revert-pkg** with mandatory GPG verification against the pinned official key.
8. **Interactive plan UX**: pickers showing the full `old-tag → new-tag` transition, "Also offered by other repos" with a **live `.dep` diff** of the alternatives, the protected-deps dialog, conflicts/suggests surfacing.
9. **Honest, transparent network identity** — it shows up in server logs as exactly what it is.

## What the others have that slacker does **not** (no sugar-coating)

- A version-constrained solver — and especially xbps's SONAME tracking.
- Installed-file verification (`rpm -V` / `xbps-pkgdb -a` / debsums).
- Snapshots / transactional rollback (zypper+snapper, dnf history) — zypper's dist-upgrade safety net remains unmatched.
- Alternatives, multiarch, delta downloads.
- Maintainer teams, external eyes/audits, decades of maturity.
- Bus factor: **1**. The project's weakest point, and it is not hidden.

## Verdict

slacker is not trying to be DNF. It is a **thin layer over pkgtools** — everything pkgtools have done reliably for thirty years, slacker refuses to reimplement. What it adds, it takes more seriously than anyone in this table: (a) the **source model** (priority + tags + pin + scoped freeze with a floor), (b) **trust in repositories** (TOFU, vetting, quarantine, doctor, proper credentials, honest identity), (c) an **auditable surface** (1 binary, 4 deps, argv-only). It pays for that in solver, rollback and team size. On its home turf — multi-repo Slackware with third-party sources and hand-built packages — it does things none of the six do.

---

*Prepared by Claude (Anthropic AI) at the request of the slacker author, based on a full read of the slacker 0.10.x source; reviewed by rizitis. Claims about the other package managers reflect their documented behavior and were not source-audited. Corrections welcome — honesty over marketing.*
