# lita: η υποθετική σύνταξη

Η `lita` είναι υποθετική γλώσσα για ό,τι κάνει σήμερα το shell: σύνδεση προγραμμάτων, αρχεία, διεργασίες, συστήματα. Σε αυτό το πεδίο στοχεύει να είναι καλύτερη από την Python 3 και την Perl. Για γενικό προγραμματισμό δεν στοχεύει (δες το τέλος).

Κάθε γραμμή διαβάζεται σαν οδηγία σε άνθρωπο. Από κάτω υπάρχει αυστηρή γραμματική και interpreter που κάνει πάντα το ίδιο.

Απλή στα φανερά, δυνατή στα κρυφά: η lita δεν ξαναγράφει εργαλεία που υπάρχουν. Ένας μικρός πυρήνας (Rust, χωρίς crates) κρατάει τη ροή, τους τύπους και τα σφάλματα. Κάθε άλλη πρόταση είναι ένα «ρήμα»: ένα αρχείο με λίγες γραμμές sh, awk ή Perl που καλεί τα εργαλεία του συστήματος. Νέα πρόταση προστίθεται με ένα αρχείο, χωρίς rebuild. Τα εσωτερικά είναι στο `lita-internals.md`.

## Οι 6 κανόνες

1. Κάθε γραμμή είναι μία πρόταση. Ό,τι ανήκει μέσα σε κάτι γράφεται με εσοχή.
2. Μια λέξη που είναι όνομα κάποιου πράγματος σημαίνει την τιμή του. Κάθε άλλη λέξη μένει όπως είναι. Αν μια λέξη θα μπορούσε να είναι και τα δύο, η γλώσσα ζητάει εισαγωγικά πριν τρέξει.
3. Μια τιμή είναι πάντα ένα πράγμα. Ένα όνομα αρχείου με κενά είναι ένα αρχείο. Δεν υπάρχει word splitting, αυτόματο globbing ή `eval`.
4. Αν κάτι αποτύχει, το script σταματάει και λέει με απλά λόγια τι απέτυχε και σε ποια γραμμή, εκτός αν έχεις γράψει τι να γίνει σε αυτή την περίπτωση.
5. Το script ξέρει μόνο ό,τι του είπες. Από έξω παίρνει μόνο όσα δηλώνει με `ask` και χρησιμοποιεί μόνο τα εργαλεία που δηλώνει με `uses`.
6. Ό,τι βγαίνει από το script είναι απλό κείμενο, μία εγγραφή ανά γραμμή, ώστε να περνάει από pipe (`grep`, `cut`, `awk`, `sort`, `tail -f`). Οι τύποι (λίστα, εγγραφή, path) υπάρχουν μόνο όσο τρέχει το script. JSON ή άλλη μορφή μόνο αν ζητηθεί ρητά.

Επιπλέον: όλο το αρχείο διαβάζεται και ελέγχεται πριν τρέξει η πρώτη γραμμή (τύποι, ονόματα, εργαλεία, λεξιλόγιο). Αν μια πρόταση δεν υπάρχει, το μήνυμα προτείνει την πιο κοντινή («Did you mean ...?»).

## Πυρήνας και λεξιλόγιο

Ο χρήστης δεν βλέπει διαφορά ανάμεσα στα δύο. Η διάκριση υπάρχει για να ξέρουμε τι αλλάζει με rebuild και τι με ένα αρχείο.

**Πυρήνας** (σταθερός, αλλάζει μόνο με νέα έκδοση της γλώσσας):

- τιμές: κείμενο, αριθμός, ναι/όχι, path, λίστα, εγγραφή, `an empty list`, `{x}`, `/` για paths, `#` για σχόλια
- `x is ...`, `add x to ...`
- `if` / `otherwise if` / `otherwise`, `is`, `is not`, `is empty`, `is in`, `more than`, `less than`, `at least`, `and`, `or`, `not`
- `for each ... in ...`, `... where ...`, `while ...`, `repeat ... times`, `skip it`, `stop looping`, `, N at a time`
- `to ...`, `the result is`, `add ... to the result`, `when asked to ...`, `when asked anything else`
- `stop`, `stop with error`, `finish ok`, `finish with error`, `at the end`
- `run` (`.out`, `.err`, `.code`), `|`, `with X set to ...`, `in DIR, ...`, `go to DIR`, `, give up after ...`
- `if that failed`, `{the reason}`, `ignore failures`, `as one step`
- `# lita 1`, `uses`, `ask`, `only one at a time`, `use other.lita`
- `say`, `complain`

