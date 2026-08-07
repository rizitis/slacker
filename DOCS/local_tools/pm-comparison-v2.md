# Σύγκριση package managers — slacker vs APT / DNF / Pacman / Zypper / APK / **XBPS (Void)**

*Τίμια αποτίμηση, Ιούλιος 2026, βάσει του πραγματικού κώδικα 0.10.1+ (πλήρης ανάγνωση και των 20 modules), του Cargo.toml (4 direct deps, rustls, MSRV 1.85.1), του SlackBuild και των production configs. Όπου ο slacker χάνει, το λέμε· όπου έχει κάτι που δεν έχει κανείς, το αναδεικνύουμε.*

---

## Ο μεγάλος πίνακας

| Κριτήριο | **slacker** | **XBPS** (Void) | APT (Debian) | DNF (Fedora) | Pacman (Arch) | Zypper (openSUSE) | APK (Alpine) |
|---|---|---|---|---|---|---|---|
| Γλώσσα / μέγεθος | Rust, **~17,8k LOC, 1 binary** | C, δεκάδες kLOC, ~15 binaries | C++, τεράστιο | Python+C (libdnf5) | C, μέτριο | C++ (libzypp), μεγάλο | C, μικρό |
| Deps του ίδιου του PM | **4 crates** (clap, ureq/rustls, md-5, regex)· gpg/bzip2/sha256sum shelled out | zlib, OpenSSL/LibreSSL, **libarchive** | πολλές | πολλές | libarchive, gpgme, curl | πολλές | μηδαμινές |
| Solver εξαρτήσεων | **Όχι** — ονόματα από `.dep` / `PACKAGE REQUIRED` μόνο, χωρίς version constraints· official tree = κανένα auto-dep (φιλοσοφία Slackware) | Πλήρης: version constraints, virtual packages, **αυτόματα SONAME/shlib deps** με έλεγχο στη συναλλαγή | Πλήρης SAT-ish | Πλήρης SAT (libsolv) | Πλήρης (versioned, provides) | Πλήρης SAT (libsolv) | Πλήρης |
| Πολλαπλά repos & προτεραιότητα | **Αριθμητική priority ανά repo + build-tag «εικονικές πηγές»** (SBo, local tags στο ίδιο μοντέλο) | Σειρά δήλωσης (πρώτο repo που έχει το όνομα κερδίζει) | apt pinning (ισχυρό αλλά δυσνόητο) | repo priority + modularity | Σειρά στο pacman.conf | repo priority + vendor stickiness | Σειρά + tagged repos |
| Never-migrate-down invariant | **Ναι — πουθενά αλλού**: εγκατεστημένο πακέτο ΠΟΤΕ δεν μεταναστεύει σιωπηλά σε πηγή χαμηλότερης priority· τα SBo/local builds προστατεύονται μέσω tag-priority | Μερικώς: `repolock` κλειδώνει πακέτο στο repo εγκατάστασής του (χειροκίνητο, ανά πακέτο) | Μέσω pinning (χειροκίνητο, εύθραυστο) | Όχι εγγενώς | Όχι | vendor-change protection (κοντινό!) | Όχι |
| Pin πακέτου σε repo | `pin repo:name` (persistent, στο blacklist, αγνοεί priority) + transient `repo:name` | `repolock` (μόνο στο repo που ήδη ήρθε) | pinning per-origin | `--repo`, versionlock plugin | Όχι εγγενώς | vendor lock | pin σε tagged repo |
| Freeze/hold | **Scoped freeze-fallback**: `@repo PATTERN` παγώνει ΜΟΝΟ τον υποψήφιο εκείνου του repo/tag και η επίλυση πέφτει στον επόμενο μη-παγωμένο κατά priority, **ποτέ κάτω από την priority της εγκατεστημένης πηγής**· held μόνο αν ΟΛΟΙ frozen· glob+regex+series | `hold` (ολικό ανά πακέτο), `ignorepkg` | `apt-mark hold` (ολικό) | versionlock (ολικό) | IgnorePkg (ολικό) | lock (ολικό) | hold (ολικό) |
| Υπογραφές / μεταφορά | GPG στο CHECKSUMS **+ per-package `.asc`** στο install· https-only credentials (HTTP Basic, zypper-modeled), rustls | Signed repodata + **per-package RSA** υπογραφές | InRelease GPG (per-repo, όχι per-pkg στο transport) | repo GPG + per-rpm | per-pkg GPG (web-of-trust keyring) | repo GPG + per-rpm | signed index |
| TOFU / key pinning | **Pin ΟΛΩΝ των fingerprints** στο πρώτο update, ξανα-έλεγχος σε ΚΑΘΕ update, KeyChanged → **αυτόματη hard καραντίνα με αιτιολογία** | **Το κοντινότερο**: fingerprint-accept στο πρώτο sync, άρνηση σε mismatch — αλλά χωρίς καραντίνα/αιτιολογία/override-ροή | Όχι (κλειδιά προ-εγκατεστημένα) | Όχι | Keyring package | Όχι | Όχι |
| Vetting / καραντίνα repo | **Μοναδικό**: add-repo/vet-repo τρέχουν 5 ελέγχους (http-χωρίς-insecure, fetch, **σάρωση PACKAGES.TXT για path-traversal ονόματα → «MALICIOUS»**, loadable pkgs, GPG)· soft (unreachable, auto-retry) / hard (trust-repo μόνο) markers με καταγεγραμμένο λόγο | Όχι | Όχι | Όχι | Όχι | Όχι | Όχι |
| Έλεγχος εγκατεστημένων αρχείων | **Όχι** (verify μόνο στο download) | **Ναι** — `xbps-pkgdb -a` (checksums όλων) | debsums (extra) | `rpm -V` | `pacman -Qk` | `rpm -V` | `apk audit` |
| Rollback / downgrade | `revert-pkg`: προηγούμενες official εκδόσεις από cumulative archive, **υποχρεωτικό GPG** από pinned key, danger-banner σε foundational — στενό scope (official, -current only) | Μόνο αν έχεις το παλιό .xbps (cache/xdowngrade)· repos κρατούν μόνο latest | Μόνο αν υπάρχει στο pool | **dnf history undo/rollback** | Cache/ALA (χειροκίνητο) | **zypper + snapper/btrfs = πλήρη snapshots** (ο βασιλιάς εδώ) | Περιορισμένο |
| Dist-upgrade | `upgrade-dist`: **escape-kit** (config backup + template), typed point-of-no-return, disk gates ΠΡΙΝ το transform, core-first σειρά (glibc-solibs→pkgtools→tar/xz/gzip/findutils), **gpg-chain τελευταία**, batches download→install→delete (ο δίσκος δεν γεμίζει ποτέ), idempotent re-run, skip clean-system σε μερική αποτυχία | Rolling — δεν χρειάζεται | do-release-upgrade | system-upgrade plugin | Rolling | `zypper dup` (+snapper δίχτυ) | Rolling/edge |
| Ιστορικό | **Χωρίς δικό του log**: αντλείται από την pkgtools DB → πιάνει ΚΑΙ αλλαγές από installpkg/sbopkg/slackpkg χειροκίνητα· tenure-pairing, reinstall detection, self-calibrating local clock | Όχι | history.log (χάνει ό,τι έγινε εκτός apt) | dnf history (ομοίως) | pacman.log (ομοίως) | zypper history (ομοίως) | Όχι |
| .new/config merge | **Ενσωματωμένο**: broken/identical/conflicts triage, bulk K/O/R/P, per-file K/O/R/M/D με diff + `$SLACKER_MERGE` (vimdiff)· τρέχει ακόμα και με σπασμένο mirror | Κρατά το δικό σου + `.new-<version>`· merge με εξωτερικά xtools | conffile prompt | .rpmnew/.rpmsave (χειροκίνητα) | .pacnew + pacdiff (εξωτερικό) | .rpmnew | .apk-new |
| Doctor / υγεία setup | **`status`**: env/tools, config validation, mirror **freshness vs upstream osuosl**, release & arch mismatch ανά repo, GPG empirical verify, **perms/symlink audit των δικών του αρχείων**, credentials transport, pins/blacklist sanity, βήματα-συνταγή στο τέλος | `xbps-pkgdb` (DB integrity μόνο) | Όχι ενιαίο | Όχι ενιαίο | Όχι | Όχι ενιαίο | Όχι |
| Mirror tooling | `find-mirror`: latency+freshness **σε ένα request/mirror**, παράλληλο probe, propose-only, δουλεύει πριν καν στηθεί mirror | **xmirror** (επίσημο, χωρίς freshness ranking) | netselect-apt (extra) | metalink **αυτόματο** (καλύτερη UX) | reflector (extra) | mirrorbrain αυτόματο | Χειροκίνητο |
| Παράλληλα downloads | Ναι (MAX_PARALLEL 1–16, default 4) + **verification summary ανά batch** (πόσα GPG / integrity-only / unverified) | **Όχι** (σειριακά) | Ναι | Ναι | Ναι (ParallelDownloads) | Ναι | Ναι |
| Ασφάλεια υλοποίησης | Memory-safe Rust· **όλα τα shell-outs argv (κανένα `sh -c`)**· path-traversal διπλό φράγμα· size caps 512MiB/8GiB + anti-decompression-bomb· symlink refusals σε κάθε write· flock με PID | C· libarchive στο μονοπάτι εξαγωγής (ιστορικό CVEs) | C++ ώριμο, πολλά μάτια | C/Python | C, λιτό | C++ | C, μικρό attack surface |
| Bus factor / audit / ωριμότητα | **1 άνθρωπος**, 2025-26, κανένα εξωτερικό audit, 189 unit tests | Μικρή ομάδα Void, ~15 χρόνια παραγωγής | Ολόκληρο το Debian, δεκαετίες | Red Hat | Ομάδα Arch | SUSE | Ομάδα Alpine |
| Οικοσύστημα | Slackware-current (+15.0 μέσω patches-repo μοντέλου) | Void | Το μισό Linux | Fedora/RHEL | Arch+παράγωγα | SUSE | Alpine/containers |

