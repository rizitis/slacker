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
| gdk-pixbuf2 (headers) | | ✓ | ✓ | λογότυπο· στο flatpak και τα εικονίδια των εφαρμογών |
| python3 | ✓ | | | `gen-completions.py --check` στο SlackBuild |
| binutils, file, gzip, pkgtools | ✓ | ✓ | (όταν πακεταριστεί) | `strip`, man σε gzip, `makepkg` |
| cargo-auditable, cargo-audit | προαιρετικά | προαιρετικά | | το SlackBuild τα χρησιμοποιεί αν υπάρχουν |

Στο Slackware τα headers είναι μέσα στα ίδια πακέτα· δεν υπάρχουν χωριστά -dev.

## Run time

| Εξάρτηση | slacker-cli | slacker-gui | slacker-flatpak | Γιατί |
|---|---|---|---|---|
| glibc | ✓ | ✓ | ✓ | `libc`, `libm`, loader |
| libgcc_s (aaa_libraries ή gcc) | ✓ | ✓ | ✓ | το `libgcc_s.so.1` υπάρχει και στα δύο πακέτα |
| pkgtools | ✓ | | | `installpkg`, `upgradepkg`, `removepkg` |
| gnupg2 | ✓ | | | `gpg`/`gpg2`, έλεγχος υπογραφών |
| coreutils | ✓ | | | `sha256sum`, `uname`, `id`, `date`, `df` |
| bzip2 | ✓ | | | MANIFEST για το `file-search` |
| diffutils | ✓ | | | `diff` στο `new-config` |
| vim | προαιρετικό | | | `vimdiff` στη συγχώνευση του `new-config` (αλλάζει με `SLACKER_MERGE`) |
| slacker | | ✓ | | όλες οι ενέργειες γίνονται μέσω του slacker |
| gtk4, libadwaita, glib2, pango | | ✓ | ✓ | |
| gdk-pixbuf2 | | ✓ | ✓ | λογότυπο· στο flatpak και τα εικονίδια των εφαρμογών |
| librsvg | | προαιρετικό | προαιρετικό | φορτώνει το SVG του λογοτύπου (και εικονίδια εφαρμογών σε SVG)· χωρίς αυτό απλώς δεν εμφανίζονται |
| polkit (`pkexec`, polkitd) | | ✓ | για το system-wide | gui: ενέργειες ως root. flatpak: δεν καλεί ποτέ `pkexec`· οι αλλαγές στο system-wide περνούν από τον `flatpak-system-helper` και το polkit |
| polkit agent στο session | | ✓ | για το system-wide | για τον κωδικό· το Plasma έχει, window managers συχνά όχι |
| flatpak | | | ✓ | εκτός Slackware· χωρίς αυτό το πρόγραμμα ανοίγει και λέει ότι λείπει |
| flatpak-system-helper | | | για το system-wide | μέρος του πακέτου flatpak· χωρίς αυτό το system-wide είναι μόνο για ανάγνωση |

## Σημειώσεις

- Το slacker-cli δεν συνδέεται με openssl ούτε με sqlite του συστήματος: TLS
  με rustls, SQLite ενσωματωμένο.
- Το slacker-flatpak δεν έχει κώδικα C στα crates του: `quick-xml` για το
  appstream και `flate2` με `miniz_oxide` (Rust) για το `.gz`. Το gcc χρειάζεται
  μόνο ως linker.
- Το slacker-flatpak (από την 0.2) κάνει και αλλαγές (install, uninstall,
  update), πάντα μέσω της εντολής `flatpak` ως χρήστης. Για την εγκατάσταση
  του χρήστη δεν χρειάζεται polkit.
- Ο σύνδεσμος «Website» ανοίγει με την προεπιλεγμένη εφαρμογή του session
  για http(s) (GTK). Αν δεν έχει οριστεί καμία, ο σύνδεσμος δεν ανοίγει τίποτα.
- Το `libgcc_s.so.1` υπάρχει και στο `aaa_libraries` και στο `gcc`
  (ελέγχθηκε στο `/var/lib/pkgtools/packages`). Για να τρέξουν τα binaries
  αρκεί το `aaa_libraries`· το gcc χρειάζεται μόνο για το build.
- Οι βιβλιοθήκες ελέγχθηκαν με `readelf -d`: slacker-cli (`libc`, `libm`, `libgcc_s`), slacker-gui 7.0.0 και slacker-flatpak 0.6.0.
