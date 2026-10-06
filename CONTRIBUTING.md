# Contributing

Thank you for helping with MISO Terminal. It is a community project, licensed
under the [AGPL-3.0](LICENSE): by contributing, you agree that your work is
licensed the same way.

## Principles

Every change keeps the [principles](README.md#principles): users' data stays
theirs (no telemetry; keys only in Windows Credential Manager), every
source's terms, robots.txt and rate limits are respected, and everything
stays free and open.

## Getting started

You need Rust stable and, on Windows, the MSVC build tools. Then:

```powershell
cargo run -- --offline     # the app against recorded data, no network
cargo test --workspace     # unit, parser, UI smoke and visual regression tests
```

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) explains how the pieces fit
and why; read it before a structural change. [`docs/EXTENDING.md`](docs/EXTENDING.md)
has recipes for the common changes: a function, a MISO dataset, another
source, a guardrail, a theme.

## How changes land

`main` is the only long-lived branch, and every change reaches it through a
pull request (the [release plan](docs/RELEASES.md#how-changes-land) has the
details):

- **One change per branch,** cut from the latest `main` and named for what it
  does (`fix/gas-workbook`, `docs/tutorials`).
- **The title** says what changes, as a sentence of about 70 characters with
  no prefix (`CMP with securities: stocks above MISO node prices on one time
  axis`).
- **The description** says why, and what to look at, in prose. Pull requests
  are merged by squash, and the title and description become the commit, so
  write them as a commit message.
- **CI must pass:** *Windows (primary)* and *Linux (non-UI crates)*.

## Checklist

- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` pass locally.
- A visual change comes with its accepted snapshots
  (`$env:UPDATE_SNAPSHOTS=force; cargo test -p mt-ui --test snapshots`), and
  the description says what changed and why.
- A user-visible change adds a line to [`CHANGELOG.md`](CHANGELOG.md) under
  *Unreleased*.
- A new function has its row in the README's function table (a test checks);
  a changed panel, argument or config key updates
  [the tutorials](docs/TUTORIALS.md).
- No keys, personal data, verbatim publisher text or verbatim Alpaca
  recordings in code, fixtures, logs or test output. Recorders write sample
  copies by default (`capture_news`, `capture_alpaca`); only the drift job
  records verbatim, and it keeps nothing.
- Orders stay behind a ticket's *Confirm*: no command, hotkey or alert may
  send one.

## Reporting a problem

Open an issue saying what you ran, what you expected and what happened, with
the version (beside the title in `HELP`). The log file (in `LOG` under
*Files*, *Open* beside *Logs*) usually shows the cause; look through it
before attaching it, since its paths name your Windows user. When a MISO
feed changes format, the weekly drift job usually notices first.
