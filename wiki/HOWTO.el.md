# slacker HOWTO (Ελληνικά)

Ένας πρακτικός, καθοδηγούμενος-από-παραδείγματα οδηγός για το `slacker`, έναν
διαχειριστή δυαδικών πακέτων για το Slackware που συνδυάζει το `slackpkg` και το
`slackpkg+` σε ένα εργαλείο.

Κάθε παράδειγμα παρακάτω είναι πραγματική εντολή. Οι ενέργειες που τροποποιούν το
σύστημα (install, upgrade, remove, ...) θέλουν root· τα ερωτήματα (search, info,
file-search, ...) όχι.

---

## Πίνακας περιεχομένων

1. Πρώτη ρύθμιση
2. Διατήρηση φρέσκων metadata
3. Αναζήτηση, επιθεώρηση και λίστα αποθετηρίων
4. Εγκατάσταση
5. Αναβάθμιση
6. Επανεγκατάσταση και αφαίρεση
7. Ολόκληρα αποθετήρια και build tags (επιλογείς `@`)
8. Διαχείριση αποθετηρίων (προσθήκη, αφαίρεση, tags, εμπιστοσύνη)
9. Λήψη χωρίς εγκατάσταση
10. «Πάγωμα» πακέτων (blacklist)
11. Templates
12. Καθαρισμός
13. Καθολικά flags και κωδικοί εξόδου
14. Συνηθισμένες ροές εργασίας
15. Επαλήθευση πακέτων

---

## 1. Πρώτη ρύθμιση

Η ρύθμιση βρίσκεται στο `/etc/slacker/` (παρακάμπτεται με `--config-dir`).
Αντίγραψε τα έτοιμα templates και τροποποίησέ τα:


Διάλεξε ακριβώς έναν mirror στο `/etc/slacker/mirrors` (κανένας δεν είναι ενεργός
εξ ορισμού· δύο ή περισσότερες ενεργές γραμμές είναι σφάλμα):

```
# /etc/slacker/mirrors  - uncomment ONE line
https://slackware.uk/slackware/slackware64-current/
```

Δήλωσε τα αποθετήριά σου στο `/etc/slacker/repos`. Τα δυαδικά αποθετήρια παίρνουν
ένα URL (ή τη λέξη-κλειδί `mirror` για το επίσημο) και πρέπει να έχουν **διακριτές**
προτεραιότητες· νικά η υψηλότερη:

```
# priority  name        url|mirror                                            [flags]
100         slackware   mirror                                                official
90          extras      https://mirror.nl.leaseweb.net/slackware/slackware64-current/extra   subtree
80          conraid     https://slackers.it/repository/slackware64-current
60          alienbob    https://slackware.nl/people/alien/sbrepos/current/x86_64
```

Τα flags (με οποιαδήποτε σειρά) μπαίνουν μετά το URL: `official` (το
παρακολουθούμενο αποθετήριο), `subtree` (Slackware distribution subtree — δες
παρακάτω), `immutable` (αποθετήριο του οποίου τα πακέτα το `clean-system` δεν
αφαιρεί ποτέ) και `verify=...` (παράκαμψη επαλήθευσης ανά αποθετήριο).

Τα τέσσερα Slackware distribution subtrees — **`extra`, `patches`, `testing`,
`pasture`** — **πρέπει πάντα να φέρουν το flag `subtree`** (σε οποιαδήποτε θέση μετά
το URL). Το `PACKAGES.TXT` τους γράφει τις τοποθεσίες των πακέτων σχετικά με τη
ρίζα της διανομής, οπότε χωρίς `subtree` τα πακέτα τους αποτυγχάνουν να κατέβουν
(διπλό τμήμα διαδρομής)· με αυτό, τα πακέτα και το `GPG-KEY` φέρνονται από το γονικό
(ρίζα) URL ενώ τα metadata εξακολουθούν να έρχονται από το URL του ίδιου του
αποθετηρίου. Αυτό δεν είναι προαιρετικό για αυτά τα τέσσερα αποθετήρια.

Προαιρετικά, πρόσθεσε γραμμές προτεραιότητας tag ώστε τα πακέτα από πηγή/τοπικά να
μην μεταναστεύουν ούτε να υποβαθμίζονται ποτέ από το `upgrade-all`:

```
100         SBo         _SBo
100         local       _rtz
```

Εισήγαγε μία φορά τα GPG κλειδιά των αποθετηρίων, μετά ανανέωσε και έλεγξε τη
ρύθμιση:

