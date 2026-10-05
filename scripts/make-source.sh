#!/bin/bash

if [ "$EUID" -eq 0 ]; then
  echo "Είσαι Μαλάκας: Αυτό το script δεν πρέπει να εκτελείται ως root." >&2
  exit 1
fi

CWD="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$CWD")"

CLI_SRC="$ROOT/slacker-cli"
GUI_SRC="$ROOT/slacker-gui"

FILES="README.md NEWS LICENSE TODO slack-desc doinst.sh slacker.SlackBuild slacker-banner.svg slacker.nfo"
DONT="containers target vendor"

PRGNAM=slacker
VERSION=$(awk -F'"' '/^version/ {print $2; exit}' "$CLI_SRC/Cargo.toml")

DIRNAME="$PRGNAM-$VERSION-source"
OUTPUT="/tmp/$DIRNAME"

# Καθαρισμός αν υπάρχει ήδη
rm -rf "$OUTPUT" || exit 1
mkdir -p "$OUTPUT" || exit 1

# Αντιγραφή πηγαίου κώδικα
cp -R "$CLI_SRC" "$GUI_SRC" "$OUTPUT/"

# Αντιγραφή λοιπών αρχείων από το ROOT
(cd "$ROOT" && cp -R $FILES "$OUTPUT/")

# Αφαίρεση ανεπιθύμητων φακέλων/αρχείων
for item in $DONT; do
    find "$OUTPUT" -name "$item" -exec rm -rf {} +
done

# Δημιουργία tarball & MD5
tar -cvzf "/tmp/$DIRNAME.tar.gz" -C /tmp "$DIRNAME"
md5sum "/tmp/$DIRNAME.tar.gz" > "/tmp/$DIRNAME.md5"

# Καθαρισμός προσωρινού φακέλου
rm -rf "$OUTPUT" || exit 1

echo ""
echo "Done: /tmp/$DIRNAME.tar.gz and MD5"
