//! Google Gemini `generateContent` request/response format.

use crate::schema;
use crate::types::{GenerateRequest, GenerateResponse, LOCAL_ID_PREFIX, Message, Part, Role, ToolSpec, Usage};
use serde_json::{Map, Value, json};

/// Tools in Gemini's function-declaration form (also used by the Live API).
pub fn function_declarations(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let mut d = json!({ "name": t.name, "description": t.description });
            if schema::has_properties(&t.parameters) {
                d["parameters"] = schema::to_gemini(&t.parameters);
            }
            d
        })
        .collect()
}

/// How a request asks the model to think.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingConfig {
    /// `thinkingBudget` in tokens (Gemini 2.x; 0 = answer without thinking).
    Budget(i64),
    /// `thinkingLevel`: "minimal", "low", "medium" or "high" (Gemini 3 and later).
    Level(&'static str),
}

impl ThinkingConfig {
    /// Output tokens to add for thinking, so the answer still fits.
    fn room(self) -> i64 {
        match self {
            Self::Budget(budget) => budget.max(0),
            // A level is not a hard cap like a budget: keep generous room so the answer is not cut off.
            Self::Level("minimal") => 512,
            Self::Level("low") => 1024,
            Self::Level("medium") => 4096,
            Self::Level(_) => 16384,
        }
    }
}

impl std::fmt::Display for ThinkingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Budget(budget) => write!(f, "thinkingBudget {budget}"),
            Self::Level(level) => write!(f, "thinkingLevel \"{level}\""),
        }
    }
}

/// Builds the request body. `thinking` = None leaves thinking settings out entirely
/// (some models reject them).
pub fn build_body(req: &GenerateRequest, thinking: Option<ThinkingConfig>) -> Value {
    let contents: Vec<Value> = req.messages.iter().map(content).collect();
    let mut body = Map::new();
    if !req.system.is_empty() {
        body.insert("systemInstruction".into(), json!({ "parts": [{ "text": req.system }] }));
    }
    body.insert("contents".into(), Value::Array(contents));

    if !req.tools.is_empty() {
        body.insert("tools".into(), json!([{ "functionDeclarations": function_declarations(&req.tools) }]));
        if !req.allow_tools {
            body.insert("toolConfig".into(), json!({ "functionCallingConfig": { "mode": "NONE" } }));
        }
    }

    let mut generation = json!({ "maxOutputTokens": req.max_output_tokens });
    if let Some(thinking) = thinking {
        generation["thinkingConfig"] = match thinking {
            ThinkingConfig::Budget(budget) => json!({ "thinkingBudget": budget }),
            ThinkingConfig::Level(level) => json!({ "thinkingLevel": level }),
        };
        // Thinking tokens count against the output limit; keep room for the answer.
        generation["maxOutputTokens"] = json!(i64::from(req.max_output_tokens) + thinking.room());
    }
    body.insert("generationConfig".into(), generation);
    Value::Object(body)
}

fn content(message: &Message) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Model => "model",
    };
    let parts: Vec<Value> = message
        .parts
        .iter()
        .map(|part| match part {
            Part::Text { text, signature } => {
                let mut p = json!({ "text": text });
                if let Some(sig) = signature {
                    p["thoughtSignature"] = json!(sig);
                }
                p
            }
            Part::ToolCall { id, name, args, signature } => {
                let mut call = json!({ "name": name, "args": args });
                if !id.starts_with(LOCAL_ID_PREFIX) {
                    call["id"] = json!(id);
                }
                let mut p = json!({ "functionCall": call });
                if let Some(sig) = signature {
                    p["thoughtSignature"] = json!(sig);
                }
                p
            }
            Part::ToolResult { id, name, result } => {
                let mut response = json!({ "name": name, "response": { "result": result } });
                if !id.starts_with(LOCAL_ID_PREFIX) {
                    response["id"] = json!(id);
                }
                json!({ "functionResponse": response })
            }
            Part::Image { mime, data } => json!({ "inlineData": { "mimeType": mime, "data": data } }),
        })
        .collect();
    json!({ "role": role, "parts": parts })
}