---

## Head-to-head με τον XBPS — γιατί αξίζει ξεχωριστό κεφάλαιο

Ο xbps είναι ο πιο «συγγενής» αντίπαλος: μικρής ομάδας, για μία διανομή, με σοβαρή στάση στην κρυπτογραφική επαλήθευση. Τίμια:

**Όπου ο xbps είναι μπροστά:**
- **SONAME/shlib dependency tracking**: παράγει αυτόματα εξαρτήσεις από τα shared libraries στο build και **αρνείται συναλλαγή που θα άφηνε σπασμένα sonames**. Αυτό είναι το μεγαλύτερο τεχνικό του προβάδισμα — ο slacker δεν έχει τίποτα ανάλογο (και συνειδητά: «ο Pat ξέρει», τα official δεν έχουν deps).
- **Πραγματικός solver** με version constraints και virtual packages.
- **`xbps-pkgdb -a`**: επαλήθευση checksums των ήδη εγκατεστημένων αρχείων — ο slacker επαληθεύει μόνο στο download.
- 15 χρόνια παραγωγής, ομάδα >1.

**Όπου ο slacker είναι μπροστά:**
- **Freeze-fallback με scoped κανόνες και όροφο priority** — ο xbps έχει μόνο ολικό `hold`. Το «πάγωσε ΜΟΝΟ τον testing υποψήφιο και άσε το official update να ρέει, χωρίς ποτέ downgrade» δεν εκφράζεται σε xbps.
- **Καραντίνα repo με σάρωση για κακόβουλα metadata**: repo που διαφημίζει path-traversal ονόματα παγώνει ΑΥΤΟΜΑΤΑ ως MALICIOUS, με αιτιολογία και ρητό override (`trust-repo`). Ο xbps θα αρνηθεί bad signature αλλά δεν έχει την έννοια «αυτή η πηγή είναι σε καραντίνα, να γιατί, να πώς τη λύνεις».
- **Doctor + freshness**: το `status` ελέγχει μέχρι και τα δικαιώματα/symlinks των ΔΙΚΩΝ του trust-αρχείων και το πόσο πίσω είναι ο mirror σου από το upstream. Ο xbps δεν έχει τίποτα ενιαίο.
- **History από την DB του συστήματος** (πιάνει και ό,τι έγινε εκτός slacker)· ο xbps δεν έχει history.
- **Ενσωματωμένο config-merge** με diff/merge tool· στο Void θέλεις xtools.
- **Παράλληλα downloads** με batch verification summary.
- Επιφάνεια ελέγχου: 1 binary / 4 crates / rustls, έναντι C+libarchive+openssl.

