# Release plan

How changes reach MISO Terminal's `main` branch, and how releases ship from
it. A plan, not yet carried out: nothing has been released so far
(2026-10-05). [The road to v0.2.0](#the-road-to-v020) puts the steps in order,
and the README's roadmap tracks them.

## Where things stand

- Until 2026-10-05 every change was pushed straight to `main`. Since then
  changes land through pull requests ([#1](https://github.com/ArenKDesai/miso-terminal/pull/1)
  was the first), and GitHub enforces it: the `main` and release-tag rulesets,
  squash merges only, auto-merge, branches deleted on merge and immutable
  releases are on, and the `v0.2.0` milestone exists. The docs site still
  deploys whatever `main` holds.
- CI (`ci.yml`) runs on pull requests and on `main`: fmt, clippy, tests and a
  release build on Windows, and the non-UI crates on Linux.
- `.github/workflows/release.yml` already exists: pushing a `v*` tag runs the
  tests, builds the release exe and attaches a zip to a GitHub release (the exe,
  `fixtures/` for `--offline`, README, LICENSE, `install.ps1` and the font
  licences), with notes GitHub generates from the commits. It has never run.
- The workspace version is still `0.1.0`, and `--version` prints it.
- The zip is ready to share in one respect: its fixtures hold no keys, personal
  data or licensed content (news and Alpaca recordings are sample copies with
  synthetic prices).

## Goals

- `main` always builds, passes its tests and could be released. Every change
  reaches it through a pull request that CI has passed.
- Anyone can download a release, check that it is the one this repository
  built, run it without building anything, and see what changed.
- A source that changes its format (the weekly drift job fails) is fixed in a
  release within days, not at the next feature release.
- The [principles](../README.md#principles) hold for releases too: no
  telemetry, and nothing contacts a server the user did not ask for. An update
  check is something the user turns on.

## How changes land

The project is trunk-based: `main` is the only long-lived branch, and work
happens on short-lived branches merged through pull requests. That suits one
maintainer with occasional contributors. There is no `develop` branch, because
nothing would sit there that should not be on `main` already.

**Branches.** One branch per change, named for what it does (`fix/gas-workbook`,
`docs/tutorials`, `markets/phase-3-port`), cut from the latest `main` and
deleted once merged. A branch holds one change a reviewer can follow: a
function, a fix, a source. A markets phase is several pull requests, not one.

**Pull requests.** Every change, the maintainer's included, opens a pull
request into `main`:

- The title says what changes, in the style of the history so far: a sentence
  of about 70 characters, no prefix (`CMP with securities: stocks above MISO
  node prices on one time axis`).
- The description says why, and what to look at. It becomes the commit
  message, so it is written as one: prose, not a list of every file touched.
  Work done with Claude ends with its `Co-Authored-By:` line.
- A template (`.github/pull_request_template.md`) carries the checklist below.
- Merged by **squash**, so each pull request is one commit on `main` and the
  history reads as a list of changes. The repository allows squash merges only,
  takes the default squash message from the pull request's title and
  description, and deletes the branch afterwards.

**The checklist** (what reviewers and CI look for):

- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` pass locally, and CI is green.
- A visual change comes with its accepted snapshots
  (`UPDATE_SNAPSHOTS=force`), and the pull request says what changed and why.
- A user-visible change adds a line to `CHANGELOG.md` under *Unreleased*.
- A new function has its README row; a changed panel, argument or config key
  updates [the tutorials](TUTORIALS.md).
- No keys, personal data, verbatim publisher text or verbatim Alpaca
  recordings in code, fixtures, logs or test output.

**Rules on GitHub.** Two rulesets, with no one allowed to bypass them (an
urgent fix still goes through a pull request; it takes as long as CI):

- *`main`* and `release/*`: changes only through pull requests
  (no review required while there is one maintainer, since GitHub does not let
  an author approve their own), the required checks below must pass on the
  pull request's latest commit, history stays linear, no force pushes, no
  deletion. A branch need not be up to date with `main` to merge, so
  auto-merge never stalls behind another merge; CI runs again on `main` after
  each merge and catches the rare clash between two changes.
- *Tags `v*`*: no updates or deletions, so a published version always points
  at the same commit. Together with GitHub's *immutable releases* setting, a
  release's tag and files cannot change after publication.

**Required checks.** *Windows (primary)* and *Linux (non-UI crates)* from
`ci.yml`, plus a new *Docs* job there that builds the site, so a broken page
or link fails the pull request. (`docs.yml` runs only when its paths change,
and a check that may not run cannot be required.) CI also builds and tests
with `--locked`, so a pull request that changes `Cargo.toml` must commit the
matching `Cargo.lock`.

**Dependencies.** Dependabot opens one grouped pull request a week for Cargo
minor and patch updates and one for GitHub Actions, replacing the manual
`cargo update` round. Major versions arrive one at a time. `windows-registry`
stays at 0.6 (it must match hyper-util's), so Dependabot ignores its major
updates. The OSV audit of `Cargo.lock` stays a monthly manual check.

**Issues and milestones.** Planned work and bugs are issues; a pull request
that finishes one says `Closes #12`. Each release has a milestone (`v0.2.0`)
holding what must be done first, so the release's state is one page. When
the drift job fails it opens (or comments on) an issue labelled `source`, so a
format change is tracked like any bug instead of waiting in the Actions tab.

**Claude.** Claude works the same way: a branch and a pull request for every
change, never a push to `main`. Claude uses GitHub as the maintainer, so it
cannot approve its own pull requests either; the required checks are the
gate. The maintainer has given standing permission (2026-10-05) for routine
pull requests: Claude turns on auto-merge as it opens one
(`gh pr merge --auto --squash`), GitHub merges it once the required checks
pass, and Claude reports the result. It asks first, and leaves the pull
request open, for:

- releases and tags (it never pushes a tag itself);
- changes to workflows, rulesets or repository settings;
- anything touching secrets or API keys;
- anything the maintainer has asked to see first.

Until auto-merge and the `main` ruleset are on, Claude waits for CI to pass
(`gh pr checks --watch`) and then merges.

## Versioning

- Semantic versioning, staying below 1.0 while formats and functions settle:
  a minor version for features (`0.3.0`), a patch version for fixes (`0.2.1`).
  1.0 comes once the markets phases are done and the config, layout and cache
  formats have held steady across a few releases.
- One version for the whole workspace (`[workspace.package] version`), shown
  by `--version`, in HELP and LOG, and in the zip's name. Builds that are not
  releases add the commit (`0.2.0+3f2a1c9`), so a bug report from a CI build
  or a source checkout says exactly what was running.
- **The first release is `v0.2.0`**: the MISO functions, themes, news,
  Alpaca stock and ETF data, and the paper account, read-only (markets phases
  0 to 3; Phase 3 was finished before the release and folded in, on
  2026-10-05), and the tutorials. After that, a minor release closes each
  markets phase (Phase 4, paper trading, is `0.3.0`; Phase 5, options,
  `0.4.0`), with patch releases as
  needed in between. No fixed calendar.
- Release candidates are tagged `v0.2.0-rc.1`, carry that version in
  `Cargo.toml` (so `--version` says so), and are published as GitHub
  pre-releases.

**Compatibility.** Settings already carry across versions in both directions:
every `config.toml` key is optional and unknown keys are ignored, so an older
version reads a newer file. Each release adds the `config.toml` it writes by
default to `fixtures/compat/`, and a test loads every file there without
error, so a later change cannot quietly break an older user's settings. The
layout has `LAYOUT_VERSION`; data the app keeps in its cache (`local://`
stores) must stay readable or be discarded cleanly when its format changes,
and the changelog says so when it happens. Live trading keys (Phase 6) stay in
their own credential entries, so no release can mix them up with paper keys.

## Changelog and notes

- A `CHANGELOG.md` in the Keep a Changelog style (Added, Changed, Fixed,
  Removed per version), in user terms: functions, settings, sources. Each pull
  request with a user-visible change adds its line under *Unreleased*, so the
  changelog is written as the work happens, not reconstructed at release time.
- A release turns *Unreleased* into that version's section, tidied.
- The release's notes are that section, plus the checksums and how to verify
  the download, instead of GitHub's generated list of commits.

## Changes to the release workflow

1. **Refuse a mismatch:** fail unless the tag equals the workspace version.
2. **Pre-releases:** tags with `-rc` are published as pre-releases.
3. **Checksums:** a `SHA256SUMS.txt` beside the zip.
4. **Provenance:** a build attestation (`actions/attest-build-provenance`),
   so `gh attestation verify <zip> --repo ArenKDesai/miso-terminal` proves the
   zip came from this repository's workflow at that tag.
5. **Third-party notices:** the Rust dependencies' licences (MIT, Apache and
   others require their notices to travel with binaries), generated with
   `cargo-about` into `THIRD-PARTY-NOTICES.html` in the zip, beside the font
   licences already there.
6. **Source:** the notes link the tag's source archive. As the AGPL asks,
   whoever has the binary can get the matching source.
7. **Locked builds:** `cargo build --release --locked`, with the toolchain
   pinned in `rust-toolchain.toml`, so a release builds from exactly the
   committed `Cargo.lock`.
8. **Notes from the changelog:** the workflow takes the tag's section from
   `CHANGELOG.md` and fails if there is none.
9. **Draft, then publish:** with immutable releases on, files cannot be added
   to a published release, so the workflow creates a draft, uploads the zip,
   checksums and notes, and publishes only once all are attached. GitHub then
   adds its own release attestation.

## Making a release

Releases are cut from `main`; there is no release branch unless a patch needs
one (below).

1. **Before:** the milestone is done, `main` is green in CI (Windows and Linux),
   and the drift workflow, run by hand that day, is green against every live
   source (MISO, NWS, EIA, the news feeds, Alpaca). The fixtures are recent
   enough for a convincing `--offline` demo; refresh the Alpaca set with the
   drift workflow's `alpaca_fixtures` run if not.
2. **Release pull request** (branch `prepare/v0.2.0-rc.1` → `main`; not
   `release/…`, which names protected maintenance branches): bump the version
   (`Cargo.toml`, `Cargo.lock`), turn *Unreleased* into the version's
   changelog section, add the compatibility fixture, update the README's
   install notes if they changed. Merge it like any other.
3. **Tag the candidate** on that merge commit with an annotated tag
   (`git tag -a v0.2.0-rc.1 -m "v0.2.0-rc.1"`) and push the tag. The workflow publishes a pre-release.
4. **Test it** on a clean Windows user account: unzip and run it portable, run
   `install.ps1` (Start Menu shortcut, first launch, uninstall), try
   `--offline`, store Alpaca keys in SET, install over the previous release
   with its config, layout and cache in place, and check the checksum and
   attestation as the notes describe. Anything wrong is fixed by ordinary pull
   requests, then a new candidate (`rc.2`).
5. **Release:** a last small pull request sets the version to `0.2.0`; tag
   `v0.2.0` on its merge commit. Check that the release page, its notes and the
   docs site say the same, and close the milestone.

## Patch releases

While `main` is releasable, a fix ships the same way: a pull request with the
fix, a release pull request for `0.2.1`, a tag. If `main` already holds
unfinished work for the next minor release, the patch comes from a maintenance
branch instead: `release/0.2` is cut from the `v0.2.0` tag (protected like
`main`), the fix lands there by pull request, `v0.2.1` is tagged there, and the
fix is merged forward into `main`. A drift failure is the usual reason for a
patch: the goal is a fixed release within days.

So that `main` stays releasable, a markets phase lands in pieces that each
leave the app working: a function is registered (and listed in HELP and the
README) only in the pull request that makes it usable. Trading functions
(phases 4 to 6) stay unregistered until their phase is finished, so a release
never carries half an order ticket.

## The docs site and releases

The site documents what people download. Until the first release it keeps
deploying from `main`; from then on, `docs.yml` deploys when a release is
published (not a pre-release), and by hand (`workflow_dispatch` on a tag) for
an urgent correction. Pull requests still build the site, through the *Docs*
check, so nothing broken waits for a release. The theme gallery's index lives
on the site, so new gallery themes reach THEME with the next release, which
is acceptable for themes.

## Signing

Unsigned executables meet SmartScreen's "unrecognised app" warning, and Smart
App Control (on by default on some new Windows 11 installations) blocks them
outright. Options:

- **SignPath Foundation**, which signs open-source projects free of charge,
  through their CI, after an application.
- **Microsoft's cloud signing service (Trusted Signing)**, for a monthly fee
  and an identity check.
- A conventional code-signing certificate, which costs more and builds
  SmartScreen reputation slowly.

The plan is to apply to SignPath Foundation after the first release. Until
releases are signed, the notes explain the SmartScreen prompt and how to check
the checksum and attestation instead. **Signing must be in place before live
trading (markets Phase 6) ships.**

## Installing and updating

- The zip and `install.ps1` stay the way to install for now (per user, no
  administrator rights). A proper installer (MSI or MSIX) and a winget manifest
  follow signing, since winget wants a stable, signed download.
- **An update check, off by default:** a setting in SET that, when on, asks
  GitHub's releases API at most once a day and shows "vX.Y.Z is available" with
  a link to the release. It never downloads or installs anything itself, and
  LOG shows the request like any other.
- Upgrading keeps everything: config and themes roam in `%APPDATA%`, cache and
  layout stay in `%LOCALAPPDATA%`, keys in Credential Manager; a portable
  install keeps its data folder beside the exe.

## When a release goes wrong

Earlier releases stay on the releases page. A bad release is marked as such in
its notes and followed by a patch release; going back a version is safe for
settings (older versions ignore newer keys). If a cached store's format
changed, the older version discards that store and fetches it again. A
published tag is never moved or reused: the fix is always a new version.

## The road to v0.2.0

In order. Each stage is one or more pull requests (the first ones under the
new rules), tracked as issues in the `v0.2.0` milestone.

1. **Working agreement.** Done: this plan, merged as the first pull request,
   `CLAUDE.md` updated to match, and the repository's settings (squash merges
   only, the default squash message from the pull request, branches deleted on
   merge, auto-merge allowed, immutable releases on, the `main` and tag
   rulesets with no bypass, and the `v0.2.0` milestone). Still to do:
   `CONTRIBUTING.md` (the branch, pull request and checklist rules above, short
   enough to read), the pull request template, and `CHANGELOG.md` with an
   *Unreleased* section that starts from what is on `main` today.
2. **CI to match.** The *Docs* job in `ci.yml` (then made a required check),
   `--locked` builds, Dependabot's configuration, and the drift job opening
   issues when it fails.
3. **The version, visible.** One workspace version in `--version`, HELP, LOG
   and the zip's name, with the commit added outside releases; the
   compatibility fixture and its test.
4. **The release workflow.** The nine changes above, tried on a fork with
   the same settings. Not with a test tag here: the tag rules and immutable
   releases mean a test release could never be deleted.
5. **Release notes material.** The README's install section and the
   tutorials checked against the release zip; the notes' text on SmartScreen,
   checksums and attestations.
6. **`v0.2.0-rc.1`**, tested as above; `rc.2` and so on if needed.
7. **`v0.2.0`.** Then: the docs site deploys from releases, the application to
   SignPath Foundation, and the update check (off by default) for `0.3.0`.
