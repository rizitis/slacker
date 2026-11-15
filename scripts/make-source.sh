#!/bin/bash

if [ "$EUID" -eq 0 ]; then
  echo "Είσαι Μαλάκας: Αυτό το script δεν πρέπει να εκτελείται ως root." >&2
  exit 1
fi

CWD="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$CWD")"

CLI_SRC="$ROOT/slacker-cli"
GUI_SRC="$ROOT/slacker-gui"
FLATPAK_SRC="$ROOT/slacker-flatpak"

FILES="README.md NEWS LICENSE TODO slack-desc doinst.sh slacker.SlackBuild slacker-banner.svg slacker.nfo"
DONT="containers target vendor"

PRGNAM=slacker
VERSION=$(awk -F'"' '/^version/ {print $2; exit}' "$CLI_SRC/Cargo.toml")

DIRNAME="$PRGNAM-$VERSION-source" # (cli+gui)
DIRNAME2="$PRGNAM-flatpak-$VERSION-source" # (cli+gui+flatpak)
DIRNAME3="$PRGNAM-cli-$VERSION-source" # (cli)
OUTPUT="/tmp/$DIRNAME"

# Καθαρισμός αν υπάρχει ήδη
rm -rf "$OUTPUT" || exit 1
mkdir -p "$OUTPUT" || exit 1

# Αντιγραφή πηγαίου κώδικα
cp -R "$CLI_SRC" "$GUI_SRC" "$FLATPAK_SRC" "$OUTPUT/"

# Αντιγραφή λοιπών αρχείων από το ROOT
(cd "$ROOT" && cp -R $FILES "$OUTPUT/")

# Αφαίρεση ανεπιθύμητων φακέλων/αρχείων
for item in $DONT; do
    find "$OUTPUT" -name "$item" -exec rm -rf {} +
done

# Make one more copy for slacker-flatpak
tar -cvzf "/tmp/$DIRNAME2.tar.gz" -C /tmp "$DIRNAME"
md5sum "/tmp/$DIRNAME2.tar.gz" > "/tmp/$DIRNAME2.md5"

# Αφαίρεση flatpak
find "$OUTPUT" -name "slacker-flatpak" -exec rm -rf {} +

# Δημιουργία slacker (cli+gui) tarball & MD5
tar -cvzf "/tmp/$DIRNAME.tar.gz" -C /tmp "$DIRNAME"
md5sum "/tmp/$DIRNAME.tar.gz" > "/tmp/$DIRNAME.md5"

# Αφαίρεση gui
find "$OUTPUT" -name "slacker-gui" -exec rm -rf {} +

# Make one more copy for slacker-cli (no gui)
tar -cvzf "/tmp/$DIRNAME3.tar.gz" -C /tmp "$DIRNAME"
md5sum "/tmp/$DIRNAME3.tar.gz" > "/tmp/$DIRNAME3.md5"

# Καθαρισμός προσωρινού φακέλου
rm -rf "$OUTPUT" || exit 1

echo ""
echo "Done: /tmp/$DIRNAME.tar.gz and MD5"
echo "Done: /tmp/$DIRNAME2.tar.gz and MD5"
echo "Done: /tmp/$DIRNAME3.tar.gz and MD5"
echo ""
