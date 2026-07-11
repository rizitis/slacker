# slacker `resolve-stock` — integration (ROUND 1: core dep injection)

This round adds ONE thing: when `RESOLVE_STOCK=yes` and you install a package
from the **official** repo, slacker also pulls that package's **stock** deps
(from a local `depgraph.db`) through its **existing** flow (plan → list → y/n,
`--yes` for auto). With `RESOLVE_STOCK` off (default) **nothing changes** — every
edit below is inert until the flag is on.

Not in this round (kept separate on purpose, so the change surface stays tiny):
download-on-`update`, `status` check, ChangeLog-head freshness. Those come next,
once you confirm the core works. For now you place `depgraph.db` by hand (below).

New file `stockdb.rs` is compile-verified as a library and functionally tested
against the real db. The 4 in-file edits are written against the current source
but could not be compiled against the full tree here — apply, build, and if the
compiler complains it will be at exactly these spots.

---

## 1. New file: `src/stockdb.rs`

Drop the provided `stockdb.rs` into `src/`. It is self-contained (only
`rusqlite` + std). Read-only db access; empty result on ANY error (missing db,
bad file, unknown package) so a stock-db problem can never crash/block an
install — it only adds nothing.

## 2. `Cargo.toml` — add one dependency

Under `[dependencies]`, add:

```toml
rusqlite = { version = "0.31", features = ["bundled"] }
```

`bundled` compiles SQLite INTO the binary: needs `cc`/gcc at BUILD time (present
on Slackware), and the RUNTIME binary stays glibc-only (no `libsqlite3` — verified).

## 3. `src/main.rs` — 2 edits

### 3a. Declare the module (near the other `mod` lines at the top, e.g. by `mod changelog;`)

```rust
mod stockdb;
```

### 3b. The hook — inside `add_with_deps`, in the `if resolve { if let Some(repo) = … {` block

FIND (one line, ~line 869):

```rust
            for dep in repo::fetch_dep(repo, &pkg) {
```

REPLACE with:

```rust
            let mut deps = repo::fetch_dep(repo, &pkg);
            // resolve-stock: in a container/minimal system, also pull the stock
            // deps this OFFICIAL package needs (linked + runtime, never build).
            // Off by default -> `deps` is exactly fetch_dep(), i.e. no change.
            if cfg.resolve_stock && repo.official {
                for e in stockdb::deps_for(&cfg.stock_db_path(), &name) {
                    if !deps.contains(&e) {
                        deps.push(e);
                    }
                }
            }
            for dep in deps {
```

Nothing else in the loop body changes — `dep` is still a `String`, exactly as
before. `name` is the package being processed (already bound at the top of
`add_with_deps`). Recursion is free: each pulled stock dep re-enters
`add_with_deps`, so its own stock deps are resolved too — the full closure, via
the existing machinery. In full-ISO the deps are already installed → satisfied,
no-op. In a container they are missing but offered by official → pulled.

## 4. `src/config.rs` — 3 edits + 1 helper

### 4a. Struct field — in `pub struct Config`, next to `resolve_deps`

```rust
    /// resolve-stock: also pull a stock package's stock deps from the local
    /// depgraph.db (RESOLVE_STOCK, default off). Containers/minimal systems.
    pub resolve_stock: bool,
```

### 4b. Parse — near the other `conf.get(...)` parses, before the `Config { … }` build

```rust
    // RESOLVE_STOCK defaults OFF. yes/on/true/1 turns it on. Explicit, user's
    // responsibility (it declares "I am on a container/minimal system").
    let resolve_stock = matches!(
        conf.get("RESOLVE_STOCK").map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("yes") | Some("on") | Some("true") | Some("1")
    );
```

### 4c. Build — in the `Config { … }` struct literal, next to `resolve_deps,`

```rust
        resolve_stock,
```

### 4d. Helper — add to the existing `impl Config` block (line ~215)

Under `state_dir` (/var/lib), NOT cache: the stock-db is data resolve-stock
depends on, and /var/cache is FHS-disposable (a cron/tmpfiles sweep would
silently break resolve-stock). This mirrors how slacker already keeps trust
state under state_dir for exactly this reason.

```rust
    /// Local path of the stock dependency database (persistent, under state_dir).
    pub fn stock_db_path(&self) -> std::path::PathBuf {
        self.state_dir.join("stock").join("depgraph.db")
    }
```

## 5. `slacker.conf` — add the switch (example files + your container image)

```
# resolve-stock: in a container/minimal system, also pull the stock deps a
# stock package needs (from the downloaded depgraph.db). Default no. Set to yes
# ONLY when you know you are in such an environment (it is your declaration).
RESOLVE_STOCK=no
```

---

## Test it (round 1)

1. Build slacker (your rust 1.85). It must build unchanged with `RESOLVE_STOCK`
   absent/`no`.
2. Put the db where slacker looks (state_dir defaults to `/var/lib/slacker`).
   The dir does not exist yet — create it by hand for this test (in round 2
   slacker will `create_dir_all` it itself on download):

   ```
   mkdir -p /var/lib/slacker/stock
   cp depgraph.db /var/lib/slacker/stock/depgraph.db
   ```

3. In a MINIMAL container, set `RESOLVE_STOCK=yes` in `/etc/slacker/slacker.conf`,
   then:

   ```
   slacker install NetworkManager      # should list stock deps incl. nftables/dnsmasq/ppp
   slacker --dry-run install ffmpeg     # inspect the plan without changing anything
   ```

4. Sanity: with `RESOLVE_STOCK=no` the plan must be IDENTICAL to today's slacker.

Confirm the plans look right, then we wire round 2 (download on `update`,
`status` check, ChangeLog-head freshness).
