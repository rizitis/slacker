# slacker: system dependencies

Slackware package names. Sources: `readelf -d` on the binaries, `Cargo.lock`
(which crates need a C compiler or pkg-config), the code (which programs are
called) and `slacker.SlackBuild`.

## Build time

| Dependency | slacker-cli | slacker-gui | slacker-flatpak | Why |
|---|---|---|---|---|
| rust (rustc, cargo) | ✓ ≥ 1.89 | ✓ ≥ 1.92 | ✓ ≥ 1.92 | compiling |
| gcc | ✓ | ✓ | ✓ | linker; in the cli also for `ring` (TLS) and the bundled SQLite |
| pkg-config | | ✓ | ✓ | the gtk-rs crates find the libraries |
| gtk4 (headers) | | ✓ ≥ 4.4 | ✓ ≥ 4.4 | |
| libadwaita (headers) | | ✓ ≥ 1.5 | ✓ ≥ 1.5 | |
| glib2, pango (headers) | | ✓ | ✓ | |
| gdk-pixbuf2 (headers) | | ✓ | ✓ | logo; in flatpak also the application icons |
| python3 | ✓ | | | `gen-completions.py --check` in the SlackBuild |
| binutils, file, gzip, pkgtools | ✓ | ✓ | (once packaged) | `strip`, gzipped man pages, `makepkg` |
| cargo-auditable, cargo-audit | optional | optional | | the SlackBuild uses them if present |

On Slackware the headers come in the same packages; there are no separate -dev packages.

## Run time

| Dependency | slacker-cli | slacker-gui | slacker-flatpak | Why |
|---|---|---|---|---|
| glibc | ✓ | ✓ | ✓ | `libc`, `libm`, loader |
| libgcc_s (aaa_libraries or gcc) | ✓ | ✓ | ✓ | `libgcc_s.so.1` is in both packages |
| pkgtools | ✓ | | | `installpkg`, `upgradepkg`, `removepkg` |
| gnupg2 | ✓ | | | `gpg`/`gpg2`, signature checks |
| coreutils | ✓ | | | `sha256sum`, `uname`, `id`, `date`, `df`, `stat` |
| bzip2 | ✓ | | | MANIFEST.bz2, fetched by `update`, for `file-search` and the disk-space check |
| diffutils | ✓ | | | `diff` in `new-config` |
| vim | optional | | | `vimdiff` when merging in `new-config` (changed with `SLACKER_MERGE`) |
| less | optional | | | pager for `history` and `show-changelog` on a terminal (changed with `PAGER`) |
| slacker | | ✓ | | every action goes through slacker |
| gtk4, libadwaita, glib2, pango | | ✓ | ✓ | |
| gdk-pixbuf2 | | ✓ | ✓ | logo; in flatpak also the application icons |
| librsvg | | optional | optional | loads the SVG logo (and SVG application icons); without it they are simply not shown |
| polkit (`pkexec`, polkitd) | | ✓ | for system-wide | gui: actions as root. flatpak: never calls `pkexec`; system-wide changes go through `flatpak-system-helper` and polkit |
| polkit agent in the session | | ✓ | for system-wide | for the password; Plasma has one, window managers often do not |
| flatpak | | | ✓ | not part of Slackware; without it the program opens and says it is missing |
| flatpak-system-helper | | | for system-wide | part of the flatpak package; without it system-wide is read-only |

## Notes

- slacker-cli does not link against the system openssl or sqlite: TLS with
  rustls, SQLite bundled.
- slacker-flatpak has no C code in its crates: `quick-xml` for appstream and
  `flate2` with `miniz_oxide` (Rust) for `.gz`. gcc is needed only as the linker.
- slacker-flatpak (since 0.2) also makes changes (install, uninstall,
  update), always through the `flatpak` command as the user. The user's own
  installation needs no polkit.
- The "Website" link opens with the session's default application for
  http(s) (GTK). If none is set, the link opens nothing.
- `libgcc_s.so.1` is in both `aaa_libraries` and `gcc` (checked in
  `/var/lib/pkgtools/packages`). `aaa_libraries` is enough to run the
  binaries; gcc is needed only to build.
- Libraries checked with `readelf -d`: slacker-cli (`libc`, `libm`, `libgcc_s`), slacker-gui 7.0.0 and slacker-flatpak 1.0.0.
