# lita: εσωτερική προδιαγραφή

Αυτό το αρχείο αφορά όσους φτιάχνουν τη lita, όχι όσους τη χρησιμοποιούν. Το κριτήριο για κάθε απόφαση εδώ: το εσωτερικό να είναι όσο πιο δυνατό χρειάζεται, ώστε η σύνταξη για τον χρήστη να μένει απλή.

## 0. Αποφάσεις σε μία ματιά

1. Πυρήνας σε Rust, μόνο με τη standard library. Κανένα crate.
2. Ο πυρήνας εκτελεί πρόταση προς πρόταση. Δεν μεταφράζει σε άλλη γλώσσα.
3. Ό,τι δεν είναι πυρήνας είναι ρήμα: αρχείο `.verb` με πρότυπο σε sh, awk ή Perl (μόνο core modules), που καλεί εργαλεία του συστήματος. Νέο ρήμα χωρίς rebuild.
4. Κανένα ρήμα σε Python, και τίποτα από pip ή CPAN. Ο χρήστης μπορεί να δηλώσει Python στα δικά του scripts με `uses`.
5. Ένα ρήμα μπορεί να έχει συνώνυμα. Δύο ρήματα που ταιριάζουν στην ίδια πρόταση είναι λάθος, και ο πυρήνας δεν ξεκινάει.
6. Git: μόνο το εκτελέσιμο `git`, μόνο οι σταθερές μορφές εξόδου του.
7. Επιλογές: `--check`, `--show`, `--dry-run`, `--debug`, `--test-verbs`, `lita help`.
8. Η έξοδος προς τον χρήστη είναι πάντα απλό κείμενο, μία εγγραφή ανά γραμμή. Το εσωτερικό πρωτόκολλο με τα ρήματα χρησιμοποιεί NUL και TAB, αλλά αυτό δεν φαίνεται ποτέ έξω.

## 1. Αρχιτεκτονική

- **Πυρήνας** σε Rust: ένα binary, `lita`. Περιέχει τον parser, τον έλεγχο πριν την εκτέλεση και τον interpreter.
- **Λεξιλόγιο** σε αρχεία `.verb`: κάθε πρόταση που δεν είναι στον πυρήνα είναι ένα αρχείο με ένα μικρό πρότυπο σε sh, awk ή Perl, που καλεί εργαλεία του συστήματος.
- Νέα πρόταση = νέο αρχείο, χωρίς rebuild. Rebuild χρειάζεται μόνο όταν αλλάζει ο πυρήνας.

### Ο πυρήνας χωρίς crates

Μόνο `std`. Ο parser γράφεται με το χέρι (η γραμματική είναι μικρή και με εσοχές). Όπου το `std` δεν φτάνει, λίγες κλήσεις στη libc με `extern "C"`:

| Συνάρτηση libc | Για τι |
|---|---|
| `flock` | `only one at a time` |
| `sigaction` | `at the end` σε Ctrl-C (σημαία `AtomicBool`, ο κύριος βρόχος καθαρίζει) |

Τίποτα άλλο. Άρα:
- κανένα `Cargo.lock` με ξένα πακέτα, κανένα vendoring στο SlackBuild
- οι editions της Rust εγγυώνται ότι χτίζεται με νεότερο compiler
- το binary εξαρτάται μόνο από τη libc (και τη libgcc_s, όπως κάθε πρόγραμμα Rust)

Δοκιμασμένο σε πρωτότυπο μόνο με `std` (Rust 2021): καθαρό περιβάλλον με `env_clear`, μεταβλητή για μία εντολή, τρέχων φάκελος ανά εντολή, pipeline με τον κωδικό κάθε κομματιού, όριο χρόνου με `try_wait`/`kill`, παράλληλα με `std::thread`, ιδιωτικός φάκελος με mode 700 και τυχαίο όνομα από `/dev/urandom`, `flock`. Το πρωτότυπο είχε `signal()` αντί για `sigaction()` και δεν δοκιμάστηκε με πραγματικό Ctrl-C.

### Γλώσσες για τα ρήματα

