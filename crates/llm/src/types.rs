//! Provider-neutral conversation types. Gemini and OpenAI-compatible formats are converted to and from these.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Google Gemini `generateContent`.
    Gemini,
    /// OpenAI-compatible `chat/completions` (OpenAI, GapGPT, OpenRouter, local servers...).
    Openai,
}

/// How much the model may think before answering. Default = think briefly (fast and still precise).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    #[default]
    Default,
    Low,
    Medium,
    High,
}

/// A model the user added. The API key is not part of it (it lives in the OS keychain).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    pub base_url: String,
    pub model_id: String,
    #[serde(default)]
    pub thinking: Thinking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Model,
}

/// Ids we invent for tool calls that came without one (Gemini often omits them). They are never sent back.
pub const LOCAL_ID_PREFIX: &str = "local-";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Part {
    Text {
        text: String,
        /// Gemini "thought signature": must be sent back unchanged with the same part.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        args: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolResult {
        id: String,
        name: String,
        result: Value,
    },
    /// A picture for the model (a screenshot), base64 encoded.
    Image {
        mime: String,
        data: String,
    },
}

impl Part {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into(), signature: None }
    }

    pub fn image(mime: impl Into<String>, data: impl Into<String>) -> Self {
        Self::Image { mime: mime.into(), data: data.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: Role::User, parts: vec![Part::text(text)] }
    }

    pub fn model(text: impl Into<String>) -> Self {
        Self { role: Role::Model, parts: vec![Part::text(text)] }
    }

    /// All text parts joined.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// A tool the model may call. `parameters` is a JSON Schema object (lowercase types).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// False on the last round: the model must answer in text.
    pub allow_tools: bool,
    pub max_output_tokens: u32,
    /// Overrides the model's thinking setting for this request (desktop steps use Low: answer at once).
    pub thinking: Option<Thinking>,
}

/// Tokens one request used, as the provider reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt: system text, history, tool results, screenshots.
    pub input: u64,
    /// The answer.
    pub output: u64,
    /// Thinking before the answer (billed like output).
    pub thinking: u64,
    /// The part of `input` the provider took from its cache (billed at a fraction).
    #[serde(default)]
    pub cached: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.thinking
    }

    pub fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.output += other.output;
        self.thinking += other.thinking;
        self.cached += other.cached;
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GenerateResponse {
    pub parts: Vec<Part>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

impl GenerateResponse {
    pub fn text(&self) -> String {
        Message { role: Role::Model, parts: self.parts.clone() }.text().trim().to_string()
    }

    pub fn tool_calls(&self) -> Vec<(&str, &str, &Value)> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall { id, name, args, .. } => Some((id.as_str(), name.as_str(), args)),
                _ => None,
            })
            .collect()
    }
}
