//! Tool framework: every tool declares its schema, a short status label and how risky it is, and runs with a
//! context that carries cancellation and a way to ask the user for confirmation.

pub mod apps;
pub mod desktop;
pub mod files;
pub mod memory;
pub mod music;
pub mod planner;
pub mod system;
pub mod web;

use anytask_llm::{Generator, ToolSpec};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Asks the user before something that cannot be undone or reaches outside (overwrite, delete, run, send).
#[async_trait]
pub trait Confirmer: Send + Sync {
    async fn confirm(&self, title: &str, detail: &str) -> bool;
}

/// A second opinion on a yes/no question about a situation (a few sentences, with or without a screenshot): the
/// probability, 0 to 1, that the answer is yes. `None` when the judge is turned off or could not answer; callers
/// then go on as if there were no judge.
#[async_trait]
pub trait Judge: Send + Sync {
    async fn yes_no(&self, state: &str, question: &str, jpeg: Option<&[u8]>) -> Option<f64>;
}

/// Confirmer that always says no (used where no user is present).
pub struct DenyAll;

#[async_trait]
impl Confirmer for DenyAll {
    async fn confirm(&self, _title: &str, _detail: &str) -> bool {
        false
    }
}

pub struct ToolCtx {
    pub cancel: CancellationToken,
    pub confirm: Arc<dyn Confirmer>,
    pub now_ms: i64,
    /// The model answering this request (tools such as desktop control ask it about screenshots).
    pub model: Arc<dyn Generator>,
    /// Reports a short live step ("clicking #12 Play") to the island and chat.
    pub progress: Arc<dyn Fn(String) + Send + Sync>,
    /// Writes a detail line (timings, decisions) to the log.
    pub log: Arc<dyn Fn(String) + Send + Sync>,
    /// What the user said for this request, in their own words. The model rewrites a job for a tool and can drop a
    /// limit on the way ("prepare it but do not send"), so tools that work on their own (desktop_task) read it too.
    pub request: String,
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use anytask_llm::{GenerateRequest, GenerateResponse, LlmError};

    /// A model that is never expected to be asked.
    pub struct NoModel;

    #[async_trait]
    impl Generator for NoModel {
        async fn generate(&self, _: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
            Err(LlmError::Answer("no model in this test".into()))
        }
        fn label(&self) -> String {
            "none".into()
        }
    }

    pub fn ctx(confirm: Arc<dyn Confirmer>) -> ToolCtx {
        ToolCtx { cancel: CancellationToken::new(), confirm, now_ms: 0, model: Arc::new(NoModel), progress: Arc::new(|_| {}), log: Arc::new(|_| {}), request: String::new() }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// Short status line shown on the island and in the chat while the tool runs (Persian, the user's language).
    /// It is also kept in the history as what was done.
    fn label(&self, args: &Value) -> String;
    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String>;
}

#[derive(Default, Clone)]
pub struct Registry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl Registry {
    pub fn add(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.spec().name, tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

/// Reads a string argument.
pub(crate) fn arg_str<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

pub(crate) fn short(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { format!("{}…", flat.chars().take(max).collect::<String>()) }
}
