//! Settings > About > "Bring over from v1": shows what v1 left on this PC, then copies it in when the user asks.
//! Keys go to the OS keychain; nothing of v1 is changed. A marker file keeps it from being imported twice.

use crate::brain::{Brain, search_key_name};
use crate::commands::{SettingsStore, apply_settings, current};
use crate::models;
use crate::wake::WakeService;
use anytask_llm::{ModelConfig, Provider, Thinking};
use anytask_migrate::{V1Summary, v1_folder};
use anytask_storage::model_key_name;
use anytask_web::SearchProvider;
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct V1Report {
    pub summary: V1Summary,
    /// When it was imported before (ms), if it was.
    pub imported_at: Option<i64>,
}

fn marker(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("v1-import.json"))
}

fn imported_at(app: &AppHandle) -> Option<i64> {
    let text = std::fs::read_to_string(marker(app)?).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()?["at"].as_i64()
}

pub fn report(app: &AppHandle) -> V1Report {
    let summary = v1_folder().map(|f| anytask_migrate::read(&f).summary()).unwrap_or_default();
    V1Report { summary, imported_at: imported_at(app) }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Copies v1's settings, models and keys, memory, MCP servers and wake word. Returns lines saying what was done.
pub async fn import(app: &AppHandle) -> Result<Vec<String>, String> {
    let folder = v1_folder().ok_or("No AnyTask v1 data was found on this PC.")?;
    let v1 = anytask_migrate::read(&folder);
    let brain = app.state::<Brain>();
    let mut done = Vec::new();

    // Settings and models (keys into the keychain).
    let store = app.state::<SettingsStore>();
    let mut settings = current(&store);
    let had_models = !settings.models.is_empty();
    if let Some(name) = v1.assistant_name.clone().filter(|_| settings.assistant_name == "Cloudy" || settings.assistant_name == "AnyTask") {
        settings.assistant_name = name;
    }
    if let Some(name) = v1.user_name.clone().filter(|_| settings.user_name.is_empty()) {
        settings.user_name = name;
    }
    if let Some(voice) = v1.voice.clone() {
        settings.voice.voice = voice;
    }
    if let Some(theme) = v1.theme.clone() {
        settings.theme.name = theme.name;
        settings.theme.colors = theme.colors;
        if !theme.weights.is_empty() {
            settings.theme.weights = theme.weights;
        }
    }
    let mut added = 0;
    let mut default_id = None;
    for m in &v1.models {
        let provider = if m.provider == "openai" { Provider::Openai } else { Provider::Gemini };
        let duplicate = settings.models.iter().any(|e| e.provider == provider && e.model_id == m.model_id && e.base_url.trim_end_matches('/') == m.base_url.trim_end_matches('/'));
        if duplicate {
            continue;
        }
        let thinking = match m.thinking.as_str() {
            "low" => Thinking::Low,
            "medium" => Thinking::Medium,
            "high" => Thinking::High,
            _ => Thinking::Default,
        };
        let config = ModelConfig { id: format!("v1-{}", m.id), name: m.name.clone(), provider, base_url: m.base_url.clone(), model_id: m.model_id.clone(), thinking };
        let saved = models::upsert(&mut settings, config, now_ms());
        if !m.api_key.is_empty() {
            brain.secrets.set(&model_key_name(&saved.id), &m.api_key).map_err(|e| format!("Could not save a key in the keychain: {e}"))?;
        }
        if m.is_default {
            default_id = Some(saved.id.clone());
        }
        added += 1;
    }
    if !had_models && let Some(id) = default_id {
        settings.active_model = id;
    }
    apply_settings(app, &store, settings)?;
    done.push(format!("{added} models"));

    if let Some((provider, key)) = &v1.search {
        let provider = if provider == "brave" { SearchProvider::Brave } else { SearchProvider::Tavily };
        if brain.secrets.get(search_key_name(provider)).is_none() {
            let _ = brain.secrets.set(search_key_name(provider), key);
            done.push("search key".into());
        }
    }

    // Memory: the conversation and facts.
    let memory = brain.agent.memory();
    for (at, user, assistant) in &v1.turns {
        let _ = memory.add_turn(*at, user, assistant, &[]);
    }
    for (at, key, value) in &v1.facts {
        let _ = memory.set_fact(*at, key, value);
    }
    done.push(format!("{} conversation turns, {} facts", v1.turns.len(), v1.facts.len()));

    // MCP servers that Next does not have yet, with the secrets they name.
    let mut config = brain.mcp.config().unwrap_or_default();
    let mut servers = 0;
    for (name, server) in &v1.mcp_servers {
        if config.mcp_servers.contains_key(name) {
            continue;
        }
        if let Ok(server) = serde_json::from_value(server.clone()) {
            config.mcp_servers.insert(name.clone(), server);
            servers += 1;
        }
    }
    for (name, secret) in &v1.mcp_secrets {
        let key = crate::mcp::secret_key_name(name);
        if brain.secrets.get(&key).is_none() {
            let _ = brain.secrets.set(&key, secret);
        }
    }
    if servers > 0 {
        config.save(&brain.mcp.path)?;
        brain.mcp.reload(app).await;
    }
    done.push(format!("{servers} MCP servers"));

    // The wake word (same saved format), unless Next already has one.
    let wake = app.state::<WakeService>();
    if let Some(model) = v1.wake_model.as_ref().and_then(anytask_wake::WakeModel::from_json)
        && !wake.status().has_model
    {
        wake.set_model(app, model, v1.wake_enabled)?;
        done.push("wake word".into());
    }

    if let Some(path) = marker(app) {
        let _ = std::fs::write(path, json!({ "at": now_ms(), "done": done }).to_string());
    }
    Ok(done)
}