| Γλώσσα | Για τι | Κανόνας |
|---|---|---|
| POSIX sh | ρήματα λίγων γραμμών, κλήσεις εργαλείων | χωρίς bashisms |
| POSIX awk | γραμμές, στήλες, μετρήσεις | χωρίς επεκτάσεις του gawk |
| Perl 5 | μορφές αρχείων, parsing, ό,τι θέλει δομή | μόνο core modules |
| εκτελέσιμο | εργαλείο που ήδη κάνει όλη τη δουλειά | καλείται απευθείας, χωρίς shell |

Python, CPAN, pip: όχι σε κανένα ρήμα. Ο χρήστης μπορεί να τα χρησιμοποιεί στα δικά του scripts με `uses`.

## 2. Ο πυρήνας

Στον πυρήνα μπαίνει ό,τι αλλάζει τη ροή, την κατάσταση ή το scope, ή πρέπει να είναι γνωστό στον έλεγχο πριν την εκτέλεση.

- **Τιμές:** κείμενο, αριθμός, ναι/όχι, path, λίστα, εγγραφή, `an empty list`, `{x}` μέσα σε κείμενο, `/` για paths, `#` για σχόλια
- **Μεταβλητές:** `x is ...`, `add x to ...`
- **Συνθήκες:** `if` / `otherwise if` / `otherwise`, `if ..., ...`, `is`, `is not`, `is empty`, `is in`, `more than`, `less than`, `at least`, `and`, `or`, `not`
- **Επανάληψη:** `for each ... in ...`, `... where ...`, `while ...`, `repeat ... times`, `skip it`, `stop looping`, `, N at a time`
- **Συναρτήσεις:** `to ...`, `the result is`, `add ... to the result`, `when asked to ...`, `when asked anything else`
- **Ροή:** `stop`, `stop with error`, `finish ok`, `finish with error`, `at the end`
- **Διεργασίες:** `run` (`.out`, `.err`, `.code`), `|`, `with X set to ...`, `in DIR, ...`, `go to DIR`, `, give up after ...`
- **Σφάλματα:** `if that failed`, `{the reason}`, `ignore failures`, `as one step`
- **Δηλώσεις:** `# lita 1`, `uses`, `ask`, `only one at a time`, `use other.lita`
- **Έξοδος:** `say`, `complain`

### Εκτέλεση

Ο πυρήνας **εκτελεί πρόταση προς πρόταση**. Δεν μεταφράζει το script σε άλλη γλώσσα. Οι τύποι και η κατάσταση (μεταβλητές, τρέχων φάκελος, ανοιχτά `as one step`, `at the end`) ζουν μέσα στον πυρήνα. Κάθε ρήμα τρέχει ως ξεχωριστή διεργασία.

Πριν την εκτέλεση, ο πυρήνας διαβάζει όλο το αρχείο και ελέγχει:
1. τη σύνταξη και το λεξιλόγιο (κάθε πρόταση πρέπει να ταιριάζει σε ακριβώς ένα ρήμα ή κατασκευή του πυρήνα)
2. τους τύπους
3. τα ονόματα μεταβλητών και συναρτήσεων
4. τα εργαλεία του `uses` και τα `needs:` όλων των ρημάτων που χρησιμοποιούνται
5. τους περιορισμούς των `effects:` (ενότητα 3)

Αν μια πρόταση δεν ταιριάζει, το μήνυμα προτείνει την πιο κοντινή: `line 12: no sentence "if file is older than days days". Did you mean "if file is newer than days days"?`

### Εργαλεία για όσους φτιάχνουν και ελέγχουν scripts

- `lita --show script`: για κάθε γραμμή, ποιο ρήμα θα τρέξει και με ποια ακριβώς ορίσματα και περιβάλλον
- `lita --dry-run script`: τρέχει μόνο ό,τι έχει `effects: none` ή `reads`. Τα υπόλοιπα τα τυπώνει χωρίς να τα εκτελέσει.
- `lita --check script`: μόνο ο έλεγχος πριν την εκτέλεση
- `lita --test-verbs`: τρέχει τα tests όλων των ρημάτων (ενότητα 7)

## 3. Η μορφή του αρχείου `.verb`

Ίδιος κανόνας με τη lita: γραμμές `key: value`, και τα blocks με εσοχή.

