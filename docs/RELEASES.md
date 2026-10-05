# Release plan

How MISO Terminal will ship releases. A plan, not yet carried out: nothing has
been released so far (2026-10-04). The README's roadmap tracks the steps.

## Where things stand

- `.github/workflows/release.yml` already exists: pushing a `v*` tag runs the
  tests, builds the release exe and attaches a zip to a GitHub release (the exe,
  `fixtures/` for `--offline`, README, LICENSE, `install.ps1` and the font
  licences), with notes GitHub generates from the commits. It has never run.
- The workspace version is still `0.1.0`, and `--version` prints it.
- The zip is ready to share in one respect: its fixtures hold no keys, personal
  data or licensed content (news and Alpaca recordings are sample copies with
  synthetic prices).

## Goals

- Anyone can download a release, check that it is the one this repository
  built, run it without building anything, and see what changed.
- A source that changes its format (the weekly drift job fails) is fixed in a
  release within days, not at the next feature release.
- The [principles](../README.md#principles) hold for releases too: no
  telemetry, and nothing contacts a server the user did not ask for. An update
  check is something the user turns on.

## Versioning

- Semantic versioning, staying below 1.0 while formats and functions settle:
  a minor version for features (`0.3.0`), a patch version for fixes (`0.2.1`).
- One version for the whole workspace (`[workspace.package] version`), shown
  by `--version`, in HELP and LOG, and in the zip's name.
- **The first release is `v0.2.0`**: the MISO functions, themes, news, and
  Alpaca stock and ETF data (markets phases 0 to 2). After that, a minor
  release closes each markets phase (Phase 3 is `0.3.0`), with patch releases
  as needed in between. No fixed calendar.
- Release candidates are tagged `v0.2.0-rc.1` and published as GitHub
  pre-releases.

**Compatibility.** Settings already carry across versions in both directions:
every `config.toml` key is optional and unknown keys are ignored, so an older
version reads a newer file. The layout has `LAYOUT_VERSION`; data the app keeps
in its cache (`local://` stores) must stay readable or be discarded cleanly
when its format changes, and the changelog says so when it happens. Live
trading keys (Phase 6) stay in their own credential entries, so no release can
mix them up with paper keys.

## Changelog and notes

- A `CHANGELOG.md` in the Keep a Changelog style (Added, Changed, Fixed,
  Removed per version), written at release time from the commits since the
  last tag, in user terms: functions, settings, sources.
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

## The checklist for each release

1. `main` is green in CI (Windows and Linux), and the drift workflow, run by
   hand that day, is green against every live source (MISO, NWS, EIA, the news
   feeds, Alpaca).
2. The fixtures are recent enough for a convincing `--offline` demo; refresh
   the Alpaca set with the drift workflow's `alpaca_fixtures` run if not.
3. Bump the version (`Cargo.toml`, `Cargo.lock`), write the changelog section,
   update the README's install notes if they changed.
4. Tag a release candidate. On a clean Windows user account: unzip and run it
   portable, run `install.ps1` (Start Menu shortcut, first launch, uninstall),
   try `--offline`, store Alpaca keys in SET, and install over the previous
   release with its config, layout and cache in place.
5. Tag the release; check that the docs site and the release page say the same.

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
changed, the older version discards that store and fetches it again.
