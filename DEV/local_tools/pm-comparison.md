# Package managers: μια τίμια σύγκριση

**APT · DNF · Pacman · Zypper · APK — και slacker**

*Σύγκριση από το Claude (Anthropic), Ιούλιος 2026. Ο κρίνων έχει διαβάσει ολόκληρο τον
κώδικα του slacker 0.10.1· για τους υπόλοιπους βασίζεται σε δημόσια τεκμηρίωση και
κοινή γνώση. Τίμια σημαίνει: ο slacker χάνει όπου πρέπει να χάσει.*

---

## Ο πίνακας

| Κριτήριο | APT (deb) | DNF (rpm) | Pacman | Zypper (rpm) | APK (Alpine) | slacker (Slackware) |
|---|---|---|---|---|---|---|
| **Ηλικία / ωριμότητα** | ~28 έτη, θεμέλιο του μισού Linux | ~10 (πάνω σε yum ~22) | ~24 | ~20 | ~19 | **<2 έτη, beta** — το νεότερο με διαφορά |
| **Εγκατεστημένη βάση** | Τεράστια (Debian/Ubuntu/Mint/…) | Τεράστια (Fedora/RHEL/clones) | Μεγάλη (+SteamOS) | Μεσαία, ισχυρό enterprise | Τεράστια σε containers | Πολύ μικρή (Slackware niche) |
| **Bus factor / συντήρηση** | Ομάδες, χρηματοδότηση | Red Hat πίσω του | Ομάδα Arch | SUSE πίσω του | Ομάδα Alpine | **1 άνθρωπος (r-tz)** — το πιο αδύνατο σημείο του |
| **Dependency resolution** | Πλήρες, version constraints, provides/conflicts, autoremove | Πλήρες SAT (libsolv), weak deps, modularity | Πλήρες, provides/conflicts/replaces | **Ο καλύτερος solver** (SAT/libsolv, προτείνει λύσεις σε conflicts) | Πλήρες, γρήγορο, provides | **Σκόπιμα μίνιμαλ**: μόνο `.dep` του ίδιου του πακέτου (ή PACKAGE REQUIRED), χωρίς version constraints, χωρίς conflicts/provides, χωρίς autoremove. Στη Slackware φιλοσοφία το full-install είναι ο κανόνας — αλλά ως *solver* είναι αντικειμενικά ο πιο αδύναμος του πίνακα |
| **Transactional safety** | Όχι atomic· dpkg journal για ανάκαμψη | Πλήρες history + rollback/undo transactions | Όχι atomic· manual downgrade από cache | snapper/btrfs snapshots πριν/μετά — **best in class** με το κατάλληλο FS | Όχι atomic· `apk fix` | Όχι atomic (κληρονομεί pkgtools)· `revert-pkg` μόνο για official πακέτα σε -current, ένα-ένα. **Πίσω από DNF/Zypper** εδώ |
| **Υπογραφές / trust model** | Υπογεγραμμένο Release/InRelease (repo-level), keyrings ανά repo | Υπογεγραμμένα πακέτα (rpm) + repo metadata, key import με πρώτη χρήση | Υπογεγραμμένα πακέτα + optional DB sig, **web-of-trust keyring** (5 master keys) | Υπογεγραμμένα repo metadata + πακέτα, GPG keys | Υπογεγραμμένο APKINDEX, κλειδιά στο image | GPG σε CHECKSUMS **και** per-package `.asc` όπου υπάρχει, **TOFU pinning ανά repo με hard quarantine σε αλλαγή κλειδιού** — πιο επιθετικό pinning από όλους· αλλά χωρίς web-of-trust και χωρίς formal audit |
| **Fail-closed στάση** | Μερική (ανυπόγραφα repos → warning/refuse ανά config) | Μερική (gpgcheck ανά repo, συχνά απενεργοποιείται) | Ρυθμιζόμενη (SigLevel) | Μερική | Καλή | **Αυστηρά fail-closed by default**: bad sig = stop, key change = quarantine, unverifiable repo = ορατό warning· ο official repo ΔΕΝ εξαιρείται |
| **Επιφάνεια επίθεσης / hardening** | Μεγάλη codebase (C++), ιστορικά CVEs, αλλά και δεκαετίες σκληροποίησης + audits | Μεγάλη (Python+C), το ίδιο | Μικρή-μεσαία (C) | Μεγάλη (C++) | **Πολύ μικρή** (C, μίνιμαλ) | Μικρή (Rust, 4 deps, rustls, όλα argv — όχι `sh -c`, path-traversal guards, size caps). Memory safety δωρεάν από τη γλώσσα· **αλλά μηδέν εξωτερικό audit και ελάχιστα μάτια** — τα CVE των μεγάλων είναι και απόδειξη ότι τους ψάχνουν |
| **Ταχύτητα** | Καλή· APT 3.x βελτιωμένο | Ιστορικά το πιο βαρύ· dnf5 το διόρθωσε αισθητά | **Πολύ γρήγορος** — reference στο desktop | Μέτριος-καλός | **Ο ταχύτερος** σε footprint/χρόνο | Γρήγορος (Rust, sync, παράλληλα downloads, μικρά metadata) — αλλά και τα repos είναι μικρά· άνιση σύγκριση |
| **Metadata / disk footprint** | Μεσαίο | Βαρύ (repodata/solv) | Ελαφρύ | Βαρύ | **Ελάχιστο** | Ελαφρύ (PACKAGES.TXT/CHECKSUMS, lazy MANIFEST μόνο για file-search) |
| **Delta updates** | Όχι πια (debdelta νεκρό στην πράξη) | Ναι (drpm, φθίνει) | Όχι | Ναι (deltarpm) | Όχι | Όχι |
| **Rollback / downgrade UX** | Μη τετριμμένο (pinning σε παλιά version, snapshot.d.o) | `dnf history undo/rollback` — καλό | Cache + downgrade tools (AUR) | snapper — εξαιρετικό | Έκδοση-pin στο APKINDEX | `revert-pkg` με GPG-verify από cumulative archive + προσφορά freeze — **καλό UX αλλά στενό scope** (official, -current μόνο) |
| **Multi-repo / priorities** | apt pinning — ισχυρό αλλά διαβόητα δυσνόητο | Priorities + modularity (η modularity απέτυχε/αποσύρθηκε) | Απλή σειρά repos, χωρίς πραγματικές προτεραιότητες | Vendor stickiness + priorities — πολύ καλό | Απλά tagged repos | **Ρητό αριθμητικό μοντέλο, distinct priorities, build-tag attribution (`_SBo`→priority), σκληρή εγγύηση κατά migration/downgrade** — για το μέγεθός του, ίσως το πιο καθαρό μοντέλο του πίνακα· και το `pin repo:name` απλούστερο από το apt pinning |
| **Config file handling** | conffiles + dpkg prompts (ώριμο, μπερδεμένο UX) | `.rpmnew`/`.rpmsave`, σιωπηλό — το ψάχνεις μόνος | `.pacnew` + pacdiff (χειροκίνητο) | `.rpmnew` + ειδοποιήσεις | Απλό | `.new` + **ενεργή ανίχνευση μετά από κάθε install/upgrade + διαδραστικό `new-config`** — ένα από τα καλύτερα UX του πίνακα εδώ |
| **Distribution upgrade** | `full-upgrade` + release notes· ώριμο αλλά χειροκίνητη τελετή | `dnf system-upgrade` — καλό | Rolling — δεν υφίσταται | `zypper dup` — reference για openSUSE | Rolling-ish, edge/stable | `upgrade-dist` με fail-closed whitelist διαδρομών, typed confirmation, escape-kit, batch downloads, disk preflight — **σχεδιαστικά ώριμο**, αλλά δοκιμασμένο σε τάξεις μεγέθους λιγότερα συστήματα |
| **Έλεγχος από χρήστη / διαφάνεια** | Πολλά layers (apt/dpkg/conf.d) | Πολλά layers, Python plugins | Καλή διαφάνεια, ALPM hooks | Πολλά layers | Πολύ διαφανές | **Όλα plain-text, όλα ορατά, τίποτα κρυφό**· χωρίς hooks/plugins όμως — λιγότερο επεκτάσιμο |
| **Οικοσύστημα τρίτων** | Αχανές (PPAs κ.λπ.) | Αχανές (COPR) | **AUR — ασυναγώνιστο** | OBS — πολύ καλό | Μικρότερο | Μικρό (SBo μέσω tag-priority, γνωστά third-party repos)· λειτουργικό αλλά δεν συγκρίνεται |
| **Τεκμηρίωση** | Δεκαετίες docs/βιβλία/StackOverflow | Πλήρης | **Arch Wiki — χρυσός κανόνας** | Πλήρης | Καλή | Πλήρες man page (και τα 38 commands), **wiki στο forge** (user guides: setup, blacklist/freeze, repos, upgrade-dist), + internal maps για forkers (structure.md, CODE_MAP) — όλα συντηρημένα από τον ίδιο τον author, συνεπή με τον κώδικα ανά release. Πλήρης κάλυψη για το scope του· αυτό που λείπει είναι η *κοινοτική* μάζα (χιλιάδες Q&A, howtos τρίτων) που μόνο ο χρόνος και η βάση χρηστών χτίζουν |
| **Γλώσσα / deps** | C++ | C/Python (dnf5: C++) | C | C++ | C | **Rust, 4 crates, ένα static-ish binary** — σύγχρονο· MSRV 1.85 όμως αποκλείει παλιά toolchains |

