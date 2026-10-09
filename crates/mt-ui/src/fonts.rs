//! Resolves a theme's font families to font data for egui.
//!
//! Lookup order: fonts bundled in the binary (IBM Plex Sans, JetBrains Mono and
//! Space Grotesk), then font files in the user fonts folder, then installed
//! system fonts. The system scan is lazy, so themes that only use bundled fonts
//! start instantly.
//!
//! egui's own default fonts are left out of the build (the workspace
//! `Cargo.toml` says why), so every font the terminal draws with comes from
//! here.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use egui::epaint::text::{FontData, FontDefinitions, FontFamily, FontTweak, VariationCoords};
use mt_theme::{FontSpec, Fonts};

use crate::skin::{HEADING, LABEL, READOUT};

const IBM_PLEX_SANS: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Variable.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Variable.ttf");

/// Fonts compiled into the binary for themes to name. All are SIL Open Font
/// License 1.1; the licences sit next to the files in `assets/fonts/`.
const BUNDLED: &[(&str, &[u8])] = &[
    ("IBM Plex Sans", IBM_PLEX_SANS),
    ("JetBrains Mono", JETBRAINS_MONO),
    (
        "Space Grotesk",
        include_bytes!("../../../assets/fonts/SpaceGrotesk-Variable.ttf"),
    ),
];

const SYMBOLS_KEY: &str = "symbols-fallback";
/// The key a theme's IBM Plex Sans at 400 also gets, so the two share one copy.
const TEXT_KEY: &str = "IBM Plex Sans@400";

/// What every family falls back to, in order, for characters its own font
/// lacks: JetBrains Mono for the symbols the UI draws (▲ ▼ ■ ● ⚠), IBM Plex
/// Sans for the rest of text (✓ ⇄), then the two emoji fonts egui used to
/// bundle. Noto Emoji (SIL Open Font License 1.1) draws emoji in headlines;
/// emoji-icon-font (MIT) has egui's own icons, such as the ⏵ on submenus, and
/// the ☆ ★ on the Watch buttons. The scales are egui's, so they look as before.
fn fallbacks() -> [(&'static str, FontData); 4] {
    let scaled = |bytes, scale| {
        FontData::from_static(bytes).tweak(FontTweak {
            scale,
            ..Default::default()
        })
    };
    [
        (SYMBOLS_KEY, FontData::from_static(JETBRAINS_MONO)),
        (
            TEXT_KEY,
            weighted(FontData::from_static(IBM_PLEX_SANS), 400),
        ),
        (
            "Noto Emoji",
            scaled(
                include_bytes!("../../../assets/fonts/NotoEmoji-Regular.ttf"),
                0.81,
            ),
        ),
        (
            "emoji-icon-font",
            scaled(
                include_bytes!("../../../assets/fonts/emoji-icon-font.ttf"),
                0.90,
            ),
        ),
    ]
}

/// Static fonts ignore the axis; variable fonts use it to pick the weight.
fn weighted(data: FontData, weight: u16) -> FontData {
    FontData {
        tweak: FontTweak {
            coords: VariationCoords::new([(b"wght", f32::from(weight))]),
            ..data.tweak
        },
        ..data
    }
}

pub fn bundled_families() -> impl Iterator<Item = &'static str> {
    BUNDLED.iter().map(|(name, _)| *name)
}

pub struct FontLibrary {
    user_dir: Option<PathBuf>,
    system: Option<fontdb::Database>,
}

impl FontLibrary {
    pub fn new(user_dir: Option<PathBuf>) -> Self {
        Self {
            user_dir,
            system: None,
        }
    }

