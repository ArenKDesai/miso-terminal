# /// script
# requires-python = ">=3.11"
# ///
"""A release's notes: its CHANGELOG.md section, then how to check the download.

    uv run tools/release_notes.py 0.2.0 --zip miso-terminal-0.2.0-windows-x64.zip
    uv run tools/release_notes.py 0.2.0 --section Unreleased --zip dry-run.zip

The release workflow publishes what this prints. It takes the version's
section from the changelog (for a release candidate such as 0.2.0-rc.1, the
0.2.0 section, which the release pull request writes), and fails if there is
none, so nothing is released without its notes. Then come the download's
SHA-256, the attestation to verify, the install commands, what Windows says about an unsigned
program, and where the source is, as the AGPL asks. `--section` names
another section: the workflow's dry run uses Unreleased, or the version's
section while Unreleased is empty (just after a release pull request).
"""

from __future__ import annotations

import argparse
import hashlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPO = "ArenKDesai/miso-terminal"
SITE = "https://arenkdesai.github.io/miso-terminal/"


def section(changelog: str, name: str) -> str | None:
    """The body under `## [name]` (any date after it), up to the next section."""
    m = re.search(
        rf"^## \[{re.escape(name)}\][^\n]*\n(.*?)(?=^## \[|\Z)",
        changelog,
        re.S | re.M | re.I,
    )
    if not m:
        return None
    # Keep a Changelog's link definitions (`[0.2.0]: https://…`) are not notes.
    body = re.sub(r"^\[[^\]]+\]:\s*\S+\s*$", "", m.group(1), flags=re.M)
    return body.strip() or None


def notes(version: str, body: str, zip_path: Path) -> str:
    tag = f"v{version}"
    digest = hashlib.sha256(zip_path.read_bytes()).hexdigest()
    zip_name = zip_path.name
    candidate = "-" in version
    lead = (
        f"A release candidate for {version.split('-')[0]}, for testing before the release. "
        "Report anything wrong in an issue.\n\n"
        if candidate
        else ""
    )
    return f"""{lead}{body}

## Download and check it

`{zip_name}` holds the program, `install.ps1` (a per-user install, no
administrator rights), the recorded data for `--offline`, and the licences:
`LICENSE`, the bundled fonts' and `THIRD-PARTY-NOTICES.html` for the Rust
crates inside.

Its SHA-256 is `{digest}` (also in `SHA256SUMS.txt`). In PowerShell, in the
folder you downloaded it to, this prints `True` for a good copy:

```powershell
(Get-FileHash .\\{zip_name}).Hash -eq '{digest}'
```

GitHub attests that this repository's release workflow built it from the
tag `{tag}`. With the [GitHub CLI](https://cli.github.com):

```powershell
gh attestation verify {zip_name} --repo {REPO}
```

To install, unzip it, open PowerShell in the folder it makes (in File
Explorer, right-click inside the folder and choose *Open in Terminal*) and run:

```powershell
powershell -ExecutionPolicy Bypass -File .\\install.ps1           # Start Menu shortcut
powershell -ExecutionPolicy Bypass -File .\\install.ps1 -Desktop  # plus a desktop shortcut
powershell -ExecutionPolicy Bypass -File .\\install.ps1 -Uninstall
```

The script is not signed yet, so double-clicking it, *Run with PowerShell*
or `.\\install.ps1` alone stop with *is not digitally signed* wherever
PowerShell runs only signed scripts; `-ExecutionPolicy Bypass` lifts that for
this one run. Or skip the script and run `miso-terminal.exe` from the folder.

**Windows SmartScreen.** Releases are not code-signed, so on first launch
Windows may say *Windows protected your PC*: choose *More info*, then *Run
anyway*. Where Smart App Control is on, Windows blocks unsigned programs
outright. The checksum and attestation above are how to check this
download. The [privacy policy]({SITE}privacy.html) says
what the terminal sends where.

## Source

MISO Terminal is free software under the GNU Affero General Public License,
version 3 or later. This release's source is the tag
[`{tag}`](https://github.com/{REPO}/tree/{tag}), also attached below as zip
and tar.gz archives.
"""


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("version", help="the version, without the v: 0.2.0 or 0.2.0-rc.1")
    ap.add_argument("--zip", required=True, type=Path, help="the release zip, for its name and SHA-256")
    ap.add_argument("--section", help="the changelog section to use instead of the version's")
    ap.add_argument("--changelog", type=Path, default=ROOT / "CHANGELOG.md")
    args = ap.parse_args()

    changelog = args.changelog.read_text(encoding="utf-8")
    names = [args.version, args.version.split("-")[0]]
    if args.section:
        names.insert(0, args.section)
    body = next((b for n in dict.fromkeys(names) if (b := section(changelog, n))), None)
    if body is None:
        sys.exit(
            f"{args.changelog.name} has no section for {' or '.join(dict.fromkeys(names))}: "
            "the release pull request turns Unreleased into it"
        )
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    print(notes(args.version, body, args.zip), end="")


if __name__ == "__main__":
    main()
