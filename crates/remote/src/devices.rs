//! Paired phones, saved as JSON (names and public keys only; nothing secret).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub name: String,
    /// The phone's public key (base64); it identifies the phone.
    pub key: String,
    pub paired_at: i64,
    pub last_seen: i64,
}

#[derive(Debug, Default)]
pub struct DeviceStore {
    path: Option<PathBuf>,
    devices: Vec<Device>,
}

impl DeviceStore {
    /// Loads the file (missing or damaged = no devices).
    pub fn load(path: PathBuf) -> Self {
        let devices = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { path: Some(path), devices }
    }

    /// Kept in memory only (tests).
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Vec<Device> {
        self.devices.clone()
    }

    pub fn get(&self, key: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.key == key)
    }

    pub fn add(&mut self, name: &str, key: &str, now: i64) {
        self.devices.retain(|d| d.key != key);
        let name: String = name.trim().chars().take(60).collect();
        self.devices.push(Device { name: if name.is_empty() { "Phone".into() } else { name }, key: key.to_string(), paired_at: now, last_seen: now });
        self.save();
    }

    pub fn seen(&mut self, key: &str, now: i64) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.key == key) {
            d.last_seen = now;
            self.save();
        }
    }

    pub fn remove(&mut self, key: &str) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.key != key);
        let removed = self.devices.len() != before;
        if removed {
            self.save();
        }
        removed
    }

    fn save(&self) {
        if let Some(path) = &self.path
            && let Ok(text) = serde_json::to_string_pretty(&self.devices)
        {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }
}
