//! Bringing v1 (the Electron app) over: settings, API keys, models, memory, MCP servers and the wake word.
//!
//! v1 kept its settings and keys in secure.dat (see `secure`), its memory in memory_working.json (recent turns)
//! and memory_longterm.json (facts), and MCP servers in mcp-servers.json (the same format Next uses). Reading
//! never changes v1's files; the desktop app decides what to apply.

pub mod secure;

use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// v1's built-in Gemini model (used with ANYTASK_API_KEY when no custom model existed).
pub const V1_DEFAULT_MODEL: &str = "gemini-3.8-flash";
pub const GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// v1's data folder: %APPDATA%\anytask.
pub fn v1_folder() -> Option<PathBuf> {
    let folder = PathBuf::from(std::env::var_os("APPDATA")?).join("anytask");
    folder.is_dir().then_some(folder)
}

/// A model from v1. The key is only for moving into the keychain (never logged: no Debug).
#[derive(Clone, PartialEq)]
pub struct V1Model {
    pub id: String,
    pub name: String,
    /// "gemini" or "openai".
    pub provider: String,
    pub base_url: String,
    pub model_id: String,
    /// "default", "low", "medium" or "high".
    pub thinking: String,
    pub api_key: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct V1Theme {
    pub name: String,
    pub colors: Vec<String>,
    pub weights: Vec<f32>,
}

#[derive(Default)]
pub struct V1Data {
    pub assistant_name: Option<String>,
    pub user_name: Option<String>,
    pub voice: Option<String>,
    pub theme: Option<V1Theme>,
    pub models: Vec<V1Model>,
    /// (provider, key) for web search.
    pub search: Option<(String, String)>,
    pub wake_model: Option<Value>,
    pub wake_enabled: bool,
    /// (time in ms, what the user said, the answer), oldest first.
    pub turns: Vec<(i64, String, String)>,
    /// (time in ms, key, value).
    pub facts: Vec<(i64, String, String)>,
    /// The "mcpServers" object.
    pub mcp_servers: Map<String, Value>,
    /// Secrets the MCP servers refer to as "secret:NAME", found in secure.dat.
    pub mcp_secrets: BTreeMap<String, String>,
    /// What could not be read (shown to the user).
    pub problems: Vec<String>,
}

/// What was found, without any secret (for the confirmation screen).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct V1Summary {
    pub found: bool,
    pub assistant_name: Option<String>,
    pub user_name: Option<String>,
    pub voice: Option<String>,
    pub theme: Option<String>,
    pub models: Vec<String>,
    pub keys: usize,
    pub search: Option<String>,
    pub wake_word: Option<String>,
    pub turns: usize,
    pub facts: usize,
    pub mcp_servers: Vec<String>,
    pub problems: Vec<String>,
}

impl V1Data {
    pub fn summary(&self) -> V1Summary {
        V1Summary {
            found: true,
            assistant_name: self.assistant_name.clone(),
            user_name: self.user_name.clone(),
            voice: self.voice.clone(),
            theme: self.theme.as_ref().map(|t| t.name.clone()),
            models: self.models.iter().map(|m| format!("{} ({})", m.name, m.model_id)).collect(),
            keys: self.models.iter().filter(|m| !m.api_key.is_empty()).count() + usize::from(self.search.is_some()) + self.mcp_secrets.len(),
            search: self.search.as_ref().map(|(p, _)| p.clone()),
            wake_word: self.wake_model.as_ref().map(|m| m["label"].as_str().filter(|l| !l.is_empty()).unwrap_or("(no label)").to_string()),
            turns: self.turns.len(),
            facts: self.facts.len(),
            mcp_servers: self.mcp_servers.keys().cloned().collect(),
            problems: self.problems.clone(),
        }
    }
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn iso_ms(value: &Value) -> i64 {
    value.as_str().and_then(parse_iso).unwrap_or(0)
}

/// "2026-10-29T14:05:00.000Z" to milliseconds (UTC); enough for v1's own timestamps.
fn parse_iso(s: &str) -> Option<i64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.trim_end_matches('Z');
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, sec) = (t.next()??, t.next()??, t.next()??);
    let ms: i64 = format!("{:0<3}", &frac[..frac.len().min(3)]).parse().ok()?;
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + min) * 60_000 + sec * 1000 + ms)
}

/// Reads everything v1 left in `folder`. Parts that cannot be read are listed in `problems`.
pub fn read(folder: &Path) -> V1Data {
    let mut data = V1Data::default();
    match secure::read(folder) {
        Ok(map) => apply_secure(&mut data, &map, folder),
        Err(e) => data.problems.push(format!("Settings and keys: {e}")),
    }
    read_memory(&mut data, folder);
    data
}

