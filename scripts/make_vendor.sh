#!/bin/bash
#
# make-vendors.sh
#
# Generate vendored Cargo dependency tarballs for slacker-cli and
# slacker-gui, and place the resulting archives in <root>/vendors.
#
# Layout:
#   <root>/
#     slacker-cli/Cargo.toml
#     slacker-gui/Cargo.toml
#     vendors/
#     <script_dir>/make-vendors.sh
#
# Note: slacker-vendor-generate.py must live next to this script;
#       staging happens in the script's directory, so it can be
#       invoked from anywhere.

set -euo pipefail

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$SCRIPT_DIR")"
VENDORS_DIR="$ROOT/vendors"
CLI_SRC="$ROOT/slacker-cli"
GUI_SRC="$ROOT/slacker-gui"
GENERATOR="slacker-vendor-generate.py"

readonly SCRIPT_DIR ROOT VENDORS_DIR CLI_SRC GUI_SRC GENERATOR

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log() {
    printf '\n---------- %s ----------\n\n' "$*"
}

# Print the version field of a crate's Cargo.toml.
crate_version() {
    awk -F'"' '/^version/ {print $2; exit}' "$1/Cargo.toml"
}

# Remove any previously generated archives from the vendors directory.
clean_vendors_dir() {
    mkdir -p "$VENDORS_DIR"
    pushd "$VENDORS_DIR" >/dev/null || exit 1
    ls -la
    rm -rf -- * || true
    popd >/dev/null || exit 1
}

# Run the vendor generator for a crate and stage its outputs.
#   $1  crate source directory
#   $2  staging directory name (e.g. slacker-cli-vendor-1.2.3)
vendor_crate() {
    local src=$1
    local stage=$2

    python "$GENERATOR" "$src"
    mv vendor.tar.gz "$stage/$stage.tar.gz"
    mv vendor.json cargo-config.toml "$stage/"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
    cd "$SCRIPT_DIR" || exit 1
    clean_vendors_dir

    local cli_ver gui_ver
    cli_ver="$(crate_version "$CLI_SRC")"
    gui_ver="$(crate_version "$GUI_SRC")"

    local cli_stage="slacker-cli-vendor-$cli_ver"
    local gui_stage="slacker-gui-vendor-$gui_ver"
    local cli_tar="slacker-cli-$cli_ver.tar.gz"
    local gui_tar="slacker-gui-$gui_ver.tar.gz"

    log "Creating staging directories"
    mkdir -p "$cli_stage" "$gui_stage"

    log "Generating vendor for slacker-cli $cli_ver"
    vendor_crate "$CLI_SRC" "$cli_stage"

    log "Generating vendor for slacker-gui $gui_ver"
    vendor_crate "$GUI_SRC" "$gui_stage"

    log "Creating archives"
    tar -cvzf "$cli_tar" "$cli_stage/"
    tar -cvzf "$gui_tar" "$gui_stage/"

    log "Moving archives to $VENDORS_DIR"
    mv "$cli_tar" "$gui_tar" "$VENDORS_DIR"

    log "Cleaning up staging directories"
    rm -rf -- "$cli_stage/" "$gui_stage/"
}

main "$@"
