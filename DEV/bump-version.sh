#!/bin/bash
#
# bump-version.sh - set the slacker version in every place it appears.
#
# Updates, in one shot:
#   - Cargo.toml          version = "X.Y.Z"   (also feeds `slacker --version`)
#   - man/slacker.8       .TH ... "slacker X.Y.Z" ...
#   - slacker.SlackBuild  VERSION=${VERSION:-X.Y.Z}  (or VERSION=X.Y.Z)
#
# Run from anywhere; it locates the project root (the parent of this DEV/ dir).
#
# Usage:
#   DEV/bump-version.sh 0.2.0
#   DEV/bump-version.sh --dry-run 0.2.0     # show changes without writing
#
# License: Apache-2.0  -  Ioannis Anagnostakis (rizitis)

set -euo pipefail

dry_run=0
if [ "${1:-}" = "--dry-run" ]; then
    dry_run=1
    shift
fi

new="${1:-}"
if [ -z "$new" ]; then
    echo "usage: $(basename "$0") [--dry-run] X.Y.Z" >&2
    exit 2
fi

# Validate semantic version X.Y.Z (digits only).
if ! printf '%s' "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "error: '$new' is not a valid version (expected X.Y.Z)" >&2
    exit 2
fi

# Project root = parent of the directory holding this script.
script_dir="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$script_dir")"

cargo_toml="$root/Cargo.toml"
man_page="$root/man/slacker.8"
slackbuild="$root/slacker.SlackBuild"

# Read the current version from Cargo.toml (source of truth).
old="$(grep -m1 -E '^version[[:space:]]*=' "$cargo_toml" | sed -E 's/.*"([^"]+)".*/\1/')"
if [ -z "$old" ]; then
    echo "error: could not read current version from $cargo_toml" >&2
    exit 1
fi

if [ "$old" = "$new" ]; then
    echo "version is already $new; nothing to do"
    exit 0
fi

echo "bumping $old -> $new"
[ "$dry_run" = 1 ] && echo "(dry-run: no files will be written)"
echo

# Helper: apply an in-place sed only if the pattern matches, reporting clearly.
apply() {
    local label="$1" file="$2" expr="$3"
    if [ ! -f "$file" ]; then
        echo "  skip  $label: $file not found"
        return 0
    fi
    if ! grep -Eq "$4" "$file"; then
        echo "  warn  $label: pattern not found in $file (left unchanged)"
        return 0
    fi
    if [ "$dry_run" = 1 ]; then
        echo "  would update $label:"
        sed -E "$expr" "$file" | grep -E "$4" | sed 's/^/      /'
    else
        sed -i -E "$expr" "$file"
        echo "  ok    $label updated"
    fi
}

# 1) Cargo.toml  - first `version = "..."` line in [package].
apply "Cargo.toml" "$cargo_toml" \
    "0,/^version[[:space:]]*=/s/^(version[[:space:]]*=[[:space:]]*\")[^\"]+(\")/\1$new\2/" \
    "^version[[:space:]]*="

# 2) man page .TH line - the "slacker X.Y.Z" field.
apply "man/slacker.8" "$man_page" \
    "s/(slacker )[0-9]+\.[0-9]+\.[0-9]+/\1$new/" \
    "slacker [0-9]+\.[0-9]+\.[0-9]+"

# 3) SlackBuild VERSION= line (handles both VERSION=${VERSION:-X} and VERSION=X).
apply "slacker.SlackBuild" "$slackbuild" \
    "s/(VERSION=\\\$\{VERSION:-)[0-9]+\.[0-9]+\.[0-9]+(\})/\1$new\2/; s/(^VERSION=)[0-9]+\.[0-9]+\.[0-9]+/\1$new/" \
    "VERSION="

echo
if [ "$dry_run" = 1 ]; then
    echo "dry-run complete - re-run without --dry-run to apply."
else
    echo "done. Next steps:"
    echo "  (NOTE: BEFORE RELEASE) update the date in man/slacker.8 .TH if needed"
fi
