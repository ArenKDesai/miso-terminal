use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::color::{Color, contrast_ratio};

/// A complete theme. See `themes/README.md` for the file format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub meta: Meta,
    pub palette: Palette,
    pub chart: ChartColors,
    /// Colours per canonical fuel key (`coal`, `gas`, ...; see
    /// `mt_core::FUEL_KEYS`). Missing keys fall back to chart series colours.
    #[serde(default)]
    pub fuel: BTreeMap<String, Color>,
    #[serde(default)]
    pub style: StyleParams,
    #[serde(default)]
    pub fonts: Fonts,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// Stable identifier used in config files and the THEME command, e.g. `default`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Whether this is a dark theme (drives egui's base visuals).
    pub dark: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Where the colours came from, for generated themes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// Semantic colour slots. Panels use these names, never raw colours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    /// The app ground behind everything.
    pub background: Color,
    /// Panels, inputs, menus: plates on the ground.
    pub surface: Color,
    /// Selected rows, active tabs, highlighted cells.
    pub surface_alt: Color,
    /// Hairlines and dividers.
    pub border: Color,
    /// Control borders, tick marks, disabled glyphs.
    pub border_strong: Color,
    pub text: Color,
    pub text_strong: Color,
    pub text_muted: Color,
    /// Primary actions, focus, active state.
    pub accent: Color,
    /// Text on an accent fill.
    pub on_accent: Color,
    /// Anything live: the data lamp, the cursor, streaming values.
    pub live: Color,
    /// Up ticks, OK states.
    pub positive: Color,
    /// Down ticks, faults, errors.
    pub negative: Color,
    /// Attention: warnings, price alerts, key hints.
    pub warning: Color,
    /// Neutral highlights and links.
    pub info: Color,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartColors {
    /// Categorical series colours, used in order.
    pub series: Vec<Color>,
    /// Grid lines; defaults to `palette.border`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<Color>,
}

/// Geometry. Defaults suit a dense, terminal-like layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleParams {
    /// Corner radius of buttons, inputs, panels (px).
    pub rounding: f32,
    /// Gap between widgets (px).
    pub spacing: f32,
    /// Inner padding of panels and windows (px).
    pub padding: f32,
    /// Border width (px).
    pub stroke: f32,
    /// Hard drop-shadow offset for floating plates (px); 0 disables.
    pub shadow_offset: f32,
    /// Diagonal cut on two opposite corners (top-right, bottom-left) of hero
    /// tiles (px); 0 keeps them square.
    #[serde(skip_serializing_if = "is_zero")]
    pub chamfer: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl Default for StyleParams {
    fn default() -> Self {
        Self {
            rounding: 2.0,
            spacing: 6.0,
            padding: 8.0,
            stroke: 1.0,
            shadow_offset: 2.0,
            chamfer: 0.0,
        }
    }
}

/// Typeface for one text role.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontSpec {
    /// Family name: a bundled font, a file in the user fonts folder, or an
    /// installed system font.
    pub family: String,
    /// CSS-style weight, 100-900. Applied through the `wght` axis of variable fonts.
    #[serde(default = "FontSpec::default_weight")]
    pub weight: u16,
    /// Size in points.
    pub size: f32,
}

impl FontSpec {
    fn default_weight() -> u16 {
        400
    }
}

/// Fonts per text role. A missing role uses the toolkit default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fonts {
    /// Running text, tables, buttons, inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<FontSpec>,
    /// Commands, logs, codes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono: Option<FontSpec>,
    /// Panel and section headings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<FontSpec>,
    /// Small uppercase labels: column heads, units, badges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<FontSpec>,
    /// Big numeric readouts on stat tiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readout: Option<FontSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
}