fn apply_secure(data: &mut V1Data, map: &Map<String, Value>, folder: &Path) {
    data.assistant_name = text(map, "ANYTASK_ASSISTANT_NAME");
    data.user_name = text(map, "ANYTASK_USER_NAME");
    data.voice = text(map, "ANYTASK_VOICE_NAME");
    data.theme = text(map, "ANYTASK_THEME_DATA").and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|t| {
        let colors: Vec<String> = t["colors"].as_array()?.iter().filter_map(|c| c.as_str().map(str::to_string)).collect();
        let weights: Vec<f32> = t["weights"].as_array().map(|w| w.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect()).unwrap_or_default();
        (!colors.is_empty()).then(|| V1Theme { name: t["themeName"].as_str().unwrap_or("Custom").to_string(), colors, weights })
    });

    let preferred = text(map, "ANYTASK_PREFERRED_BRAIN");
    for m in map.get("ANYTASK_CUSTOM_MODELS").and_then(Value::as_array).into_iter().flatten() {
        let Some(model) = m.as_object() else { continue };
        let (Some(id), Some(model_id)) = (text(model, "id"), text(model, "modelId")) else { continue };
        let provider = if text(model, "apiType").as_deref() == Some("openai") { "openai" } else { "gemini" };
        let thinking = text(model, "thinkingLevel").filter(|t| ["low", "medium", "high"].contains(&t.as_str())).unwrap_or_else(|| "default".into());
        data.models.push(V1Model {
            is_default: model.get("isDefault").and_then(Value::as_bool).unwrap_or(false) || preferred.as_deref() == Some(id.as_str()),
            name: text(model, "name").unwrap_or_else(|| model_id.clone()),
            base_url: text(model, "baseUrl").unwrap_or_else(|| if provider == "gemini" { GEMINI_URL.into() } else { String::new() }),
            api_key: text(model, "apiKey").unwrap_or_default(),
            provider: provider.into(),
            thinking,
            model_id,
            id,
        });
    }
    // The built-in Gemini key: one model for it, unless a Gemini model already carries that key.
    if let Some(key) = text(map, "ANYTASK_API_KEY")
        && !data.models.iter().any(|m| m.provider == "gemini" && m.api_key == key)
    {
        data.models.push(V1Model {
            id: "v1-gemini".into(),
            name: "Gemini (v1)".into(),
            provider: "gemini".into(),
            base_url: GEMINI_URL.into(),
            model_id: V1_DEFAULT_MODEL.into(),
            thinking: "default".into(),
            api_key: key,
            is_default: false,
        });
    }
    if let Some(key) = text(map, "ANYTASK_SEARCH_KEY") {
        data.search = Some((text(map, "ANYTASK_SEARCH_PROVIDER").unwrap_or_else(|| "tavily".into()), key));
    }
    data.wake_model = map.get("ANYTASK_WAKE_MODEL").filter(|m| m.is_object()).cloned();
    data.wake_enabled = map.get("ANYTASK_WAKE_ENABLED").and_then(Value::as_bool).unwrap_or(false);

    if let Some(servers) = std::fs::read_to_string(folder.join("mcp-servers.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        data.mcp_servers = servers["mcpServers"].as_object().cloned().unwrap_or_default();
        let wanted = data.mcp_servers.values().flat_map(|s| ["env", "headers"].map(|k| s[k].as_object().cloned().unwrap_or_default())).flat_map(|m| m.into_iter().map(|(_, v)| v));
        for value in wanted {
            if let Some(name) = value.as_str().and_then(|v| v.strip_prefix("secret:")).map(str::trim)
                && let Some(secret) = text(map, name)
            {
                data.mcp_secrets.insert(name.to_string(), secret);
            }
        }
    }
}

