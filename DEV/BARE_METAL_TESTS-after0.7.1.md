# slacker — bare-metal test plan (γύρος: pin-aware `collect` + install-new fill-missing)

Repos αναφοράς: `slackware (100)`, `extras (90)`, `conraid (80)`, `alienbob (60)`.

> Συμβουλή ασφαλείας: τρέξε πρώτα **ό,τι μπορείς με `--dry-run`** (δεν αλλάζει τίποτα). Τα destructive (remove, πραγματικό install/install-new) μόνο αφού δεις σωστό plan.

---

## A. No-regression — ένας selector (πρέπει να δουλεύουν ΑΚΡΙΒΩΣ όπως πριν)

```
slacker search firefox
slacker info bash
slacker install <not-installed-pkg> --dry-run
slacker upgrade <installed-pkg> --dry-run
slacker reinstall <installed-pkg> --dry-run
slacker remove <pkg> --dry-run
slacker install @gnome --dry-run        # ένα @repo: λίστα των μη-εγκατεστημένων του
```
Αναμενόμενο: ίδια συμπεριφορά με το προηγούμενο build. (Single pattern = μηδέν cross-pattern, το collect είναι no-op εκεί.)

---

## B. collect priority — πολλά `@repo` (ο πυρήνας της αλλαγής)

Διάλεξε ένα όνομα πακέτου που το σερβίρουν **δύο** repos (π.χ. κάτι που έχουν και conraid και alienbob).

```
slacker install @conraid @alienbob --dry-run
slacker install @alienbob @conraid --dry-run     # αντίστροφη σειρά
```
Αναμενόμενο: για το κοινό όνομα, στο plan ο candidate πρέπει να είναι από **conraid (80)**, όχι alienbob (60) — **και στις δύο σειρές** (αποφασίζει η priority, όχι η σειρά). Τα μοναδικά πακέτα κάθε repo εμφανίζονται κανονικά.

---

## C. pins `repo:pkg` — να ΜΗΝ σπάσουν και να παρακάμπτουν priority

```
slacker install alienbob:vlc --dry-run                  # single pin → vlc του alienbob
slacker install alienbob:vlc @slackware --dry-run       # pin νικά την υψηλότερη priority
slacker install @slackware alienbob:vlc --dry-run       # ίδιο, αντίστροφη σειρά
slacker install slackware:vlc alienbob:vlc --dry-run    # δύο pins ίδιου ονόματος
slacker install alienbob:vlc alienbob:<foo> --dry-run    # δύο pins, διαφορετικά ονόματα
```
Αναμενόμενο:
- single pin → πάντα το pinned repo.
- `alienbob:vlc @slackware` (κάθε σειρά) → vlc από **alienbob** (το pin υπερισχύει, αν και slackware=100 > alienbob=60).
- δύο pins ίδιου ονόματος → κερδίζει το **πρώτο** στη γραμμή (εδώ `slackware:vlc`). Άλλαξε σειρά → κερδίζει το πρώτο.
- δύο pins διαφορετικών ονομάτων → μπαίνουν **και τα δύο**.

---

## D. Άκυρο `@repo:pkg` — καθαρό μήνυμα λάθους

```
slacker install @alienbob:vlc
slacker install @alienbob:vlc conraid:<foo>
```
Αναμενόμενο: σφάλμα `unknown repo or tag '@alienbob:vlc'` (με πρόταση «did you mean…»), **πριν** γίνει οτιδήποτε. Η σωστή σύνταξη pin είναι **χωρίς** `@`.

---

## E. install-new — η νέα συμπεριφορά (fill-missing-from-official)

```
# 1) σβήσε δοκιμαστικά 1-2 πακέτα
slacker remove <pkgA> <pkgB> --dry-run        # δες το plan
slacker remove <pkgA> <pkgB>                  # μετά πραγματικά

# 2) τώρα το install-new ΠΡΕΠΕΙ να τα ξαναπιάνει
slacker install-new --dry-run                 # πρέπει να εμφανίζονται τα <pkgA> <pkgB>
slacker install-new                           # με το βήμα επιλογής (δες παρακάτω)
```
Έλεγχος **selection** (μπαίνει όταν ≥2 πακέτα): εμφανίζεται αριθμημένη λίστα και
`Enter numbers to install (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel:` →
δοκίμασε `Enter` (όλα), `1 3` / `2-4` (επιλεκτικά), `n` (άκυρο).

Έλεγχος **frozen**: 
```
slacker frozen <pkgA>
slacker install-new --dry-run                 # το <pkgA> ΔΕΝ προσφέρεται· φαίνεται στη γραμμή "frozen"
```
Έλεγχος **priority / άλλο repo**: επιβεβαίωσε ότι ένα πακέτο που το έχεις από conraid/_SBo **δεν** προσφέρεται από το install-new (είναι εγκατεστημένο κατά όνομα → προσπερνιέται σιωπηλά — σωστό).

Έλεγχος **scope**:
```
slacker install-new alienbob --dry-run        # μόνο από το συγκεκριμένο repo
```

---

## F. installed-target family με πολλά `@repo` (πρέπει να ήταν ΗΔΗ σωστά — regression)

```
slacker upgrade @conraid @alienbob --dry-run
slacker reinstall @conraid --dry-run
slacker remove @alienbob --dry-run            # ΜΟΝΟ dry-run για έλεγχο!
slacker upgrade conraid:<installed-pkg> --dry-run
```
Αναμενόμενο: ένωση **εγκατεστημένων** πακέτων ανά build-tag κάθε repo, καμία μετανάστευση σε χαμηλότερη πηγή, η γραμμή "kept (… higher/equal-priority …)" εμφανίζεται όπου χρειάζεται.

---

## G. Selection στο upgrade-all (από προηγούμενο γύρο)

```
slacker upgrade-all --dry-run                 # δες ότι λύνει deps & δείχνει plan
slacker upgrade-all                           # όταν υπάρχουν ≥2 updates: αριθμημένη επιλογή
```
Αναμενόμενο: όταν τα διαθέσιμα upgrades είναι ≥2, εμφανίζεται η αριθμημένη λίστα επιλογής
(`Enter`=όλα, αριθμοί/ranges, `n`=άκυρο) **πριν** το resolve· μετά το κανονικό plan + `Proceed?`.

---

## H. Επιβεβαίωση kept / protected-deps (output & δικαίωμα επιλογής)

- Σε `upgrade`/`reinstall` ενός πακέτου που θα μετανάστευε σε χαμηλότερη πηγή: δες τη μπλε γραμμή
  `kept (installed from a higher/equal-priority source):` με το «γιατί».
- Σε οποιοδήποτε install που τραβά **dep** ήδη εγκατεστημένο από ίση/μεγαλύτερη πηγή: δες τον πίνακα
  `These dependencies are already installed from a higher-or-equal priority source:` με επιλογή
  `[k]eep / [r]eplace / keep-[a]ll` (default keep). Με `--yes` δείχνεται και κρατά όλα.

---

### Τι ψάχνουμε συνολικά
- Β/Γ: η priority αποφασίζει στα `@a @b`, το pin πάντα υπερισχύει, ανεξαρτήτως σειράς.
- Δ: το `@repo:pkg` βγάζει καθαρό error.
- Ε: το install-new πιάνει removed + νέα, σέβεται frozen/installed/scope.
- ΣΤ/Ζ/Η: τίποτα από τα προϋπάρχοντα δεν άλλαξε (regression-free).

Αν κάτι αποκλίνει, στείλε μου το ακριβές command + output και το κοιτάμε.