```
slacker update gpg
slacker update
slacker status                  # επιβεβαιώνει ότι η ρύθμιση είναι υγιής και επισημαίνει ό,τι θέλει διόρθωση
```

Το `update gpg` καρφιτσώνει (pin) το κλειδί κάθε αποθετηρίου (trust on first use).
Για ένα αποθετήριο `subtree` καρφιτσώνει το ίδιο κλειδί του Slackware από τη ρίζα,
κι αυτό ακριβώς επιτρέπει στην per-package GPG επαλήθευση να πετύχει για τα πακέτα
`extra/`/`testing/`/`patches/` κατά την εγκατάσταση.

---

## 2. Διατήρηση φρέσκων metadata

```
slacker update                  # ανανέωση PACKAGES.TXT/CHECKSUMS, επαλήθευση GPG
slacker update gpg              # (επαν)εισαγωγή GPG κλειδιών, μετά ανανέωση
slacker check-updates           # έλεγχος κάθε αποθετηρίου· έξοδος 100 αν εκκρεμεί ενημέρωση
slacker show-changelog          # προβολή του cached ChangeLog (με pager σε TTY)
slacker show-changelog conraid  # ChangeLog συγκεκριμένου αποθετηρίου (λήψη on demand)
```

---

## 3. Αναζήτηση, επιθεώρηση και λίστα αποθετηρίων

```
slacker search firefox          # εύρεση πακέτου με το ακριβές του όνομα (χωρίς διάκριση πεζών/κεφαλαίων)
slacker info bash               # υποψήφια ανά αποθετήριο + εγκατεστημένη έκδοση
slacker file-search bin/bash    # ποιο πακέτο περιέχει αυτό το αρχείο (χρησιμοποιεί MANIFEST)
slacker list-repos              # αποθετήρια: προτεραιότητα, πλήθος εγκατεστημένων, verify, flags
slacker status                  # έλεγχος υγείας όλης της ρύθμισης· λέει τι να διορθώσεις
```

Το `info` δείχνει ποιο αποθετήριο νικά κατά προτεραιότητα. Για παράδειγμα, αν το
`ffmpeg` υπάρχει σε πολλά αποθετήρια, υποψήφιο είναι αυτό με την υψηλότερη
προτεραιότητα· κάρφωσε άλλο με `repo:name` (δες παρακάτω).

---

## 4. Εγκατάσταση

```
slacker install vlc                     # ένα πακέτο (+ την αλυσίδα .dep του)
slacker install vlc mpv obs-studio      # πολλά μαζί
slacker install conraid:ffmpeg          # εξανάγκασε τη build του conraid (pin)
slacker --dry-run install vlc           # μόνο προεπισκόπηση, καμία αλλαγή
slacker --no-deps install vlc           # παράκαμψη επίλυσης εξαρτήσεων
slacker -y install vlc                  # «ναι» σε όλα τα prompts
```

Αν ένα pattern ταιριάζει σε περισσότερα από ένα πακέτα, το slacker τυπώνει
αριθμημένη λίστα:

```
slacker install python
# 'install' matched 12 packages:
#   1) [slackware] python-build-...
#   2) [slackware] python-cffi-...
#   ...
# Enter numbers to install (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel:
```

Τα ήδη εγκατεστημένα πακέτα απορρίπτονται από το `install` (χρησιμοποίησε `upgrade`
ή `reinstall`).

**Οι εξαρτήσεις είναι δυνατότητα τρίτων, όχι επίσημη.** Τα ίδια τα αποθετήρια του
Slackware — το `slackware` και τα subtrees `extra`/`patches`/`testing`/`pasture` —
ούτε παρέχουν ούτε αναμένουν πληροφορία εξαρτήσεων: η επίσημη προϋπόθεση είναι
**πλήρης** εγκατάσταση **όλων** των Slackware package sets, οπότε κάθε εξάρτηση
θεωρείται ήδη παρούσα, και το slacker δεν κάνει καμία επίλυση εξαρτήσεων γι' αυτά
(δεν χρειάζεται). Ανεξάρτητα αποθετήρια όπως το `alienbob` και το `conraid`
παρέχουν per-package αρχεία `.dep`· το slacker τα διαβάζει **μόνο για τα αποθετήρια
που τα παρέχουν**, τραβώντας τυχόν εξαρτήσεις που λείπουν από το ίδιο αποθετήριο.
Έτσι, ένα `install`/`upgrade` από το επίσημο δέντρο δεν επιλύει τίποτα, ενώ από
αποθετήριο τρίτου με αρχεία `.dep` επιλύει. Απενεργοποίησέ το οπουδήποτε με
`--no-deps` (ανά εκτέλεση) ή `RESOLVE_DEPS=no` (στο `slacker.conf`).

