# Code signing policy

Free code signing provided by [SignPath.io](https://signpath.io),
certificate by [SignPath Foundation](https://signpath.org).

**Status:** MISO Terminal is applying to SignPath Foundation. Releases up to
`v0.2.0` are not signed; this page says so when that changes, and the
[changelog](../CHANGELOG.md) names the first signed release.

## What is signed

Only MISO Terminal's own files, from this repository: `miso-terminal.exe`
and the installer script, `install.ps1`, in each release's zip on the
[releases page](https://github.com/ArenKDesai/miso-terminal/releases). The
libraries and fonts built into the program are open source, under
OSI-approved licences (`THIRD-PARTY-NOTICES.html` and the `licenses` folder
in the zip list them); nobody else's binaries are signed with this
certificate.

Each release is built by the [release workflow](RELEASES.md#the-release-workflow)
on GitHub Actions, from a version tag on `main`, with the toolchain and
dependencies the repository pins. SignPath verifies that the files it signs
came from that workflow, and the maintainer approves every signing request by
hand. The release notes give each zip's SHA-256 and a GitHub build
attestation, which `gh attestation verify` checks.

## Team roles

- **Committers:** [Aren Desai](https://github.com/ArenKDesai), the
  maintainer, the only person with write access to the repository.
- **Reviewers:** changes from anyone else arrive as pull requests and are
  reviewed by the maintainer before they merge. Every change, the
  maintainer's included, lands through a pull request that must pass CI
  ([how changes land](RELEASES.md#how-changes-land)). Much of the code is
  written with an AI coding assistant (Claude Code) working through those
  pull requests under the maintainer's direction.
- **Approvers:** Aren Desai approves each signing request.

Everyone in these roles uses multi-factor authentication on GitHub and
SignPath.

## Privacy

The terminal sends nothing anywhere but the sources it shows and the
services you set it up to use; the [privacy policy](PRIVACY.md) lists them.

## Checking a signature

In File Explorer, *Properties* on `miso-terminal.exe`, then *Digital
Signatures*: the signer is SignPath Foundation. In PowerShell:

```powershell
Get-AuthenticodeSignature .\miso-terminal.exe
```

A file from this project that is unsigned, or signed by anyone else, did not
come from a signed release. Report it, or anything else that looks wrong with
a release, in an [issue](https://github.com/ArenKDesai/miso-terminal/issues).