**Λεξιλόγιο** (ρήματα, ένα αρχείο το καθένα): όλα τα υπόλοιπα, π.χ. `fix permissions`, `strip`, `download`, `check md5`, `the name of`, `newer than`, `the size of`, `the records in`, `write ... to`, `copy`, `move`, `delete`, `make folders`, `a new private folder`, `confirm`, `must run as root`, τα ρήματα git και όσα είναι ειδικά για Slackware.

Ένα ρήμα μπορεί να έχει συνώνυμα: το `delete P` και το `remove P` είναι το ίδιο ρήμα. Δύο διαφορετικά ρήματα δεν επιτρέπεται να ταιριάζουν στην ίδια πρόταση.

## Πίνακες σύνταξης

Στήλες: η σύνταξη της `lita`, το αντίστοιχο σε Python 3 και σε Perl, και τι κερδίζεις.

### 1. Δηλώσεις στην αρχή του script

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `uses tar, make` | δεν υπάρχει, το λάθος φαίνεται όταν φτάσει στην εντολή | δεν υπάρχει | Τα εργαλεία βρίσκονται μία φορά, πριν ξεκινήσει οτιδήποτε. Αν λείπει κάποιο, δεν τρέχει τίποτα. |
| `uses /sbin/makepkg as makepkg` | πλήρης διαδρομή σε κάθε κλήση | πλήρης διαδρομή σε κάθε κλήση | Η διαδρομή γράφεται σε ένα σημείο. |
| `must run as root` | `if os.geteuid() != 0: sys.exit(...)` | `die ... if $> != 0` | Μία πρόταση, ελέγχεται πριν τρέξει οτιδήποτε. |
| `only one at a time` | `fcntl.flock(...)` σε αρχείο που ανοίγεις μόνος σου | `flock(...)` | Κλείδωμα χωρίς αρχεία και χειροκίνητο ξεκλείδωμα. |
| `ask days as number, else 7` | `int(os.environ.get("DAYS") or 7)` | `$ENV{DAYS} // 7` (χωρίς έλεγχο τύπου) | Έλεγχος τύπου πριν την εκτέλεση. Η άδεια τιμή μετράει ως απούσα. |
| `ask logdir as folder, else /var/log/x` | `os.environ.get(...)` + `os.path.isdir` | `$ENV{...}` + `-d` | Ο φάκελος πρέπει να υπάρχει και δεν μπορεί να είναι άδειος. |
| `ask arch as text, else the machine type` | `os.environ.get("ARCH") or platform.machine()` | `$ENV{ARCH} // qx(uname -m)` | Χωρίς εξωτερική εντολή. |

Ό,τι δεν δηλώνεται με `ask` δεν υπάρχει για το script: `BASH_ENV`, `LD_PRELOAD`, exported συναρτήσεις, ξένο `PATH`. Στην Python και στην Perl ολόκληρο το `os.environ`/`%ENV` περνάει σε κάθε διεργασία που ξεκινάς.

### 2. Τιμές και τύποι

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `name is foo` | `name = "foo"` | `my $name = 'foo';` | Χωρίς εισαγωγικά για απλές λέξεις. |
| `total is 0` | `total = 0` | `my $total = 0;` | Ο τύπος (αριθμός) προκύπτει από την πρώτη τιμή και ελέγχεται πριν τρέξει. |
| `total is "abc"` (μετά το παραπάνω) | επιτρέπεται, σκάει αργότερα | επιτρέπεται, γίνεται 0 χωρίς λάθος | Απορρίπτεται πριν την εκτέλεση. |
| `failed is an empty list` | `failed = []` | `my @failed;` | |
| `report is` + πεδία με εσοχή | `report = {"a": 1, ...}` | `my %report = (...)` | Φωλιασμένες δομές χωρίς σύμβολα. |
| `pkg is tmp / "package-{name}"` | `os.path.join(tmp, f"package-{name}")` | `"$tmp/package-$name"` | Το path είναι τύπος. Δεν μπορεί να είναι άδειο, άρα το `/package-foo` δεν προκύπτει ποτέ. |
| `"{version}"` | `f"{version}"` | `"$version"` | Η παρεμβολή δίνει πάντα ένα όρισμα. |
| `version` ως τύπος: `if a is older than b` | `packaging.version` (εξωτερική βιβλιοθήκη) | `version->parse` (μόνο για ορισμένες μορφές) | `1.10 > 1.9`, `2.0rc1 < 2.0` χωρίς βιβλιοθήκη. |