```
phrase:   fix permissions in DIR
DIR:      folder
returns:  nothing
effects:  files
needs:    find, chmod
lang:     sh
code:
    find "$DIR" -type d -exec chmod 755 {} + \
        -o -type f -perm /111 -exec chmod 755 {} + \
        -o -type f -exec chmod 644 {} +
undo:     none
help:     Directories and executables become 755, other files 644.
lita:     1
```

### Πεδία

| Πεδίο | Υποχρεωτικό | Τι είναι |
|---|---|---|
| `phrase:` | ναι, μία ή περισσότερες | Η πρόταση. Λέξεις με κεφαλαία είναι θέσεις για τιμές. `[the]` = προαιρετική λέξη, `delete\|remove` = εναλλακτικές. Πολλές γραμμές `phrase:` = συνώνυμα για το ίδιο ρήμα. |
| `ΟΝΟΜΑ:` | ναι, για κάθε θέση | Ο τύπος της θέσης: `text`, `number`, `yes/no`, `path`, `folder`, `file`, `list of text`, `list of paths`, `record`, `list of records`. Ο πυρήνας ελέγχει τον τύπο πριν καλέσει το ρήμα. |
| `returns:` | ναι | `nothing`, ένας από τους παραπάνω τύπους, ή `yes/no`. |
| `effects:` | ναι | `none`, `reads`, `files`, `system`, `network`. |
| `needs:` | ναι | Εργαλεία και γλώσσες που χρειάζεται. Ελέγχονται πριν την εκτέλεση. |
| `inherit:` | όχι | Μεταβλητές περιβάλλοντος του καλούντα που χρειάζεται το ρήμα (π.χ. `HOME, SSH_AUTH_SOCK` για git). Ό,τι δεν γράφεται εδώ δεν περνάει. |
| `lang:` | ναι | `sh`, `awk`, `perl`, `exec`. |
| `code:` | ναι | Το πρότυπο. |
| `undo:` | ναι | `none`, ή κώδικας που αναιρεί το `code:` (ίδια γλώσσα). |
| `help:` | ναι | Μία γραμμή. Χρησιμοποιείται στο `lita help ...` και στα «did you mean». |
| `lita:` | ναι | Για ποια έκδοση της γλώσσας ισχύει αυτή η συμπεριφορά (ενότητα 6). |

### Τι σημαίνουν τα `effects:`

| Τιμή | Παράδειγμα | Μέσα σε `if` | Στο `--dry-run` | Μέσα σε `as one step` |
|---|---|---|---|---|
| `none` | `the name of P` | ναι | τρέχει | ναι |
| `reads` | `the size of F`, `the changes in REPO` | ναι | τρέχει | ναι |
| `files` | `delete P`, `commit all in REPO` | όχι | τυπώνεται | μόνο αν έχει `undo:` |
| `system` | `run upgradepkg` μέσα από ρήμα | όχι | τυπώνεται | μόνο αν έχει `undo:` |
| `network` | `download`, `push REPO` | όχι | τυπώνεται | όχι |

Αυτό εγγυάται ότι μια συνθήκη `if` δεν αλλάζει ποτέ τίποτα στο σύστημα.

## 4. Το πρωτόκολλο πυρήνα και ρήματος

### Είσοδος

- **Απλές τιμές** (κείμενο, αριθμός, path): ως μεταβλητή περιβάλλοντος με το όνομα της θέσης (`DIR`, `P`, `N`). Ποτέ ως κείμενο μέσα στον κώδικα. Η τιμή δεν γίνεται ποτέ κώδικας (κανόνας 3).
  - sh: `"$DIR"`, awk: `ENVIRON["DIR"]`, perl: `$ENV{DIR}`
- **Λίστες**: ο πυρήνας γράφει τη λίστα σε προσωρινό αρχείο, στοιχεία χωρισμένα με NUL, και η μεταβλητή περιέχει τη διαδρομή του αρχείου. Έτσι ένα ρήμα μπορεί να έχει πολλές λίστες, και τα paths με newline δεν σπάνε.
- **Εγγραφές**: όπως οι λίστες, σε αρχείο με τη μορφή εγγραφής (παρακάτω).

