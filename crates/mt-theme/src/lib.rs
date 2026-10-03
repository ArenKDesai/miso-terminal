//! Themes for MISO Terminal.
//!
//! A theme is a TOML file of *semantic* slots (background, text, accent,
//! positive, negative, chart series, fuel colours, fonts, spacing). Panels only
//! ever ask for slots, never raw colours, so any theme works with any panel.
//!
//! Built-in themes live in the repository's `themes/` directory and are compiled
//! in. Users add or override themes by dropping `*.toml` files into the themes
//! folder under the app's config directory; files with the same `id` as a
//! built-in replace it. This crate is UI-toolkit agnostic; `mt-ui` maps a
//! [`Theme`] onto egui.

mod color;
mod model;
mod registry;

pub use color::{Color, contrast_ratio};
pub use model::*;
pub use registry::{ThemeLoadError, ThemeRegistry, dir_fingerprint};

/// The theme used on first launch and whenever the configured theme is missing.
pub const DEFAULT_THEME_ID: &str = "everforge-dark";

/// Built-in theme sources, compiled into the binary.
pub const BUILTIN_SOURCES: &[(&str, &str)] = &[
    (
        "everforge-dark.toml",
        include_str!("../../../themes/everforge-dark.toml"),
    ),
    (
        "everforge-light.toml",
        include_str!("../../../themes/everforge-light.toml"),
    ),
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

    #[test]
    fn themes_round_trip_through_toml() {
        for t in builtin() {
            let back = Theme::from_toml(&t.to_toml().unwrap()).unwrap();
            assert_eq!(back, t);
        }
    }
}