### 3. Συνθήκες

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `if files is empty` | `if not files:` | `if (!@files)` | |
| `if arch is i586` | `if arch == "i586":` | `if ($arch eq 'i586')` | Δεν υπάρχει `==`/`eq`/`=`, δεν μπερδεύονται. |
| `if arch looks like i?86` | `fnmatch.fnmatch(arch, "i?86")` | `$arch =~ /^i.86$/` | Glob χωρίς import ή regex. |
| `if arch starts with arm` | `arch.startswith("arm")` | `$arch =~ /^arm/` | |
| `if file is newer than days days` | `time.time() - os.path.getmtime(f) < days*86400` | `-M $f < $days` | Χωρίς αριθμητική χρόνου. |
| `if x is in list` | `if x in lst:` | `grep { $_ eq $x } @list` | |
| `if n is more than 3` / `less than` / `at least` | `>` / `<` / `>=` | `>` / `<` / `>=` | Λέξεις αντί για σύμβολα. Η σύγκριση κειμένου και αριθμού δεν μπερδεύεται, γιατί ο τύπος είναι γνωστός. |
| `if a and not b` / `or` | `and`, `not`, `or` | `&&`, `!`, `\|\|` | |
| `otherwise if ...` / `otherwise` | `elif` / `else` | `elsif` / `else` | |
| `if ..., arch is i586` (σε μία γραμμή) | `if ...: arch = "i586"` | `$arch = 'i586' if ...;` | |

### 4. Επανάληψη και λίστες

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `for each file in files` | `for f in files:` | `for my $f (@files)` | Το loop τρέχει στο ίδιο scope, χωρίς κρυφά subshells. |
| `skip it` | `continue` | `next` | |
| `stop looping` | `break` | `last` | |
| `while x is less than 10` | `while x < 10:` | `while ($x < 10)` | |
| `repeat 3 times` | `for _ in range(3):` | `for (1..3)` | |
| `for each file in files, 4 at a time` | `ThreadPoolExecutor(max_workers=4)` + συλλογή αποτελεσμάτων | `Parallel::ForkManager` (CPAN) | Παράλληλα με όριο. Καμία αποτυχία δεν χάνεται. |
| `for each entry in catalogue where its "NAME" contains word` | list comprehension με `if` | `grep { ... } @catalogue` | Φίλτρο μέσα στο loop. |
| `the first entry in catalogue where ...` | `next((e for e in c if ...), None)` | `first { ... } @c` (List::Util) | |
| `add file to failed` | `failed.append(f)` | `push @failed, $f;` | |
| `count of todo` | `len(todo)` | `scalar @todo` | |
| `the words of info.REQUIRES` | `info["REQUIRES"].split()` | `split ' ', $info{REQUIRES}` | |
| `..., without duplicates` | `list(dict.fromkeys(x))` | `uniq` (List::Util) | |

### 5. Συναρτήσεις

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `to find the build order of pkg` | `def build_order(pkg):` | `sub build_order { my ($pkg) = @_; ... }` | Ο ορισμός διαβάζεται ως πρόταση. |
| `the result is ...` | `return ...` | `return ...;` | |
| `add ... to the result` | `result.append(...)` + `return result` | `push @r, ...;` + `return @r` | Το αποτέλεσμα υπάρχει αυτόματα. |
| `entry, fetched` / `the build order of dep` | `fetch(entry)` / `build_order(dep)` | `fetch($entry)` | |
| κύκλος σε αναδρομή | `RecursionError` ή ατέρμονο loop | ατέρμονο loop | Η γλώσσα βρίσκει τον κύκλο και λέει ποια ονόματα τον σχηματίζουν. |

Οι μεταβλητές ζουν μόνο στο block τους. Δεν υπάρχει dynamic scoping όπως στο `local` της Perl.