---

## 5. Αναβάθμιση

```
slacker upgrade vlc             # αναβάθμιση συγκεκριμένων πακέτων
slacker upgrade-all             # αναβάθμιση όλων όσων έχουν νεότερη αναθεώρηση
slacker --dry-run upgrade-all   # προεπισκόπηση όλου του σχεδίου αναβάθμισης πρώτα
slacker -y upgrade-all          # χωρίς prompts
```

Το `upgrade-all` σέβεται την προτεραιότητα και τα build tags: ένα πακέτο
αντικαθίσταται μόνο από υποψήφιο αποθετηρίου ίσης ή υψηλότερης προτεραιότητας,
οπότε τα πακέτα SBo/τοπικά/από-πηγή δεν μεταναστεύουν ποτέ σιωπηλά σε άλλο
αποθετήριο ούτε υποβαθμίζονται. Οι νέες εξαρτήσεις που χρειάζεται εμφανίζονται πριν
την επιβεβαίωση ως `new-dep: [repo] pkg (for parent)`.

Εγκατάσταση πακέτων που προστέθηκαν στη διανομή από την τελευταία σου ενημέρωση:

```
slacker install-new             # μόνο επίσημα αποθετήρια (προεπιλογή)
slacker install-new conraid     # μόνο τα νεοπροστιθέμενα πακέτα του conraid
slacker install-new slackware extras
```

---

## 6. Επανεγκατάσταση και αφαίρεση

```
slacker reinstall bash          # επανεγκατάσταση της τρέχουσας έκδοσης
slacker reinstall y             # επανεγκατάσταση ολόκληρης σειράς (εδώ: games)
slacker reinstall ap            # η σειρά 'ap'
slacker remove libfoo           # αφαίρεση εγκατεστημένων πακέτων
slacker --dry-run remove libfoo # προεπισκόπηση
```

Τα ονόματα σειρών (`a`, `ap`, `d`, `k`, `kde`, `l`, `n`, `t`, `x`, `xap`, `xfce`,
`y`, ...) ταιριάζουν ακριβώς αυτή τη σειρά, όχι κάθε πακέτο του οποίου το όνομα
περιέχει αυτά τα γράμματα. Ένα πολλαπλό ταίριασμα δείχνει και πάλι την αριθμημένη
λίστα επιλογής.

---

## 7. Ολόκληρα αποθετήρια και build tags (επιλογείς `@`)

Το πρόθεμα `@` είναι ρητός επιλογέας συνόλου. Είναι υποχρεωτικό — μια σκέτη λέξη δεν
θεωρείται ποτέ αποθετήριο.

```
slacker install @gnome          # εγκατάσταση κάθε πακέτου του αποθετηρίου gnome
slacker remove  @gnome          # αφαίρεση των εγκατεστημένων πακέτων από αυτό το αποθετήριο
slacker remove  @_SBo           # αφαίρεση όλων των εγκατεστημένων πακέτων SlackBuilds.org
slacker download @alienbob      # λήψη κάθε πακέτου του αποθετηρίου alienbob
```

Το `@repo` σημαίνει «κάθε πακέτο σε αυτό το αποθετήριο»· το `@_tag` σημαίνει «κάθε
πακέτο με αυτό το build tag». Ένα τυπογραφικό δίνει χρήσιμο σφάλμα:

```
slacker install @gnme
# error: unknown repo or tag '@gnme'; did you mean '@gnome'?
#   available repos: conraid, gnome, slackware
#   available tags:  _gnome, cf
```

Τυπική χρήση: βάλε ένα desktop αποθετήριο (π.χ. gnome) σε διακριτή, υψηλή
προτεραιότητα όπως 101 και εγκατέστησέ το ως σύνολο. Τα πακέτα του με tag `_gnome`
τότε κλειδώνονται — κανένα κατώτερο αποθετήριο δεν μπορεί να τα αντικαταστήσει ή να
τα «αναβαθμίσει», ακόμη και με νεότερη έκδοση:

```
# /etc/slacker/repos
101  gnome  https://your-gnome-repo/...
```
```
slacker update
slacker install @gnome
slacker upgrade-all             # αφήνει τα πακέτα gnome ανέγγιχτα
```

---

## 8. Διαχείριση αποθετηρίων (προσθήκη, αφαίρεση, tags, εμπιστοσύνη)

Μπορείς να τροποποιήσεις το `/etc/slacker/repos` με το χέρι, ή να αφήσεις το
slacker να το κάνει για σένα (με έλεγχο εγκυρότητας και prompt επιβεβαίωσης):

```
slacker add-repo 70 extras https://.../slackware64-current/extra subtree
slacker add-repo 80 conraid https://slackers.it/repository/slackware64-current
slacker del-repo conraid
slacker add-tag 100 SBo _SBo            # γραμμή προτεραιότητας build-tag
slacker del-tag _SBo
```

Flags του `add-repo` (οποιαδήποτε σειρά): `official`, `immutable`, `subtree`,
`verify=...`. Ένα Slackware **subtree** (`extra/`, `patches/`, `testing/`,
`pasture/`) πρέπει να πάρει `subtree` αλλιώς τα πακέτα του αποτυγχάνουν να
κατέβουν· το `immutable` κρατά τα πακέτα ενός αποθετηρίου εκτός `clean-system`
(δες §12).

Επιθεώρηση και έλεγχος υγείας ανά πάσα στιγμή:

```
slacker list-repos              # προτεραιότητα, πλήθος εγκατεστημένων, πολιτική verify, flags
slacker status                  # έλεγχος υγείας όλης της ρύθμισης + τι να διορθώσεις
```

Το `list-repos` δείχνει πίνακα και σημειώνει `(official)`, `(immutable)`,
`(subtree)` και τυχόν quarantine. Το `status` ομαδοποιεί τα ευρήματά του
(Setup / Installed / Online) με δείκτες ✓/!/✗ και κλείνει με ετυμηγορία σε απλή
γλώσσα και επόμενα βήματα.

### Ασφάλεια αποθετηρίων: quarantine και εμπιστοσύνη

Το slacker ελέγχει (vets) τα αποθετήρια και βάζει σε **quarantine** όποιο είναι
απρόσιτο ή σερβίρει κακοσχηματισμένα/εχθρικά metadata· ένα αποθετήριο σε quarantine
δεν παρέχει κανένα πακέτο μέχρι να ενεργήσεις. Νέα ή μη-έμπιστα ακόμη αποθετήρια
ελέγχονται ελαφρά σε κάθε `update`· το `add-repo` και το `vet-repo` κάνουν ενδελεχή
έλεγχο.

```
slacker vet-repo conraid        # επανέλεγχος on demand (quarantine αν αποτύχει, καθάρισμα αν περάσει)
slacker trust-repo conraid      # άρση quarantine που κρίνεις false positive (override)
slacker distrust-repo conraid   # πάγωσε ένα αποθετήριο εσύ ο ίδιος
```

Τα GPG κλειδιά καρφιτσώνονται στην πρώτη εισαγωγή (trust on first use): αν το κλειδί
ενός αποθετηρίου αλλάξει ποτέ, το slacker το απορρίπτει ως πιθανή επίθεση
αντικατάστασης κλειδιού αντί να εμπιστευτεί σιωπηλά το νέο κλειδί. Το `list-repos`
και το `status` δείχνουν την κατάσταση.

---

## 9. Λήψη χωρίς εγκατάσταση

Τα αρχεία αποθηκεύονται στο `CACHE_DIR/packages/<repo>/` εξ ορισμού, στο ίδιο σημείο
που κοιτά το `install`, ώστε μια μετέπειτα εγκατάσταση να τα επαναχρησιμοποιεί.

```
slacker download pandoc-bin             # στην cache
slacker download -o /tmp pandoc-bin     # στο /tmp αντ' αυτού
slacker download -o . pandoc-bin        # στον τρέχοντα κατάλογο
slacker download @alienbob              # ολόκληρο αποθετήριο (ζητά επιβεβαίωση αν >10)
```

Το slacker αρνείται να γράψει μέσω προϋπάρχοντος symlink, οπότε η λήψη σε
κοινόχρηστο κατάλογο όπως το `/tmp` είναι ασφαλής.

---

## 10. «Πάγωμα» πακέτων (blacklist)