### Περιβάλλον του ρήματος

Πάντα καθαρό. Περιέχει μόνο:
- τις θέσεις του `phrase:`
- `PATH=/usr/bin:/bin:/usr/sbin:/sbin`
- `LC_ALL=C.UTF-8`
- `TMPDIR` = ιδιωτικός φάκελος της κλήσης, σβήνεται μετά
- `LITA_STASH` = ιδιωτικός φάκελος, μόνο μέσα σε `as one step` (ενότητα 5)
- ό,τι γράφει το `inherit:`

Τρέχων φάκελος: ο τρέχων φάκελος του script (`go to` / `in`).

### Έξοδος, ανάλογα με το `returns:`

| `returns:` | Πώς επιστρέφεται |
|---|---|
| `nothing` | Το stdout περνάει στον χρήστη όπως είναι. |
| `text`, `number`, `path` | Stdout, χωρίς το τελικό newline. Ο αριθμός ελέγχεται. |
| `yes/no` | Κωδικός εξόδου: 0 = ναι, 1 = όχι, 2 και πάνω = σφάλμα. |
| `list of ...` | Stdout, στοιχεία χωρισμένα με NUL. |
| `record` | Stdout, ένα πεδίο ανά γραμμή: `ΟΝΟΜΑ<TAB>τιμή`. |
| `list of records` | Όπως το `record`, με μία κενή γραμμή ανάμεσα στις εγγραφές. |

Στα πεδία εγγραφής, οι ειδικοί χαρακτήρες γράφονται έτσι: `\` ως `\\`, tab ως `\t`, newline ως `\n`.

### Σφάλμα

- Κωδικός εξόδου διάφορος του 0 (για `yes/no`: 2 και πάνω).
- Η τελευταία μη κενή γραμμή του stderr γίνεται το `{the reason}`. Ολόκληρο το stderr φαίνεται με `lita --debug`.

## 5. `as one step`

- Μέσα σε `as one step` επιτρέπονται μόνο ρήματα με `effects: none` ή `reads`, ή με `undo:` που δεν είναι `none`. Αυτό ελέγχεται πριν την εκτέλεση.
- Για κάθε κλήση, ο πυρήνας δίνει ένα νέο `LITA_STASH`. Το ρήμα μπορεί να κρατήσει εκεί ό,τι χρειάζεται για την αναίρεση (π.χ. το `delete` μετακινεί το αρχείο εκεί αντί να το σβήσει).
- Αν αποτύχει κάτι, ο πυρήνας καλεί τα `undo:` των ρημάτων που πρόλαβαν να τρέξουν, με αντίστροφη σειρά, με τις ίδιες τιμές και το ίδιο `LITA_STASH`.
- Αν πετύχουν όλα, τα `LITA_STASH` σβήνονται.

Παράδειγμα:

```
phrase:   delete P
phrase:   remove P
P:        path
returns:  nothing
effects:  files
needs:    rm, mv
lang:     sh
code:
    if [ -n "$LITA_STASH" ]; then
        mv -- "$P" "$LITA_STASH/${P##*/}"
    else
        rm -rf -- "$P"
    fi
undo:
    mv -- "$LITA_STASH/${P##*/}" "$P"
help:     Delete a file or folder.
lita:     1
```

## 6. Εκδόσεις

- Η πρώτη γραμμή του script (`# lita 1`) διαλέγει την έκδοση της γλώσσας: και τη σύνταξη του πυρήνα και τη συμπεριφορά των ρημάτων.
- Αν αλλάξει η συμπεριφορά ενός ρήματος, δεν αλλάζει το παλιό αρχείο. Γράφεται νέο αρχείο με `lita: 2`. Τα scripts με `# lita 1` συνεχίζουν να παίρνουν την παλιά συμπεριφορά.
- Διορθώσεις bug που δεν αλλάζουν τη συμπεριφορά γίνονται στο ίδιο αρχείο.

## 7. Πού ζουν τα ρήματα και ασφάλεια

