//! API keys for cloud providers (FR-PRV-04): stored in the OS keyring,
//! managed from Settings. The UI never gets a key back, only whether one is
//! saved and its last 4 characters.

use std::sync::Arc;

use jifar_providers::{
    ADDIS_AI_ACCOUNT, ActionKind, AddisScribe, ApiKey, ApiKeyStore, KeyStoreError, KeyringStore,
    ProviderError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Addis AI: one key for Scribe (transcription) and the notes LLM.
    AddisAi,
}

impl Provider {
    /// Keyring account name.
    pub fn account(self) -> &'static str {
        match self {
            Self::AddisAi => ADDIS_AI_ACCOUNT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyStatus {
    pub saved: bool,
    /// Last 4 characters of the saved key.
    pub hint: Option<String>,
    /// Set when the keyring can't be used at all (e.g. no Secret Service).
    pub keyring_error: Option<String>,
}

/// Result of "Test": a working key, or what's wrong in plain terms.
#[derive(Debug, Clone, PartialEq)]
pub enum TestOutcome {
    Ok {
        balance: Option<f64>,
        currency: Option<String>,
    },
    NoKey,
    InvalidKey(String),
    NoCredits(String),
    /// Network down or the service busy; the key may be fine.
    Unreachable(String),
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("the key is empty")]
    Empty,
    #[error("{0}")]
    Keyring(#[from] KeyStoreError),
}

pub struct ProviderKeys {
    store: Arc<dyn ApiKeyStore>,
    /// Test override for the provider's server.
    base_url: Option<String>,
}

impl Default for ProviderKeys {
    fn default() -> Self {
        Self::new(Arc::new(KeyringStore))
    }
}

impl ProviderKeys {
    pub fn new(store: Arc<dyn ApiKeyStore>) -> Self {
        Self {
            store,
            base_url: None,
        }
    }

    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = Some(url.to_owned());
        self
    }

    pub fn status(&self, provider: Provider) -> KeyStatus {
        match self.store.get(provider.account()) {
            Ok(key) => KeyStatus {
                saved: key.is_some(),
                hint: key.map(|k| k.hint()),
                keyring_error: None,
            },
            Err(e) => KeyStatus {
                saved: false,
                hint: None,
                keyring_error: Some(e.to_string()),
            },
        }
    }

    pub fn set(&self, provider: Provider, key: &str) -> Result<KeyStatus, KeyError> {
        let key = ApiKey::new(key).ok_or(KeyError::Empty)?;
        self.store.set(provider.account(), &key)?;
        Ok(self.status(provider))
    }

    pub fn delete(&self, provider: Provider) -> Result<KeyStatus, KeyError> {
        self.store.delete(provider.account())?;
        Ok(self.status(provider))
    }

    /// Checks the saved key against the provider. Free (no transcription).
    pub async fn test(&self, provider: Provider) -> TestOutcome {
        let key = match self.store.get(provider.account()) {
            Ok(Some(k)) => k,
            Ok(None) => return TestOutcome::NoKey,
            Err(e) => return TestOutcome::Failed(e.to_string()),
        };
        let client = match AddisScribe::with_key(key) {
            Ok(c) => c,
            Err(e) => return TestOutcome::Failed(e.to_string()),
        };
        let client = match &self.base_url {
            Some(url) => client.with_base_url(url),
            None => client,
        };
        match client.usage().await {
            Ok(u) => TestOutcome::Ok {
                balance: u.balance,
                currency: u.currency,
            },
            Err(ProviderError::ActionRequired {
                kind: ActionKind::Auth,
                message,
            }) => TestOutcome::InvalidKey(message),
            Err(ProviderError::ActionRequired {
                kind: ActionKind::Credits,
                message,
            }) => TestOutcome::NoCredits(message),
            Err(ProviderError::Retryable { message, .. }) => TestOutcome::Unreachable(message),
            Err(ProviderError::Permanent { message }) => TestOutcome::Failed(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use jifar_providers::MemoryKeyStore;
    use wiremock::matchers::{header, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn keys() -> ProviderKeys {
        ProviderKeys::new(Arc::new(MemoryKeyStore::default()))
    }

    #[test]
    fn set_status_delete_without_revealing_the_key() {
        let k = keys();
        assert!(!k.status(Provider::AddisAi).saved);
        let s = k.set(Provider::AddisAi, " sk_live_abcd1234 \n").unwrap();
        assert_eq!(
            s,
            KeyStatus {
                saved: true,
                hint: Some("1234".into()),
                keyring_error: None
            }
        );
        assert!(matches!(
            k.set(Provider::AddisAi, "  "),
            Err(KeyError::Empty)
        ));
        assert!(!k.delete(Provider::AddisAi).unwrap().saved);
    }

    #[tokio::test]
    async fn test_reports_balance_or_what_is_wrong() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/scribe/usage"))
            .and(header("x-api-key", "sk_good"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"data":{"balance":50,"currency":"ETB"}}"#),
            )
            .mount(&server)
            .await;
        Mock::given(path("/api/v1/scribe/usage"))
            .respond_with(ResponseTemplate::new(401).set_body_string("{}"))
            .mount(&server)
            .await;
        let k = keys().with_base_url(&server.uri());

        assert_eq!(k.test(Provider::AddisAi).await, TestOutcome::NoKey);
        k.set(Provider::AddisAi, "sk_good").unwrap();
        assert_eq!(
            k.test(Provider::AddisAi).await,
            TestOutcome::Ok {
                balance: Some(50.0),
                currency: Some("ETB".into())
            }
        );
        k.set(Provider::AddisAi, "sk_bad").unwrap();
        assert!(matches!(
            k.test(Provider::AddisAi).await,
            TestOutcome::InvalidKey(_)
        ));

        let offline = keys().with_base_url("http://127.0.0.1:9");
        offline.set(Provider::AddisAi, "sk_good").unwrap();
        assert!(matches!(
            offline.test(Provider::AddisAi).await,
            TestOutcome::Unreachable(_)
        ));
    }
}
