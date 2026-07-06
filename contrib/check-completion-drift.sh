#!/bin/sh
# check-completion-drift.sh — verify contrib/slacker-completion.bash lists
# exactly the subcommands the real binary exposes. Hand-written completions
# drift silently when a subcommand is added or removed; this makes the drift
# a visible failure instead. Run from the repo root after changing the clap
# Cmd enum, or wire it into CI.
#
# Usage: contrib/check-completion-drift.sh [path/to/slacker]
#   SLACKER_BIN may also name the binary (default: `slacker` from PATH).

set -u
bin=${1:-${SLACKER_BIN:-slacker}}
comp_file=contrib/slacker-completion.bash

[ -f "$comp_file" ] || { echo "error: $comp_file not found (run from the repo root)"; exit 2; }
command -v "$bin" >/dev/null 2>&1 || { echo "error: slacker binary '$bin' not found"; exit 2; }

# List 1: the completion script's hardcoded _slacker_commands.
comp=$(sed -n "/^_slacker_commands='/,/'/p" "$comp_file" \
       | sed "s/^_slacker_commands=//" | tr -d "'" \
       | tr ' ' '\n' | grep -v '^$' | sort)

# List 2: what the binary actually exposes (clap prints them indented
# under "Commands:", first word of each line, until the next section).
real=$("$bin" --help 2>/dev/null \
       | awk '/^Commands:/{f=1;next} /^[A-Z][a-z]*:/{f=0} f && /^  [a-z]/{print $1}' \
       | sort)

[ -n "$real" ] || { echo "error: could not read the command list from '$bin --help'"; exit 2; }

if [ "$comp" = "$real" ]; then
    echo "OK: completion is in sync with the binary ($(printf '%s\n' "$real" | wc -l) commands)"
    exit 0
fi

echo "DRIFT between $comp_file and '$bin --help':"
printf '%s\n' "$comp" > /tmp/.drift_comp.$$
printf '%s\n' "$real" > /tmp/.drift_real.$$
diff /tmp/.drift_comp.$$ /tmp/.drift_real.$$ | sed -n \
    -e 's/^< /  only in completion (stale?):    /p' \
    -e 's/^> /  missing from completion (add):  /p'
rm -f /tmp/.drift_comp.$$ /tmp/.drift_real.$$
exit 1
