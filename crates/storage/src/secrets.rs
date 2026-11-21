//! API keys and tokens. On the desktop they live in the OS keychain (Windows Credential Manager), never in
//! settings files or logs.

use std::collections::HashMap;
use std::sync::Mutex;

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

/// Secrets in the OS keychain under one service name ("AnyTask").
pub struct Keychain {
    service: String,
}

impl Keychain {
    pub fn new(service: impl Into<String>) -> Self {
        Self { service: service.into() }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(&self.service, name).map_err(|e| e.to_string())
    }
}

impl SecretStore for Keychain {
    fn get(&self, name: &str) -> Option<String> {
        self.entry(name).ok()?.get_password().ok().filter(|v| !v.is_empty())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.entry(name)?.set_password(value).map_err(|e| e.to_string())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// In-memory store for tests and the phone bridge.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, name: &str) -> Option<String> {
        self.0.lock().ok()?.get(name).cloned()
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.remove(name);
        Ok(())
    }
}

/// Name under which a model's key is stored.
pub fn model_key_name(model_id: &str) -> String {
    format!("model:{model_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip() {
        let s = MemorySecrets::default();
        assert_eq!(s.get("a"), None);
        s.set("a", "1").unwrap();
        assert_eq!(s.get("a").as_deref(), Some("1"));
        s.delete("a").unwrap();
        assert_eq!(s.get("a"), None);
    }

    /// Touches the real Windows Credential Manager: run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let k = Keychain::new("AnyTask-test");
        k.set("probe", "secret-value").unwrap();
        assert_eq!(k.get("probe").as_deref(), Some("secret-value"));
        k.delete("probe").unwrap();
        assert_eq!(k.get("probe"), None);
        k.delete("probe").unwrap();
    }
}