impl Issue {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl Theme {
    pub fn from_toml(src: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(src)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Series colour `i`, cycling.
    pub fn series(&self, i: usize) -> Color {
        match self.chart.series.len() {
            0 => self.palette.accent,
            n => self.chart.series[i % n],
        }
    }

    pub fn grid(&self) -> Color {
        self.chart.grid.unwrap_or(self.palette.border)
    }

    /// Colour for a canonical fuel key, falling back to a stable series colour.
    pub fn fuel_color(&self, key: &str) -> Color {
        self.fuel.get(key).copied().unwrap_or_else(|| {
            let i = key.bytes().fold(0usize, |h, b| {
                h.wrapping_mul(31).wrapping_add(usize::from(b))
            });
            self.series(i)
        })
    }

    /// Check identifiers and WCAG contrast of the pairings the UI actually uses.
    /// Errors make text unreadable; warnings are worth a look.
    pub fn validate(&self) -> Vec<Issue> {
        let mut out = Vec::new();
        let mut push = |severity, message: String| out.push(Issue { severity, message });

        if self.meta.id.is_empty()
            || !self
                .meta
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            push(
                Severity::Error,
                format!(
                    "meta.id {:?} must be lowercase letters, digits and '-'",
                    self.meta.id
                ),
            );
        }
        if self.chart.series.is_empty() {
            push(
                Severity::Error,
                "chart.series must list at least one colour".into(),
            );
        }

        let p = &self.palette;
        // (name, foreground, background, error below, warn below)
        let pairs = [
            ("text on background", p.text, p.background, 4.5, 4.5),
            ("text on surface", p.text, p.surface, 4.5, 4.5),
            ("text on surface_alt", p.text, p.surface_alt, 3.0, 4.5),
            ("text_strong on surface", p.text_strong, p.surface, 4.5, 7.0),
            ("text_muted on surface", p.text_muted, p.surface, 3.0, 4.5),
            ("accent on surface", p.accent, p.surface, 3.0, 4.5),
            ("on_accent on accent", p.on_accent, p.accent, 4.5, 4.5),
            ("positive on surface", p.positive, p.surface, 3.0, 4.5),
            ("negative on surface", p.negative, p.surface, 3.0, 4.5),
            ("warning on surface", p.warning, p.surface, 3.0, 4.5),
            ("info on surface", p.info, p.surface, 3.0, 4.5),
            ("live on background", p.live, p.background, 3.0, 4.5),
            (
                "border_strong on surface",
                p.border_strong,
                p.surface,
                2.0,
                3.0,
            ),
        ];
        for (name, fg, bg, error_below, warn_below) in pairs {
            let ratio = contrast_ratio(fg, bg);
            if ratio < error_below {
                push(
                    Severity::Error,
                    format!("{name}: contrast {ratio:.2}:1 is below {error_below}:1"),
                );
            } else if ratio < warn_below {
                push(
                    Severity::Warning,
                    format!("{name}: contrast {ratio:.2}:1 is below {warn_below}:1"),
                );
            }
        }
        for (i, c) in self.chart.series.iter().enumerate() {
            let ratio = contrast_ratio(*c, p.background);
            if ratio < 2.0 {
                push(
                    Severity::Warning,
                    format!("chart.series[{i}] {c}: only {ratio:.2}:1 against background"),
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r##"
[meta]
id = "test"
name = "Test"
dark = true

[palette]
background = "#000000"
surface = "#111111"
surface_alt = "#222222"
border = "#333333"
border_strong = "#888888"
text = "#eeeeee"
text_strong = "#ffffff"
text_muted = "#aaaaaa"
accent = "#a7c080"
on_accent = "#000000"
live = "#6ef0b0"
positive = "#83c092"
negative = "#e67e80"
warning = "#dbbc7f"
info = "#7fbbb3"

[chart]
series = ["#a7c080", "#7fbbb3"]
"##;

    #[test]
    fn minimal_theme_uses_defaults() {
        let t = Theme::from_toml(MINIMAL).unwrap();
        assert_eq!(t.style, StyleParams::default());
        assert_eq!(t.fonts, Fonts::default());
        assert_eq!(t.grid(), t.palette.border);
        assert_eq!(t.series(3), t.chart.series[1]);
        // Unknown fuel keys still get a stable colour.
        assert_eq!(t.fuel_color("geothermal"), t.fuel_color("geothermal"));
        assert!(t.validate().iter().all(|i| !i.is_error()));
    }

    #[test]
    fn validation_catches_unreadable_text_and_bad_ids() {
        let mut t = Theme::from_toml(MINIMAL).unwrap();
        t.palette.text = t.palette.background;
        t.meta.id = "Bad Id".into();
        let errors: Vec<_> = t.validate().into_iter().filter(Issue::is_error).collect();
        assert!(
            errors
                .iter()
                .any(|i| i.message.contains("text on background"))
        );
        assert!(errors.iter().any(|i| i.message.contains("meta.id")));
    }

    #[test]
    fn missing_slot_is_a_parse_error() {
        let src = MINIMAL.replace("info = \"#7fbbb3\"\n", "");
        let err = Theme::from_toml(&src).unwrap_err().to_string();
        assert!(err.contains("info"), "{err}");
    }
}