/// Reads the first candidate. Thought summaries are skipped; signatures are kept for the replay.
pub fn parse_response(body: &Value) -> Result<GenerateResponse, String> {
    let Some(candidate) = body.get("candidates").and_then(|c| c.get(0)) else {
        let reason = body.pointer("/promptFeedback/blockReason").and_then(Value::as_str).unwrap_or("no answer");
        return Err(format!("The model returned no answer ({reason})."));
    };
    let mut parts = Vec::new();
    let mut local = 0;
    for part in candidate.pointer("/content/parts").and_then(Value::as_array).into_iter().flatten() {
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let signature = part.get("thoughtSignature").and_then(Value::as_str).map(str::to_string);
        if let Some(call) = part.get("functionCall") {
            let id = call.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| {
                local += 1;
                format!("{LOCAL_ID_PREFIX}{local}")
            });
            parts.push(Part::ToolCall {
                id,
                name: call.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                args: call.get("args").cloned().unwrap_or_else(|| json!({})),
                signature,
            });
        } else if let Some(text) = part.get("text").and_then(Value::as_str) {
            parts.push(Part::Text { text: text.to_string(), signature });
        }
    }
    let count = |name: &str| body.pointer(&format!("/usageMetadata/{name}")).and_then(Value::as_u64).unwrap_or(0);
    let usage = body.get("usageMetadata").map(|_| Usage {
        input: count("promptTokenCount"),
        output: count("candidatesTokenCount"),
        thinking: count("thoughtsTokenCount"),
        cached: count("cachedContentTokenCount"),
    });
    Ok(GenerateResponse { parts, finish_reason: candidate.get("finishReason").and_then(Value::as_str).map(str::to_string), usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolSpec;

    fn request() -> GenerateRequest {
        GenerateRequest {
            system: "Be brief.".into(),
            messages: vec![
                Message::user("price of gold?"),
                Message {
                    role: Role::Model,
                    parts: vec![Part::ToolCall { id: "local-1".into(), name: "web_search".into(), args: json!({"query": "gold"}), signature: Some("sig".into()) }],
                },
                Message { role: Role::User, parts: vec![Part::ToolResult { id: "local-1".into(), name: "web_search".into(), result: json!("262,073,000") }] },
            ],
            tools: vec![ToolSpec {
                name: "web_search".into(),
                description: "Search".into(),
                parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
            }],
            allow_tools: false,
            max_output_tokens: 1000,
            thinking: None,
        }
    }

    #[test]
    fn body_has_system_tools_history_and_thinking() {
        let body = build_body(&request(), Some(ThinkingConfig::Budget(512)));
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "Be brief.");
        assert_eq!(body["tools"][0]["functionDeclarations"][0]["parameters"]["type"], "OBJECT");
        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "NONE");
        let call = &body["contents"][1]["parts"][0];
        assert_eq!(call["functionCall"]["name"], "web_search");
        assert!(call["functionCall"].get("id").is_none(), "invented ids are not sent");
        assert_eq!(call["thoughtSignature"], "sig");
        assert_eq!(body["contents"][2]["parts"][0]["functionResponse"]["response"]["result"], "262,073,000");
        assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingBudget"], 512);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 1512);
    }

    #[test]
    fn a_thinking_level_is_sent_as_a_string_with_room_for_the_answer() {
        let body = build_body(&request(), Some(ThinkingConfig::Level("minimal")));
        assert_eq!(body["generationConfig"]["thinkingConfig"], json!({ "thinkingLevel": "minimal" }));
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 1512);
        let body = build_body(&request(), Some(ThinkingConfig::Level("low")));
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 2024);
        let body = build_body(&request(), Some(ThinkingConfig::Level("medium")));
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 5096);
        let body = build_body(&request(), Some(ThinkingConfig::Level("high")));
        assert_eq!(body["generationConfig"]["thinkingConfig"], json!({ "thinkingLevel": "high" }));
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 17384);
    }

    #[test]
    fn thinking_can_be_left_out() {
        let body = build_body(&request(), None);
        assert!(body["generationConfig"].get("thinkingConfig").is_none());
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 1000);
    }

    #[test]
    fn parses_text_calls_and_skips_thoughts() {
        let body = json!({ "candidates": [{ "finishReason": "STOP", "content": { "parts": [
            { "text": "thinking...", "thought": true },
            { "functionCall": { "name": "read_web_page", "args": { "url": "https://tgju.org" } }, "thoughtSignature": "abc" },
            { "text": "Checking." }
        ]}}]});
        let r = parse_response(&body).unwrap();
        assert_eq!(r.parts.len(), 2);
        assert_eq!(r.tool_calls()[0].1, "read_web_page");
        assert!(r.tool_calls()[0].0.starts_with(LOCAL_ID_PREFIX));
        assert_eq!(r.text(), "Checking.");
        assert_eq!(r.finish_reason.as_deref(), Some("STOP"));
    }

    #[test]
    fn blocked_prompt_is_an_error() {
        let err = parse_response(&json!({ "promptFeedback": { "blockReason": "SAFETY" } })).unwrap_err();
        assert!(err.contains("SAFETY"));
    }

    #[test]
    fn images_become_inline_data() {
        let req = GenerateRequest {
            system: String::new(),
            messages: vec![Message { role: Role::User, parts: vec![Part::text("look"), Part::image("image/jpeg", "QUJD")] }],
            tools: vec![],
            allow_tools: false,
            max_output_tokens: 100,
            thinking: None,
        };
        let body = build_body(&req, None);
        assert_eq!(body["contents"][0]["parts"][1]["inlineData"]["mimeType"], "image/jpeg");
        assert_eq!(body["contents"][0]["parts"][1]["inlineData"]["data"], "QUJD");
    }

    #[test]
    fn reads_token_usage() {
        let body = json!({
            "candidates": [{ "content": { "parts": [{ "text": "ok" }] } }],
            "usageMetadata": { "promptTokenCount": 1770, "candidatesTokenCount": 12, "thoughtsTokenCount": 40, "totalTokenCount": 1822, "cachedContentTokenCount": 1024 }
        });
        let usage = parse_response(&body).unwrap().usage.unwrap();
        assert_eq!(usage, Usage { input: 1770, output: 12, thinking: 40, cached: 1024 });
        assert_eq!(usage.total(), 1822);
    }
}