### 6. Εξωτερικές εντολές

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `run gzip -k file` | `subprocess.run(["gzip","-k",f], check=True)` | `system('gzip','-k',$f) == 0 or die` | Σταματάει σε αποτυχία από μόνο του. Το `file` είναι ένα όρισμα. |
| όνομα αρχείου που ξεκινάει με `-` | περνάει ως επιλογή | περνάει ως επιλογή | Περνάει ως `./-...`, δεν διαβάζεται ποτέ ως επιλογή. |
| `size is the size of file` | `os.path.getsize(f)` | `-s $f` | |
| `r is run du -sb p` → `r.out`, `r.err`, `r.code` | `capture_output=True, text=True` | `` `du -sb $p` `` (μόνο stdout, με shell) | Stdout, stderr και κωδικός σε μία τιμή, χωρίς shell. |
| `run find dir -name "*.gz" \| run wc -l` | δύο `Popen` συνδεδεμένα με το χέρι | `open(my $fh, '-|', ...)` ή shell string | Αποτυγχάνει αν αποτύχει οποιοδήποτε κομμάτι (pipefail πάντα). |
| `run ./configure with CFLAGS set to flags` | `env={**os.environ, "CFLAGS": flags}` | `local $ENV{CFLAGS} = $flags;` | Η μεταβλητή ισχύει μόνο για εκείνη την εντολή. |
| συνέχεια σε επόμενες γραμμές με εσοχή | λίστα σε πολλές γραμμές | λίστα σε πολλές γραμμές | Χωρίς `\` στο τέλος κάθε γραμμής. |
| `--libdir={libdir}` | `f"--libdir={libdir}"` | `"--libdir=$libdir"` | Άγκιστρα μόνο όταν η λέξη κολλάει σε κείμενο. |
| `run make, give up after 1 hour` | `subprocess.run(..., timeout=3600)` | `alarm` + `eval` | |
| `in tmp, run tar xvf ...` | `subprocess.run(..., cwd=tmp)` | `chdir` και επιστροφή με το χέρι | Ο κατάλογος αλλάζει μόνο για την εντολή. |
| `go to src` | `os.chdir(src)` | `chdir $src or die` | Μόνιμη αλλαγή, ρητά διαφορετική από το `in`. |

### 7. Σφάλματα

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| (τίποτα: κάθε αποτυχία σταματάει) | ισχύει μόνο με `check=True` | ισχύει μόνο με `or die` σε κάθε κλήση | Δεν υπάρχει τρόπος να ξεχαστεί. |
| `if that failed` + εσοχή | `try:` / `except CalledProcessError:` | `if (system(...) != 0) { ... }` | Ο χειρισμός γράφεται αμέσως κάτω από την εντολή. |
| `{the reason}` | `str(e)`, `e.stderr` | `$!`, `$?`, `$@` (τρία διαφορετικά) | Ένα όνομα για την αιτία, με το stderr του εργαλείου μέσα. |
| `ignore failures` (στο τέλος μιας εντολής) | `check=False` | χωρίς `or die` | Η εξαίρεση είναι ρητή και φαίνεται. |
| `stop` / `stop with error` | `sys.exit(0)` / `sys.exit(1)` | `exit 0` / `die` | |
| `finish ok` / `finish with error` | ίδιο με το παραπάνω | ίδιο | Ο κωδικός εξόδου είναι πάντα δηλωμένος. |
| `as one step` + εσοχή | χειροκίνητο rollback | χειροκίνητο rollback | Όσα ξέρει να αναιρέσει η γλώσσα (αρχεία, φάκελοι) αναιρούνται αν αποτύχει κάτι. Ό,τι δεν μπορεί να αναιρεθεί απορρίπτεται μέσα σε `as one step` πριν την εκτέλεση. |
| `confirm "Upgrade?"` | `input()` + έλεγχος | `<STDIN>` + έλεγχος | Χωρίς τερματικό ή με «όχι», σταματάει. Δεν συνεχίζει ποτέ σιωπηλά. |

### 8. Αρχεία και φάκελοι

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `here is the current folder` | `os.getcwd()` | `getcwd()` | |
| `files are all "*.log" in logdir` | `glob.glob(os.path.join(d, "*.log"))` | `glob("$d/*.log")` | Πάντα λίστα από paths. Χωρίς αποτέλεσμα, άδεια λίστα. |
| `scratch is a new private folder` | `tempfile.mkdtemp()` | `File::Temp::tempdir()` | Τυχαίο όνομα, δικαιώματα 700. |
| `at the end, delete scratch` | `atexit` ή `try/finally` | `END { }` ή `CLEANUP => 1` | Τρέχει και σε σφάλμα και σε Ctrl-C. |
| `delete pkg if it exists` | `shutil.rmtree(p, ignore_errors=True)` | `File::Path::remove_tree` | Ο τύπος path δεν επιτρέπει ποτέ `/` ή άδειο. |
| `make folders tmp, pkg, output` | `os.makedirs(..., exist_ok=True)` × 3 | `make_path(...)` | |
| `copy a to b` / `move ... replacing` | `shutil.copy` / `shutil.move` | `File::Copy` | |
| `write report to file` | `f.write(...)` με δική σου μορφοποίηση | `print $fh ...` με δική σου μορφοποίηση | Μία γραμμή `KEY=value` ανά εγγραφή, έτοιμη για `grep` και `cut`. Τιμές με κενά μπαίνουν σε `"..."`. |
| `write report to file as json` | `json.dump(...)` | `JSON::PP` | Μόνο όταν ο αποδέκτης είναι πρόγραμμα που απαιτεί JSON. |
| `write rows to file as columns` | `csv.writer(f, delimiter="\t")` | `join("\t", ...)` | Τιμές χωρισμένες με tab, για `cut -f` και `awk -F'\t'`. |
| `fix permissions in src` | `os.walk` + `chmod` | `File::Find` + `chmod` | Εκτελέσιμα 755, τα υπόλοιπα 644, σε μία πρόταση. |
| `strip programs and libraries in pkg` | `os.walk` + αναγνώριση ELF + `strip` | `File::Find` + `file` + `strip` | Η γλώσσα αναγνωρίζει τα ELF μόνη της. |

### 9. Δεδομένα από αρχεία

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `settings is the record in /etc/x.conf` | `configparser` ή χειροκίνητο parsing | χειροκίνητο parsing | Γραμμές `KEY=value` ή `KEY="value"` γίνονται εγγραφή. |
| `lines are the records in f` | χειροκίνητο parsing ανά γραμμή | `split` ανά γραμμή | Κάθε γραμμή `KEY=value KEY=value` γίνεται μία εγγραφή. Η ίδια μορφή που γράφει το `write`. |
| `rows are the columns in f` | `csv.reader(f, delimiter="\t")` | `split /\t/` | Γραμμές με τιμές χωρισμένες με tab. |
| `entries are the records in f, split by empty lines` | χειροκίνητο parsing | `$/ = ""` (paragraph mode) | Μορφές όπως το PACKAGES.TXT σε μία πρόταση. |
| `entry."PACKAGE NAME"` | `entry["PACKAGE NAME"]` | `$entry{'PACKAGE NAME'}` | Πεδία με κενά στο όνομα. |
| `the name of p` (πακέτο Slackware) | `rsplit("-", 3)[0]` | `s/-[^-]+-[^-]+-[^-]+$//` | Ρήμα του λεξιλογίου (λίγες γραμμές awk), όχι μέρος του πυρήνα. |
| `... without extension` | `os.path.splitext(x)[0]` | `s/\.[^.]+$//` | |

