# Theme format

A theme is one TOML file. Built-in themes live here and are compiled into the
binary. User themes go in the themes folder (`THEME` → *Open themes folder*;
on Windows `%APPDATA%\MISO Terminal\config\themes`) and reload as you save them.
A user theme with the same `id` as a built-in replaces it.

The fastest start is `THEME` → *Copy “…” to edit*, which writes a complete copy
of the current theme for you to change.

```toml
[meta]
id = "my-theme"            # lowercase letters, digits, '-'; used by `THEME my-theme`
name = "My Theme"
dark = true                # base for anything not covered below
description = "optional"
author = "optional"

[palette]                  # all required
background    = "#16191a"  # app ground behind everything
surface       = "#1e2223"  # panels, inputs, menus, tiles
surface_alt   = "#283127"  # selected rows, active tabs, hover
border        = "#343a3a"  # hairlines and dividers
border_strong = "#7f8886"  # control borders, tick marks
text          = "#d9d8d0"
text_strong   = "#f2f1ea"  # headings, readouts
text_muted    = "#9a9e9a"  # labels, units, secondary text
accent        = "#a7c080"  # primary actions, focus, the brand colour
on_accent     = "#16191a"  # text on an accent fill
live          = "#6ef0b0"  # the live lamp, cursor, streaming values
positive      = "#83c092"  # up ticks, OK
negative      = "#e67e80"  # down ticks, errors, extreme prices
warning       = "#dbbc7f"  # attention, alert-level prices, shadow prices
info          = "#7fbbb3"  # links, negative prices

[chart]
series = ["#a7c080", "#dbbc7f", "#7fbbb3"]   # used in order; at least one
grid = "#343a3a"                              # optional, defaults to border

[fuel]                     # optional; keys are mt_core::FUEL_KEYS
coal = "#7f8886"           # coal gas nuclear wind solar hydro storage imports other
gas = "#a7c080"

[style]                    # optional; these are the defaults
rounding = 2.0             # corner radius, px
spacing = 6.0              # gap between widgets, px
padding = 8.0              # panel padding, px
stroke = 1.0               # border width, px
shadow_offset = 2.0        # hard drop shadow on floating plates, px (0 = none)

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

## Built-ins

| id | Notes |
|---|---|
| `everforge-dark` | Default. **Generated** from the Everforge design tokens; do not edit by hand. |
| `everforge-light` | **Generated**, as above. |
| `amber-terminal` | Hand-written amber-on-black look, monospace throughout. |

Regenerate the Everforge pair after a token change:

```powershell
cargo run -p mt-theme --example sync_everforge -- ..\everforge
```
