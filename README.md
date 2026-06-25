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
- Dependencies come **only** from a package's own `.dep` file 
  no dependency *guessing* for official packages - Slackware tradition.
- Synchronous; heavy lifting (bzip2 for MANIFEST, GPG) shells out to the
  system tools Slackware already ships, so no extra Rust deps.
- Everything a user edits is plain text.

# wiki
[wiki](https://forge.slackware.nl/rizitis/slacker/wiki)

## NOTE: 
**slacker** source code AND **slacker** [wiki](https://forge.slackware.nl/rizitis/slacker/wiki) are a **Work in process** (WIP) beta and unstable.
