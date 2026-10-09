# Release plan

How changes reach MISO Terminal's `main` branch, and how releases ship from
it. The first release, `v0.2.0`, came out on 2026-10-07; what comes next is
[after v0.2.0](#after-v020).

## Goals

- `main` always builds, passes its tests and could be released. Every change
  reaches it through a pull request that CI has passed.
- Anyone can download a release, check that this repository built it, run it
  without building anything, and see what changed.
- A source that changes its format (the weekly drift job fails) is fixed in a
  release within days.
- The [principles](../README.md#principles) hold for releases too: no
  telemetry, and nothing contacts a server the user did not ask for.

## How changes land

Trunk-based: `main` is the only long-lived branch, and every change, the
maintainer's included, is a short-lived branch (`fix/gas-workbook`,
`history/backfill`) merged through a pull request.

- **One change per pull request,** one a reviewer can follow: a function, a
  fix, a source. A large piece of work is several pull requests, each leaving
  the app working; a function is registered (and listed in HELP and the
  README) only in the one that makes it usable.
- **The title** says what changes, in a sentence of about 70 characters with
  no prefix. **The description** says why and what to look at, in prose: it
  becomes the commit message. Work done with Claude ends with its
  `Co-Authored-By:` line. There is no pull request template, since its
  checklist would end up in every commit.
- **Squash merges** only, so `main` reads as a list of changes; branches are
  deleted on merge. [CONTRIBUTING](../CONTRIBUTING.md) has the checklist.
- **Rulesets,** with no bypass: `main` and `release/*` change only through
  pull requests whose required checks pass (*Windows (primary)*, *Linux
  (non-UI crates)*, *Docs*), with linear history and no force pushes; `v*`
  tags cannot be moved or deleted, and with GitHub's immutable releases a
  release's tag and files cannot change after publication. No review is
  required while there is one maintainer.
- **CI** builds everything `--locked` with the compiler `rust-toolchain.toml`
  pins, so a new Rust release cannot turn it red overnight. *Docs* builds the
  site and fails on a broken link or anchor; the Linux job generates the
  third-party notices, so a dependency under an unaccepted licence fails its
  own pull request.
- **Dependencies:** Dependabot opens weekly grouped pull requests (Cargo
  lockfile-only, and GitHub Actions). Upgrades past `Cargo.toml`'s ranges stay
  deliberate, since several crates must move together (the egui family,
  quick-xml with calamine's, `windows-registry` with hyper-util's). An OSV
  audit of `Cargo.lock` is a monthly manual check.
- **Issues:** a drift failure opens (or comments on) an issue labelled
  `source`; each release has a milestone.

## Versioning

- Semantic versioning below 1.0 while formats settle: a minor version for
  features (`0.3.0`), a patch for fixes (`0.2.1`). 1.0 comes once the markets
  phases are done and the config, layout and cache formats have held steady
  for a few releases.
- One version for the whole workspace, shown by `--version`, HELP, LOG, the
  executable's properties and the zip's name. Builds that are not releases
  add the commit (`0.2.0+3f2a1c9`). A release build is a windowed program, so
  `--version` at a prompt prints only when piped (`miso-terminal --version |
  more`).
- **`0.3.0`** is the [analytics plan](ANALYTICS-PLAN.md) with the update
  check; [signing](#signing) does not wait for it. **`0.4.0`** is live
  trading (markets Phase 6), only once releases are signed. Patch releases come
  as needed; there is no fixed calendar.
- Release candidates are tagged `v0.3.0-rc.1`, carry that version in
  `Cargo.toml`, and are published as GitHub pre-releases.

**Compatibility.** Every `config.toml` key is optional and unknown keys are
ignored, so settings carry across versions both ways. Each release adds the
config it writes to `fixtures/compat/` (`cargo run -p mt-ui --example
default_config -- fixtures/compat/0.3.0.toml`), and a test loads every file
there strictly. The layout has `LAYOUT_VERSION`; data the app keeps in its
cache must stay readable or be discarded cleanly, and the changelog says so.
Live trading keys get their own credential entries.

## Changelog and notes

`CHANGELOG.md` follows Keep a Changelog (Added, Changed, Fixed, Removed), in
user terms. Each pull request with a user-visible change adds its line under
*Unreleased*; a release turns that into its own section, and the release's
notes are that section plus the checksums and how to verify the download.

## The release workflow

`.github/workflows/release.yml`, run by a `v*` tag:

1. Fails unless the tag equals the workspace version.
2. Builds `--locked` from scratch with the pinned toolchain and
   `MT_RELEASE=1`, in a job with read-only access.
3. Packs the zip with `THIRD-PARTY-NOTICES.html` (`cargo-about`,
   `packaging/about.toml`) and the font licences; `install.ps1` installs them.
4. Writes `SHA256SUMS.txt`, and a build provenance attestation, so
   `gh attestation verify <zip> --repo ArenKDesai/miso-terminal` proves the
   zip came from this workflow at that tag.
5. Takes the notes from the version's changelog section
   (`tools/release_notes.py`; a candidate uses its version's), adding the
   checksum, the attestation, the SmartScreen note and a link to the source,
   as the AGPL asks.
6. Publishes from a second job, the only one that can write: a draft first,
   files attached, then published (immutable releases allow no later
   additions). A tag with a pre-release part becomes a pre-release.

Pull requests that change the release's files, and a run by hand on `main`,
go through the whole build as a dry run: the zip, checksums and notes come out
as an artifact, and nothing is published.

## Making a release

1. **Before:** the milestone is done, CI is green, and the drift workflow, run
   by hand that day, is green against every live source. Refresh the Alpaca
   fixtures (the drift workflow's `alpaca_fixtures` run) if they are stale.
2. **Release pull request** (`prepare/v0.3.0-rc.1`): the version in
   `Cargo.toml` and `Cargo.lock`, *Unreleased* turned into `## [0.3.0]`, the
   compatibility fixture, and the README's install notes if they changed.
3. **Tag the candidate** on its merge commit (`git tag -a v0.3.0-rc.1 -m
   "v0.3.0-rc.1"`) and push the tag; the workflow publishes a pre-release.
4. **Test it** on a clean Windows account: run it portable, run
   `install.ps1` (shortcut, first launch, uninstall), try `--offline`, store
   Alpaca keys, install over the previous release with its data in place, and
   check the checksum and attestation. Fixes go in by pull request, then
   `rc.2`.
5. **Release:** a last pull request sets the version and the changelog's
   date; tag `v0.3.0` on its merge commit, check the release page and the
   docs site, and close the milestone.

**Patch releases** ship the same way while `main` is releasable. If `main`
already holds unfinished work for the next minor release, the fix goes on a
`release/0.3` branch cut from the tag, is tagged there, and is merged forward.

## The docs site and releases

The site documents what people download. It deploys from `main` today; it is
to deploy when a release is published (and by hand for an urgent
correction), while pull requests keep building it through *Docs*. The theme
gallery's index lives on the site, so new gallery themes then reach THEME
with the next release.

## Signing

Unsigned executables meet SmartScreen's warning, and Smart App Control (on by
default on some new Windows 11 installations) blocks them outright. MISO
Terminal is applying to **SignPath Foundation**, which signs open-source
projects free of charge: SignPath's service signs what the release workflow
built, with the Foundation's certificate, once the maintainer approves the
request. The [Code signing policy](CODE-SIGNING.md) is the public side of
it. The Foundation's [conditions](https://signpath.org/terms), and where the
project stands:

- **OSI-approved licences for every component.** The AGPL, and
  `packaging/about.toml` accepts only OSI-approved licences for the crates
  (egui's default fonts, which include Ubuntu Light, are left out for this);
  the bundled fonts are under the OFL and MIT.
- **Released in the form to be signed,** with its functionality described
  on the download page: `v0.2.0`, the README and the release notes.
- **A code signing policy** named so on the home page and the download page
  (the README and the release notes link it), with the team's roles and a
  [privacy policy](PRIVACY.md).
- **Product name and version in the executable** (`build.rs`), the same in
  every build of a version.
- **Multi-factor authentication** on GitHub and SignPath for everyone in a
  role.
- **A verifiable reputation:** the Foundation decides whether a program is
  known well enough to sign, so a young project's application may have to
  wait for users and downloads.

Once accepted, the release workflow sends the zip to SignPath before
publishing; SignPath signs `miso-terminal.exe` and `install.ps1` inside it
after the maintainer approves, and the signed zip is what gets checksummed,
attested and published. Microsoft's Trusted Signing (a monthly fee and an
identity check) and a conventional certificate are the alternatives. Until
then the notes explain the SmartScreen prompt and how to check the checksum
and attestation. **Signing must be in place before live trading ships.**

## Installing and updating

- The zip and `install.ps1` (per user, no administrator rights) stay the way
  to install; an MSI or MSIX installer and a winget manifest follow signing.
- **An update check, off by default:** when turned on in SET, it asks
  GitHub's releases API at most once a day and shows a link to a newer
  release. It never downloads anything, and LOG shows the request.
- Upgrading keeps everything: config and themes in `%APPDATA%`, cache and
  layout in `%LOCALAPPDATA%`, keys in Credential Manager.

## When a release goes wrong

Earlier releases stay on the releases page. A bad release is marked as such in
its notes and followed by a patch; going back a version is safe for settings.
A published tag is never moved or reused: the fix is always a new version.

## After v0.2.0

1. **Signing,** alongside the work for `0.3.0`: the application to SignPath
   Foundation, then signing in the release workflow once it is accepted.
2. **`0.3.0`:** the [analytics plan](ANALYTICS-PLAN.md) (the price history is
   built; studies and BETA, forecasts and the assistant to come), the update
   check and the docs site deploying from releases, released with a candidate
   first.
3. **`0.4.0`:** live trading (markets Phase 6), once releases are signed and
   the open findings of the [security review](SECURITY-REVIEW.md) are closed.
