//! OpenAI-compatible `chat/completions` request/response format.

use crate::types::{GenerateRequest, GenerateResponse, Message, Part, Role, Usage};
use serde_json::{Map, Value, json};

/// Options learned per model: some servers reject `reasoning_effort`, some only accept `max_completion_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options<'a> {
    pub effort: Option<&'a str>,
    pub completion_tokens_field: bool,
}

pub fn build_body(req: &GenerateRequest, model_id: &str, options: Options<'_>) -> Value {
    let mut messages = Vec::new();
    if !req.system.is_empty() {
        messages.push(json!({ "role": "system", "content": req.system }));
    }
    for message in &req.messages {
        push_message(&mut messages, message);
    }

    let mut body = Map::new();
    body.insert("model".into(), json!(model_id));
    body.insert("messages".into(), Value::Array(messages));
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let parameters = if t.parameters.is_object() { t.parameters.clone() } else { json!({ "type": "object", "properties": {} }) };
                json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": parameters } })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
        if !req.allow_tools {
            body.insert("tool_choice".into(), json!("none"));
        }
    }
    let field = if options.completion_tokens_field { "max_completion_tokens" } else { "max_tokens" };
    body.insert(field.into(), json!(req.max_output_tokens));
    if let Some(effort) = options.effort {
        body.insert("reasoning_effort".into(), json!(effort));
    }
    Value::Object(body)
}

fn push_message(out: &mut Vec<Value>, message: &Message) {
    let text: String = message.text();
    match message.role {
        Role::User => {
            for part in &message.parts {
                if let Part::ToolResult { id, result, .. } = part {
                    let content = match result {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    out.push(json!({ "role": "tool", "tool_call_id": id, "content": content }));
                }
            }
            let images: Vec<Value> = message
                .parts
                .iter()
                .filter_map(|p| match p {
                    Part::Image { mime, data } => Some(json!({ "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{data}") } })),
                    _ => None,
                })
                .collect();
            if !images.is_empty() {
                let mut content = Vec::with_capacity(images.len() + 1);
                if !text.is_empty() {
                    content.push(json!({ "type": "text", "text": text }));
                }
                content.extend(images);
                out.push(json!({ "role": "user", "content": content }));
            } else if !text.is_empty() {
                out.push(json!({ "role": "user", "content": text }));
            }
        }
        Role::Model => {
            let calls: Vec<Value> = message
                .parts
                .iter()
                .filter_map(|p| match p {
                    Part::ToolCall { id, name, args, .. } => {
                        Some(json!({ "id": id, "type": "function", "function": { "name": name, "arguments": args.to_string() } }))
                    }
                    _ => None,
                })
                .collect();
            if calls.is_empty() {
                out.push(json!({ "role": "assistant", "content": text }));
            } else {
                out.push(json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }, "tool_calls": calls }));
            }
        }
    }
}

