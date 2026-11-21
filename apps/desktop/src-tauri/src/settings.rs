//! User settings stored as JSON in the app config folder.
//! (Moves to SQLite with the rest of the storage in a later milestone; the shape stays the same.)

use anytask_llm::ModelConfig;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Global keyboard shortcuts. Ctrl+Alt combinations by default so no normal app key is taken over
/// (v1 used F5-F9, which broke refresh in every browser).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Hotkeys {
    pub talk: String,
    pub type_text: String,
    pub toggle_island: String,
    pub stop: String,
    pub open_settings: String,
    pub open_chat: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            talk: "Ctrl+Alt+Space".into(),
            type_text: "Ctrl+Alt+T".into(),
            toggle_island: "Ctrl+Alt+H".into(),
            stop: "Ctrl+Alt+X".into(),
            open_settings: "Ctrl+Alt+S".into(),
            open_chat: "Ctrl+Alt+C".into(),
        }
    }
}

impl Hotkeys {
    /// (action name, shortcut) pairs, in a fixed order.
    pub fn entries(&self) -> [(&'static str, &str); 6] {
        [
            ("talk", self.talk.as_str()),
            ("type", self.type_text.as_str()),
            ("toggle-island", self.toggle_island.as_str()),
            ("stop", self.stop.as_str()),
            ("settings", self.open_settings.as_str()),
            ("chat", self.open_chat.as_str()),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemeSettings {
    pub name: String,
    pub colors: Vec<String>,
    pub weights: Vec<f32>,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            name: "Cloudy".into(),
            colors: vec!["#38bdf8".into(), "#ffffff".into(), "#bae6fd".into()],
            weights: vec![45.0, 35.0, 20.0],
        }
    }
}

/// Voice conversations (Gemini Live; the key comes from a Gemini model in Models).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VoiceSettings {
    pub model: String,
    /// Prebuilt Gemini voice.
    pub voice: String,
    /// The microphone closes after this long without speech.
    pub quiet_seconds: u32,
    /// Written answers are read aloud too (with the voice model).
    pub speak_answers: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self { model: "gemini-3.8-live".into(), voice: "Aoede".into(), quiet_seconds: 20, speak_answers: true }
    }
}

/// The user's proxy (a VPN app such as v2rayN). Some services only answer through it, others only without it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProxySettings {
    /// "127.0.0.1:10808" (SOCKS5, as v2rayN uses) or a full address such as http://127.0.0.1:10809; empty = none.
    pub address: String,
    /// Ids of the models whose requests go through the proxy.
    pub models: Vec<String>,
    /// The voice (Gemini Live) connects through the proxy.
    pub voice: bool,
    /// Web pages and searches that fail straight out are tried once more through the proxy.
    pub web: bool,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self { address: "127.0.0.1:10808".into(), models: Vec::new(), voice: false, web: true }
    }
}

impl ProxySettings {
    /// The full proxy URL, or None when no address is set.
    pub fn url(&self) -> Option<String> {
        anytask_llm::proxy::proxy_url(&self.address)
    }

    /// The proxy for the voice connection, when the voice goes through it.
    pub fn voice_url(&self) -> Option<String> {
        self.url().filter(|_| self.voice)
    }
}

/// A decision model for the second check (Liquid's decisions API: d1-ultra and the other d1 models).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct DecisionModel {
    pub id: String,
    pub name: String,
    /// The decisions endpoint, e.g. https://api.liquid.ai/decisions/v1/systemone.
    pub base_url: String,
    pub model_id: String,
}

/// The second check: a decision model looks again before the screen agent's "done" and before a click that is hard
/// to undo. Off by default; its keys are in the keychain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct DecisionSettings {
    pub enabled: bool,
    pub models: Vec<DecisionModel>,
    /// Id of the model in use; empty means the first one.
    pub active: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// The app's language: "en" (default) or "fa".
    pub language: String,
    pub assistant_name: String,
    pub user_name: String,
    pub hotkeys: Hotkeys,
    pub theme: ThemeSettings,
    /// Island visible on start.
    pub show_island: bool,
    /// Start Cloudy when the user signs in to Windows (HKCU Run; see autostart.rs).
    pub start_with_windows: bool,
    /// Download a new version from GitHub and install it when no job is running (see updater.rs).
    pub auto_update: bool,
    /// Stop all animations (an app choice; the Windows animation switch is not followed, see tokens.css).
    pub reduce_motion: bool,
    /// Models the user added (their API keys are in the OS keychain, not here).
    pub models: Vec<ModelConfig>,
    /// Id of the model that answers; empty means the first one.
    pub active_model: String,
    /// Model that takes over when the chosen one is busy (overloaded, rate limited); empty = none.
    pub backup_model: String,
    pub voice: VoiceSettings,
    pub proxy: ProxySettings,
    pub decision: DecisionSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "en".into(),
            assistant_name: "Cloudy".into(),
            user_name: String::new(),
            hotkeys: Hotkeys::default(),
            theme: ThemeSettings::default(),
            show_island: true,
            start_with_windows: false,
            auto_update: true,
            reduce_motion: false,
            models: Vec::new(),
            active_model: String::new(),
            backup_model: String::new(),
            voice: VoiceSettings::default(),
            proxy: ProxySettings::default(),
            decision: DecisionSettings::default(),
        }
    }
}

impl Settings {
    /// The model that answers: the chosen one, or the first one when the choice was removed.
    pub fn active_model(&self) -> Option<&ModelConfig> {
        self.models.iter().find(|m| m.id == self.active_model).or_else(|| self.models.first())
    }

    /// Reads settings; a missing or broken file gives the defaults (a broken file is kept aside, not lost).
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Self>(&text) {
                Ok(mut settings) => {
                    if settings.assistant_name == "AnyTask" {
                        settings.assistant_name = "Cloudy".into();
                    }
                    settings
                }
                Err(_) => {
                    let _ = fs::rename(path, path.with_extension("broken.json"));
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Writes atomically (temp file + rename) so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp: PathBuf = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(io::Error::other)?)?;
        fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anytask-settings-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("settings.json")
    }

    #[test]
    fn missing_file_gives_defaults() {
        assert_eq!(Settings::load(&temp_file("missing")), Settings::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let path = temp_file("roundtrip");
        let settings = Settings {
            assistant_name: "Liara".into(),
            hotkeys: Hotkeys { talk: "Ctrl+Shift+Space".into(), ..Hotkeys::default() },
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
    }

    #[test]
    fn broken_file_is_kept_aside_and_defaults_used() {
        let path = temp_file("broken");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        assert!(path.with_extension("broken.json").exists());
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let path = temp_file("partial");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"assistantName":"Nova","hotkeys":{"talk":"Ctrl+Alt+V"}}"#).unwrap();
        let settings = Settings::load(&path);
        assert_eq!(settings.assistant_name, "Nova");
        assert_eq!(settings.hotkeys.talk, "Ctrl+Alt+V");
        assert_eq!(settings.hotkeys.stop, Hotkeys::default().stop);
        assert!(settings.show_island);
    }

    #[test]
    fn default_hotkeys_avoid_function_keys() {
        for (_, key) in Hotkeys::default().entries() {
            assert!(key.starts_with("Ctrl+Alt+"), "{key}");
        }
    }
}