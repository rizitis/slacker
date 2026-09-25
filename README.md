![slacker](slacker-banner.svg)


---

# Slacker

---


## slacker-cli - slackpkg + slackpkg+ in one

![slacker-cli](./slacker-cli/DOCS/files/slacker-cli.png)


A Slackware package manager in Rust from sratch, with full **slackpkg action parity**, plus
**slackpkg+-style multi-repo priority** resolution.

- slackpkg: official mirror, update/install/upgrade/remove/clean-system, file-
  search, templates, ChangeLog tracking, GPG, .new config handling.
- slackpkg+: many repos in one priority-ordered model; the official mirror is
  just a repo whose priority you choose, so it can sit in any position.

## Philosophy

![slacker-gui](./slacker-gui/dev-docs/icon-sizes.png)

- Thin layer over the native pkgtools - never reimplements
  installpkg/upgradepkg/removepkg, just calls them.
- **Dependency resolution is the repository's responsibility, not the package
  manager's.** Official Slackware ships none, so official packages get no
  auto-resolved deps - the Slackware tradition, kept on purpose. Where a
  *third-party* repo chooses to declare them (a package's own `.dep`, or a
  `PACKAGE REQUIRED:` line in `PACKAGES.TXT`), slacker honours that declaration
  at the repo's responsibility - it never *guesses* - and you can switch it off
  (`RESOLVE_DEPS=off`, or `--no-deps` per run).
- Synchronous; heavy lifting (bzip2 for MANIFEST, GPG) shells out to the
  system tools Slackware already ships, so no extra Rust deps.
- Everything a user edits is plain text.


---

## Quick start

After installing the package, you edit two files and run a few commands:
```
# 1) find the fastest fresh mirror, then activate exactly one in /etc/slacker/mirrors
slacker find-mirror
$EDITOR /etc/slacker/mirrors

# 2) set your repo priorities
$EDITOR /etc/slacker/repos

# 3) check the setup - works even before update, and lists what to do next
slacker status

# 4) import keys once, then refresh
slacker update gpg
slacker update
slacker install-new
slacker upgrade-all
```
---

## Documentation

