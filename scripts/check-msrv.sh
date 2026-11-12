#!/bin/sh
# check-msrv.sh - is the rust-version in each Cargo.toml still true for the
# crates in its Cargo.lock? For slacker-cli and slacker-gui it prints the highest
# rust-version the locked crates declare, and fails when it is above ours.
# Run after every `cargo update`. Lives in scripts/, next to slacker-cli/ and
# slacker-gui/; it finds them from its own location, so it runs from anywhere.
#
# Needs cargo and python3. Reads only, changes nothing.

set -u
top=$(cd "$(dirname "$0")/.." && pwd)
rc=0

for crate in slacker-cli slacker-gui; do
  echo "== $crate"
  if [ ! -f "$top/$crate/Cargo.toml" ]; then
    echo "not found: $top/$crate"
    rc=1
    continue
  fi
  ( cd "$top/$crate" && cargo metadata --format-version 1 --locked 2>/dev/null ) | python3 -c '
import json, sys
try:
    meta = json.load(sys.stdin)
except ValueError:
    print("FAIL: cargo metadata failed (no access to the crate index, or Cargo.lock does not match Cargo.toml)")
    sys.exit(1)
ours = set(meta["workspace_members"])
key = lambda v: tuple((list(map(int, v.split("."))) + [0, 0])[:3])
own = None
need = []
for p in meta["packages"]:
    rv = p.get("rust_version")
    if p["id"] in ours:
        own = rv
    elif rv:
        need.append((key(rv), rv, p["name"], p["version"]))
need.sort(reverse=True)
top = need[0] if need else None
print("rust-version in Cargo.toml:", own or "(not set)")
if top:
    print("highest needed by Cargo.lock:", top[1], "(" + top[2] + " " + top[3] + ")")
if own and top and top[0] > key(own):
    print("FAIL: raise rust-version to", top[1])
    sys.exit(1)
print("OK")
' || rc=1
done

exit $rc
