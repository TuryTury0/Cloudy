//! Gemini Live: a two-way voice conversation over a WebSocket. Microphone audio (16 kHz PCM) goes up, the model's
//! voice (24 kHz PCM), transcripts and tool calls come down. Ported from v1 live.js (same setup: audio answers,
//! both transcriptions, background noise never cuts the model off, a longer pause before the user is "done").

use crate::gemini::function_declarations;
use crate::types::{ToolSpec, Usage};
use crate::LlmError;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const URL: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";
const SETUP_TIMEOUT: Duration = Duration::from_secs(15);
/// Opening the connection itself (TCP, TLS, WebSocket) takes about a second; a way out that is blocked hangs
/// instead. Giving up sooner lets the other way (the proxy) be tried: tonight a blocked direct attempt took 15 s
/// before the proxy connected in 4 (voice ready in 19-20 s, 05:17, 05:20, 06:06).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
pub const INPUT_RATE: u32 = 16_000;
pub const OUTPUT_RATE: u32 = 24_000;

#[derive(Debug, Clone, PartialEq)]
pub struct LiveConfig {
    /// e.g. "gemini-3.8-live"
    pub model: String,
    /// Prebuilt voice, e.g. "Aoede".
    pub voice: String,
    pub system: String,
    pub tools: Vec<ToolSpec>,
    /// The user's proxy (full URL) to connect through, or None for a direct connection.
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    /// The model's voice, 24 kHz mono PCM.
    Audio(Vec<i16>),
    /// Part of what the user said (transcription).
    Heard(String),
    /// Part of what the model is saying (transcription).
    Said(String),
    ToolCalls(Vec<(String, String, Value)>),
    /// The model stopped talking because the user spoke over it.
    Interrupted,
    TurnComplete,
    /// The server will close soon (session time limit).
    GoingAway,
    /// Tokens of the model's last answer.
    Usage(Usage),
    Closed(String),
}

/// Builds the setup message.
pub fn setup_message(config: &LiveConfig) -> Value {
    let mut setup = json!({
        "model": format!("models/{}", config.model),
        "generationConfig": {
            "responseModalities": ["AUDIO"],
            "speechConfig": { "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": config.voice } } }
        },
        "inputAudioTranscription": {},
        "outputAudioTranscription": {},
        // Background sound must never cut the model off mid-answer; fast speech with short pauses must not be
        // split into pieces (v1 lessons).
        "realtimeInputConfig": {
            "activityHandling": "NO_INTERRUPTION",
            "automaticActivityDetection": {
                "startOfSpeechSensitivity": "START_SENSITIVITY_HIGH",
                "endOfSpeechSensitivity": "END_SENSITIVITY_LOW",
                "prefixPaddingMs": 300,
                "silenceDurationMs": 900
            }
        },
        "systemInstruction": { "parts": [{ "text": config.system }] }
    });
    if !config.tools.is_empty() {
        setup["tools"] = json!([{ "functionDeclarations": function_declarations(&config.tools) }]);
    }
    json!({ "setup": setup })
}

fn pcm_from_base64(data: &str) -> Vec<i16> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map(|bytes| bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
        .unwrap_or_default()
}

