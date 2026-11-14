# slacker: εξαρτήσεις από το σύστημα

Ονόματα πακέτων Slackware. Πηγές: `readelf -d` στα binaries, `Cargo.lock`
(ποια crates θέλουν C compiler ή pkg-config), ο κώδικας (ποια προγράμματα
καλούνται) και το `slacker.SlackBuild`.

## Build time

| Εξάρτηση | slacker-cli | slacker-gui | slacker-flatpak | Γιατί |
|---|---|---|---|---|
| rust (rustc, cargo) | ✓ ≥ 1.89 | ✓ ≥ 1.92 | ✓ ≥ 1.92 | μεταγλώττιση |
| gcc | ✓ | ✓ | ✓ | linker· στο cli και για το `ring` (TLS) και το ενσωματωμένο SQLite |
| pkg-config | | ✓ | ✓ | τα crates του gtk-rs βρίσκουν τις βιβλιοθήκες |
| gtk4 (headers) | | ✓ ≥ 4.4 | ✓ ≥ 4.4 | |
| libadwaita (headers) | | ✓ ≥ 1.5 | ✓ ≥ 1.5 | |
| glib2, pango (headers) | | ✓ | ✓ | |
| gdk-pixbuf2 (headers) | | ✓ | | λογότυπο |
| python3 | ✓ | | | `gen-completions.py --check` στο SlackBuild |
| binutils, file, gzip, pkgtools | ✓ | ✓ | (όταν πακεταριστεί) | `strip`, man σε gzip, `makepkg` |
| cargo-auditable, cargo-audit | προαιρετικά | προαιρετικά | | το SlackBuild τα χρησιμοποιεί αν υπάρχουν |

Στο Slackware τα headers είναι μέσα στα ίδια πακέτα· δεν υπάρχουν χωριστά -dev.

## Run time

| Εξάρτηση | slacker-cli | slacker-gui | slacker-flatpak | Γιατί |
|---|---|---|---|---|
| glibc | ✓ | ✓ | ✓ | `libc`, `libm`, loader |
| libgcc_s (gcc) | ✓ | ✓ | ✓ | |
| pkgtools | ✓ | | | `installpkg`, `upgradepkg`, `removepkg` |
| gnupg2 | ✓ | | | `gpg`/`gpg2`, έλεγχος υπογραφών |
| coreutils | ✓ | | | `sha256sum`, `uname`, `id`, `date`, `df` |
| bzip2 | ✓ | | | MANIFEST για το `file-search` |
| diffutils | ✓ | | | `diff` στο `new-config` |
| vim | προαιρετικό | | | `vimdiff` στη συγχώνευση του `new-config` (αλλάζει με `SLACKER_MERGE`) |
| slacker | | ✓ | | όλες οι ενέργειες γίνονται μέσω του slacker |
| gtk4, libadwaita, glib2, pango | | ✓ | ✓ | |
| gdk-pixbuf2 | | ✓ | | λογότυπο |
| librsvg | | προαιρετικό | | φορτώνει το SVG του λογοτύπου· χωρίς αυτό απλώς δεν εμφανίζεται |
| polkit (`pkexec`, polkitd) | | ✓ | | ενέργειες ως root |
| polkit agent στο session | | ✓ | | για τον κωδικό· το Plasma έχει, window managers συχνά όχι |
| flatpak | | | ✓ | εκτός Slackware· χωρίς αυτό το πρόγραμμα ανοίγει και λέει ότι λείπει |

## Σημειώσεις

- Το slacker-cli δεν συνδέεται με openssl ούτε με sqlite του συστήματος: TLS
  με rustls, SQLite ενσωματωμένο.
- Το slacker-flatpak 0.1 μόνο διαβάζει· δεν χρειάζεται ακόμα system helper ή
  polkit. Θα τα χρειαστεί όταν μπουν ενημερώσεις και εγκαταστάσεις.
- Το `libgcc_s.so.1` υπάρχει, όσο ξέρω, και στο `aaa_libraries`, όχι μόνο
  στο gcc· δεν το έχω ελέγξει σε Slackware.
