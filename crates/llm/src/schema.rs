//! JSON Schema (what tools and MCP servers describe) -> the schema subset Gemini function declarations accept.
//! Ported from v1 (src/main/mcp.js `toGeminiSchema`).

use serde_json::{Map, Value, json};

const DESCRIPTION_CHARS: usize = 300;

fn short(text: &str) -> String {
    text.chars().take(DESCRIPTION_CHARS).collect()
}

/// Converts one schema node. Free-form objects become strings ("JSON object as text").
pub fn to_gemini(schema: &Value) -> Value {
    convert(schema, 0)
}

/// True when the top-level schema has at least one property (Gemini rejects empty OBJECT parameters).
pub fn has_properties(schema: &Value) -> bool {
    schema.get("properties").and_then(Value::as_object).is_some_and(|p| !p.is_empty())
}

fn convert(schema: &Value, depth: usize) -> Value {
    let Some(obj) = schema.as_object() else { return json!({ "type": "STRING" }) };
    if depth > 6 {
        return json!({ "type": "STRING" });
    }

    let mut nullable = false;
    let mut kind = match obj.get("type") {
        Some(Value::String(t)) => Some(t.clone()),
        Some(Value::Array(list)) => {
            nullable = list.iter().any(|t| t == "null");
            list.iter().filter_map(Value::as_str).find(|t| *t != "null").map(str::to_string)
        }
        _ => None,
    };

    if kind.is_none() {
        let variants = obj.get("anyOf").or_else(|| obj.get("oneOf")).and_then(Value::as_array);
        if let Some(variant) = variants.and_then(|v| v.iter().find(|x| x.get("type") != Some(&json!("null")))) {
            let mut out = convert(variant, depth + 1);
            if let (Some(desc), Some(map)) = (obj.get("description").and_then(Value::as_str), out.as_object_mut()) {
                map.insert("description".into(), json!(short(desc)));
            }
            return out;
        }
        kind = Some(
            if obj.contains_key("properties") {
                "object"
            } else if obj.contains_key("items") {
                "array"
            } else {
                "string"
            }
            .to_string(),
        );
    }

    let mut kind = kind.unwrap_or_else(|| "string".into()).to_uppercase();
    if !["OBJECT", "ARRAY", "STRING", "NUMBER", "INTEGER", "BOOLEAN"].contains(&kind.as_str()) {
        kind = "STRING".into();
    }

    let mut out = Map::new();
    let description = obj.get("description").and_then(Value::as_str).map(short);

    if kind == "OBJECT" {
        let props = obj.get("properties").and_then(Value::as_object);
        match props {
            Some(props) if !props.is_empty() => {
                let converted: Map<String, Value> = props.iter().map(|(k, v)| (k.clone(), convert(v, depth + 1))).collect();
                let required: Vec<Value> = obj
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|r| r.iter().filter(|n| n.as_str().is_some_and(|n| converted.contains_key(n))).cloned().collect())
                    .unwrap_or_default();
                out.insert("type".into(), json!("OBJECT"));
                out.insert("properties".into(), Value::Object(converted));
                if !required.is_empty() {
                    out.insert("required".into(), Value::Array(required));
                }
            }
            _ => {
                let text = format!("{}(JSON object as text)", description.as_ref().map(|d| format!("{d} ")).unwrap_or_default());
                return json!({ "type": "STRING", "description": text });
            }
        }
    } else {
        out.insert("type".into(), json!(kind));
    }

    if let Some(desc) = description {
        out.insert("description".into(), json!(desc));
    }
    if kind == "STRING"
        && let Some(values) = obj.get("enum").and_then(Value::as_array)
    {
        out.insert("enum".into(), Value::Array(values.iter().map(|v| json!(v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect()));
    }
    if kind == "ARRAY" {
        out.insert("items".into(), convert(obj.get("items").unwrap_or(&json!({ "type": "string" })), depth + 1));
    }
    if nullable {
        out.insert("nullable".into(), json!(true));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_types_required_enum_and_nullable() {
        let schema = json!({
            "type": "object",
            "$schema": "http://json-schema.org/draft-07/schema#",
            "additionalProperties": false,
            "properties": {
                "item": { "type": "string", "enum": ["gold", "dollar"] },
                "count": { "type": ["integer", "null"] },
                "tags": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["item", "missing"]
        });
        let out = to_gemini(&schema);
        assert_eq!(out["type"], "OBJECT");
        assert_eq!(out["properties"]["item"]["enum"], json!(["gold", "dollar"]));
        assert_eq!(out["properties"]["count"], json!({ "type": "INTEGER", "nullable": true }));
        assert_eq!(out["properties"]["tags"]["items"]["type"], "STRING");
        assert_eq!(out["required"], json!(["item"]));
        assert!(out.get("$schema").is_none() && out.get("additionalProperties").is_none());
    }

    #[test]
    fn free_form_object_becomes_json_text() {
        let out = to_gemini(&json!({ "type": "object", "description": "Options" }));
        assert_eq!(out["type"], "STRING");
        assert!(out["description"].as_str().unwrap().contains("JSON object as text"));
    }

    #[test]
    fn any_of_takes_first_non_null_variant() {
        let out = to_gemini(&json!({ "anyOf": [{ "type": "null" }, { "type": "number" }], "description": "Price" }));
        assert_eq!(out, json!({ "type": "NUMBER", "description": "Price" }));
    }

    #[test]
    fn detects_properties() {
        assert!(has_properties(&json!({ "type": "object", "properties": { "a": {} } })));
        assert!(!has_properties(&json!({ "type": "object", "properties": {} })));
        assert!(!has_properties(&json!({ "type": "object" })));
    }
}
