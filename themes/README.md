# Themes

A theme is one TOML file of semantic colour, font and spacing slots, so any
theme works with any panel. Three themes are built in; more are in the
[gallery](#gallery), one click away in `THEME`. Switch with `THEME`, or
`THEME <id>`.

## Built in

`default` (the trading-desk look: black, orange labels, white figures, blue
selection), `default-light` (the same look on paper; the light half of *Follow
Windows light/dark*) and `high-contrast` (white and gold on black, heavier and
larger type, 2 px borders; every text pairing 7:1 or better). Their files are in `themes/` and are
compiled into the binary.

<!-- builtin-cards -->

## Gallery

Themes that do not ship with the terminal. The easy way to install one is in
the terminal: run `THEME` and click *Install* (or *Install and use*) under
*Gallery*. The same list can install updates and remove themes. By hand:

1. Download its `.toml` file.
2. In the terminal, run `THEME` and click *Open themes folder*
   (on Windows `%APPDATA%\MISO Terminal\config\themes`).
3. Drop the file there. It shows up in `THEME` straight away; click it to switch.

<!-- gallery-cards -->

Catppuccin, Gruvbox, Monokai, Rosé Pine and Tokyo Night use their projects'
published palettes (Catppuccin, Gruvbox, Rosé Pine and Tokyo Night are MIT
licensed; each file names its source). Which colour fills which of the
terminal's slots is this repository's choice.

The gallery's files live in [`themes/gallery/`](gallery/). To add a theme, put
its file there, named after its `id` (`my-theme.toml`), and refresh the recorded
index with `uv run tools/build_docs.py site --update-fixture`. Tests check that
every gallery theme loads, passes the contrast checks, has an id of its own and
is in the recorded index. Render a preview for this page with

```powershell
cargo run -p mt-ui --example render -- out.png --theme my-theme --theme-file themes\gallery\my-theme.toml
```

and save it, 960 × 576, as `docs/screenshots/themes/my-theme.webp`.

## Theme format

User themes go in the themes folder (`THEME` → *Open themes folder*) and reload
as you save them. A user theme with the same `id` as a built-in replaces it. The
fastest start is `THEME` → *Copy “…” to edit*, which writes a complete copy of
the current theme for you to change.

```toml
[meta]
id = "my-theme"            # lowercase letters, digits, '-'; used by `THEME my-theme`
name = "My Theme"
dark = true                # base for anything not covered below
description = "optional"
author = "optional"

[palette]                  # all required
background    = "#000000"  # app ground behind everything
surface       = "#0a0a0a"  # panels, inputs, menus, tiles
surface_alt   = "#102a54"  # selected rows, active tabs, hover
border        = "#2a2a2a"  # hairlines and dividers
border_strong = "#707070"  # control borders, tick marks
text          = "#e6e6e6"
text_strong   = "#ffffff"  # headings, readouts
text_muted    = "#fb8b1e"  # labels, units, secondary text
accent        = "#fb8b1e"  # primary actions, focus, the brand colour
on_accent     = "#000000"  # text on an accent fill
live          = "#00e0ff"  # the live lamp, cursor, streaming values
positive      = "#4af6c3"  # up ticks, OK
negative      = "#ff433d"  # down ticks, errors, extreme prices
warning       = "#ffd400"  # attention, alert-level prices, shadow prices
info          = "#5b9dff"  # links, negative prices

[chart]
series = ["#fb8b1e", "#5b9dff", "#4af6c3"]   # used in order; at least one
grid = "#262626"                              # optional, defaults to border

[fuel]                     # optional; keys are mt_core::FUEL_KEYS
coal = "#8c8c8c"           # coal gas nuclear wind solar hydro storage imports other
gas = "#fb8b1e"

[style]                    # optional; these are the defaults
rounding = 2.0             # corner radius, px
spacing = 6.0              # gap between widgets, px
padding = 8.0              # panel padding, px
stroke = 1.0               # border width, px
shadow_offset = 2.0        # hard drop shadow on floating plates, px (0 = none)
chamfer = 0.0              # 45° cut on two opposite corners of hero tiles, px (0 = square)

[fonts]                    # optional; any role left out uses egui's default
body    = { family = "IBM Plex Sans",  weight = 400, size = 14.0 }
mono    = { family = "JetBrains Mono", weight = 400, size = 13.0 }
heading = { family = "Space Grotesk",  weight = 600, size = 17.0 }
label   = { family = "Space Grotesk",  weight = 600, size = 11.0 }  # uppercase column heads
readout = { family = "Space Grotesk",  weight = 500, size = 26.0 }  # big numbers on tiles
```

Colours are `#rrggbb` or `#rrggbbaa`. Font families resolve in this order:
bundled (IBM Plex Sans, JetBrains Mono, Space Grotesk; variable, so any weight
from 100 to 900 works), then files in the fonts folder, then installed system
fonts.

The `THEME` function shows contrast problems for the active theme. Errors mean
text that is hard to read (body text under 4.5:1). Warnings are worth a look.

## Light and dark

To switch with Windows' light/dark setting, turn on *Follow Windows light/dark*
in `THEME` and pick one theme for each mode (or set `follow_system_theme`,
`light_theme` and `dark_theme` under `[ui]` in `config.toml`). Picking a theme by
hand turns following off again. By default the pair is Default Light and
Default.

## Everforge

The gallery's Everforge Dark and Everforge Light are **generated** from the
Everforge design tokens; do not edit them by hand. Regenerate them after a token
change:

```powershell
cargo run -p mt-theme --example sync_everforge -- ..\everforge
```