**Ισοπαλία (και οι δύο σοβαροί):** TOFU στο πρώτο repo contact — ο xbps ρωτά fingerprint στο πρώτο sync και αρνείται mismatch· ο slacker κάνει pin-όλων-των-fingerprints, ξανα-έλεγχο σε κάθε update, και μετατρέπει το mismatch σε hard καραντίνα. Ίδιο πνεύμα, ο slacker πιο θεσμοθετημένο.

---

## Τι έχει ο slacker που δεν έχει **κανένας** από τους 6

1. **Scoped freeze-fallback + priority floor** — μοναδικός συνδυασμός: κατάψυξη ανά repo/tag/pattern/series που «πέφτει» ελεγχόμενα στην επόμενη πηγή χωρίς ποτέ downgrade πηγής.
2. **Build-tag εικονικές πηγές** στο ενιαίο μοντέλο priority: τα `_SBo`/local builds σου είναι πολίτες πρώτης κατηγορίας με δική τους priority — και το invariant «ποτέ μετανάστευση προς τα κάτω» τα προστατεύει αυτόματα.
3. **Repo vetting/καραντίνα με ανίχνευση κακόβουλων metadata** (path-traversal scan) και soft/hard σημασιολογία με αιτιολογία.
4. **`status` doctor** που ελέγχει και τον ίδιο του τον εαυτό (perms/symlinks των trust anchors) και τη φρεσκάδα του mirror έναντι upstream.
5. **History χωρίς log** — από την pkgtools DB, άρα πλήρες ακόμα κι αν δούλεψες με installpkg/sbopkg στο ενδιάμεσο.
6. **upgrade-dist με escape-kit** και batch download→install→delete ώστε η cache να μη χρειάζεται ποτέ ολόκληρη τη διανομή.
7. **revert-pkg** με υποχρεωτική GPG επαλήθευση από το pinned official κλειδί (στενό, αλλά δεν το έχει κανείς στον χώρο Slackware).
8. **Interactive plan UX**: pickers με πλήρη `old-tag → new-tag` μετάβαση, «Also offered by other repos» με **live diff των .dep** των εναλλακτικών, protected-deps διάλογος, conflicts/suggests surfacing.

