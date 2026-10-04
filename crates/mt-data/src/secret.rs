//! Secrets (API keys) and where they are kept.
//!
//! Keys live in the operating system's credential store (Windows Credential
//! Manager), never in `config.toml`, saved state or logs. A [`Secret`] wipes its
//! memory when dropped and prints as `***`.

use std::collections::HashMap;
use std::fmt;

use parking_lot::Mutex;
use zeroize::Zeroizing;

/// A secret string. `Debug` never shows it; read it with [`Secret::expose`]
/// only where it goes on the wire.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("credential store: {0}")]
pub struct SecretError(pub String);

/// Somewhere to keep secrets, by name (`alpaca/paper/key-id`).
pub trait SecretStore: Send + Sync + fmt::Debug {
    /// `Ok(None)` when nothing is stored under `name`.
    fn get(&self, name: &str) -> Result<Option<Secret>, SecretError>;
    fn set(&self, name: &str, value: &Secret) -> Result<(), SecretError>;
    /// Removing a missing entry is not an error.
    fn delete(&self, name: &str) -> Result<(), SecretError>;
    /// Where the secrets are, for the settings panel.
    fn describe(&self) -> String;
}

/// Secrets in memory only: tests, offline replay, and platforms without a store.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    values: Mutex<HashMap<String, Secret>>,
}

impl MemorySecrets {
    pub fn with(entries: impl IntoIterator<Item = (impl Into<String>, Secret)>) -> Self {
        Self {
            values: Mutex::new(entries.into_iter().map(|(k, v)| (k.into(), v)).collect()),
        }
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<Secret>, SecretError> {
        Ok(self.values.lock().get(name).cloned())
    }

    fn set(&self, name: &str, value: &Secret) -> Result<(), SecretError> {
        self.values.lock().insert(name.to_owned(), value.clone());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.values.lock().remove(name);
        Ok(())
    }

    fn describe(&self) -> String {
        "memory (not saved)".into()
    }
}

/// Windows Credential Manager, through the keyring crates. Each secret is a
/// generic credential named `<prefix>/<name>` with local-machine persistence
/// (it does not roam with a domain profile).
#[cfg(windows)]
pub struct CredentialManager {
    prefix: String,
    store: std::sync::Arc<windows_native_keyring_store::Store>,
}

#[cfg(windows)]
impl CredentialManager {
    /// `prefix` names the application, e.g. `miso-terminal`.
    pub fn new(prefix: &str) -> Result<Self, SecretError> {
        let store = windows_native_keyring_store::Store::new().map_err(platform)?;
        Ok(Self {
            prefix: prefix.to_owned(),
            store,
        })
    }

    fn entry(&self, name: &str) -> Result<keyring_core::Entry, SecretError> {
        use keyring_core::api::CredentialStoreApi;
        let target = format!("{}/{name}", self.prefix);
        let modifiers = HashMap::from([("target", target.as_str()), ("persistence", "Local")]);
        self.store
            .build(&self.prefix, name, Some(&modifiers))
            .map_err(platform)
    }
}

#[cfg(windows)]
fn platform(e: keyring_core::Error) -> SecretError {
    SecretError(e.to_string())
}

#[cfg(windows)]
impl fmt::Debug for CredentialManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CredentialManager({})", self.prefix)
    }
}

#[cfg(windows)]
impl SecretStore for CredentialManager {
    fn get(&self, name: &str) -> Result<Option<Secret>, SecretError> {
        match self.entry(name)?.get_password() {
            Ok(v) => Ok(Some(Secret::new(v))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(platform(e)),
        }
    }

    fn set(&self, name: &str, value: &Secret) -> Result<(), SecretError> {
        self.entry(name)?
            .set_password(value.expose())
            .map_err(platform)
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(platform(e)),
        }
    }

    fn describe(&self) -> String {
        format!("Windows Credential Manager ({}/…)", self.prefix)
    }
}

/// The platform's credential store, or memory where there is none (secrets
/// then last for the session only, which the settings panel says).
pub fn os_store(prefix: &str) -> std::sync::Arc<dyn SecretStore> {
    #[cfg(windows)]
    match CredentialManager::new(prefix) {
        Ok(store) => return std::sync::Arc::new(store),
        Err(e) => tracing::warn!("no credential store ({e}); secrets will not be saved"),
    }
    let _ = prefix;
    std::sync::Arc::new(MemorySecrets::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print() {
        let s = Secret::new("PKSECRET");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(s.expose(), "PKSECRET");
    }

    #[test]
    fn memory_store_round_trips() {
        let store = MemorySecrets::default();
        assert_eq!(store.get("a/b").unwrap(), None);
        store.set("a/b", &Secret::new("x")).unwrap();
        assert_eq!(store.get("a/b").unwrap().unwrap().expose(), "x");
        store.delete("a/b").unwrap();
        store.delete("a/b").unwrap();
        assert_eq!(store.get("a/b").unwrap(), None);
    }

    /// Writes, reads and deletes a throwaway entry in the real Credential
    /// Manager. Some CI sessions have no credential store; that is reported,
    /// not failed.
    #[cfg(windows)]
    #[test]
    fn credential_manager_round_trips() {
        let store = match CredentialManager::new("miso-terminal-test") {
            Ok(s) => s,
            Err(e) => return eprintln!("skipped: {e}"),
        };
        let name = format!("probe/{}", std::process::id());
        if let Err(e) = store.set(&name, &Secret::new("value 1")) {
            return eprintln!("skipped: {e}");
        }
        assert_eq!(store.get(&name).unwrap().unwrap().expose(), "value 1");
        store.set(&name, &Secret::new("value 2")).unwrap();
        assert_eq!(store.get(&name).unwrap().unwrap().expose(), "value 2");
        store.delete(&name).unwrap();
        assert_eq!(store.get(&name).unwrap(), None);
        store.delete(&name).unwrap();
    }
}