fn read_memory(data: &mut V1Data, folder: &Path) {
    let load = |name: &str| std::fs::read_to_string(folder.join(name)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    for turn in load("memory_working.json").and_then(|v| v.as_array().cloned()).unwrap_or_default() {
        let (user, reply) = (turn["prompt"].as_str().unwrap_or_default().trim(), turn["reply"].as_str().unwrap_or_default().trim());
        if !user.is_empty() && !reply.is_empty() {
            data.turns.push((iso_ms(&turn["timestamp"]), user.to_string(), reply.to_string()));
        }
    }
    data.turns.sort_by_key(|t| t.0);
    if let Some(facts) = load("memory_longterm.json").and_then(|v| v.as_object().cloned()) {
        for (key, fact) in facts {
            if let Some(value) = fact["value"].as_str().map(str::trim).filter(|v| !v.is_empty()) {
                data.facts.push((iso_ms(&fact["updatedAt"]), key, value.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn iso_times_become_milliseconds() {
        assert_eq!(parse_iso("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso("2026-10-29T14:05:01.250Z"), Some(1_793_282_701_250));
        assert_eq!(parse_iso("nonsense"), None);
    }

    #[test]
    fn settings_models_wake_word_and_mcp_secrets_are_read() {
        let dir = std::env::temp_dir().join(format!("anytask-v1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mcp-servers.json"),
            json!({ "mcpServers": { "github": { "command": "npx", "env": { "TOKEN": "secret:GITHUB_TOKEN" } } } }).to_string(),
        )
        .unwrap();
        let secure = json!({
            "ANYTASK_ASSISTANT_NAME": "Nova", "ANYTASK_USER_NAME": "Sara", "ANYTASK_VOICE_NAME": "Kore",
            "ANYTASK_THEME_DATA": "{\"themeName\":\"Sunset\",\"colors\":[\"#f97316\",\"#ec4899\"],\"weights\":[60,40]}",
            "ANYTASK_API_KEY": "AIza-test-key",
            "ANYTASK_PREFERRED_BRAIN": "m2",
            "ANYTASK_CUSTOM_MODELS": [
                { "id": "m1", "name": "Flash", "apiType": "gemini", "modelId": "gemini-2.5-flash", "baseUrl": "", "apiKey": "AIza-test-key", "thinkingLevel": "low" },
                { "id": "m2", "name": "GapGPT", "apiType": "openai", "modelId": "gpt-5-mini", "baseUrl": "https://api.gapgpt.app/v1", "apiKey": "sk-x" },
                { "name": "broken" }
            ],
            "ANYTASK_WAKE_ENABLED": true,
            "ANYTASK_WAKE_MODEL": { "v": 3, "label": "hey nova" },
            "GITHUB_TOKEN": "ghp_x"
        });
        let mut data = V1Data::default();
        apply_secure(&mut data, secure.as_object().unwrap(), &dir);
        assert_eq!((data.assistant_name.as_deref(), data.user_name.as_deref(), data.voice.as_deref()), (Some("Nova"), Some("Sara"), Some("Kore")));
        assert_eq!(data.theme.as_ref().unwrap().colors.len(), 2);
        assert_eq!(data.models.len(), 2, "the built-in key is already on a Gemini model; the broken one is skipped");
        assert_eq!(data.models[0].base_url, GEMINI_URL);
        assert_eq!(data.models[0].thinking, "low");
        assert!(data.models[1].is_default && data.models[1].provider == "openai");
        assert!(data.wake_enabled);
        assert_eq!(data.mcp_secrets.get("GITHUB_TOKEN").map(String::as_str), Some("ghp_x"));
        let summary = data.summary();
        assert_eq!(summary.keys, 3);
        assert_eq!(summary.wake_word.as_deref(), Some("hey nova"));
        assert!(!serde_json::to_string(&summary).unwrap().contains("AIza"), "the summary never carries keys");

        // Without a custom Gemini model the built-in key gets its own model.
        let only_key = json!({ "ANYTASK_API_KEY": "AIza-only" });
        let mut data = V1Data::default();
        apply_secure(&mut data, only_key.as_object().unwrap(), &dir);
        assert_eq!(data.models.len(), 1);
        assert_eq!(data.models[0].model_id, V1_DEFAULT_MODEL);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn memory_turns_and_facts_are_read_oldest_first() {
        let dir = std::env::temp_dir().join(format!("anytask-v1-mem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("memory_working.json"),
            json!([
                { "prompt": "b", "reply": "2", "timestamp": "2026-10-02T00:00:00.000Z" },
                { "prompt": "a", "reply": "1", "timestamp": "2026-10-01T00:00:00.000Z" },
                { "prompt": "", "reply": "skipped" }
            ])
            .to_string(),
        )
        .unwrap();
        std::fs::write(dir.join("memory_longterm.json"), json!({ "city": { "value": "Tehran", "updatedAt": "2026-10-01T00:00:00.000Z" } }).to_string()).unwrap();
        let data = read(&dir);
        assert_eq!(data.turns.iter().map(|t| t.1.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(data.facts, vec![(parse_iso("2026-10-01T00:00:00.000Z").unwrap(), "city".to_string(), "Tehran".to_string())]);
        assert_eq!(data.problems.len(), 1, "no secure.dat here");
        let _ = std::fs::remove_dir_all(dir);
    }
}