## Τι έχουν οι άλλοι που **δεν** έχει ο slacker (χωρίς ωραιοποίηση)

- Solver με version constraints — και ειδικά το SONAME tracking του xbps.
- Επαλήθευση εγκατεστημένων αρχείων (rpm -V / xbps-pkgdb -a / debsums).
- Snapshots/transactional rollback (zypper+snapper, dnf history) — το δίχτυ του zypper σε dist-upgrade παραμένει ασυναγώνιστο.
- Alternatives, multiarch, delta downloads.
- Ομάδες συντήρησης, εξωτερικά μάτια/audits, δεκαετίες ωρίμανσης.
- Bus factor: **1**. Είναι το πιο αδύναμο σημείο του project και δεν κρύβεται.

## Ετυμηγορία

Ο slacker δεν προσπαθεί να είναι DNF. Είναι **λεπτό στρώμα πάνω στα pkgtools** με τρία πράγματα που παίρνει πιο σοβαρά από οποιονδήποτε στη σύγκριση: (α) το **μοντέλο πηγών** (priority + tags + pin + scoped freeze με floor), (β) την **εμπιστοσύνη προς τα repos** (TOFU, vetting, καραντίνα, doctor), (γ) την **ελέγξιμη επιφάνεια** (1 binary, 4 deps, argv-only). Πληρώνει το τίμημα σε solver, rollback και ομάδα. Στο δικό του γήπεδο — multi-repo Slackware με τρίτες πηγές και χειροποίητα builds — κάνει πράγματα που κανένας από τους έξι δεν κάνει.
