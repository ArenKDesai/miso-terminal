//! Regenerate `themes/gallery/everforge-{dark,light}.toml` from the Everforge tokens.
//!
//!     cargo run -p mt-theme --example sync_everforge -- [path/to/everforge]
//!
//! The argument is a checkout of the Everforge repository (default: a sibling
//! `../everforge` next to this repository). The tool reads its generated
//! `dist/json/everforge.json`, so run Everforge's own `build.py` first. Edit the
//! tokens there, never the generated theme files here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mt_theme::{ChartColors, Color, FontSpec, Fonts, Meta, Palette, StyleParams, Theme};
use serde_json::Value;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let everforge = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join("../everforge"));
    let dist = everforge.join("dist/json/everforge.json");
    let json: Value = serde_json::from_str(
        &std::fs::read_to_string(&dist).map_err(|e| format!("{}: {e}", dist.display()))?,
    )?;
    let version = json["version"].as_u64().unwrap_or(0);
    let px = |group: &str, name: &str| -> Result<f32, String> {
        json[group][name]
            .as_str()
            .and_then(|v| v.strip_suffix("px"))
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| format!("missing or non-px token {group}.{name}"))
    };

    for (variant, dark) in [("dark", true), ("light", false)] {
        let t = &json["themes"][variant];
        let c = |name: &str| -> Result<Color, String> {
            t["colors"][name]
                .as_str()
                .or_else(|| t["extras"][name].as_str())
                .ok_or_else(|| format!("{variant}: missing token {name}"))?
                .parse()
                .map_err(|e| format!("{variant}.{name}: {e}"))
        };
        let theme = Theme {
            meta: Meta {
                id: format!("everforge-{variant}"),
                name: format!("Everforge {}", if dark { "Dark" } else { "Light" }),
                dark,
                author: "Aren Desai".into(),
                description: "Neutral graphite plates, brass fittings, rust for faults, mint-neon for anything live.".into(),
                source: format!("Everforge design tokens v{version} (dist/json/everforge.json)"),
            },
            palette: Palette {
                background: c("surface")?,
                surface: c("surface-raised")?,
                surface_alt: c("moss-soft")?,
                border: c("line")?,
                border_strong: c("steel")?,
                text: c("ink")?,
                text_strong: c("ink-strong")?,
                text_muted: c("ink-muted")?,
                accent: c("moss")?,
                on_accent: c("on-moss")?,
                live: c("signal")?,
                positive: c("pine")?,
                negative: c("rust")?,
                warning: c("brass")?,
                info: c("blue")?,
            },
            chart: ChartColors {
                // Rust is reserved for faults in Everforge, so it comes last.
                series: vec![
                    c("moss")?, c("brass")?, c("blue")?, c("purple")?,
                    c("pine")?, c("signal")?, c("steel")?, c("rust")?,
                ],
                grid: Some(c("line")?),
            },
            fuel: BTreeMap::from([
                ("coal".into(), c("steel")?),
                ("gas".into(), c("moss")?),
                ("nuclear".into(), c("purple")?),
                ("wind".into(), c("blue")?),
                ("solar".into(), c("brass")?),
                ("hydro".into(), c("pine")?),
                ("storage".into(), c("signal")?),
                ("imports".into(), c("ink")?),
                ("other".into(), c("ink-muted")?),
            ]),
            // radius-sm, space-2 gutters, a dense take on space-4 padding, shadow-plate
            // offset, and the cut-md chamfer on hero tiles.
            style: StyleParams {
                rounding: px("radius", "radius-sm")?,
                spacing: 6.0,
                padding: 8.0,
                stroke: 1.0,
                shadow_offset: 2.0,
                chamfer: px("radius", "cut-md")?,
            },
            // Proportional type for reading, monospace only for code (tokens v3).
            fonts: Fonts {
                body: Some(FontSpec { family: "IBM Plex Sans".into(), weight: 400, size: 14.0 }),
                mono: Some(FontSpec { family: "JetBrains Mono".into(), weight: 400, size: 13.0 }),
                heading: Some(FontSpec { family: "Space Grotesk".into(), weight: 600, size: 17.0 }),
                label: Some(FontSpec { family: "Space Grotesk".into(), weight: 600, size: 11.0 }),
                readout: Some(FontSpec { family: "Space Grotesk".into(), weight: 500, size: 26.0 }),
            },
        };
        for issue in theme.validate() {
            println!("{}: {:?} {}", theme.meta.id, issue.severity, issue.message);
        }
        let out = repo_root.join(format!("themes/gallery/everforge-{variant}.toml"));
        let header = format!(
            "# Generated by `cargo run -p mt-theme --example sync_everforge` from Everforge tokens v{version}.\n\
             # Do not edit by hand: change the tokens in the Everforge repo and re-run the sync.\n\n"
        );
        std::fs::write(&out, header + &theme.to_toml()?)?;
        println!("wrote {}", out.display());
    }
    Ok(())
}
