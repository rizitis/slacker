# slacker blacklist reference

How to add packages and patterns to the slacker blacklist, both with the
`frozen` command and by editing the file by hand. This is a complete catalog of
the rule forms and the commands that create them.

## What the blacklist does

A blacklist rule matches packages. The effect depends on whether a matched
package is installed:

- Installed and matched: the package is **frozen**. It is never installed,
  upgraded, reinstalled, or removed by slacker, and it is never listed by
  `clean-system`.
- Not installed and matched: the package is **hidden** from `install-new`,
  from upgrades, and from `check-updates`. It is still shown by `search` and
  `info`, marked `[blacklisted]`.

## Rule syntax

Every entry is one rule:

```
[@repo] PATTERN
```

- `PATTERN` ending in `/` is a **series** rule (e.g. `kde/`); it matches a
  Slackware series.
- Any other `PATTERN` is an unanchored **regular expression**, matched against
  the full package id `name-version-arch-build` (slackpkg style).
- The optional `@repo` prefix scopes the rule to a single repository. For an
  available package this is its candidate repo; for an installed package it is
  the repo the package came from.

Because the regex is matched against the whole id and is unanchored:

- a bare `vlc` matches `vlc` and also `vlc-plugin-*` (substring),
- `^vlc-[0-9]` matches only the `vlc` package,
- `xf86-.*-202.*` matches date-versioned builds such as
  `xf86-video-intel-20260518_...`,
- anchor with `^` and `$` for strict matching.

The official repository is just a repo with a name. Use that name with `@`. In
a default setup it is `slackware`, so official-only rules look like
`@slackware PATTERN`.

---

## Method 1: the `frozen` command

```
slacker frozen RULE [RULE ...]
```

Each argument is one rule. The command validates every rule, shows what each
one will freeze, asks for confirmation, then appends the new rules to
`/etc/slacker/blacklist`. It requires root.

### Quoting

Quotes are for the shell, not for slacker. Use double quotes when a rule:

- contains a space (any `@repo PATTERN` rule), or
- contains a shell glob or metacharacter: `*` `?` `[` `]` `(` `)` `|` `$`.

Plain names and `series/` need no quotes. When in doubt, quote; double quotes
never hurt.

```
slacker frozen vlc                 # fine unquoted
slacker frozen kde/                # fine unquoted
slacker frozen "xlibre-*"          # MUST quote: * is a shell glob
slacker frozen "@alienbob vlc"     # MUST quote: contains a space
```

### One rule per call or many

```
slacker frozen vlc
slacker frozen vlc "xf86-.*-202.*" "kde/" "@alienbob discover"
```

### Forms

```
# exact-ish single package (note: also matches names containing it)
slacker frozen mozilla-firefox

# strict single package only
slacker frozen "^mozilla-firefox-[0-9]"

# regex across all repos
slacker frozen "xf86-.*-202.*"
slacker frozen "xlibre-.*"

# a whole series (all repos)
slacker frozen kde/

# scope to one repo: a package from a specific repo
slacker frozen "@alienbob discover"

# scope to one repo: a whole series from a specific repo
slacker frozen "@conraid kde/"

# official repo only (default name is slackware)
slacker frozen "@slackware mozilla-firefox"
slacker frozen "@slackware kde/"
slacker frozen "@slackware ^mozilla-firefox-"
```

### Validation and warnings

- A syntax error (for example `@repo` with no pattern) is fatal: nothing is
  written, and all problems found are listed at once so they can be fixed in
  one pass.
- A rule that parses but looks like a mistake is flagged, and you are asked
  whether to declare it anyway:
  - an `@repo` that names no active repository (likely a typo),
  - a plain regex that contains a space; package ids never contain spaces, so
    this usually means a forgotten `@` (for example `conraid foo` instead of
    `@conraid foo`) or a quoting slip.
- Rules already present in the file are skipped, and the confirmation count
  reflects only the rules actually being added.

### Flags

- `--yes` (or `-y`): skip the warnings and the confirmation prompt.
- `--config-dir DIR`: use a blacklist under `DIR` instead of `/etc/slacker`.

---

## Method 2: editing the file by hand

The blacklist is a plain text file at:

```
/etc/slacker/blacklist
```

Rules are one per line. Empty lines are ignored. A `#` starts a comment (whole
line or trailing). Edit it with any editor; no command is required.

Example file:

```
# Freeze the installed firefox; keep our current build.
mozilla-firefox

# Pin every xlibre package, any version.
xlibre-.*

# Hold back the whole KDE series.
kde/

# Only the alienbob build of discover.
@alienbob discover

# Official repo only: never let the official tree replace these.
@slackware ^mozilla-firefox-
@slackware kde/
```

The `frozen` command writes the exact same lines, so the two methods are
interchangeable. Hand-editing is the way to remove or change a rule.

---

## Rule catalog

| Rule you write                 | What it freezes / hides                                              |
|--------------------------------|---------------------------------------------------------------------|
| `vlc`                          | any package whose id contains `vlc` (vlc and vlc-plugin-*)           |
| `^vlc-[0-9]`                   | the `vlc` package only                                              |
| `mozilla-firefox`              | any id containing `mozilla-firefox`                                  |
| `xf86-.*-202.*`                | xf86 packages with a `202x` version                                 |
| `xlibre-.*`                    | every `xlibre-*` package                                            |
| `kde/`                         | the whole `kde` series, all repos                                   |
| `ap/`                          | the whole `ap` series, all repos                                    |
| `@alienbob vlc`                | `vlc` only when it comes from the `alienbob` repo                    |
| `@conraid kde/`                | the `kde` series only from the `conraid` repo                        |
| `@slackware mozilla-firefox`   | `mozilla-firefox` only from the official repo                        |
| `@slackware kde/`              | the `kde` series only from the official repo                        |

---

## Listing and removing

- There is no separate "list" command; read the file directly:

  ```
  cat /etc/slacker/blacklist
  ```

  You can also confirm an effect: a blacklisted but uninstalled package still
  appears in `slacker search NAME` and `slacker info NAME` marked
  `[blacklisted]`.

- There is no "unfreeze" command. To remove or change a rule, edit
  `/etc/slacker/blacklist` and delete or modify the line.

---

## Notes

- `frozen` only appends; it never removes. Re-adding an existing rule is a
  no-op (it is reported as already present).
- The regex is matched against the full id `name-version-arch-build`, not just
  the name. Use `^name-` style anchors when you need to be precise.
- `@repo` scoping uses the repository name from your `repos` file. Check the
  names with the repo list shown by `frozen` when a rule names an unknown repo,
  or read `/etc/slacker/repos`.
- Writing the blacklist requires root, because the file lives under
  `/etc/slacker`.