/// Reads one server message (snake_case and camelCase both appear in the wild).
pub fn parse_server_message(msg: &Value) -> Vec<LiveEvent> {
    let get = |v: &Value, camel: &str, snake: &str| v.get(camel).or_else(|| v.get(snake)).cloned();
    let mut out = Vec::new();
    if let Some(call) = get(msg, "toolCall", "tool_call") {
        let calls: Vec<(String, String, Value)> = call
            .get("functionCalls")
            .or_else(|| call.get("function_calls"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|c| {
                (
                    c.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    c.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                    c.get("args").cloned().unwrap_or_else(|| json!({})),
                )
            })
            .collect();
        if !calls.is_empty() {
            out.push(LiveEvent::ToolCalls(calls));
        }
    }
    if get(msg, "goAway", "go_away").is_some() {
        out.push(LiveEvent::GoingAway);
    }
    if let Some(meta) = get(msg, "usageMetadata", "usage_metadata") {
        let count = |camel: &str, snake: &str| get(&meta, camel, snake).and_then(|v| v.as_u64()).unwrap_or(0);
        out.push(LiveEvent::Usage(Usage {
            input: count("promptTokenCount", "prompt_token_count") + count("toolUsePromptTokenCount", "tool_use_prompt_token_count"),
            output: count("responseTokenCount", "response_token_count"),
            thinking: count("thoughtsTokenCount", "thoughts_token_count"),
            cached: count("cachedContentTokenCount", "cached_content_token_count"),
        }));
    }
    let Some(content) = get(msg, "serverContent", "server_content") else { return out };
    if content.get("interrupted").and_then(Value::as_bool) == Some(true) {
        out.push(LiveEvent::Interrupted);
    }
    if let Some(text) = get(&content, "inputTranscription", "input_transcription").and_then(|t| t.get("text").and_then(Value::as_str).map(str::to_string))
        && !text.trim().is_empty()
    {
        out.push(LiveEvent::Heard(text));
    }
    if let Some(turn) = get(&content, "modelTurn", "model_turn") {
        for part in turn.get("parts").and_then(Value::as_array).into_iter().flatten() {
            if let Some(data) = get(part, "inlineData", "inline_data").and_then(|d| d.get("data").and_then(Value::as_str).map(str::to_string)) {
                out.push(LiveEvent::Audio(pcm_from_base64(&data)));
            }
        }
    }
    if let Some(text) = get(&content, "outputTranscription", "output_transcription").and_then(|t| t.get("text").and_then(Value::as_str).map(str::to_string))
        && !text.is_empty()
    {
        out.push(LiveEvent::Said(text));
    }
    if get(&content, "turnComplete", "turn_complete").and_then(|v| v.as_bool()) == Some(true) {
        out.push(LiveEvent::TurnComplete);
    }
    out
}

/// Sends to an open session. Cheap to clone; dropping every clone closes the connection.
#[derive(Clone)]
pub struct LiveSession {
    tx: mpsc::UnboundedSender<WsMessage>,
}

impl LiveSession {
    /// Connects and waits until the server accepted the setup. Events arrive on the returned receiver.
    pub async fn connect(config: &LiveConfig, api_key: &str) -> Result<(LiveSession, mpsc::UnboundedReceiver<LiveEvent>), LlmError> {
        if api_key.trim().is_empty() {
            return Err(LlmError::MissingKey("Gemini Live".into()));
        }
        let mut request = URL.into_client_request().map_err(|e| LlmError::Network(e.to_string()))?;
        // The key travels in a header, never in the URL (URLs end up in logs).
        request.headers_mut().insert("x-goog-api-key", api_key.parse().map_err(|_| LlmError::Answer("The API key has invalid characters.".into()))?);
        let connecting = async {
            match &config.proxy {
                None => tokio_tungstenite::connect_async(request).await.map(|(s, _)| s).map_err(|e| e.to_string()),
                Some(proxy) => {
                    let host = request.uri().host().unwrap_or_default().to_string();
                    let stream = crate::proxy::connect_through(proxy, &host, 443).await?;
                    tokio_tungstenite::client_async_tls(request, stream).await.map(|(s, _)| s).map_err(|e| e.to_string())
                }
            }
        };
        let socket = tokio::time::timeout(CONNECT_TIMEOUT, connecting)
            .await
            .map_err(|_| LlmError::Network("connecting to Gemini Live timed out".into()))?
            .map_err(LlmError::Network)?;
        let (mut write, mut read) = socket.split();
        write.send(WsMessage::text(setup_message(config).to_string())).await.map_err(|e| LlmError::Network(e.to_string()))?;

        // Wait for setupComplete; a refusal arrives as a close frame with the reason.
        let ready = tokio::time::timeout(SETUP_TIMEOUT, async {
            while let Some(frame) = read.next().await {
                match frame {
                    Ok(WsMessage::Text(t)) if t.contains("setupComplete") || t.contains("setup_complete") => return Ok(()),
                    Ok(WsMessage::Binary(b)) if String::from_utf8_lossy(&b).contains("setupComplete") => return Ok(()),
                    Ok(WsMessage::Close(reason)) => return Err(reason.map(|r| r.reason.to_string()).unwrap_or_else(|| "closed".into())),
                    Err(e) => return Err(e.to_string()),
                    _ => {}
                }
            }
            Err("closed before setup".to_string())
        })
        .await;
        match ready {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => return Err(LlmError::Answer(format!("Gemini Live refused the session: {reason}").replace(api_key, "***"))),
            Err(_) => return Err(LlmError::Network("Gemini Live did not answer the setup".into())),
        }

        let (tx, mut outgoing) = mpsc::unbounded_channel::<WsMessage>();
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                let closing = matches!(message, WsMessage::Close(_));
                if write.send(message).await.is_err() || closing {
                    break;
                }
            }
            let _ = write.close().await;
        });
        let (events_tx, events) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let reason = loop {
                let text = match read.next().await {
                    Some(Ok(WsMessage::Text(t))) => t.to_string(),
                    Some(Ok(WsMessage::Binary(b))) => String::from_utf8_lossy(&b).to_string(),
                    Some(Ok(WsMessage::Close(reason))) => break reason.map(|r| r.reason.to_string()).unwrap_or_else(|| "closed".into()),
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => break e.to_string(),
                    None => break "closed".into(),
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
                for event in parse_server_message(&value) {
                    if events_tx.send(event).is_err() {
                        return;
                    }
                }
            };
            let _ = events_tx.send(LiveEvent::Closed(reason));
        });
        Ok((LiveSession { tx }, events))
    }

    fn send(&self, value: Value) -> bool {
        self.tx.send(WsMessage::text(value.to_string())).is_ok()
    }

    /// Streams microphone audio (16 kHz mono PCM).
    pub fn send_audio(&self, pcm: &[i16]) -> bool {
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        self.send(json!({ "realtimeInput": { "audio": { "mimeType": "audio/pcm;rate=16000", "data": data } } }))
    }

    /// Tells the server the microphone stopped, so it answers what it has heard.
    pub fn end_audio(&self) -> bool {
        self.send(json!({ "realtimeInput": { "audioStreamEnd": true } }))
    }

    /// A typed message in the same conversation.
    pub fn send_text(&self, text: &str) -> bool {
        self.send(json!({ "clientContent": { "turns": [{ "role": "user", "parts": [{ "text": text }] }], "turnComplete": true } }))
    }

    pub fn send_tool_results(&self, results: &[(String, String, Value)]) -> bool {
        let responses: Vec<Value> = results.iter().map(|(id, name, result)| json!({ "id": id, "name": name, "response": { "result": result } })).collect();
        self.send(json!({ "toolResponse": { "functionResponses": responses } }))
    }

    pub fn close(&self) {
        let _ = self.tx.send(WsMessage::Close(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_has_voice_transcripts_and_tools() {
        let config = LiveConfig {
            model: "gemini-3.8-live".into(),
            voice: "Aoede".into(),
            system: "Be brief.".into(),
            tools: vec![ToolSpec { name: "web_search".into(), description: "Search".into(), parameters: json!({ "type": "object", "properties": { "query": { "type": "string" } } }) }],
            proxy: None,
        };
        let m = setup_message(&config);
        assert_eq!(m["setup"]["model"], "models/gemini-3.8-live");
        assert_eq!(m["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"], "Aoede");
        assert_eq!(m["setup"]["realtimeInputConfig"]["activityHandling"], "NO_INTERRUPTION");
        assert_eq!(m["setup"]["tools"][0]["functionDeclarations"][0]["parameters"]["type"], "OBJECT");
    }

    #[test]
    fn reads_audio_transcripts_tool_calls_and_turn_ends() {
        let audio = base64::engine::general_purpose::STANDARD.encode([1u8, 0, 0xFF, 0xFF]);
        let msg = json!({ "serverContent": {
            "inputTranscription": { "text": "سلام" },
            "modelTurn": { "parts": [{ "inlineData": { "mimeType": "audio/pcm;rate=24000", "data": audio } }] },
            "outputTranscription": { "text": "Hi" },
            "turnComplete": true
        } });
        assert_eq!(
            parse_server_message(&msg),
            vec![LiveEvent::Heard("سلام".into()), LiveEvent::Audio(vec![1, -1]), LiveEvent::Said("Hi".into()), LiveEvent::TurnComplete]
        );
        let call = json!({ "tool_call": { "function_calls": [{ "id": "c1", "name": "media_control", "args": { "action": "next" } }] } });
        assert_eq!(parse_server_message(&call), vec![LiveEvent::ToolCalls(vec![("c1".into(), "media_control".into(), json!({ "action": "next" }))])]);
        assert_eq!(parse_server_message(&json!({ "serverContent": { "interrupted": true } })), vec![LiveEvent::Interrupted]);
        assert_eq!(parse_server_message(&json!({ "goAway": { "timeLeft": "10s" } })), vec![LiveEvent::GoingAway]);
    }
}
