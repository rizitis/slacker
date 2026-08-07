# Freezing slacker's dependencies for the next Slackware stable

A guide to how you lock slacker's dependencies so that it builds reproducibly for
the whole life of the next Slackware stable release, and why the correct way to
find the minimum rustc goes through rustup.

## 1. What actually breaks, and what does not

Your own code never breaks on its own. Rust gives a backwards-compatibility
guarantee: a newer rustc builds your old code without changes. So the danger is
never the newer compiler.

The danger is one single thing: the open version ranges in Cargo.toml. Lines like
`regex = "1"` or `clap = "4"` mean "take whatever is newest within the range". At
some point in the future a new release of a crate will do one of two things:

- raise its own minimum rustc (MSRV) above the one you have, or
- change its API.

In either case, a build that pulls "the newest" will fail. The whole freezing
process exists to close that one hole.

The solution is three things working together:

1. Pin: lock the exact versions through Cargo.lock.
2. Vendor: copy the source code of the dependencies locally, so the build is
   offline and self-contained.
3. Declare: declare the minimum rustc through the rust-version field, as a safety
   net.

## 2. When you freeze

Not "when the new stable comes out" in the abstract, but the moment you finalize a
version of slacker against the rustc that the new stable ships. That pair (a
specific rustc together with specific crate versions) is a known-good snapshot.
From then on, a newer rustc builds just fine, and newer crates are not pulled in
because Cargo.lock blocks them.

For now, stable is too far behind to build slacker. When the new stable release
based on the current you are running today comes out, that is when you run the
steps below.

## 3. The "dangerous" files

Cargo.toml: this is the danger surface, that is, the open ranges. You do not need
to touch them. Cargo.lock tames them.

Cargo.lock: this is the antidote. slacker is an application (binary), not a
library, so you put this file in git (commit it). When it exists, every build uses
exactly these versions, never "the newest". Make sure .gitignore does not exclude
it.

## 4. The "dangerous" crates

You do not need to do anything per crate by hand, because Cargo.lock catches them
all at once. It is still good to know where to look if you ever unfreeze and
something breaks:

- The TLS stack: native-tls, and beneath it openssl-sys and openssl. They have
  build scripts and depend on the system openssl. This is the most common breakage
  point in distribution builds.
- The proc-macro foundations: syn, proc-macro2, quote. They come in transitively
  (through clap) and have historically raised the MSRV floor.
- clap (pulls in transitively clap_lex 1.1.0, which is written in Rust edition
  2024 and therefore raises the floor to 1.85), the regex stack (regex-automata,
  regex-syntax), and low-level crates such as libc, cc, once_cell.

## 5. The correct solution for the minimum rustc: rustup

To know which rustc version you must declare slacker builds on, the correct
solution is rustup. You already have it installed (rustup-1.29.0 from conraid), so
no fake setup is needed.

Why rustup and not guessing: the cargo-msrv tool runs a bisection over real rustc
versions, installing them and testing whether the build succeeds with each one.
This requires rustup, because the various old toolchains are downloaded and run
through rustup. That way you get the exact floor, together with which crate or
which line of code sets it, instead of assuming.

Run:

```
cargo install cargo-msrv
cd /mnt/data/GITHUB/slacker
cargo msrv find
```

The result is the minimum rustc that builds slacker. In the measurement that was
done it came out as 1.85.1. Note: the floor is not set by the clap version as
such, but by the fact that a dependency (clap_lex 1.1.0, pulled in through clap)
is written in Rust edition 2024, which was stabilized in Rust 1.85. Any rustc
older than 1.85 fails with the message "feature edition2024 is required". You put
that number, 1.85.1, into rust-version (see section 7, step 3).

### Caution: two toolchains on the same machine

rustup has its own toolchain, which it downloads itself, and it runs in parallel
with the system rust 1.96 from conraid. That is, two rustc versions can coexist on
the machine. For cargo-msrv this is not a problem; on the contrary, that is
exactly what you want, for it to test many versions.

But the release build you will package must be built with the rustc that the new
stable ships, not with some random rustup toolchain. Otherwise the MSRV you
measured does not correspond to the real target. Check which toolchain is active
before you package:

```
rustup show
rustup which cargo
which -a cargo rustc
```

The first shows which toolchains are installed and which one is the default. The
second shows which cargo will actually run through rustup. The third shows the
paths for system (conraid) and rustup separately.

## 6. MSRV as a safety net

Once you have the number from cargo-msrv, declare it in Cargo.toml:

```
[package]
rust-version = "1.85.1"
```

This activates cargo's MSRV-aware resolver (you have it in 1.96): a future cargo
update will refuse to pull a crate that exceeds this floor. Cargo.lock remains the
hard guarantee. rust-version is the extra protection so that the floor does not
silently break on a future update.

## 7. The freezing procedure step by step

Run it when the new stable comes out, on the build machine, with the stable's
rustc active.

Step 1. Target toolchain. Confirm that you are building with the rustc the new
stable ships:

```
rustc --version
```

This is your floor.

Step 2. Build and test cleanly, with your own real Cargo.toml (not any version
tweaked for an older rustc):

```
cargo build --release
cargo test
```

If you want the freshest compatible set of dependencies, run cargo update and
re-test. Whatever comes out green here is your snapshot.

Step 3. Declare the MSRV in the [package] section of Cargo.toml with the number
that cargo msrv find gave you:

```
rust-version = "1.85.1"
```

Step 4. Lock and confirm that the lock is authoritative:

```
cargo build --release --locked
git add -f Cargo.lock
```

--locked fails if the lock does not match, so it guarantees you are building
exactly what you locked.

Step 5. Vendor the dependency sources:

```
cargo vendor vendor/
```

The command prints a configuration block to stdout. Save it into
.cargo/config.toml:

```
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
```

Step 6. Confirm that it builds completely offline. If you want, cut the network
too for certainty:

```
cargo build --release --offline --locked
```

Shorthand: --frozen is equivalent to --locked and --offline together.

Step 7. Packaging for distribution. Two tarballs, tied to the release's git tag:

- slacker-X.Y.Z.tar.xz with the source code, the Cargo.lock and the
  .cargo/config.toml.
- slacker-X.Y.Z-vendor.tar.xz with the vendor directory. It is large; that is the
  price of a self-contained build.

Step 8. SlackBuild. It unpacks both side by side and builds with no network:

```
cargo build --release --offline --locked
```

This way the build is reproducible forever.

## 8. Tricks and pitfalls

Do not vendor openssl with its vendored feature. In a distribution package you
want it linked against the system openssl, so that Slackware's security updates
reach slacker automatically too. The openssl-sys source goes into vendor normally
(as a -sys crate), but at build time it links against the system library. That is
the correct behavior.

The direction of the danger is one-way. Only newer crates threaten you, never a
newer rustc. So lock together with vendor means slacker builds with the same or a
newer toolchain indefinitely.

Unfreezing is a deliberate action, not something accidental. When you want
security updates in the crates or you move to a next stable: cargo update, then
cargo vendor again, then re-test, and you bump slacker's version.

One snapshot per release. Tie the Cargo.lock and the vendor directory to each git
tag. Every released version of slacker has its own reproducible set of
dependencies.

The frozen Cargo.toml you ship to the distribution is your own, on the stable's
rustc. Not any Cargo.toml tweaked for an older rustc.

## 9. One-line summary

Find the minimum rustc with rustup and cargo-msrv, declare it in rust-version,
commit the Cargo.lock, and distribute vendored dependencies. Then slacker will
build reproducibly for as long as the target Slackware lives, regardless of what
crates.io does or how high the crates' MSRVs climb in the future.
