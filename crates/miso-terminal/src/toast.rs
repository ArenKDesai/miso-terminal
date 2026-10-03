//! Windows toast notifications for alerts.
//!
//! An unpackaged app's toasts are attributed through an AppUserModelID
//! registered under `HKCU\Software\Classes\AppUserModelId`, with the name and
//! icon Windows shows on them. That per-user key is written the first time a
//! toast is shown; `install.ps1 -Uninstall` removes it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use mt_ui::notify::Notifier;

/// How Windows knows this app's notifications.
pub const APP_USER_MODEL_ID: &str = "MisoTerminal.Desktop";

/// A notifier, or `None` where toasts are unavailable.
pub fn notifier(state_dir: &Path) -> Option<Arc<dyn Notifier>> {
    #[cfg(windows)]
    {
        Some(Arc::new(Toasts {
            icon: state_dir.join("miso-terminal.png"),
            registered: OnceLock::new(),
        }))
    }
    #[cfg(not(windows))]
    {
        let _ = state_dir;
        None
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
struct Toasts {
    /// A copy of the app icon on disk: toasts reference images by path.
    icon: PathBuf,
    registered: OnceLock<bool>,
}

#[cfg(windows)]
impl Toasts {
    /// Write the icon and the AppUserModelID key once per run.
    fn register(&self) -> bool {
        *self.registered.get_or_init(|| {
            if !self.icon.exists() {
                let png = include_bytes!("../../../assets/icon/miso-terminal.png");
                if let Err(e) = std::fs::write(&self.icon, png) {
                    tracing::warn!("could not write the notification icon: {e}");
                }
            }
            let key = format!(r"Software\Classes\AppUserModelId\{APP_USER_MODEL_ID}");
            let result = windows_registry::CURRENT_USER.create(&key).and_then(|k| {
                k.set_string("DisplayName", "MISO Terminal")?;
                k.set_string("IconUri", self.icon.to_string_lossy().as_ref())
            });
            match result {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!("could not register for notifications: {e}");
                    false
                }
            }
        })
    }
}

#[cfg(windows)]
impl Notifier for Toasts {
    fn notify(&self, title: &str, body: &str) {
        if !self.register() {
            return;
        }
        let (title, body, icon) = (title.to_owned(), body.to_owned(), self.icon.clone());
        // Showing a toast is a WinRT round trip; keep it off the UI thread.
        std::thread::spawn(move || {
            use tauri_winrt_notification::{Duration, IconCrop, Sound, Toast};
            let mut toast = Toast::new(APP_USER_MODEL_ID)
                .title(&title)
                .text1(&body)
                .sound(Some(Sound::Default))
                .duration(Duration::Short);
            if icon.exists() {
                toast = toast.icon(&icon, IconCrop::Square, "MISO Terminal");
            }
            if let Err(e) = toast.show() {
                tracing::warn!("notification failed: {e}");
            }
        });
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Shows a real notification, so it only runs on request:
    /// `cargo test -p miso-terminal toast -- --ignored`
    #[test]
    #[ignore = "shows a Windows notification"]
    fn shows_a_test_notification() {
        let dir = std::env::temp_dir().join("mt-toast-test");
        std::fs::create_dir_all(&dir).unwrap();
        let n = notifier(&dir).expect("toasts on Windows");
        n.notify(
            "MISO Terminal",
            "Alert notifications are working. This is a test.",
        );
        // The toast is shown from a background thread.
        std::thread::sleep(std::time::Duration::from_secs(3));
        let key = windows_registry::CURRENT_USER
            .open(format!(
                r"Software\Classes\AppUserModelId\{APP_USER_MODEL_ID}"
            ))
            .unwrap();
        assert_eq!(key.get_string("DisplayName").unwrap(), "MISO Terminal");
    }
}
