# slacker-sbo: προδιαγραφές

Nov 17, 2026 · @r-tz

## Σκοπός και όρια

Εργαλείο σε Rust για maintainers του SBo: καθαρό build των SlackBuilds σε απομονωμένο περιβάλλον, και ροή PR που ακολουθεί τον [οδηγό του SBo](https://slackbuilds.org/SBo-Github-Workflows.txt).

- **Δεν είναι package manager.** Δεν εγκαθιστά, δεν αναβαθμίζει και δεν αφαιρεί τίποτα στο σύστημα.
- **Δεν αγγίζει το σύστημα.** Ό,τι γίνεται, γίνεται μέσα στο sandbox. Τίποτα δεν βγαίνει από αυτό· ό,τι θέλει ο χρήστης το παίρνει με το χέρι.
- **Μόνο Slackware 15.0.** Το SBo δεν δέχεται PR για το development branch.
- **PR μόνο για updates.** Για νέα SlackBuilds το εργαλείο κάνει μόνο build και έλεγχο· η υποβολή γίνεται από τη φόρμα του SBo.
- **Τρέχει ως root** (sudo), για να στήσει τα namespaces. Το git τρέχει πάντα ως `GIT_USER`, ποτέ ως root.

## Αρχείο ρυθμίσεων

Ό,τι μπορεί να ρυθμιστεί, ρυθμίζεται στο `/etc/slacker/slacker-sbo.conf`. Μορφή `KEY="VALUE"`, οι τιμές πάντα σε εισαγωγικά. Τα paths είναι παραδείγματα.

```sh
# Slackware 15.0
MIRROR_URL=""                      # υποχρεωτικό, χωρίς προεπιλογή
MIRROR_DIR="/srv/sbo/mirror"       # slackware64/, slackware/ και τα patches τους
MIRROR_GPG="yes"                   # yes | no (ο έλεγχος md5 γίνεται πάντα)
BASE_DIR="/srv/sbo/base"           # τα δύο εγκατεστημένα δέντρα

# git
GIT_USER="omen"
GIT_DIR="/srv/sbo/slackbuilds"     # ένα clone του fork
FORK_URL="git@github.com:rizitis/slackbuilds.git"   # git@... ή https://...
UPSTREAM_URL="https://github.com/SlackBuildsOrg/slackbuilds.git"

# build
SRC_DIR="/srv/sbo/sources"         # tarballs, έλεγχος με MD5SUM
WORK_DIR="/srv/sbo/work"           # sandboxes και cache εξαρτήσεων
WORK_FS="loop"                     # loop | tmpfs
WORK_SIZE="50G"                    # μέγεθος loop image ή όριο tmpfs
ARCH="x86_64 i586"

# ασφάλεια
SUBID_BASE="100000"                # μέσα 0..65535 = έξω 100000..165535 (προεπιλογή)
BUILD_MEM="75%"                    # memory.max / memory.limit_in_bytes
BUILD_PIDS="4096"                  # pids.max
CGROUP_MISSING="stop"              # stop | warn

# εργαλεία
EDITOR="simple"                          # simple | vim (ενσωματωμένος editor)
```

Στο `/etc` μπαίνει μόνο αυτό το αρχείο. Τα `pre-install.<pkg>`, `post-install.<pkg>` και τα hash των README μένουν στο `WORK_DIR/hooks/`, που κανένα `clean` ή `update-*` δεν αγγίζει.

Το `MIRROR_URL` δεν έχει προεπιλογή· αν είναι κενό, το `update-slackware` δεν ξεκινά.

## Εντολές

Έντεκα εντολές, καθεμία με έναν μόνο σκοπό. Διαφορετικά πράγματα δεν μπαίνουν στην ίδια εντολή.

| Εντολή | Τι κάνει |
| --- | --- |
| `update-sbo` | git: upstream SBo και fork. Τελειώνει με πλήρες clean |
| `update-slackware` | πακέτα 15.0 από το mirror με GPG και md5, μετά ενημέρωση των δύο βασικών δέντρων. Τελειώνει με πλήρες clean |
| `clean` | σβήνει sandboxes και cache εξαρτήσεων |
| `build vlc` | καθαρό build από το upstream, x86\_64 και i586 |
| `build --dev vlc` | το ίδιο, από το branch ανάπτυξης `vlc`. Με `--use-dev <pkg>`, μια εξάρτηση από το δικό της branch |
| `dev vlc` | ανοίγει branch `vlc` με δικό του worktree |
| `shell vlc` | sandbox με τις εξαρτήσεις του vlc, shell ως root μέσα |
| `lint vlc` | `sbolint` στον φάκελο και `sbopkglint` στα `.tgz`, μέσα στο sandbox |
| `commit vlc` | έλεγχος permissions, πρόταση μηνύματος, commit μόλις εγκριθεί |
| `pr vlc` | push και PR με ό,τι έχει στήσει ο χρήστης, αλλιώς τυπώνει το link |
| `done vlc` | σβήνει το branch τοπικά και στο fork |

Μεταβλητές για το SlackBuild μπαίνουν μπροστά: `sudo NAME=MARIA slacker-sbo build vlc`. Το `FOO=x sudo ...` τις χάνει, γιατί το sudo καθαρίζει το περιβάλλον.

## Slackware: mirror και βασικά δέντρα

Δύο πλήρως ενημερωμένα δέντρα Slackware 15.0 (x86\_64 και i586), πάνω στα οποία γίνονται όλα τα builds.

**Mirror (`MIRROR_DIR`)**

- Κατεβαίνουν μόνο `slackware64/` και `slackware/` του 15.0, μαζί με τα `patches/` τους. Όχι `source/`, `extra/`, `testing/`.
- rsync με `--delete`: κατεβαίνει μόνο ό,τι άλλαξε, φεύγουν όσα αφαιρέθηκαν.

**Έλεγχος**

1. GPG: η υπογραφή `CHECKSUMS.md5.asc` επιβεβαιώνει ότι το `CHECKSUMS.md5` είναι γνήσιο. Ρυθμίζεται με `MIRROR_GPG`.
2. md5: κάθε πακέτο ελέγχεται απέναντι στο `CHECKSUMS.md5`. Γίνεται πάντα, χωρίς ρύθμιση.

Το κλειδί του Slackware μένει σε δικό μας keyring μέσα στο `MIRROR_DIR`, όχι στο keyring του root. Σε οποιαδήποτε αποτυχία, το `update-slackware` σταματά και δεν αγγίζει τα βασικά δέντρα.

**Βασικά δέντρα (`BASE_DIR`)**

- Πλήρης εγκατάσταση (περίπου 15 GB το καθένα), με `installpkg --root` και μετά `upgradepkg --root` για τα patches. Το SBo υποθέτει full install.
- Η εγκατάσταση γίνεται μέσα σε sandbox, με όλο το `/` του host μόνο για ανάγνωση. Έτσι, ακόμα κι αν ένα `doinst.sh` καλεί εντολή με απόλυτο path, δεν γράφει στον host.
- Τα αρχεία ανήκουν στους μετατοπισμένους uids (`SUBID_BASE`), άρα τα δέντρα χρησιμεύουν μόνο μέσα από το εργαλείο.
- Τα δέντρα αλλάζουν μόνο με `update-slackware`, όταν δεν τρέχει build. Αλλαγή στο lowerdir ενώ υπάρχει overlay δίνει απροσδιόριστη συμπεριφορά.
- Τα builds στο i586 δέντρο τρέχουν με `personality(PER_LINUX32)`, ώστε το `uname -m` να δίνει `i686` και το SlackBuild να βγάζει `ARCH=i586`.

## git

Ένα clone του fork σου (`origin`) με το SBo ως δεύτερο remote (`upstream`), όπως στον οδηγό του SBo.

- **Clone:** partial clone (`--filter=blob:none`). Το `upstream` δηλώνεται κι αυτό promisor.
- **Πιστοποίηση:** το git τρέχει ως `GIT_USER` και χρησιμοποιεί ό,τι έχει στήσει ο χρήστης (ssh key, credential helper). Το εργαλείο δεν κρατά tokens. Αν αποτύχει το push, δείχνει το μήνυμα του git και σταματά.
- **Συγχρονισμός (`update-sbo`):** `fetch upstream`, μετά push του `upstream/master` στο `origin/master` μόνο με fast-forward. Αν δεν γίνεται fast-forward, σταματά. Ποτέ force push στο master.
- **Worktree ανάγνωσης:** πάντα στο `upstream/master`, για το `build vlc`. Το εργαλείο δεν γράφει ποτέ μέσα στα git trees.
- **Worktree ανάπτυξης:** ένα για κάθε branch που ανοίγει το `dev`.
- **Remote branches:** τα σβήνει μόνο το `done`. Κανένα clean δεν τα αγγίζει, γιατί το GitHub κλείνει το PR όταν σβηστεί το branch του.

## Sandbox

Μέσα στο sandbox ο χρήστης είναι root· έξω δεν είναι ποτέ root, σε καμία περίπτωση.

**Απομόνωση**

- **User namespace:** uid/gid 0..65535 μέσα αντιστοιχούν στο `SUBID_BASE`.. έξω. Αν το εύρος συγκρούεται με υπαρκτούς χρήστες ή με τα `/etc/subuid`, `/etc/subgid`, το εργαλείο δεν ξεκινά.
- **mount:** δικό του namespace, όλα τα mounts private, `pivot_root` αντί για chroot.
- **pid, net, ipc, uts:** δεν βλέπει διεργασίες του host, δεν έχει δίκτυο, έχει δικό του hostname.
- **`/dev`:** tmpfs με μόνο `null`, `zero`, `full`, `random`, `urandom`, `tty`, `ptmx`. Το `/sys` λείπει ή είναι μόνο για ανάγνωση.
- **Κανένα mount από τον host**, ούτε στο `shell`.

**Δίσκος**

- overlay: lower = το βασικό δέντρο (μόνο για ανάγνωση), upper = στο loop image ή στο tmpfs (`WORK_FS`, `WORK_SIZE`). Ο host δεν γεμίζει ποτέ.

**cgroups (v1 και v2)**

|  | cgroup v2 | cgroup v1 |
| --- | --- | --- |
| μνήμη (`BUILD_MEM`) | `memory.max` | `memory.limit_in_bytes` |
| διεργασίες (`BUILD_PIDS`) | `pids.max` | `pids.max` |
| καθάρισμα στο τέλος | `cgroup.kill` | `freezer` FROZEN, kill, THAWED |

Αν λείπει controller, αποφασίζει το `CGROUP_MISSING`: `stop` δεν ξεκινά το build, `warn` συνεχίζει χωρίς το όριο. Για CPU δεν υπάρχει όριο.

**Περιβάλλον**

- Το περιβάλλον με το οποίο τρέχει το εργαλείο περνάει μέσα όπως είναι. Το εργαλείο δεν φιλτράρει και δεν κρίνει· αυτό το κάνει το SlackBuild.
- Τα `PATH`, `HOME`, `ARCH`, `OUTPUT` τα ορίζει πάντα το εργαλείο. Ο χρήστης δεν μπορεί να τα αλλάξει.

**Ανταλλαγή με τον host**

- Μέσα μπαίνουν μόνο, με αντιγραφή: ο φάκελος του SlackBuild, τα sources και τα `pre`/`post-install`.
- Έξω δεν βγαίνει τίποτα. Το `.tgz` μένει μέσα στο sandbox, στο `WORK_DIR`, μέχρι το επόμενο build ή το `clean`. Το εργαλείο τυπώνει μόνο τη διαδρομή του· ο χρήστης το αντιγράφει με το χέρι, αν θέλει.
- Ποτέ `installpkg`, `upgradepkg`, `removepkg` στο `/` του host. Κανένα repository (όχι `PACKAGES.TXT`, `CHECKSUMS`, υπογραφές).
- Γράφει μόνο στους φακέλους του conf.

## Ροή build

Κάθε πακέτο της αλυσίδας χτίζεται σε δικό του καθαρό sandbox, από κάτω προς τα πάνω.

1. Λύνει τα `REQUIRES` από τα `.info`, αναδρομικά.
2. Κατεβάζει τα sources στον host, στο `SRC_DIR`, και ελέγχει το `MD5SUM` (το `DOWNLOAD_x86_64` για x86\_64 όταν υπάρχει).
3. Για κάθε εξάρτηση: build σε καθαρό sandbox. Το `.tgz` μπαίνει στην cache εξαρτήσεων, μέσα στον χώρο του εργαλείου, προσβάσιμη μόνο από τα sandboxes.
4. Για το τελικό πακέτο: `installpkg` των εξαρτήσεων από την cache μέσα στο sandbox, μετά build.

**`%README%` στο `REQUIRES`:** το εργαλείο δείχνει το README και ρωτά. Δεν μαντεύει τις προαιρετικές εξαρτήσεις και δεν υπάρχει επιλογή -y· αποφασίζει πάντα ο άνθρωπος.

**Αρχιτεκτονικές:** και οι δύο by default.

- `DOWNLOAD="UNSUPPORTED"`: όχι i586.
- `DOWNLOAD_x86_64="UNSUPPORTED"`: όχι x86\_64.
- Κενό `DOWNLOAD_x86_64`: το x86\_64 χρησιμοποιεί το `DOWNLOAD`.
- Εξάρτηση UNSUPPORTED σε μια αρχιτεκτονική: το πακέτο δεν χτίζεται εκεί, και το εργαλείο λέει ποια εξάρτηση φταίει.
- Παράκαμψη από τη γραμμή εντολών (--arch i586) ή από το `ARCH` του conf.

**pre-install και post-install**

- Αρχεία `pre-install.<pkg>` και `post-install.<pkg>` στο `WORK_DIR/hooks/`. Τα γράφει μόνο ο άνθρωπος, με το χέρι, αφού διαβάσει τα README. Το εργαλείο δεν τα γράφει και δεν προτείνει περιεχόμενο.
- Το `pre` τρέχει μέσα στο sandbox πριν από το build ή το `installpkg` του πακέτου. Το `post` μετά το `installpkg` του.
- Ξανατρέχουν κάθε φορά που το πακέτο μπαίνει σε sandbox, γιατί κάθε sandbox ξεκινά από το μηδέν.
- Αν δεν υπάρχουν αρχεία και το πακέτο έχει `README*`: τα δείχνει και ρωτά: συνέχεια, διακοπή για να γραφτούν τα αρχεία, ή «δεν χρειάζεται τίποτα».
- Το εργαλείο κρατά στο `WORK_DIR/hooks/` ένα hash των README που έχει δει ο χρήστης και ξαναρωτά μόνο αν αλλάξει κάποιο README.

**Editor**

Ο editor είναι ενσωματωμένος στο εργαλείο. Δεν τρέχει κανένα εξωτερικό πρόγραμμα και δεν χρησιμοποιείται το `VISUAL`/`EDITOR` του συστήματος.

- **`EDITOR="simple"`** (tui-textarea): χωρίς modes, βέλη, Home/End, PgUp/PgDn, Backspace/Delete, `Ctrl+S` αποθήκευση, `Ctrl+Q` έξοδος (ρωτά αν δεν έχει αποθηκευτεί), `Ctrl+Z`/`Ctrl+Y` undo/redo, μπάρα βοήθειας πάντα ορατή.
- **`EDITOR="vim"`** (edtui): vim style.
- Ανοίγει για ανάγνωση των README (μόνο για ανάγνωση) και για γράψιμο των `pre`/`post-install`, κατευθείαν στο `WORK_DIR/hooks/`.
- Νέες εξαρτήσεις του εργαλείου: `ratatui`, `crossterm`, `tui-textarea`, `edtui`.

## Ανάπτυξη και PR

Η ροή ακολουθεί τον οδηγό του SBo βήμα προς βήμα. Το «καθαρό VM» του οδηγού είναι το sandbox.

1. **`dev vlc`:** branch `vlc` και worktree. Αν υπάρχει `origin/vlc`, ρωτά αν θα συνεχίσεις από εκεί ή από την αρχή (τότε το επόμενο push είναι `push -f`). Αν δεν υπάρχει, νέο branch από φρέσκο upstream.
2. **Αλλαγές** στα αρχεία του worktree, με το χέρι.
3. **`build --dev vlc`:** καθαρό build από το branch, x86\_64 και i586. Κάθε πακέτο έχει το δικό του branch. Οι εξαρτήσεις έρχονται πάντα από το SBo, σύμφωνα με το REQUIRES του .info του branch. Εξάρτηση από το δικό της branch ανάπτυξης μόνο ρητά, με --use-dev libplacebo (και στο shell)· αν το branch δεν υπάρχει, σταματά. Εξάρτηση που δεν υπάρχει στο SBo: σταματά, γιατί η υποβολή της πάει από τη φόρμα.
4. **`lint vlc`:** `sbolint` και `sbopkglint` μέσα στο sandbox. Το sbo-maintainer-tools χτίζεται εκεί σαν εξάρτηση.
5. **`commit vlc`:** έλεγχος permissions (0644 αρχεία, 0755 φάκελοι), πρόταση μηνύματος `<category>/<pkg>: Updated for version X.`, commit μόλις εγκριθεί. Ένα commit ανά πακέτο.
6. **`pr vlc`:** `push -f origin vlc` ως `GIT_USER`. Αν υπάρχει και είναι συνδεδεμένο το CLI της πλατφόρμας (`gh`, `glab`, `tea`, ανάλογα με το host του `FORK_URL`), ανοίγει το PR. Αλλιώς τυπώνει το link για τον browser.
7. **`done vlc`:** μετά το merge, σβήνει το branch τοπικά και στο fork, μαζί με το worktree.

Για νέα SlackBuilds δεν γίνεται PR· η υποβολή γίνεται από τη φόρμα του SBo.

## Κανόνες clean

Όταν λέμε clean, είναι όλα clean. Ένα υπόλοιπο από παλιό build ακυρώνει τον έλεγχο.

|  | `clean` | `update-sbo` / `update-slackware` |
| --- | --- | --- |
| sandboxes | ναι | ναι |
| cache εξαρτήσεων | ναι | ναι |
| tarballs (`SRC_DIR`) | όχι | ναι |
| worktree ανάγνωσης | όχι | ναι |
| τοπικά branches και worktrees ανάπτυξης | όχι | ναι |
| remote branches στο fork | όχι | όχι |
| `WORK_DIR/hooks/` (pre/post-install, hash README) | όχι | όχι |

Ό,τι δεν έχει γίνει push πριν από ένα `update-*` χάνεται· είναι ευθύνη του χρήστη. Θα το γράφει καθαρά το `--help` και η τεκμηρίωση.

## Ανοιχτά

Τα παρακάτω είναι προτάσεις ή προεπιλογές που δεν έχουν εγκριθεί ρητά. Δεν μπαίνουν στον κώδικα πριν αποφασιστούν.

Κανένα αυτή τη στιγμή.
