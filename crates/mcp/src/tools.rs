//! MCP tools as brain tools: names the models accept, arguments back in the server's shape, results as text.

use crate::McpManager;
use anytask_core::{Tool, ToolCtx};
use anytask_llm::ToolSpec;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

const NAME_CHARS: usize = 64;
const DESCRIPTION_CHARS: usize = 300;
const RESULT_CHARS: usize = 20_000;

pub(crate) struct McpTool {
    pub(crate) manager: Arc<McpManager>,
    pub(crate) exposed: String,
    pub(crate) server: String,
    pub(crate) tool: String,
    pub(crate) description: String,
    pub(crate) schema: Value,
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        let description = format!("[MCP {}] {}", self.server, if self.description.trim().is_empty() { &self.tool } else { &self.description });
        let description: String = description.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(DESCRIPTION_CHARS).collect();
        let parameters = if self.schema.is_object() { self.schema.clone() } else { serde_json::json!({ "type": "object", "properties": {} }) };
        ToolSpec { name: self.exposed.clone(), description, parameters }
    }

    fn label(&self, _args: &Value) -> String {
        format!("{}: {}", self.server, self.tool)
    }

    async fn run(&self, args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        self.manager.call(&self.server, &self.tool, args, &self.schema).await
    }
}

fn safe(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

/// "mcp__<server>__<tool>", only letters, digits and _, at most 64 characters, never one already `taken`.
pub fn exposed_name(server: &str, tool: &str, taken: &[String]) -> String {
    let base: String = format!("mcp__{}__{}", safe(server), safe(tool)).chars().take(NAME_CHARS).collect();
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{}_{n}", base.chars().take(NAME_CHARS - 3).collect::<String>());
        n += 1;
    }
    name
}

/// Free-form object parameters reach Gemini as JSON text (it cannot take them as objects); parse them back.
pub fn restore_json_args(args: Value, schema: &Value) -> Value {
    let (Value::Object(mut args), Some(properties)) = (args.clone(), schema.get("properties").and_then(Value::as_object)) else { return args };
    for (key, property) in properties {
        let free_object = property.get("type").and_then(Value::as_str) == Some("object") && property.get("properties").and_then(Value::as_object).is_none_or(|p| p.is_empty());
        if free_object
            && let Some(Value::String(text)) = args.get(key)
            && let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(text)
        {
            args.insert(key.clone(), parsed);
        }
    }
    Value::Object(args)
}

/// A tool result (as JSON) as plain text for the model: text parts, placeholders for images and files.
pub fn result_to_text(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for item in result.get("content").and_then(Value::as_array).into_iter().flatten() {
        let kind = item["type"].as_str().unwrap_or("unknown");
        parts.push(match kind {
            "text" => item["text"].as_str().unwrap_or_default().to_string(),
            "image" => "[image]".into(),
            "audio" => "[audio]".into(),
            "resource" => item["resource"]["text"].as_str().map(str::to_string).unwrap_or_else(|| format!("[resource {}]", item["resource"]["uri"].as_str().unwrap_or("?"))),
            "resource_link" => format!("[link {}]", item["uri"].as_str().unwrap_or("?")),
            other => format!("[{other}]"),
        });
    }
    if parts.is_empty()
        && let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null())
    {
        parts.push(structured.to_string());
    }
    let text = parts.join("\n");
    if text.chars().count() > RESULT_CHARS { format!("{}\n...[cut]", text.chars().take(RESULT_CHARS).collect::<String>()) } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_are_safe_short_and_unique() {
        assert_eq!(exposed_name("git-hub", "create.issue", &[]), "mcp__git_hub__create_issue");
        let long = exposed_name("server", &"x".repeat(100), &[]);
        assert_eq!(long.chars().count(), 64);
        let second = exposed_name("server", &"x".repeat(100), std::slice::from_ref(&long));
        assert_ne!(second, long);
        assert!(second.ends_with("_2") && second.chars().count() <= 64);
    }

    #[test]
    fn json_text_for_free_objects_is_parsed_back() {
        let schema = json!({ "type": "object", "properties": { "options": { "type": "object" }, "item": { "type": "string" } } });
        let args = json!({ "options": "{\"fast\":true}", "item": "{\"not\":\"parsed\"}" });
        let out = restore_json_args(args, &schema);
        assert_eq!(out["options"], json!({ "fast": true }));
        assert_eq!(out["item"], "{\"not\":\"parsed\"}");
    }

    #[test]
    fn results_become_text() {
        let result = json!({ "content": [
            { "type": "text", "text": "gold = 262" },
            { "type": "image", "data": "...", "mimeType": "image/png" },
            { "type": "resource", "resource": { "uri": "file:///a.txt" } }
        ] });
        assert_eq!(result_to_text(&result), "gold = 262\n[image]\n[resource file:///a.txt]");
        assert_eq!(result_to_text(&json!({ "content": [], "structuredContent": { "a": 1 } })), "{\"a\":1}");
        assert!(result_to_text(&json!({ "content": [{ "type": "text", "text": "y".repeat(30_000) }] })).ends_with("...[cut]"));
    }
}
