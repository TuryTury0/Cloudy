//! What the assistant is doing right now, as shown on the island (and later on the phone).

use serde::{Deserialize, Serialize};

/// The visible state of the assistant. The UI picks a shape and animation for each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssistantState {
    /// Resting: small pill.
    Idle,
    /// Microphone open, the user is talking.
    Listening,
    /// Waiting for the model.
    Thinking,
    /// The assistant is talking.
    Speaking,
    /// A task is running (desktop control or a background job).
    Working,
    /// The last action finished well.
    Success,
    /// Something failed; the label says what.
    Error,
    /// Text box open for a typed request.
    TextInput,
    /// A written answer is shown as a card.
    Answer,
}

impl AssistantState {
    /// States that end by themselves after a short moment and return to idle.
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Success | Self::Error)
    }

    /// States in which the island needs keyboard focus.
    pub fn needs_keyboard(self) -> bool {
        matches!(self, Self::TextInput)
    }
}

/// One change of the visible state, with optional short texts for the island.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateUpdate {
    pub state: AssistantState,
    /// Main line, for example the assistant name or "Listening".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Second line, for example what was heard or the current task step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl StateUpdate {
    pub fn new(state: AssistantState) -> Self {
        Self { state, title: None, detail: None }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_match_the_ui() {
        let json = serde_json::to_string(&AssistantState::TextInput).unwrap();
        assert_eq!(json, "\"text-input\"");
        let parsed: AssistantState = serde_json::from_str("\"working\"").unwrap();
        assert_eq!(parsed, AssistantState::Working);
    }

    #[test]
    fn transient_and_keyboard_states() {
        assert!(AssistantState::Success.is_transient());
        assert!(!AssistantState::Listening.is_transient());
        assert!(AssistantState::TextInput.needs_keyboard());
        assert!(!AssistantState::Idle.needs_keyboard());
    }

    #[test]
    fn update_skips_empty_texts() {
        let json = serde_json::to_string(&StateUpdate::new(AssistantState::Idle)).unwrap();
        assert_eq!(json, r#"{"state":"idle"}"#);
        let full = StateUpdate::new(AssistantState::Working).with_detail("Opening Spotify");
        assert_eq!(serde_json::to_value(&full).unwrap()["detail"], "Opening Spotify");
    }
}