- `/usr/lib/lita/verbs/`: τα ρήματα του πακέτου lita
- `/usr/local/lib/lita/verbs/`: ρήματα του διαχειριστή του συστήματος
- Ο πυρήνας αρνείται να ξεκινήσει αν κάποιος από τους φακέλους ή κάποιο αρχείο `.verb` δεν ανήκει στον root ή μπορεί να το γράψει group ή others.
- Δεν υπάρχει μεταβλητή περιβάλλοντος ή φάκελος στο `$HOME` για ρήματα. Αλλιώς θα ξανάνοιγε η κρυφή είσοδος από το περιβάλλον.
- Αν δύο ρήματα ταιριάζουν στην ίδια πρόταση, ο πυρήνας δεν ξεκινάει και λέει ποια αρχεία συγκρούονται. Κανένα ρήμα δεν κρύβει άλλο.
- Ο πυρήνας κρατάει τη γραμματική που προκύπτει από όλα τα ρήματα σε cache (`/var/cache/lita/`) και την ξαναφτιάχνει όταν αλλάξει κάποιο αρχείο. Αν δεν μπορεί να γράψει εκεί, τη φτιάχνει στη μνήμη.
- Δίπλα σε κάθε `name.verb` υπάρχει `name.test`: ένα script lita που δοκιμάζει το ρήμα. Το `lita --test-verbs` τα τρέχει όλα.

## 8. Git

Ο χρήστης έχει πάντα όλο το git με `uses git` και `run git ...`. Τα ρήματα git καλύπτουν τα συνηθισμένα και επιστρέφουν τιμές αντί για κείμενο.

### Κανόνες για τα ρήματα git

- Μόνο το εκτελέσιμο `git`. Όχι libgit2.
- Μόνο μορφές εξόδου που το git εγγυάται ότι μένουν σταθερές: `status --porcelain=v2 -z`, `log --format=... -z`, `for-each-ref --format=...`, `rev-parse`. Ποτέ η έξοδος που προορίζεται για άνθρωπο.
- Πάντα `git -C "$REPO"`, ποτέ αλλαγή φακέλου.
- `inherit: HOME, SSH_AUTH_SOCK` (για `~/.gitconfig` και ssh κλειδιά). Τα `GIT_DIR`, `GIT_WORK_TREE` και τα υπόλοιπα `GIT_*` του καλούντα δεν περνάνε ποτέ.
- Αν δεν υπάρχει τερματικό, ο πυρήνας βάζει `GIT_TERMINAL_PROMPT=0`, ώστε το git να αποτυγχάνει αμέσως αντί να περιμένει κωδικό.
- `commit`, `tag`, `branch`: `effects: files` με `undo:`. `push`, `pull`, `fetch`, `clone`: `effects: network`, άρα όχι μέσα σε `as one step`.
- Οι διαδραστικές εντολές (`rebase -i`, `add -p`, επίλυση conflicts) δεν έχουν ρήμα. Τρέχουν με `run git ...`.

### Λίστα ρημάτων git (πρώτη έκδοση)

| Πρόταση | Επιστρέφει | effects |
|---|---|---|
| `the git repository in DIR` | path | reads |
| `REPO has uncommitted changes` | yes/no | reads |
| `the changes in REPO` | list of records (`status`, `path`) | reads |
| `the current branch of REPO` | text | reads |
| `the last N commits of REPO` | list of records (`hash`, `author`, `date`, `subject`) | reads |
| `the tags of REPO` | list of text | reads |
| `clone URL to DIR[, branch B][, shallow]` | nothing | network |
| `pull REPO` | nothing | network |
| `push REPO[ with tags]` | nothing | network |
| `commit all in REPO with message M` | nothing | files |
| `tag REPO as T` | nothing | files |
| `switch REPO to branch B` | nothing | files |

### Παράδειγμα: `the changes in REPO`

