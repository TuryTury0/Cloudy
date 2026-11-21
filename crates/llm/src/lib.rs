//! Model access for AnyTask: Gemini and OpenAI-compatible APIs behind one set of types, with tool calling.

pub mod backup;
pub mod client;
pub mod gemini;
pub mod live;
pub mod openai;
pub mod proxy;
pub mod schema;
pub mod types;

pub use backup::WithBackup;
pub use client::{ConfiguredModel, Generator, LlmClient, LlmError};
pub use live::{LiveConfig, LiveEvent, LiveSession};
pub use types::{GenerateRequest, GenerateResponse, Message, ModelConfig, Part, Provider, Role, Thinking, ToolSpec, Usage};
