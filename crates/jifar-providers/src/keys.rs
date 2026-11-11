//! API keys in the OS keyring (FR-PRV-04), never in settings or the repo.
//!
//! On Linux this is the Secret Service (GNOME Keyring / KWallet). Keys are
//! wrapped in [`ApiKey`], whose `Debug` doesn't print the secret, so they
//! can't leak into logs.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

/// Keyring service name for all Jifar secrets.
pub const SERVICE: &str = "jifar";

/// A secret API key. `Debug` and `Display` print only a redacted hint.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Trims whitespace (pasted keys often end in a newline). `None` if empty.
    pub fn new(key: &str) -> Option<Self> {
        let k = key.trim();
        (!k.is_empty()).then(|| Self(k.to_owned()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tail: String = self
            .0
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        write!(f, "ApiKey(…{tail})")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    #[error("no system keyring available: {0}")]
    Unavailable(String),
    #[error("keyring error: {0}")]
    Backend(String),
}

/// Where provider keys live. `account` is the provider ID (`addis-scribe`).
pub trait ApiKeyStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<ApiKey>, KeyStoreError>;
    fn set(&self, account: &str, key: &ApiKey) -> Result<(), KeyStoreError>;
    fn delete(&self, account: &str) -> Result<(), KeyStoreError>;
}

/// The OS keyring.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringStore;

impl KeyringStore {
    fn entry(account: &str) -> Result<keyring::Entry, KeyStoreError> {
        keyring::Entry::new(SERVICE, account).map_err(|e| match e {
            keyring::Error::NoDefaultStore => KeyStoreError::Unavailable(e.to_string()),
            other => KeyStoreError::Backend(other.to_string()),
        })
    }
}

impl ApiKeyStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<ApiKey>, KeyStoreError> {
        match Self::entry(account)?.get_password() {
            Ok(k) => Ok(ApiKey::new(&k)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(KeyStoreError::Backend(e.to_string())),
        }
    }

    fn set(&self, account: &str, key: &ApiKey) -> Result<(), KeyStoreError> {
        Self::entry(account)?
            .set_password(key.expose())
            .map_err(|e| KeyStoreError::Backend(e.to_string()))
    }

    fn delete(&self, account: &str) -> Result<(), KeyStoreError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(KeyStoreError::Backend(e.to_string())),
        }
    }
}

/// In-memory store for tests and the browser preview.
#[derive(Debug, Default)]
pub struct MemoryKeyStore(Mutex<HashMap<String, ApiKey>>);

impl ApiKeyStore for MemoryKeyStore {
    fn get(&self, account: &str) -> Result<Option<ApiKey>, KeyStoreError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| KeyStoreError::Backend("poisoned".into()))?
            .get(account)
            .cloned())
    }

    fn set(&self, account: &str, key: &ApiKey) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .map_err(|_| KeyStoreError::Backend("poisoned".into()))?
            .insert(account.to_owned(), key.clone());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .map_err(|_| KeyStoreError::Backend("poisoned".into()))?
            .remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_trimmed_and_never_printed() {
        let k = ApiKey::new("  sk_live_abcdef123456\n").unwrap();
        assert_eq!(k.expose(), "sk_live_abcdef123456");
        let dbg = format!("{k:?}");
        assert_eq!(dbg, "ApiKey(…3456)");
        assert!(!dbg.contains("abcdef"));
        assert!(ApiKey::new("   ").is_none());
    }

    #[test]
    fn memory_store_roundtrip() {
        let s = MemoryKeyStore::default();
        assert!(s.get("addis-scribe").unwrap().is_none());
        s.set("addis-scribe", &ApiKey::new("sk_1").unwrap())
            .unwrap();
        assert_eq!(s.get("addis-scribe").unwrap().unwrap().expose(), "sk_1");
        s.delete("addis-scribe").unwrap();
        assert!(s.get("addis-scribe").unwrap().is_none());
    }
}