### 10. Δίκτυο

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `download url to here, try 3 times` | `urllib` + loop + χειρισμός σφαλμάτων | `LWP` ή `HTTP::Tiny` + loop | |
| `..., check md5 is info.MD5SUM` | `hashlib.md5` + σύγκριση | `Digest::MD5` + σύγκριση | Αν δεν ταιριάζει, το αρχείο σβήνεται και το script σταματάει. |
| `check md5 of f against CHECKSUMS.md5` | χειροκίνητο parsing του αρχείου | χειροκίνητο parsing | |

### 11. Έξοδος και υποεντολές

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `say "text {x}"` | `print(f"text {x}")` | `print "text $x\n";` | Χωρίς `\n`. Πάντα μία γραμμή: newlines μέσα στην τιμή γράφονται ως `\n`, ώστε μια εγγραφή να μη σπάει σε δύο. |
| `complain "text {x}"` (στο stderr) | `print(..., file=sys.stderr)` | `print STDERR ...` | Ξεχωριστή λέξη, όχι ανακατεύθυνση. Μία γραμμή, με μπροστά το όνομα του script: `lpm: text`. |
| μήνυμα σφάλματος της γλώσσας | traceback σε πολλές γραμμές | `... at script line N.` | Μία γραμμή στο stderr: `lpm: line 42: gzip failed on foo.log: No space left on device`. Το `grep 'line 42'` ή `grep failed` δουλεύουν. |
| `when asked to install names` | `argparse` με subparsers | `Getopt::Long` + χειροκίνητη διανομή | Κάθε υποεντολή είναι ένα block. Το `names` παίρνει τα υπόλοιπα ορίσματα. |
| `when asked anything else` | `parser.print_help()` | χειροκίνητο | |

