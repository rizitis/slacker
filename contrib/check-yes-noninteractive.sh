#!/bin/bash
# check-yes-noninteractive.sh - build gate: every mutating command must run
# unattended under --yes.
#
#   sh contrib/check-yes-noninteractive.sh target/release/slacker
#
# Why this exists
# ---------------
# slacker is used inside Docker images (see containers/README) where nothing can
# answer a prompt. A command that forgets to thread `cli.yes` down to its
# confirm() call still compiles, still passes `cargo test`, and only shows up as
# a container build that hangs on stdin or silently does nothing. This catches
# that at build time.
#
# How it works
# ------------
# It builds a throw-away sandbox (its own config dir, cache, state dir and
# package database - nothing under / is touched) with a fake PACKAGES.TXT, then
# runs each mutating command TWICE:
#
#   1. WITHOUT --yes -> the prompt MUST appear.  This is the control: it proves
#      the sandbox actually reaches the decision point, so a "pass" in step 2
#      can never be vacuous.
#   2. WITH --yes    -> no prompt may appear, and the command must not block.
#
# The mirror is a file:// URL pointing at nothing, so downloads fail instantly
# and offline: the test never needs the network and never installs anything.
# Package names are all prefixed `slkyes-` and ROOT is redirected, so the
# removal paths cannot touch a real package.
#
# Exit 0 = all good. Exit 1 = a command ignores --yes (or stopped prompting,
# which is also a bug worth knowing about). Exit 0 with a loud SKIP if not root.

set -u

SLACKER=${1:-target/release/slacker}

if [ ! -x "$SLACKER" ]; then
    echo "check-yes-noninteractive: no such binary: $SLACKER" >&2
    exit 1
fi
SLACKER=$(cd "$(dirname "$SLACKER")" && pwd)/$(basename "$SLACKER")

if [ "$(id -u)" -ne 0 ]; then
    echo "check-yes-noninteractive: SKIP (needs root; slacker refuses mutating"
    echo "                           commands as an ordinary user)"
    exit 0
fi

SB=$(mktemp -d /tmp/slacker-yes-check.XXXXXX) || exit 1
trap 'rm -rf "$SB"' EXIT

CONF=$SB/etc/slacker
mkdir -p "$CONF/templates" "$SB/cache/repos/slackware" "$SB/state" "$SB/pkgdb" \
         "$SB/root/var/lib/pkgtools/packages" "$SB/root/var/log/packages"

cat > "$CONF/slacker.conf" <<EOF
CACHE_DIR=$SB/cache
STATE_DIR=$SB/state
PKG_DB_DIR=$SB/pkgdb
ARCH=x86_64
RESOLVE_DEPS=on
RESOLVE_STOCK=no
VERIFY=none
EOF

# A file:// mirror that does not exist: metadata is read from the cache we write
# below, and any package download fails immediately, offline.
echo "file://$SB/nonexistent-mirror/" > "$CONF/mirrors"
echo "100 slackware mirror official" > "$CONF/repos"
: > "$CONF/blacklist"

# ---------------------------------------------------------------------------
# Fake repository metadata.
#   slkyes-hello / slkyes-world : plain new installs (two of them -> the picker)
#   slkyes-upme  2.0            : installed below as 1.0 -> an upgrade candidate
#   slkyes-clash                : declares a conflict with an installed package
# ---------------------------------------------------------------------------
emit_pkg() {
    cat >> "$SB/cache/repos/slackware/PACKAGES.TXT" <<EOF
PACKAGE NAME:  $1-$2-x86_64-$3.txz
PACKAGE LOCATION:  ./slackware64/l
${4:-}PACKAGE SIZE (compressed):  100 K
PACKAGE SIZE (uncompressed):  200 K
PACKAGE DESCRIPTION:
$1: $1 (slacker --yes build check)
$1:

EOF
}
: > "$SB/cache/repos/slackware/PACKAGES.TXT"
emit_pkg slkyes-hello 1.0 1
emit_pkg slkyes-world 2.0 3
emit_pkg slkyes-upme  2.0 1
emit_pkg slkyes-clash 1.0 1 "PACKAGE CONFLICTS:  slkyes-old
"

# Installed set: an older upme (upgradable) and a package the repo conflicts
# with / that clean-system will see as foreign.
touch "$SB/pkgdb/slkyes-upme-1.0-x86_64-1" "$SB/pkgdb/slkyes-old-0.9-x86_64-1"

printf 'slkyes-hello\nslkyes-world\n' > "$CONF/templates/yescheck.template"
printf 'slkyes-upme\n'                > "$CONF/templates/yesrm.template"

# ---------------------------------------------------------------------------
# Anything slacker prints when it is about to wait on stdin. Colour is off for
# piped output, but strip ANSI anyway so a future change cannot blind the test.
# ---------------------------------------------------------------------------
PROMPTS='\[y/N\]|Choice \[|Enter numbers to|Pinned packages|press \[Enter\] to remove all'

pass=0
fail=0

# run <label> <args...>
run() {
    ROOT="$SB/root" NO_COLOR=1 timeout 60 "$SLACKER" --config-dir "$CONF" "$@" \
        </dev/null 2>&1 | sed 's/\x1b\[[0-9;]*m//g'
    return "${PIPESTATUS[0]}"
}

check() {
    label=$1; shift

    out=$(run "$@")
    if ! printf '%s\n' "$out" | grep -Eq "$PROMPTS"; then
        echo "FAIL  $label"
        echo "      control run (no --yes) never reached a prompt - the test"
        echo "      case no longer exercises the confirmation path."
        printf '%s\n' "$out" | sed 's/^/      | /' | tail -12
        fail=$((fail + 1))
        return
    fi

    out=$(run "$@" --yes)
    rc=$?
    if [ "$rc" -eq 124 ]; then
        echo "FAIL  $label --yes  (timed out: blocked waiting on stdin)"
        fail=$((fail + 1))
        return
    fi
    if printf '%s\n' "$out" | grep -Eq "$PROMPTS"; then
        echo "FAIL  $label --yes  (prompted anyway)"
        printf '%s\n' "$out" | grep -E "$PROMPTS" | sed 's/^/      > /'
        fail=$((fail + 1))
        return
    fi
    echo "ok    $label"
    pass=$((pass + 1))
}

echo "check-yes-noninteractive: $SLACKER"

check "install (picker + confirm)"  install slkyes-hello slkyes-world
check "install (conflict)"          install slkyes-clash
check "upgrade"                     upgrade slkyes-upme
check "upgrade-all"                 upgrade-all
check "install-new"                 install-new
check "install-template"            install-template yescheck
check "remove"                      remove slkyes-upme
check "remove-template"             remove-template yesrm
check "clean-system"                clean-system
check "frozen"                      frozen slkyes-hello

echo
if [ "$fail" -ne 0 ]; then
    echo "check-yes-noninteractive: $fail failed, $pass passed"
    echo "Either a command ignores --yes - in a container that hangs on stdin or"
    echo "silently changes nothing, so thread cli.yes into its confirm() - or a"
    echo "test case above no longer reaches the prompt it was written to guard."
    exit 1
fi
echo "check-yes-noninteractive: all $pass commands honour --yes"
exit 0
