//! Themes for MISO Terminal.
//!
//! A theme is a TOML file of *semantic* slots (background, text, accent,
//! positive, negative, chart series, fuel colours, fonts, spacing). Panels only
//! ever ask for slots, never raw colours, so any theme works with any panel.
//!
//! Built-in themes live in the repository's `themes/` directory and are compiled
//! in. More themes live in `themes/gallery/`: they are not compiled in but
//! published on the docs site to download. Users add or override themes by
//! dropping `*.toml` files into the themes folder under the app's config
//! directory; files with the same `id` as a built-in replace it. This crate is UI-toolkit agnostic; `mt-ui` maps a
//! [`Theme`] onto egui.

mod color;
mod model;
mod registry;

pub use color::{Color, contrast_ratio};
pub use model::*;
pub use registry::{ThemeLoadError, ThemeRegistry, dir_fingerprint};

/// The theme used on first launch and whenever the configured theme is missing.
pub const DEFAULT_THEME_ID: &str = "default";

/// Where the downloadable themes (`themes/gallery/`) are published.
pub const GALLERY_URL: &str = "https://arenkdesai.github.io/miso-terminal/themes.html#gallery";

/// Built-in theme sources, compiled into the binary.
pub const BUILTIN_SOURCES: &[(&str, &str)] = &[
    ("default.toml", include_str!("../../../themes/default.toml")),
    (
        "amber-terminal.toml",
        include_str!("../../../themes/amber-terminal.toml"),
    ),
    (
        "high-contrast.toml",
        include_str!("../../../themes/high-contrast.toml"),
    ),
];

/// Parse the built-in themes. A built-in that fails to parse is a bug caught by
/// the test suite, so at runtime it is skipped rather than fatal.
pub fn builtin() -> Vec<Theme> {
    BUILTIN_SOURCES
        .iter()
        .filter_map(|(name, src)| match Theme::from_toml(src) {
            Ok(t) => Some(t),
            Err(e) => {
                debug_assert!(false, "built-in theme {name} is invalid: {e}");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_and_pass_validation() {
        let themes = builtin();
        assert_eq!(themes.len(), BUILTIN_SOURCES.len());
        assert!(themes.iter().any(|t| t.meta.id == DEFAULT_THEME_ID));
        for t in &themes {
            let errors: Vec<_> = t.validate().into_iter().filter(|i| i.is_error()).collect();
            assert!(errors.is_empty(), "{}: {errors:#?}", t.meta.id);
        }
    }

    #[test]
    fn builtin_ids_are_unique() {
        let mut ids: Vec<_> = builtin().into_iter().map(|t| t.meta.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), BUILTIN_SOURCES.len());
    }

    /// The downloadable themes in `themes/gallery/`: they must load, pass
    /// validation and not collide with each other or with a built-in.
    fn gallery() -> Vec<(String, Theme)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes/gallery");
        let mut out: Vec<(String, Theme)> = std::fs::read_dir(&dir)
            .expect("themes/gallery exists")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .map(|p| {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                let src = std::fs::read_to_string(&p).unwrap();
                let theme = Theme::from_toml(&src).unwrap_or_else(|e| panic!("{name}: {e}"));
                (name, theme)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn gallery_themes_parse_and_pass_validation() {
        let gallery = gallery();
        assert!(!gallery.is_empty());
        let mut ids: Vec<String> = builtin().into_iter().map(|t| t.meta.id).collect();
        for (name, t) in &gallery {
            assert_eq!(
                *name,
                format!("{}.toml", t.meta.id),
                "file name must be the id"
            );
            let errors: Vec<_> = t.validate().into_iter().filter(|i| i.is_error()).collect();
            assert!(errors.is_empty(), "{name}: {errors:#?}");
            assert!(!ids.contains(&t.meta.id), "{name}: id already taken");
            ids.push(t.meta.id.clone());
        }
    }

    #[test]
    fn themes_round_trip_through_toml() {
        for t in builtin()
            .into_iter()
            .chain(gallery().into_iter().map(|(_, t)| t))
        {
            let back = Theme::from_toml(&t.to_toml().unwrap()).unwrap();
            assert_eq!(back, t);
        }
    }
}