### 12. Git

Όλο το git είναι πάντα διαθέσιμο με `uses git` και `run git ...`. Τα παρακάτω είναι ρήματα για τα συνηθισμένα, που επιστρέφουν τιμές αντί για κείμενο.

| lita | Python 3 | Perl | Τι κερδίζεις |
|---|---|---|---|
| `repo is the git repository in here` | `subprocess` + `git rev-parse` | `system` + `git rev-parse` | Αν ο φάκελος δεν είναι repo, σταματάει. |
| `if repo has uncommitted changes` | `git status --porcelain` + έλεγχος εξόδου | το ίδιο | Ναι/όχι, χωρίς parsing. |
| `for each change in the changes in repo` → `change.status`, `change.path` | parsing του `--porcelain=v2 -z` με το χέρι | το ίδιο | Εγγραφές. Ονόματα με κενά ή newlines δεν σπάνε. |
| `branch is the current branch of repo` | `git rev-parse --abbrev-ref HEAD` | το ίδιο | |
| `the last 10 commits of repo` → `.hash`, `.author`, `.date`, `.subject` | `git log --format=...` + parsing | το ίδιο | |
| `the tags of repo` | `git tag` + `splitlines()` | το ίδιο | |
| `clone url to src, branch main, shallow` | `git clone --branch main --depth 1 ...` | το ίδιο | Οι επιλογές ως λέξεις. |
| `pull repo` / `push repo with tags` | `subprocess.run([...], check=True)` | `system(...) == 0 or die` | |
| `commit all in repo with message "..."` | `git commit -a -m ...` | το ίδιο | |
| `tag repo as "v{version}"` / `switch repo to branch dev` | `git tag` / `git switch` | το ίδιο | |
| `in src, run git bisect start` | `subprocess.run(..., cwd=src)` | `chdir` + `system` | Ό,τι δεν έχει πρόταση, με `run`. |

Το git παίρνει από το περιβάλλον μόνο το `HOME` (για το `~/.gitconfig`) και το `SSH_AUTH_SOCK` (για τα ssh κλειδιά). Χωρίς τερματικό δεν ζητάει ποτέ κωδικό: αποτυγχάνει αμέσως. Τα `commit`, `tag` και `switch` επιτρέπονται μέσα σε `as one step`, τα `push`, `pull` και `clone` όχι. Οι διαδραστικές εντολές (`rebase -i`, `add -p`) δεν έχουν πρόταση και τρέχουν με `run git ...`.

### 13. Άλλες γλώσσες

| lita | Τι κάνει |
|---|---|
| `uses python3, perl` | Δηλώνει τις γλώσσες όπως κάθε άλλο εργαλείο. Αν λείπει κάποια, το script δεν ξεκινάει. |
| `order is run python3 here / "resolve.py"` | Τρέχει ένα πρόγραμμα σε άλλη γλώσσα. Η έξοδος είναι απλό κείμενο (κανόνας 6). |
| `run python3 here / "x.py" with PYTHONPATH set to here / "lib"` | Το `PYTHONPATH`, `PERL5LIB` κλπ. δεν περνάνε από μόνα τους (κανόνας 5). Δηλώνονται ρητά. |

Η ίδια η lita δεν χρησιμοποιεί Python σε κανένα ρήμα. Ο χρήστης μπορεί να τη χρησιμοποιεί στα scripts του, αλλά τότε είναι δική του εξάρτηση, φανερή στο `uses`. Οι αλγόριθμοι και ό,τι είναι πραγματικό πρόγραμμα γράφονται σε κανονική γλώσσα και καλούνται με `run`, όπως το `make`.

### 14. Η εντολή `lita`