| | |
|---|---|
| [Installation](https://forge.slackware.nl/rizitis/slacker/wiki/Installation) | binary package and building from source |
| [Quick Start](https://forge.slackware.nl/rizitis/slacker/wiki/Quick-Start) | first-time setup, step by step |
| [Configuration](https://forge.slackware.nl/rizitis/slacker/wiki/Configuration) | `slacker.conf`, `mirrors`, `repos`, `blacklist` |
| [Repositories and Priority](https://forge.slackware.nl/rizitis/slacker/wiki/Repositories-and-Priority) | the priority model, `subtree`, pins, `@` selectors |
| [Commands](https://forge.slackware.nl/rizitis/slacker/wiki/Commands) | full reference for all 38 actions |
| [Common Workflows](https://forge.slackware.nl/rizitis/slacker/wiki/Common-Workflows) | the recipes you will actually use |
| [Distribution Upgrade](https://forge.slackware.nl/rizitis/slacker/wiki/Distribution-Upgrade) | `upgrade-dist` - moving to a new Slackware release |
| [Package History](https://forge.slackware.nl/rizitis/slacker/wiki/Package-History) | the `history` command |
| [Dependencies](https://forge.slackware.nl/rizitis/slacker/wiki/Dependencies) | `.dep` resolution |
| [Docker](https://forge.slackware.nl/rizitis/slacker/wiki/Docker) | the minimal container image and its resolve-stock bootstrap |
| [Security](https://forge.slackware.nl/rizitis/slacker/wiki/Security) | GPG, key pinning, verification, quarantine |
| [Blacklist](https://forge.slackware.nl/rizitis/slacker/wiki/Blacklist) | freezing and hiding packages |
| [Templates](https://forge.slackware.nl/rizitis/slacker/wiki/Templates) | snapshot and replay package sets |
| [Comparison](https://forge.slackware.nl/rizitis/slacker/wiki/Comparison) | slacker vs `slackpkg` vs `slackpkg+` |
| [FAQ](https://forge.slackware.nl/rizitis/slacker/wiki/FAQ) | troubleshooting and common questions |
| [Contributing](https://forge.slackware.nl/rizitis/slacker/wiki/Contributing) | architecture and how to build/test |
| [Building and Releasing](https://forge.slackware.nl/rizitis/slacker/wiki/Building-and-Releasing) | freezing dependencies for a distro package |
| [Status and Roadmap](https://forge.slackware.nl/rizitis/slacker/wiki/Status-and-Roadmap) | what works today, what is planned |
| [The Emblem](https://forge.slackware.nl/rizitis/slacker/wiki/The-Emblem) | the labyrinth logo and where it comes from |

---

## slacker-gui -- GTK4/libadwaita front-end for slacker

> slacker-gui is **OPTIONAL**

It contains no package logic: it
runs the slacker binary, reads what slacker prints, and lays it out.

<!-- αυτή η γραμμή δεν φαίνεται  ![repos](./slacker-gui/dev-docs/slacker-2.png) -->

<video src="/attachments/4b3ab746-b83f-4794-8765-2269bd03086f" controls></video>
---

## slacker-src

Does a slacker-src exists? Can slacker build packages from source code?

NO, it doesn't need to. The reasons:
1. if we're talking about SBo there are already excellent tools for that, so there's no reason for anything new.
2. if we're talking about the official slackbuilds of the distribution, it's still not needed.
They would be needed or  to be more precise, it would make sense if the dependencies were officially part and prerequisite of every slackbuild. The philosophy of the distribution is different from that, so there's no need for a slacker-src nor would it have anything to offer...

---

## Contributing

If you run **slackware-current** (always up to date), you can build slacker from source or install the binary provided in every release, use it, and
report what you find:

- **Bug reports / issues** are very welcome.
- Please include your **slackware-current state** (e.g. the date you last upgraded)
  and your **Rust version** (`rustc --version`).
- **Ideas are welcome too** - thanks to the fluid `0.x` model, a strong idea can
  genuinely make it into the codebase.
  
- If you can code PR's are more than welcome. Help from AI assistans are also welcome but a PR direclty from an AI is not.
- NOTE: That this project it cannot succeed if it remains the work of one person, no matter how well-made it is.
  So do it, try slacker, and if you like it get involved, take project and make it better...

> **Note:** GitHub repository is a **read-only mirror**. Development happens
> upstream at <https://forge.slackware.nl/rizitis/slacker>. You may open **issues** there, download releases,
> but send any **patches upstream**.

---

## Acknowledgements

* [Patrick Volkerding](http://www.slackware.com/~volkerdi/) - For Slackware and for pkgtools that slacker use.
* [Eric Hameleers](https://forge.slackware.nl/forgeadmin) - For hosting and maintain `https://forge.slackware.nl`(slacker`s home), slacker use his repos and packages.
* [Darren Austin](https://slackware.uk/cumulative/) - For hosting slackware.uk, slacker use `https://slackware.uk/cumulative/` repo.
* [Nathaniel Russell](https://forge.slackware.nl/n4t3r) - For `https://reddoglinux.ddns.net/` slacker use his repos and packages.
* [Jay Lanagan](https://slackware.lngn.net/) - slacker use his repo and packages for current repo `https://slackware.lngn.net/pub/x86_64/slackware64-current/`
* [Corrado Franco](https://forge.slackware.nl/conraid) - For requests and all the bug reports. slacker use his repo and packages for current.
* [Willy Sudiarto Raharjo ](https://github.com/willysr) - slacker use his packages for MATE and Cinnamon hosted in `https://slackware.uk` for stable.
* [Georgi Sotirov](https://sotirov-bg.net/slackpack/about.cgi?q=site) - slacker use his slackpack repos and packages for stable.
* [Matteo Bernardini ](https://slackware.ponce.cc/blog/) - slacker use his repo and packages.
* [Danilo Macrì](https://forge.slackware.nl/danix) - For Pull Requests (shell-completions for slacker) and bug reports.
* My real friend **@gcosbug** for the code oversight, the fixes and the endless time he has put in...

> slacker is dedicated to the memory of my good friend from France Didier Spaier. Who was the creator and maintainer of Slint Linux.

> Καλή ανάπαυση στην ψυχή και καλή ανάσταση φίλε...

---

## Legal Disclaimer and Notices

```
SLACKER CODE AND PRECOMPILED BINARIES ARE PROVIDED “AS IS” AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL SLACKER DEVELOPERS OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) SUSTAINED BY YOU OR A THIRD PARTY, HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT ARISING IN ANY WAY OUT OF THE USE OF THIS SAMPLE CODE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Code and binaries of slacker are not covered by any slacker Service Level Agreements.
```

**slacker** is free software under the Apache License 2.0 LICENSE, by Ioannis Anagnostakis (rizitis) and it is not affiliated with or endorsed by Slackware Linux.<br>

**Slackware** is a registered trademark of **Patrick J. Volkerding** and **Linux** is a registered trademark of **Linus Torvalds**.<br>

**Use slacker at your own risk**