---

## Τίμια συμπεράσματα

**Πού ο slacker χάνει καθαρά — και πρέπει να το λέμε:**

1. **Bus factor 1.** Ό,τι κι αν λέει ο κώδικας, ένα project ενός ανθρώπου σε beta δεν
   συγκρίνεται σε αξιοπιστία-ως-θεσμό με τα APT/DNF/Zypper που έχουν εταιρείες και
   δεκαετίες από πίσω. Αυτό είναι το #1 πραγματικό ρίσκο του.
2. **Dependency solver.** Δεν υπάρχει. Είναι *συνειδητή επιλογή* που ταιριάζει στη
   Slackware (full install, ο χρήστης αποφασίζει), αλλά σε ωμή σύγκριση δυνατοτήτων
   κάθε άλλος στον πίνακα λύνει προβλήματα που ο slacker δεν επιχειρεί καν.
3. **Transactional rollback.** Zypper+snapper και DNF history είναι μπροστά. Το
   `revert-pkg` είναι έντιμο και ασφαλές αλλά στενό.
4. **Μάτια πάνω στον κώδικα.** Το Rust και τα guards είναι πραγματικά πλεονεκτήματα,
   αλλά η ασφάλεια που έχει *ελεγχθεί* από χιλιάδες χρήστες/ερευνητές είναι άλλης
   κατηγορίας από την ασφάλεια που έχει *σχεδιαστεί* σωστά. Ο slacker έχει το δεύτερο,
   όχι ακόμα το πρώτο.