pub fn parse_response(body: &Value) -> Result<GenerateResponse, String> {
    let Some(choice) = body.get("choices").and_then(|c| c.get(0)) else {
        let error = body.pointer("/error/message").and_then(Value::as_str).unwrap_or("no choices");
        return Err(format!("The model returned no answer ({error})."));
    };
    let message = choice.get("message").ok_or("The answer has no message.")?;
    let mut parts = Vec::new();
    match message.get("content") {
        Some(Value::String(text)) if !text.is_empty() => parts.push(Part::text(text.clone())),
        Some(Value::Array(items)) => {
            let text: String = items.iter().filter_map(|i| i.get("text").and_then(Value::as_str)).collect();
            if !text.is_empty() {
                parts.push(Part::text(text));
            }
        }
        _ => {}
    }
    for call in message.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
        let arguments = call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}");
        parts.push(Part::ToolCall {
            id: call.get("id").and_then(Value::as_str).unwrap_or("call").to_string(),
            name: call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default().to_string(),
            args: serde_json::from_str(arguments).unwrap_or_else(|_| json!({})),
            signature: None,
        });
    }
    // completion_tokens includes the reasoning tokens; keep them apart like Gemini does.
    let usage = body.get("usage").map(|u| {
        let n = |p: &str| u.pointer(p).and_then(Value::as_u64).unwrap_or(0);
        let thinking = n("/completion_tokens_details/reasoning_tokens");
        Usage { input: n("/prompt_tokens"), output: n("/completion_tokens").saturating_sub(thinking), thinking, cached: n("/prompt_tokens_details/cached_tokens") }
    });
    Ok(GenerateResponse { parts, finish_reason: choice.get("finish_reason").and_then(Value::as_str).map(str::to_string), usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolSpec;

    fn request() -> GenerateRequest {
        GenerateRequest {
            system: "Be brief.".into(),
            messages: vec![
                Message::user("price?"),
                Message {
                    role: Role::Model,
                    parts: vec![Part::text("Let me look."), Part::ToolCall { id: "call_1".into(), name: "web_search".into(), args: json!({"query": "gold"}), signature: None }],
                },
                Message { role: Role::User, parts: vec![Part::ToolResult { id: "call_1".into(), name: "web_search".into(), result: json!({"price": 1}) }] },
            ],
            tools: vec![ToolSpec { name: "web_search".into(), description: "Search".into(), parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}}) }],
            allow_tools: true,
            max_output_tokens: 800,
            thinking: None,
        }
    }

    #[test]
    fn keeps_tool_calls_even_when_text_comes_first() {
        // v1 bug: tool_calls were dropped when the model turn started with text.
        let body = build_body(&request(), "gpt-x", Options { effort: Some("low"), completion_tokens_field: false });
        let assistant = &body["messages"][2];
        assert_eq!(assistant["content"], "Let me look.");
        assert_eq!(assistant["tool_calls"][0]["function"]["arguments"], "{\"query\":\"gold\"}");
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["messages"][3]["tool_call_id"], "call_1");
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["max_tokens"], 800);
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn options_change_token_field_and_effort() {
        let mut req = request();
        req.allow_tools = false;
        let body = build_body(&req, "gpt-x", Options { effort: None, completion_tokens_field: true });
        assert_eq!(body["max_completion_tokens"], 800);
        assert!(body.get("max_tokens").is_none() && body.get("reasoning_effort").is_none());
        assert_eq!(body["tool_choice"], "none");
    }

    #[test]
    fn parses_text_and_tool_calls() {
        let body = json!({ "choices": [{ "finish_reason": "tool_calls", "message": {
            "content": null,
            "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read_web_page", "arguments": "{\"url\":\"https://x\"}" } }]
        }}]});
        let r = parse_response(&body).unwrap();
        assert_eq!(r.tool_calls(), vec![("c1", "read_web_page", &json!({"url": "https://x"}))]);
        let text = parse_response(&json!({ "choices": [{ "message": { "content": [{ "type": "text", "text": "Hi" }] } }] })).unwrap();
        assert_eq!(text.text(), "Hi");
    }

    #[test]
    fn images_go_into_a_content_array() {
        let req = GenerateRequest {
            system: String::new(),
            messages: vec![Message { role: Role::User, parts: vec![Part::text("what is on screen?"), Part::image("image/jpeg", "QUJD")] }],
            tools: vec![],
            allow_tools: false,
            max_output_tokens: 100,
            thinking: None,
        };
        let body = build_body(&req, "m", Options { effort: None, completion_tokens_field: false });
        assert_eq!(body["messages"][0]["content"][0]["text"], "what is on screen?");
        assert_eq!(body["messages"][0]["content"][1]["image_url"]["url"], "data:image/jpeg;base64,QUJD");
    }

    #[test]
    fn reads_token_usage() {
        let body = json!({
            "choices": [{ "message": { "content": "ok" } }],
            "usage": { "prompt_tokens": 900, "completion_tokens": 50, "completion_tokens_details": { "reasoning_tokens": 30 } }
        });
        assert_eq!(parse_response(&body).unwrap().usage, Some(Usage { input: 900, output: 20, thinking: 30, cached: 0 }));
        assert_eq!(parse_response(&json!({ "choices": [{ "message": { "content": "x" } }] })).unwrap().usage, None);
    }
}