Πάγωσε ένα πακέτο ώστε τα update, upgrade-all, reinstall και clean-system να το
αφήνουν ήσυχο (προστίθεται στο `/etc/slacker/blacklist`):

```
slacker frozen pandoc-bin               # πάγωσε ένα
slacker frozen firefox chromium vlc     # πάγωσε πολλά
slacker frozen "@alienbob vlc-*"        # περιόρισε σε repo + ένα pattern (τα εισαγωγικά είναι απαραίτητα)
```

Βάλε εισαγωγικά σε κάθε κανόνα που έχει κενό (κανόνας `@repo`) ή χαρακτήρα shell
glob (`*`, `?`, `[`, `]`, ...). Το `"@alienbob vlc-*"` περιορίζει τον κανόνα στο
repo `alienbob`, και το `vlc-*` ταιριάζεται ως **unanchored regex** στο πλήρες id
— στο regex το `-*` σημαίνει «μηδέν ή περισσότερες παύλες», όχι «οτιδήποτε», οπότε
παγώνει κάθε εγκατεστημένο πακέτο του `alienbob` του οποίου το id περιέχει `vlc`
(π.χ. `vlc`, `vlc-plugin-qt`), ακριβώς όπως θα έκανε ένα σκέτο `vlc`. Για να
παγώσεις μόνο το πακέτο `vlc`, αγκύρωσέ το: `"@alienbob ^vlc-[0-9]"`.

Χρησιμοποίησε το ακριβές όνομα πακέτου (όχι το πλήρες version-tag). Για ξεπάγωμα,
αφαίρεσε τη γραμμή από το `/etc/slacker/blacklist`.

Η blacklist παγώνει μεμονωμένα **πακέτα**. Για να επέμβεις σε ολόκληρο
**αποθετήριο** υπάρχει ξεχωριστός μηχανισμός — *quarantine*: το `distrust-repo`
παγώνει ένα repo, το `vet-repo` το επανελέγχει, το `trust-repo` το ξεπαγώνει (§8).

Το blacklisting είναι ο per-package τρόπος να κρατήσεις κάτι εκτός `clean-system`.
Για να προστατεύσεις μια ολόκληρη ομάδα μονομιάς, προτίμησε το `IGNORE_TAGS` (ανά
build tag, π.χ. `_SBo cf alien`) ή το να μαρκάρεις ένα αποθετήριο `immutable`
(§8, §12) αντί να παγώνεις κάθε πακέτο με το χέρι.

---

## 11. Templates

Ένα template είναι ένα στιγμιότυπο των ονομάτων εγκατεστημένων πακέτων που μπορείς
να αναπαράγεις σε άλλο μηχάνημα ή μετά από επανεγκατάσταση.

```
slacker generate-template mybox         # στιγμιότυπο τρεχόντων πακέτων -> mybox.template
slacker install-template mybox          # εγκατάσταση όλων όσων αναφέρει το template
slacker remove-template mybox           # ΑΠΕΓΚΑΤΑΣΤΑΣΗ κάθε πακέτου που αναφέρει το template
slacker delete-template mybox           # διαγραφή μόνο του αρχείου template (κρατά τα πακέτα)
```

Πρόσεξε τη διάκριση: το `remove-template` αφαιρεί τα *πακέτα*· το `delete-template`
αφαιρεί μόνο το *αρχείο*.

---

## 12. Καθαρισμός

```
slacker clean-system            # λίστα πακέτων που δεν είναι πια στο επίσημο baseline, διάλεξε τι θα αφαιρεθεί
slacker --dry-run clean-system  # προεπισκόπηση πρώτα — κάνε το πάντα
slacker clean-cache             # διαγραφή κατεβασμένων *.txz από την cache
slacker clean-cache alienbob    # μόνο τα cached αρχεία αυτού του αποθετηρίου
slacker --dry-run clean-cache   # δείξε τι θα ελευθερωθεί
slacker new-config              # χειρισμός εναπομεινάντων *.new αρχείων ρύθμισης
```

Το `clean-cache` δεν αγγίζει ποτέ metadata αποθετηρίων ή GPG κλειδιά (αυτά ζουν στο
`CACHE_DIR/repos`), οπότε είναι πάντα ασφαλές να τρέξει.