```
phrase:   the changes in REPO
REPO:     folder
returns:  list of records
effects:  reads
needs:    git, perl
inherit:  HOME
lang:     perl
code:
    use strict; use warnings;
    sub esc { my $s = shift; $s =~ s/\\/\\\\/g; $s =~ s/\t/\\t/g; $s =~ s/\n/\\n/g; $s }
    open my $g, '-|', 'git', '-C', $ENV{REPO}, 'status', '--porcelain=v2', '-z'
        or do { print STDERR "git: $!\n"; exit 1 };
    my @e = do { local $/ = "\0"; my @x = <$g>; chomp @x; @x };
    close $g or do { print STDERR "git status failed in $ENV{REPO}\n"; exit 1 };
    for (my $i = 0; $i < @e; $i++) {
        my $l = $e[$i];
        my ($st, $path);
        if    ($l =~ /^1 (\S\S) (?:\S+ ){6}(.*)$/s) { ($st, $path) = ($1, $2) }
        elsif ($l =~ /^2 (\S\S) (?:\S+ ){7}(.*)$/s) { ($st, $path) = ($1, $2); $i++ }
        elsif ($l =~ /^u (\S\S) (?:\S+ ){8}(.*)$/s) { ($st, $path) = ($1, $2) }
        elsif ($l =~ /^\? (.*)$/s)                  { ($st, $path) = ('??', $1) }
        else  { next }
        print "status\t", esc($st), "\npath\t", esc($path), "\n\n";
    }
undo:     none
help:     Changed, staged and untracked files, one record each.
lita:     1
```

### Παράδειγμα: `REPO has uncommitted changes`

```
phrase:   REPO has uncommitted changes
REPO:     folder
returns:  yes/no
effects:  reads
needs:    git
inherit:  HOME
lang:     sh
code:
    out=$(git -C "$REPO" status --porcelain=v1 -uno) || exit 2
    [ -n "$out" ]
undo:     none
help:     Yes if tracked files have changes that are not committed.
lita:     1
```

## 9. Άλλα παραδείγματα ρημάτων

### `the name of P` (awk, επιστρέφει text)

```
phrase:   the name of P
P:        text
returns:  text
effects:  none
needs:    awk
lang:     awk
code:
    BEGIN {
        n = split(ENVIRON["P"], a, "-")
        if (n < 4) { print "not a package name: " ENVIRON["P"] > "/dev/stderr"; exit 2 }
        s = a[1]
        for (i = 2; i <= n - 3; i++) s = s "-" a[i]
        print s
    }
undo:     none
help:     Name part of a Slackware package name (name-version-arch-build).
lita:     1
```

### `P is newer than N days` (perl, επιστρέφει yes/no)

```
phrase:   P is newer than N days
P:        path
N:        number
returns:  yes/no
effects:  reads
needs:    perl
lang:     perl
code:
    my $age = -M $ENV{P};
    defined $age or do { print STDERR "$ENV{P}: $!\n"; exit 2 };
    exit($age < $ENV{N} ? 0 : 1);
undo:     none
help:     Yes if P was modified less than N days ago.
lita:     1
```

### `the records in F, split by empty lines` (perl, επιστρέφει list of records)

```
phrase:   the records in F, split by empty lines
F:        file
returns:  list of records
effects:  reads
needs:    perl
lang:     perl
code:
    use strict; use warnings;
    sub esc { my $s = shift; $s =~ s/\\/\\\\/g; $s =~ s/\t/\\t/g; $s =~ s/\n/\\n/g; $s }
    local $/ = "";
    open my $fh, '<', $ENV{F} or do { print STDERR "$ENV{F}: $!\n"; exit 1 };
    while (my $para = <$fh>) {
        my ($desc, @out);
        for my $line (split /\n/, $para) {
            if (defined $desc) { $desc .= ($desc eq '' ? '' : "\n") . $line; next }
            next unless $line =~ /^([^:]+):\s*(.*)$/;
            if ($1 eq 'PACKAGE DESCRIPTION') { $desc = ''; next }
            push @out, esc($1) . "\t" . esc($2);
        }
        push @out, "PACKAGE DESCRIPTION\t" . esc($desc) if defined $desc;
        print join("\n", @out), "\n\n" if @out;
    }
undo:     none
help:     Paragraph-style files like PACKAGES.TXT: KEY: value lines, records separated by empty lines.
lita:     1
```

Τα παραδείγματα κώδικα των ενοτήτων 5, 8 και 9 δοκιμάστηκαν μόνα τους (sh, awk, perl, git 2.43). Ο πυρήνας δεν υπάρχει ακόμα, οπότε δεν έχουν δοκιμαστεί μέσα από τη lita.
