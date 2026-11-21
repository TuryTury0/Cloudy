//! Memory tools: save a fact, recall past conversations and facts.

use super::{Tool, ToolCtx, arg_str, short};
use crate::lang::tr;
use anytask_llm::ToolSpec;
use anytask_storage::Memory;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct RememberFact {
    pub memory: Arc<Memory>,
}

#[async_trait]
impl Tool for RememberFact {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "remember_fact".into(),
            description: "Saves a durable fact about the user (a preference, a name, a routine) for future conversations. Never store passwords or secrets.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "key": { "type": "string", "description": "Short key, e.g. favorite_editor." }, "value": { "type": "string" } },
                "required": ["key", "value"]
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("Remembering", "به خاطر سپردن"), short(arg_str(args, "key").unwrap_or(""), 30))
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let key = arg_str(&args, "key").ok_or("key is required.")?;
        let value = arg_str(&args, "value").ok_or("value is required.")?;
        if key.chars().count() > 100 || value.chars().count() > 2000 {
            return Err("The fact is too long.".into());
        }
        self.memory.set_fact(ctx.now_ms, key, value).map_err(|e| e.to_string())?;
        Ok(json!({ "saved": true }))
    }
}

pub struct RecallMemory {
    pub memory: Arc<Memory>,
}

#[async_trait]
impl Tool for RecallMemory {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "recall_memory".into(),
            description: "Returns the latest conversation turns (newest first) and the best matches from past conversations and saved facts. \
                          When the user refers to something they just mentioned ('that song', 'what I said'), the NEWEST matching item wins."
                .into(),
            parameters: json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }),
        }
    }

    fn label(&self, _args: &Value) -> String {
        tr("Looking through memory", "مرور حافظه").into()
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let query = arg_str(&args, "query").unwrap_or("");
        let recall = self.memory.recall(query, 5, ctx.now_ms).map_err(|e| e.to_string())?;
        serde_json::to_value(recall).map_err(|e| e.to_string())
    }
}