5. **Οικοσύστημα.** AUR/COPR/PPA/OBS δεν έχουν αντίστοιχο εδώ και δεν θα αποκτήσουν.

**Πού ο slacker κερδίζει πραγματικά — χωρίς υπερβολή:**

1. **Trust model με TOFU pinning + quarantine.** Κανείς στον πίνακα δεν καρφιτσώνει
   fingerprints ανά repo και δεν βάζει σε καραντίνα repo που άλλαξε κλειδί, by default,
   fail-closed. Για το threat model «MITM/παραβιασμένο mirror» είναι το πιο
   παρανοϊκό-με-την-καλή-έννοια εργαλείο εδώ.
2. **Το priority/build-tag μοντέλο.** Η εγγύηση «distinct high priority = κλείδωμα,
   ποτέ migration, ποτέ downgrade» + attribution από build tags είναι πιο καθαρή
   από το apt pinning και πιο ισχυρή από ό,τι έχει ο pacman. Είναι η καρδιά του
   εργαλείου και είναι καλά σχεδιασμένη.
3. **Thin-layer πειθαρχία.** Δεν ξαναγράφει pkgtools· ό,τι κάνει το σύστημα, το κάνει
   το σύστημα. Μηδέν «μαγεία», όλα plain-text, όλα επιθεωρήσιμα. Στο δικό του
   φιλοσοφικό πλαίσιο (Slackware) αυτό είναι feature, όχι έλλειψη.
4. **Config handling UX** (`new-config` + ενεργή ανίχνευση) και **έντιμα, δραστικά
   μηνύματα** (advisories, DANGER banners, «did you mean», shell-expansion guard) —
   λεπτομέρειες ποιότητας ζωής που αρκετοί μεγάλοι δεν έχουν.
5. **Τεκμηρίωση συνεπής με τον κώδικα.** Man page, wiki και code maps γράφονται από
   το ίδιο χέρι που γράφει τον κώδικα και ενημερώνονται ανά release — δεν υπάρχει το
   κλασικό χάσμα «τα docs λένε Χ, το εργαλείο κάνει Ψ» που ταλαιπωρεί μεγαλύτερα
   projects. Η *κοινοτική* μάζα (Arch-Wiki-scale) λείπει· η *ακρίβεια* όχι.
6. **Μικρή, ελέγξιμη επιφάνεια.** 4 dependencies, ~17k γραμμές συνολικά, ένα binary.
   Ένας άνθρωπος μπορεί να το διαβάσει ΟΛΟ σε ένα Σαββατοκύριακο — κανένα άλλο
   εργαλείο του πίνακα δεν το επιτρέπει αυτό.

**Η μία πρόταση:** ο slacker δεν είναι «καλύτερος» από τους 5 μεγάλους — είναι ο πιο
συνεπής με τη φιλοσοφία της διανομής που υπηρετεί, με trust model πάνω από το μέσο
όρο του πίνακα και με τα ρεαλιστικά όρια ενός νέου, μονοπρόσωπου project. Για
Slackware user είναι σοβαρή αναβάθμιση από slackpkg+slackpkg+· για Ubuntu user δεν
είναι καν ερώτημα — και τα δύο είναι σωστά.
