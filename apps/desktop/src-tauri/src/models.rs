//! Adding, changing and removing models in the settings. API keys never pass through here; they go straight to
//! the OS keychain.

use crate::settings::Settings;
use anytask_llm::ModelConfig;

pub fn validate(model: &ModelConfig) -> Result<(), String> {
    if model.name.trim().is_empty() {
        return Err("Give the model a name.".into());
    }
    if model.model_id.trim().is_empty() {
        return Err("Enter the model id (for example gemini-2.5-flash).".into());
    }
    let url = model.base_url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() < 10 {
        return Err("The base URL must start with https:// (or http:// for a local server).".into());
    }
    Ok(())
}

/// Adds the model or replaces the one with the same id; a new model gets an id and becomes active if none is.
pub fn upsert(settings: &mut Settings, mut model: ModelConfig, now_ms: i64) -> ModelConfig {
    model.name = model.name.trim().to_string();
    model.model_id = model.model_id.trim().to_string();
    model.base_url = model.base_url.trim().trim_end_matches('/').to_string();
    if model.id.trim().is_empty() {
        model.id = format!("m{now_ms:x}");
    }
    match settings.models.iter_mut().find(|m| m.id == model.id) {
        Some(existing) => *existing = model.clone(),
        None => settings.models.push(model.clone()),
    }
    if settings.models.iter().all(|m| m.id != settings.active_model) {
        settings.active_model = model.id.clone();
    }
    model
}

/// Removes a model; if it was active, the first remaining one takes over.
pub fn remove(settings: &mut Settings, id: &str) -> bool {
    let before = settings.models.len();
    settings.models.retain(|m| m.id != id);
    if settings.active_model == id {
        settings.active_model = settings.models.first().map(|m| m.id.clone()).unwrap_or_default();
    }
    settings.models.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use anytask_llm::{Provider, Thinking};

    fn model(id: &str) -> ModelConfig {
        ModelConfig {
            id: id.into(),
            name: " Gemini ".into(),
            provider: Provider::Gemini,
            base_url: "https://generativelanguage.googleapis.com/v1beta/".into(),
            model_id: "gemini-2.5-flash".into(),
            thinking: Thinking::Default,
        }
    }

    #[test]
    fn validation_explains_what_is_missing() {
        assert!(validate(&model("a")).is_ok());
        assert!(validate(&ModelConfig { name: " ".into(), ..model("a") }).unwrap_err().contains("name"));
        assert!(validate(&ModelConfig { base_url: "ftp://x".into(), ..model("a") }).unwrap_err().contains("https://"));
        assert!(validate(&ModelConfig { model_id: "".into(), ..model("a") }).unwrap_err().contains("model id"));
    }

    #[test]
    fn first_model_becomes_active_and_edits_replace() {
        let mut settings = Settings::default();
        let saved = upsert(&mut settings, model(""), 255);
        assert_eq!(saved.id, "mff");
        assert_eq!(saved.name, "Gemini");
        assert_eq!(saved.base_url, "https://generativelanguage.googleapis.com/v1beta");
        assert_eq!(settings.active_model, "mff");

        upsert(&mut settings, model("second"), 1);
        assert_eq!(settings.active_model, "mff", "adding another model keeps the choice");
        upsert(&mut settings, ModelConfig { model_id: "gemini-2.5-pro".into(), ..model("second") }, 2);
        assert_eq!(settings.models.len(), 2);
        assert_eq!(settings.models[1].model_id, "gemini-2.5-pro");
    }

    #[test]
    fn removing_the_active_model_picks_the_next() {
        let mut settings = Settings::default();
        upsert(&mut settings, model("a"), 0);
        upsert(&mut settings, model("b"), 0);
        assert!(remove(&mut settings, "a"));
        assert_eq!(settings.active_model, "b");
        assert_eq!(settings.active_model().unwrap().id, "b");
        assert!(!remove(&mut settings, "zzz"));
        remove(&mut settings, "b");
        assert!(settings.active_model().is_none());
    }
}
