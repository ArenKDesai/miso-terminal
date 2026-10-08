//! The theme gallery: themes that do not ship with the terminal but are
//! published with the docs site (from `themes/gallery/` in the repository), and
//! installing them into the user themes folder.
//!
//! The docs build writes an index of the gallery, `themes/index.json`, holding
//! each theme's file verbatim. The app fetches it (`mt-ui`'s gallery query),
//! [`parse_index`] turns it into themes, and [`install`] writes one into the
//! themes folder, where the registry picks it up like any user theme.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Theme, is_valid_id};

/// Where the docs site publishes the gallery index.
pub const GALLERY_INDEX_URL: &str = "https://arenkdesai.github.io/miso-terminal/themes/index.json";

/// The newest index format this build understands.
pub const INDEX_VERSION: u32 = 1;

#[derive(Deserialize)]
struct IndexRaw {
    version: u32,
    themes: Vec<EntryRaw>,
}

#[derive(Deserialize)]
struct EntryRaw {
    /// The theme file, exactly as it is installed.
    toml: String,
}

/// A theme the gallery offers, with the file that installs it.
#[derive(Clone, Debug, PartialEq)]
pub struct GalleryTheme {
    pub theme: Theme,
    pub source: String,
}

/// The gallery as published.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gallery {
    pub themes: Vec<GalleryTheme>,
    /// Entries left out because they do not load or are unreadable, and why.
    pub skipped: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GalleryError {
    #[error("not a gallery index: {0}")]
    Index(#[from] serde_json::Error),
    #[error("the gallery index is version {0}; update the terminal to read it")]
    Version(u32),
}

/// Parse a gallery index. Entries that do not load, have an unusable id or
/// fail the contrast errors are skipped (and listed), not fatal.
pub fn parse_index(body: &str) -> Result<Gallery, GalleryError> {
    let raw: IndexRaw = serde_json::from_str(body)?;
    if raw.version > INDEX_VERSION {
        return Err(GalleryError::Version(raw.version));
    }
    let mut gallery = Gallery::default();
    for (i, entry) in raw.themes.into_iter().enumerate() {
        let theme = match Theme::from_toml(&entry.toml) {
            Ok(t) => t,
            Err(e) => {
                gallery.skipped.push(format!("entry {i}: {e}"));
                continue;
            }
        };
        let errors: Vec<String> = theme
            .validate()
            .into_iter()
            .filter(super::model::Issue::is_error)
            .map(|issue| issue.message)
            .collect();
        if !errors.is_empty() {
            gallery
                .skipped
                .push(format!("{}: {}", theme.meta.id, errors.join("; ")));
            continue;
        }
        gallery.themes.push(GalleryTheme {
            theme,
            source: entry.toml,
        });
    }
    Ok(gallery)
}

/// The file a gallery theme installs to.
pub fn install_path(dir: &Path, id: &str) -> std::io::Result<PathBuf> {
    // The id becomes a file name, and it came over the network.
    if !is_valid_id(id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{id:?} is not a valid theme id"),
        ));
    }
    Ok(dir.join(format!("{id}.toml")))
}

/// Write a gallery theme into the themes folder as `<id>.toml`, replacing any
/// file of that name.
pub fn install(theme: &GalleryTheme, dir: &Path) -> std::io::Result<PathBuf> {
    let path = install_path(dir, &theme.theme.meta.id)?;
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, &theme.source)?;
    Ok(path)
}

/// Remove an installed theme's `<id>.toml` from the themes folder.
pub fn uninstall(id: &str, dir: &Path) -> std::io::Result<PathBuf> {
    let path = install_path(dir, id)?;
    std::fs::remove_file(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ThemeRegistry, builtin};

    fn entry(theme: &Theme) -> serde_json::Value {
        serde_json::json!({ "id": theme.meta.id, "toml": theme.to_toml().unwrap() })
    }

    /// The recorded index (offline mode, smoke tests) is what the docs build
    /// publishes, so it must match `themes/gallery/` file for file.
    #[test]
    fn recorded_index_matches_the_gallery_folder() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let body = std::fs::read_to_string(
            root.join("fixtures/arenkdesai.github.io/miso-terminal/themes/index.json"),
        )
        .unwrap();
        let gallery = parse_index(&body).unwrap();
        assert!(gallery.skipped.is_empty(), "{:?}", gallery.skipped);
        let mut files: Vec<(String, String)> = std::fs::read_dir(root.join("themes/gallery"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .map(|p| {
                let id = p.file_stem().unwrap().to_string_lossy().into_owned();
                (id, std::fs::read_to_string(&p).unwrap())
            })
            .collect();
        files.sort();
        let recorded: Vec<(String, String)> = gallery
            .themes
            .into_iter()
            .map(|g| (g.theme.meta.id, g.source))
            .collect();
        assert!(
            recorded == files,
            "the recorded gallery index is stale: run `uv run tools/build_docs.py site --update-fixture`"
        );
    }

    #[test]
    fn index_parses_and_skips_bad_entries() {
        let mut good = builtin().remove(0);
        good.meta.id = "from-gallery".into();
        let mut unreadable = good.clone();
        unreadable.meta.id = "unreadable".into();
        unreadable.palette.text = unreadable.palette.surface;
        let body = serde_json::json!({
            "version": 1,
            "themes": [entry(&good), entry(&unreadable), { "toml": "not toml [" }],
        })
        .to_string();
        let gallery = parse_index(&body).unwrap();
        assert_eq!(gallery.themes.len(), 1);
        assert_eq!(gallery.themes[0].theme, good);
        assert_eq!(gallery.skipped.len(), 2, "{:?}", gallery.skipped);

        let newer = serde_json::json!({ "version": 2, "themes": [] }).to_string();
        assert!(matches!(parse_index(&newer), Err(GalleryError::Version(2))));
        assert!(parse_index("<html>").is_err());
    }

    #[test]
    fn install_and_uninstall_round_trip_through_the_registry() {
        let dir = std::env::temp_dir().join(format!("mt-gallery-test-{}", std::process::id()));
        let mut theme = builtin().remove(0);
        theme.meta.id = "from-gallery".into();
        let g = GalleryTheme {
            source: theme.to_toml().unwrap(),
            theme,
        };
        let path = install(&g, &dir).unwrap();
        assert_eq!(path, dir.join("from-gallery.toml"));
        assert_eq!(
            ThemeRegistry::load(Some(&dir)).get("from-gallery"),
            Some(&g.theme)
        );
        uninstall("from-gallery", &dir).unwrap();
        assert!(
            ThemeRegistry::load(Some(&dir))
                .get("from-gallery")
                .is_none()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ids_cannot_escape_the_themes_folder() {
        let dir = Path::new("themes");
        for id in ["../evil", "..", "a/b", r"a\b", "", "-x", "C:evil", "Évil"] {
            assert!(install_path(dir, id).is_err(), "{id:?}");
        }
        assert!(install_path(dir, "tokyo-night").is_ok());
    }
}