    fn lookup(&mut self, spec: &FontSpec) -> Option<(Cow<'static, [u8]>, u32)> {
        if let Some((_, bytes)) = BUNDLED
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&spec.family))
        {
            return Some((Cow::Borrowed(*bytes), 0));
        }
        let user_dir = self.user_dir.clone();
        let db = self.system.get_or_insert_with(|| {
            let mut db = fontdb::Database::new();
            if let Some(dir) = &user_dir {
                db.load_fonts_dir(dir);
            }
            db.load_system_fonts();
            db
        });
        let id = db.query(&fontdb::Query {
            families: &[fontdb::Family::Name(&spec.family)],
            weight: fontdb::Weight(spec.weight),
            ..Default::default()
        })?;
        db.with_face_data(id, |data, index| (Cow::Owned(data.to_vec()), index))
    }

    /// Build egui font definitions for a theme. Roles without a font (or whose
    /// font cannot be found) use the fallbacks alone; the messages say why.
    pub fn definitions(&mut self, fonts: &Fonts) -> (FontDefinitions, Vec<String>) {
        let mut defs = FontDefinitions::empty();
        let mut warnings = Vec::new();
        let fallback: Vec<String> = fallbacks()
            .into_iter()
            .map(|(key, data)| {
                defs.font_data.insert(key.to_owned(), Arc::new(data));
                key.to_owned()
            })
            .collect();

        let mut load = |spec: &Option<FontSpec>, defs: &mut FontDefinitions| -> Option<String> {
            let spec = spec.as_ref()?;
            let key = format!("{}@{}", spec.family, spec.weight);
            if defs.font_data.contains_key(&key) {
                return Some(key);
            }
            let Some((bytes, index)) = self.lookup(spec) else {
                warnings.push(format!(
                    "font {:?} not found; using the default",
                    spec.family
                ));
                return None;
            };
            let mut data = match bytes {
                Cow::Borrowed(b) => FontData::from_static(b),
                Cow::Owned(v) => FontData::from_owned(v),
            };
            data.index = index;
            defs.font_data
                .insert(key.clone(), Arc::new(weighted(data, spec.weight)));
            Some(key)
        };

        // The role's own font, then what it falls back to, each once.
        let with = |key: Option<String>, fallback: &[String]| -> Vec<String> {
            let mut seen = HashSet::new();
            key.into_iter()
                .chain(fallback.iter().cloned())
                .filter(|k| seen.insert(k.clone()))
                .collect()
        };
        let body = load(&fonts.body, &mut defs);
        let mono = load(&fonts.mono, &mut defs);
        let heading = load(&fonts.heading, &mut defs);
        let label = load(&fonts.label, &mut defs);
        let readout = load(&fonts.readout, &mut defs);

        let prop = with(body, &fallback);
        defs.families
            .insert(FontFamily::Monospace, with(mono, &fallback));
        // Named families must always exist: text styles refer to them.
        for (name, key) in [(HEADING, heading), (LABEL, label), (READOUT, readout)] {
            defs.families
                .insert(FontFamily::Name(name.into()), with(key, &prop));
        }
        defs.families.insert(FontFamily::Proportional, prop);
        (defs, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_resolve_without_touching_system_fonts() {
        let theme = mt_theme::builtin()
            .into_iter()
            .find(|t| t.meta.id == "high-contrast")
            .unwrap();
        let mut lib = FontLibrary::new(None);
        let (defs, warnings) = lib.definitions(&theme.fonts);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(
            lib.system.is_none(),
            "bundled fonts must not trigger a system scan"
        );
        assert!(defs.font_data.contains_key("IBM Plex Sans@500"));
        assert!(defs.font_data.contains_key("Space Grotesk@700"));
        let prop = &defs.families[&FontFamily::Proportional];
        assert_eq!(prop[0], "IBM Plex Sans@500");
        assert_eq!(
            prop[1], SYMBOLS_KEY,
            "symbol fallback comes right after the body face"
        );
        for name in [HEADING, LABEL, READOUT] {
            assert!(defs.families.contains_key(&FontFamily::Name(name.into())));
        }
    }

    /// Without egui's default fonts, every character the terminal draws comes
    /// from a bundled font: the UI's symbols, egui's own (the submenu arrow,
    /// arrow keys' names) and emoji, in every family of every built-in theme.
    #[test]
    fn bundled_fonts_cover_what_the_ui_draws() {
        const DRAWN: &str = "·…−⚠→✕–≥×☆★“”▲✓±≤▼↑↓Δ°—■●•›↗◀▶◆⇄⏵⏷⏴⏶😊";
        use skrifa::MetadataProvider;
        // The fonts' own character maps: egui's `has_glyph` says no to
        // characters from the face holding its replacement glyph (Noto Emoji).
        let covers = |data: &FontData, c: char| {
            skrifa::FontRef::from_index(&data.font, data.index)
                .is_ok_and(|font| font.charmap().map(c).is_some())
        };
        for theme in mt_theme::builtin() {
            let (defs, _) = FontLibrary::new(None).definitions(&theme.fonts);
            for (family, chain) in &defs.families {
                let missing: String = DRAWN
                    .chars()
                    .filter(|&c| !chain.iter().any(|key| covers(&defs.font_data[key], c)))
                    .collect();
                assert!(
                    missing.is_empty(),
                    "{} {family:?} lacks {missing}",
                    theme.meta.id
                );
            }
        }
    }

    #[test]
    fn themes_without_fonts_still_define_named_families() {
        let (defs, warnings) = FontLibrary::new(None).definitions(&Fonts::default());
        assert!(warnings.is_empty());
        assert!(defs.families.contains_key(&FontFamily::Name(LABEL.into())));
    }
}
