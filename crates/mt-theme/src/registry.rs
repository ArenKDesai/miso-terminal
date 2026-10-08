use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::{DEFAULT_THEME_ID, Theme, builtin};

/// A user theme file that could not be loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeLoadError {
    pub path: PathBuf,
    pub message: String,
}

/// Built-in themes plus any `*.toml` themes in a user directory. User themes
/// with a built-in's `id` replace it.
#[derive(Clone, Debug)]
pub struct ThemeRegistry {
    themes: Vec<Theme>,
    errors: Vec<ThemeLoadError>,
    user_dir: Option<PathBuf>,
}

impl ThemeRegistry {
    pub fn load(user_dir: Option<&Path>) -> Self {
        let mut reg = Self {
            themes: builtin(),
            errors: Vec::new(),
            user_dir: user_dir.map(Path::to_path_buf),
        };
        if let Some(dir) = user_dir {
            reg.load_user_dir(dir);
        }
        reg
    }

    /// Re-read the user directory (for hot reload).
    pub fn reload(&mut self) {
        *self = Self::load(self.user_dir.as_deref());
    }

    fn load_user_dir(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
            })
            .collect();
        paths.sort();
        for path in paths {
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|src| Theme::from_toml(&src).map_err(|e| e.to_string()));
            match parsed {
                Ok(theme) => {
                    let errors: Vec<String> = theme
                        .validate()
                        .into_iter()
                        .filter(super::model::Issue::is_error)
                        .map(|i| i.message)
                        .collect();
                    if !errors.is_empty() {
                        // Still load it (the author is probably iterating), but say why it looks wrong.
                        self.errors.push(ThemeLoadError {
                            path: path.clone(),
                            message: errors.join("; "),
                        });
                    }
                    self.themes.retain(|t| t.meta.id != theme.meta.id);
                    self.themes.push(theme);
                }
                Err(message) => self.errors.push(ThemeLoadError { path, message }),
            }
        }
    }

    pub fn themes(&self) -> &[Theme] {
        &self.themes
    }

    pub fn errors(&self) -> &[ThemeLoadError] {
        &self.errors
    }

    pub fn user_dir(&self) -> Option<&Path> {
        self.user_dir.as_deref()
    }

    pub fn get(&self, id: &str) -> Option<&Theme> {
        self.themes
            .iter()
            .find(|t| t.meta.id.eq_ignore_ascii_case(id))
    }

    /// `id` if it exists, else the default theme, else the first theme.
    pub fn resolve(&self, id: &str) -> &Theme {
        self.get(id)
            .or_else(|| self.get(DEFAULT_THEME_ID))
            .or_else(|| self.themes.first())
            .expect("at least one built-in theme")
    }
}

/// A cheap fingerprint of a directory's `*.toml` files (names, sizes, mtimes),
/// polled to hot-reload themes while someone edits them.
pub fn dir_fingerprint(dir: &Path) -> u64 {
    let mut h = DefaultHasher::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut items: Vec<_> = entries
            .flatten()
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((e.file_name(), m.len(), m.modified().ok()))
            })
            .collect();
        items.sort();
        items.hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_themes_override_builtins_and_report_errors() {
        let dir = std::env::temp_dir().join(format!("mt-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fp0 = dir_fingerprint(&dir);

        let mut custom = builtin()
            .into_iter()
            .find(|t| t.meta.id == DEFAULT_THEME_ID)
            .unwrap();
        custom.meta.name = "My Default".into();
        std::fs::write(dir.join("mine.toml"), custom.to_toml().unwrap()).unwrap();
        std::fs::write(dir.join("broken.toml"), "[meta]\nid = 'x'").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let reg = ThemeRegistry::load(Some(&dir));
        assert_eq!(reg.resolve(DEFAULT_THEME_ID).meta.name, "My Default");
        assert_eq!(
            reg.themes().len(),
            builtin().len(),
            "override replaces, not adds"
        );
        assert_eq!(reg.errors().len(), 1);
        assert!(reg.errors()[0].path.ends_with("broken.toml"));
        assert_eq!(reg.resolve("does-not-exist").meta.id, DEFAULT_THEME_ID);
        assert_ne!(dir_fingerprint(&dir), fp0);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
