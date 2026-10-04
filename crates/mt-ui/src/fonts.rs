//! Resolves a theme's font families to font data for egui.
//!
//! Lookup order: fonts bundled in the binary (IBM Plex Sans, JetBrains Mono and
//! Space Grotesk), then font files in the user fonts folder, then installed
//! system fonts. The system scan is lazy, so themes that only use bundled fonts
//! start instantly.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use egui::epaint::text::{FontData, FontDefinitions, FontFamily, FontTweak, VariationCoords};
use mt_theme::{FontSpec, Fonts};

use crate::skin::{HEADING, LABEL, READOUT};

const JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Variable.ttf");

/// Fonts compiled into the binary. All are SIL Open Font License 1.1; the
/// licences sit next to the files in `assets/fonts/`.
const BUNDLED: &[(&str, &[u8])] = &[
    (
        "IBM Plex Sans",
        include_bytes!("../../../assets/fonts/IBMPlexSans-Variable.ttf"),
    ),
    ("JetBrains Mono", JETBRAINS_MONO),
    (
        "Space Grotesk",
        include_bytes!("../../../assets/fonts/SpaceGrotesk-Variable.ttf"),
    ),
];

const SYMBOLS_KEY: &str = "symbols-fallback";
const SYMBOL_FONT: &[u8] = JETBRAINS_MONO;

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
    /// font cannot be found) fall back to egui's defaults; the messages say why.
    pub fn definitions(&mut self, fonts: &Fonts) -> (FontDefinitions, Vec<String>) {
        let mut defs = FontDefinitions::default();
        let mut warnings = Vec::new();
        // JetBrains Mono covers the symbols the UI draws (▲ ▼ ■ ● ⚠) that the
        // proportional faces lack, so it backs up every family before egui's own fonts.
        defs.font_data.insert(
            SYMBOLS_KEY.into(),
            Arc::new(FontData::from_static(SYMBOL_FONT)),
        );
        let default_prop: Vec<String> = std::iter::once(SYMBOLS_KEY.to_owned())
            .chain(
                defs.families
                    .get(&FontFamily::Proportional)
                    .cloned()
                    .unwrap_or_default(),
            )
            .collect();
        let default_mono: Vec<String> = std::iter::once(SYMBOLS_KEY.to_owned())
            .chain(
                defs.families
                    .get(&FontFamily::Monospace)
                    .cloned()
                    .unwrap_or_default(),
            )
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
            // Static fonts ignore the axis; variable fonts use it to pick the weight.
            data.tweak = FontTweak {
                coords: VariationCoords::new([(b"wght", f32::from(spec.weight))]),
                ..data.tweak
            };
            defs.font_data.insert(key.clone(), Arc::new(data));
            Some(key)
        };

        let with = |key: Option<String>, fallback: &[String]| -> Vec<String> {
            key.into_iter().chain(fallback.iter().cloned()).collect()
        };
        let body = load(&fonts.body, &mut defs);
        let mono = load(&fonts.mono, &mut defs);
        let heading = load(&fonts.heading, &mut defs);
        let label = load(&fonts.label, &mut defs);
        let readout = load(&fonts.readout, &mut defs);

        let prop = with(body, &default_prop);
        defs.families
            .insert(FontFamily::Monospace, with(mono, &default_mono));
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

    #[test]
    fn themes_without_fonts_still_define_named_families() {
        let (defs, warnings) = FontLibrary::new(None).definitions(&Fonts::default());
        assert!(warnings.is_empty());
        assert!(defs.families.contains_key(&FontFamily::Name(LABEL.into())));
    }
}