Το `clean-system` είναι σε στυλ slackpkg: αφαιρεί πακέτα που **δεν είναι πια μέρος
του επίσημου baseline** — το `PACKAGES.TXT` του επίσημου αποθετηρίου συν οποιοδήποτε
αποθετήριο μαρκαρισμένο `immutable`. Έτσι, ένα πακέτο που η ίδια η διανομή αφαίρεσε
αφαιρείται ακόμη κι αν ένα αποθετήριο τρίτου εξακολουθεί να σερβίρει το όνομα. Ένα
πακέτο διατηρείται (δεν εμφανίζεται ποτέ) όταν ισχύει ένα από τα τρία:

- ταιριάζει με κανόνα **blacklist** (`slacker frozen NAME`)·
- το **build tag** του είναι στο `IGNORE_TAGS` (`slacker.conf`), π.χ. `_SBo cf alien`·
- αποδίδεται σε αποθετήριο **`immutable`** (το αποθετήριο που κατέχει το build tag
  του, ή για πακέτο χωρίς tag οποιοδήποτε immutable αποθετήριο παρέχει το όνομά του).

Έτσι, πριν το πρώτο σου `clean-system`, όρισε `IGNORE_TAGS` για τα δικά σου
SBo/τοπικά/από-πηγή tags ή/και μαρκάρισε τα αποθετήρια `extra/`/`testing/`/`patches/`
ως `immutable` — αλλιώς αυτά τα πακέτα θα εμφανιστούν ως ξένα. Ως δικλείδα ασφαλείας
το `clean-system` αρνείται να τρέξει αν ένα baseline αποθετήριο δεν έχει φορτωμένα
metadata (τρέξε `update` πρώτα), και το `--dry-run` δείχνει ακριβώς τι θα αφαιρούσε
χωρίς να αγγίξει τίποτα.

---

## 13. Καθολικά flags και κωδικοί εξόδου

Οι read-only εντολές (`search`, `info`, `file-search`, `check-updates`,
`show-changelog`) τρέχουν ως οποιοσδήποτε χρήστης. Ό,τι αλλάζει το σύστημα, την
cache ή τη ρύθμιση πρέπει να τρέξει ως root (ή μέσω sudo από μέλος του wheel)· μια
μη-root προσπάθεια σταματά αμέσως με σαφές μήνυμα.

Αυτές οι εντολές παίρνουν επίσης ένα αποκλειστικό lock (`/run/slacker.lock`) ώστε
να μην μπορούν να τρέξουν δύο ταυτόχρονα· μια δεύτερη κλήση βγαίνει αμέσως
αναφέροντας το PID που τρέχει. Το lock απελευθερώνεται αυτόματα αν το slacker
τερματίσει ή σκοτωθεί, οπότε ένα crash δεν σε κλειδώνει έξω ποτέ. Τα ερωτήματα δεν
παίρνουν lock.

Flags (δουλεύουν με κάθε εντολή):

```
--config-dir <DIR>    χρήση διαφορετικού καταλόγου ρύθμισης (προεπιλογή /etc/slacker)
-y, --yes             «ναι» σε όλα τα prompts
--dry-run             δείξε τι θα γινόταν, μην αλλάξεις τίποτα
--no-deps             μη διαβάζεις αρχεία .dep γι' αυτή την εκτέλεση
```

Κωδικοί εξόδου:

```
0     επιτυχία
1     σφάλμα
20    τίποτα δεν βρέθηκε / τίποτα προς εκτέλεση
50    υπάρχει διαθέσιμη αυτο-αναβάθμιση του slacker
100   εκκρεμείς ενημερώσεις (από check-updates)
```

Παράδειγμα ελέγχου σε script:

```
slacker check-updates ; [ $? -eq 100 ] && slacker -y upgrade-all
```

---

## 14. Συνηθισμένες ροές εργασίας

Τακτική ενημέρωση συστήματος:

```
slacker update
slacker upgrade-all
slacker --dry-run clean-system   # έλεγξε πρώτα (αφαιρεί ό,τι είναι εκτός baseline)
slacker clean-system             # μετά τρέξ' το, αφού έχεις ορίσει IGNORE_TAGS/immutable (δες §12)
```

Πρώτος συγχρονισμός μετά από επεξεργασία των repos, με εισαγωγή κλειδιών:

```
slacker update gpg
slacker update
slacker check-updates ; echo "exit=$?"
```

Προεπισκόπηση όλων πριν δεσμευτείς:

```
slacker --dry-run upgrade-all
slacker --dry-run install @gnome
slacker --dry-run clean-cache
```

Μεταφορά του συνόλου πακέτων ενός μηχανήματος σε άλλο:

```
# στο μηχάνημα-πηγή
slacker generate-template snapshot
# αντίγραψε το /etc/slacker/templates/snapshot.template στον στόχο, μετά:
slacker update
slacker install-template snapshot
```

Ελευθέρωσε χώρο χωρίς να ρισκάρεις metadata ή κλειδιά:

```
slacker clean-cache
```

---

## 15. Επαλήθευση πακέτων

Το slacker επαληθεύει τα πακέτα πριν τα εγκαταστήσει. Η πολιτική ορίζεται καθολικά
με το `VERIFY` στο `slacker.conf` και παρακάμπτεται ανά αποθετήριο με flag `verify=`
στη γραμμή του repo.

Προεπιλογή (`VERIFY=all`):

```
# slacker.conf
VERIFY=all
```

Με `all`, για κάθε πακέτο: η GPG υπογραφή ελέγχεται όταν το αποθετήριο παρέχει μία
(μια κακή υπογραφή πάντα αποτυγχάνει· μια απούσα παραβλέπεται), και τουλάχιστον ένα
checksum ακεραιότητας (md5 ή sha) πρέπει να υπάρχει και να ταιριάζει. Αν δεν υπάρχει
ούτε md5 ούτε sha, η εγκατάσταση σταματά — το αρχείο checksum του αποθετηρίου λείπει
ή είναι χαλασμένο.

Το Slackware παρέχει ένα per-package `.txz.asc` δίπλα σε κάθε πακέτο, οπότε υπό
`all` το slacker GPG-επαληθεύει το ίδιο το πακέτο και τυπώνει, π.χ.,
`verified: gpg (signer) + md5`. Γι' αυτό πρέπει να έχεις καρφιτσώσει το κλειδί του
αποθετηρίου με `slacker update gpg`· μέχρι τότε παίρνεις `integrity only: md5` (το
πακέτο εξακολουθεί να ελέγχεται με md5 έναντι του GPG-υπογεγραμμένου `CHECKSUMS.md5`,
απλώς δεν αυθεντικοποιείται per package). Για αποθετήριο `subtree` το κλειδί φέρνεται
από τη ρίζα, όπου το Slackware κρατά το ένα κλειδί που υπογράφει όλο το δέντρο — έτσι
τα `extra/`/`testing/`/`patches/` καρφιτσώνουν το ίδιο fingerprint με το επίσημο
αποθετήριο.

**Καρφίτσωμα κλειδιού (trust on first use):** η πρώτη εισαγωγή καρφιτσώνει το
fingerprint του αποθετηρίου· αν αλλάξει ποτέ, το slacker απορρίπτει το αποθετήριο ως
πιθανή επίθεση αντικατάστασης κλειδιού αντί να εμπιστευτεί σιωπηλά το νέο κλειδί. Δες
§8 για τις εντολές quarantine/εμπιστοσύνης.

Απαίτησε συγκεκριμένες μεθόδους (σταματά αν λείπει κάποια, λέγοντάς σου πώς να τη
χαλαρώσεις):

```
VERIFY=gpg,md5,sha
VERIFY=gpg,md5
VERIFY=md5
```

Πλήρης απενεργοποίηση (δεν συνιστάται):

```
VERIFY=none
```

Παράκαμψη ανά αποθετήριο — χρήσιμη όταν ένα αποθετήριο έχει χαλασμένο ή απόν checksum
ή υπογραφή, ώστε να χαλαρώσεις μόνο αυτό αντί να αποδυναμώσεις τα πάντα:

```
# repos
100  slackware  mirror                       official
80   conraid    https://slackers.it/...      verify=gpg,md5
60   alienbob   https://slackware.nl/...      verify=md5
```

Οι ίδιοι κανόνες ισχύουν για κάθε αποθετήριο, συμπεριλαμβανομένου του επίσημου — δεν
υπάρχει εξαίρεση. Το flag `official` επηρεάζει μόνο το εύρος του `install-new` και
την παρακολούθηση ChangeLog, όχι την επαλήθευση.

Αν μια λήψη αποτύχει στην επαλήθευση θα δεις σαφές μήνυμα, για παράδειγμα:

```
md5 mismatch for foo-1.0-x86_64-1cf.txz: expected ..., got ...
no usable checksum (md5 or sha) for foo-...: the repo's checksum file may be
  missing or broken. ... relax verification for it with a `verify=` flag ...
```