| Εντολή | Τι κάνει |
|---|---|
| `lita script` | Ελέγχει όλο το script και μετά το τρέχει. |
| `lita --check script` | Μόνο ο έλεγχος, χωρίς εκτέλεση. |
| `lita --show script` | Για κάθε γραμμή, τι ακριβώς θα τρέξει, με ποια ορίσματα και ποιο περιβάλλον. |
| `lita --dry-run script` | Τρέχει μόνο ό,τι δεν αλλάζει τίποτα. Τα υπόλοιπα τα τυπώνει. |
| `lita --debug script` | Τρέχει κανονικά και δείχνει όλο το stderr των εργαλείων, όχι μόνο την αιτία. |
| `lita --test-verbs` | Τρέχει τα tests όλων των ρημάτων. |
| `lita help fix permissions` | Τι κάνει μια πρόταση, μία γραμμή. |

## Συνολικά, ως προς τα 8 κενά του bash

| Κενό | Python 3 | Perl | lita |
|---|---|---|---|
| 1. Όλα είναι strings | λυμένο | λυμένο (χαλαρά: `"abc" + 1` = 1) | λυμένο, με έλεγχο πριν την εκτέλεση |
| 2. Δεδομένα που γίνονται κώδικας | λυμένο αν αποφεύγεις `shell=True` | λυμένο αν αποφεύγεις shell string και `eval` | αδύνατο, δεν υπάρχει τρόπος |
| 3. Σφάλματα | εξαιρέσεις, αλλά το `subprocess` θέλει `check=True` | πρέπει να γράψεις `or die` παντού | σταματάει πάντα, ο χειρισμός είναι ρητός |
| 4. Scope | λυμένο | `my` λύνει, `local` ξανανοίγει το πρόβλημα | λυμένο |
| 5. Διεργασίες | φλύαρο αλλά σωστό | σύντομο αλλά εύκολα λάθος | σύντομο και σωστό |
| 6. Περιβάλλον | περνάει όλο | περνάει όλο | περνάει μόνο ό,τι δηλώνεις |
| 7. Έλεγχος πριν την εκτέλεση | μόνο σύνταξη (`mypy` εξωτερικά) | μόνο σύνταξη (`perl -c`) | σύνταξη, τύποι, ονόματα, εργαλεία |
| 8. Αναγνωσιμότητα | καλή για προγραμματιστές | κακή χωρίς εμπειρία | καλή για όποιον διαβάζει αγγλικά |

## Πού δεν είναι ανώτερη

- **Αλγόριθμοι και πραγματικά προγράμματα.** Ταξινόμηση με δικό σου κριτήριο, parsing άγνωστου format, υπολογισμοί, μεγάλος κώδικας με πολλά modules: εκεί η Python και η Perl είναι καλύτερες, και γι' αυτό η lita τις καλεί (ενότητα 13) αντί να τις αντικαταστήσει.
- **Βιβλιοθήκες.** Η Python έχει PyPI, η Perl CPAN. Η `lita` έχει τα εργαλεία του συστήματος και όσα ρήματα έχουν γραφτεί. Κάθε νέα πρόταση θέλει ένα νέο αρχείο ρήματος (χωρίς rebuild), αλλά κάποιος πρέπει να το γράψει και να το δοκιμάσει.
- **Λεξιλόγιο.** Μοιάζει με αγγλικά, αλλά δέχεται μόνο τις δικές της διατυπώσεις. Το `older than` δεν είναι το ίδιο με το `newer than` με αντίστροφη σύγκριση, αν δεν οριστεί. Τα μηνύματα σφάλματος πρέπει να λένε «εννοούσες ...;».
- **Απόδοση.** Για τη δουλειά της (σύνδεση εργαλείων) δεν μετράει. Για υπολογισμούς θα ήταν πιο αργή.
- **Το `as one step`** είναι υπόσχεση μόνο για όσα μπορούν να αναιρεθούν. Μια εντολή που έτρεξε (`upgradepkg`, επανεκκίνηση υπηρεσίας) δεν αναιρείται.

Η σωστή θέση της: εκεί που σήμερα γράφεται bash ή μικρά scripts Python/Perl για το σύστημα. Όταν χρειάζεται πραγματικό πρόγραμμα, το καλεί με `run`, όπως καλεί το `make`.

## Ανοιχτά

- **Κώδικας άλλης γλώσσας μέσα στο script.** Τώρα επιτρέπεται μόνο σε ξεχωριστό αρχείο (`run python3 here / "x.py"`). Αν θα επιτρέπονται και λίγες γραμμές μέσα στο ίδιο το script, δεν έχει αποφασιστεί.
