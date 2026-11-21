//! Sends requests to a model and adapts to what it accepts.
//!
//! Lessons from v1, built in:
//! - Answers are waited for (no time-based switching); only a dead connection is cut, after `SAFETY_TIMEOUT`.
//! - Models are asked to think briefly (Gemini 3+ level "low", older Gemini budget 512, OpenAI effort "low") unless
//!   the user chose a level. Gemini 3+ get a thinking level, older Gemini models a token budget.
//! - A Gemini 400 while thinking settings are sent is asked again with the next, simpler form for that level
//!   (level, then budget, then none). The form that worked is remembered per model and level, only after it answered.
//! - An OpenAI 400 caused by effort or by `max_tokens` is retried once without it, and remembered per model.
//! - Busy servers (5xx, for example Gemini's 503 "high demand") and rate limits (429) are retried a few times with
//!   growing pauses, so a short spike does not fail the request. It is still the same model; nothing is switched.

use crate::gemini::{self, ThinkingConfig};
use crate::openai;
use crate::types::{GenerateRequest, GenerateResponse, ModelConfig, Provider, Thinking};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SAFETY_TIMEOUT: Duration = Duration::from_secs(120);
/// An answer slower than this is noted in the log with how many tries it took (the logs showed 20 s per call with
/// nothing saying why).
const SLOW: Duration = Duration::from_secs(8);
/// Pauses before each retry of a busy server; their count is the number of retries.
#[cfg(not(test))]
const BUSY_RETRY_DELAYS: [Duration; 3] = [Duration::from_millis(1000), Duration::from_millis(2500), Duration::from_millis(5000)];
/// Same count in tests, without the waiting.
#[cfg(test)]
const BUSY_RETRY_DELAYS: [Duration; 3] = [Duration::from_millis(1); 3];

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum LlmError {
    #[error("No API key is saved for {0}.")]
    MissingKey(String),
    #[error("HTTP {status}: {detail}")]
    Http { status: u16, detail: String },
    #[error("Network error: {0}")]
    Network(String),
    #[error("{0}")]
    Answer(String),
}

/// Something that turns a request into an answer. The agent depends on this, so tests can script answers.
#[async_trait]
pub trait Generator: Send + Sync {
    async fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, LlmError>;
    fn label(&self) -> String;
}

/// What an OpenAI-compatible model refused, per model.
#[derive(Debug, Default, Clone, Copy)]
struct Quirks {
    no_effort: bool,
    completion_tokens: bool,
}

pub struct LlmClient {
    /// Follows the Windows proxy settings (a VPN app often sets one), like a browser.
    http: reqwest::Client,
    /// Straight out, ignoring any proxy.
    direct: reqwest::Client,
    /// The client through the user's proxy, and the ids of the models that use it first.
    proxy: std::sync::RwLock<Option<(reqwest::Client, std::collections::HashSet<String>)>>,
    /// Per model: the way out (see `routes`) that last reached it.
    routes: Mutex<HashMap<String, usize>>,
    quirks: Mutex<HashMap<String, Quirks>>,
    /// The Gemini thinking form that answered (an index into `gemini_thinking_forms`), per model and thinking level.
    thinking_forms: Mutex<HashMap<String, usize>>,
    /// Where retries and slow answers are reported (the app's log).
    log: std::sync::RwLock<Option<LogSink>>,
}

/// Receives one log line.
type LogSink = Arc<dyn Fn(String) + Send + Sync>;

impl Default for LlmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClient {
    pub fn new() -> Self {
        let http = Self::builder().build().unwrap_or_default();
        let direct = Self::builder().no_proxy().build().unwrap_or_default();
        Self {
            http,
            direct,
            proxy: Default::default(),
            routes: Default::default(),
            quirks: Mutex::new(HashMap::new()),
            thinking_forms: Default::default(),
            log: Default::default(),
        }
    }

    /// Reports retries (busy server, a setting the model refused) and slow answers to `log`.
    pub fn set_log(&self, log: impl Fn(String) + Send + Sync + 'static) {
        *self.log.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(log));
    }

    fn note(&self, config: &ModelConfig, text: String) {
        if let Some(log) = self.log.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            log(format!("{} ({}): {text}", config.name, config.model_id));
        }
    }

    fn builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder().timeout(SAFETY_TIMEOUT).connect_timeout(Duration::from_secs(20))
    }

    /// The user's proxy (a full proxy URL): these models go through it first, the others try it when they cannot
    /// get out another way. None: no proxy.
    pub fn set_proxy(&self, url: Option<&str>, models: &[String]) -> Result<(), String> {
        let proxy = match url {
            Some(url) => Some((crate::proxy::client_through(url, Self::builder())?, models.iter().cloned().collect())),
            None => None,
        };
        *self.proxy.write().unwrap_or_else(|e| e.into_inner()) = proxy;
        self.routes.lock().unwrap_or_else(|e| e.into_inner()).clear();
        Ok(())
    }

    /// The ways out for a model, in order, with a name for the log. A VPN's local proxy can be up but broken while a
    /// straight connection works, and the other way round (11-05 01:06: every screen step failed with "error
    /// sending request" through the Windows proxy, while a direct request reached Google in 0.5 s), so a request
    /// that cannot get out one way tries the next.
    fn routes(&self, config: &ModelConfig) -> Vec<(reqwest::Client, &'static str)> {
        let windows = (self.http.clone(), "through the Windows proxy settings");
        let direct = (self.direct.clone(), "straight out");
        match self.proxy.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            Some((proxy, models)) if models.contains(&config.id) => vec![(proxy.clone(), "through the proxy"), windows, direct],
            Some((proxy, _)) => vec![windows, direct, (proxy.clone(), "through the proxy")],
            None => vec![windows, direct],
        }
    }

    fn quirks(&self, key: &str) -> Quirks {
        self.quirks.lock().map(|q| q.get(key).copied().unwrap_or_default()).unwrap_or_default()
    }

    fn set_quirks(&self, key: &str, quirks: Quirks) {
        if let Ok(mut q) = self.quirks.lock() {
            q.insert(key.to_string(), quirks);
        }
    }

    fn thinking_form(&self, key: &str) -> usize {
        self.thinking_forms.lock().map(|f| f.get(key).copied().unwrap_or_default()).unwrap_or_default()
    }

    fn set_thinking_form(&self, key: &str, form: usize) {
        if let Ok(mut f) = self.thinking_forms.lock() {
            f.insert(key.to_string(), form);
        }
    }

    pub async fn generate(&self, config: &ModelConfig, api_key: &str, request: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
        if api_key.trim().is_empty() {
            return Err(LlmError::MissingKey(config.name.clone()));
        }
        let quirk_key = format!("{}|{}", config.base_url, config.model_id);
        let level = request.thinking.unwrap_or(config.thinking);
        // Gemini: the thinking settings to try, best first. OpenAI's effort is handled by `Quirks`.
        let forms = match config.provider {
            Provider::Gemini => gemini_thinking_forms(&config.model_id, level),
            Provider::Openai => vec![None],
        };
        // Per level, so a form refused for Low never changes what High sends.
        let form_key = format!("{quirk_key}|{level:?}");
        let remembered = self.thinking_form(&form_key).min(forms.len() - 1);
        let mut form = remembered;
        let mut busy_retries = 0;
        let started = std::time::Instant::now();
        // At most: every Gemini thinking form (for OpenAI the first try, one effort fix and one token-field fix), and
        // the busy-server retries.
        let fixes = match config.provider {
            Provider::Gemini => forms.len(),
            Provider::Openai => 3,
        };
        for attempt in 1..=(fixes + BUSY_RETRY_DELAYS.len()) {
            let quirks = self.quirks(&quirk_key);
            let (url, body) = build(config, request, quirks, forms[form]);
            match self.post(config, api_key, &url, &body).await {
                Ok(json) => {
                    if form != remembered {
                        // Only now is it known that this form works.
                        self.set_thinking_form(&form_key, form);
                    }
                    if started.elapsed() >= SLOW {
                        let tries = if attempt == 1 { "first try".to_string() } else { format!("{attempt} tries") };
                        self.note(config, format!("slow answer: {:.1} s ({tries})", started.elapsed().as_secs_f32()));
                    }
                    return match config.provider {
                        Provider::Gemini => gemini::parse_response(&json),
                        Provider::Openai => openai::parse_response(&json),
                    }
                    .map_err(LlmError::Answer);
                }
                Err(LlmError::Http { status: 400, detail })
                    if config.provider == Provider::Gemini
                        && body.pointer("/generationConfig/thinkingConfig").is_some()
                        && form + 1 < forms.len()
                        && !names_another_cause(&detail) =>
                {
                    // Nothing is remembered yet: a 400 can have another cause, and the next form may fail too.
                    let refused = forms[form].map(|f| f.to_string()).unwrap_or_default();
                    form += 1;
                    let again = forms[form].map_or_else(|| "without thinking settings".to_string(), |f| format!("with {f}"));
                    self.note(config, format!("HTTP 400 ({}) for {refused}; asking again {again}", short(&detail)));
                }
                Err(LlmError::Http { status: 400, detail }) => {
                    let mut next = quirks;
                    let lower = detail.to_lowercase();
                    match config.provider {
                        Provider::Openai if !quirks.no_effort && body.get("reasoning_effort").is_some() && lower.contains("reasoning") => {
                            next.no_effort = true;
                        }
                        Provider::Openai if !quirks.completion_tokens && lower.contains("max_tokens") => {
                            next.completion_tokens = true;
                        }
                        _ => return Err(LlmError::Http { status: 400, detail }),
                    }
                    self.note(config, format!("HTTP 400 ({}); asking again without that setting", short(&detail)));
                    self.set_quirks(&quirk_key, next);
                }
                Err(LlmError::Http { status: 429, detail }) if long_wait(&detail) => {
                    // A daily quota (or a wait of minutes): asking again now only wastes the user's time.
                    return Err(LlmError::Http { status: 429, detail });
                }
                Err(LlmError::Http { status, detail }) if (status == 429 || status >= 500) && busy_retries < BUSY_RETRY_DELAYS.len() => {
                    let pause = BUSY_RETRY_DELAYS[busy_retries];
                    self.note(config, format!("busy (HTTP {status}: {}); asking again in {:.1} s", short(&detail), pause.as_secs_f32()));
                    tokio::time::sleep(pause).await;
                    busy_retries += 1;
                }
                Err(other) => return Err(other),
            }
        }
        Err(LlmError::Answer("The model kept rejecting the request.".into()))
    }

    /// Sends the request the way that last reached this model; when it cannot get out (no connection at all, not an
    /// HTTP error), the other ways once each. The way that worked is kept for the next requests.
    async fn post(&self, config: &ModelConfig, api_key: &str, url: &str, body: &Value) -> Result<Value, LlmError> {
        let routes = self.routes(config);
        let first = self.routes.lock().unwrap_or_else(|e| e.into_inner()).get(&config.id).copied().unwrap_or(0).min(routes.len() - 1);
        let mut failed = Vec::new();
        let mut refused: Option<LlmError> = None;
        for step in 0..routes.len() {
            let at = (first + step) % routes.len();
            let (client, name) = &routes[at];
            match self.post_with(client, config, api_key, url, body).await {
                Err(LlmError::Network(e)) => failed.push(format!("{name}: {}", short(&e))),
                // 11-11: a request through the VPN got no answer, the next went straight out, got Google's 403 page,
                // and "straight out" was kept: every later request failed at once, even with the VPN working.
                Err(e) if e.is_blocked_here() => {
                    failed.push(format!("{name}: refused from this address"));
                    refused = Some(e);
                }
                answer => {
                    if at != first || step > 0 {
                        self.routes.lock().unwrap_or_else(|e| e.into_inner()).insert(config.id.clone(), at);
                        self.note(config, format!("could not connect ({}); reached it {name}", failed.join("; ")));
                    }
                    return answer;
                }
            }
        }
        // Every way failed: a refusal says more than a dead connection (turn on the VPN).
        match refused {
            Some(LlmError::Http { status, detail }) => Err(LlmError::Http { status, detail: format!("{detail} ({})", failed.join("; ")) }),
            _ => Err(LlmError::Network(failed.join("; "))),
        }
    }

    async fn post_with(&self, client: &reqwest::Client, config: &ModelConfig, api_key: &str, url: &str, body: &Value) -> Result<Value, LlmError> {
        let mut builder = client.post(url).json(body);
        builder = match config.provider {
            // Header instead of ?key= so the key never ends up in URLs or logs.
            Provider::Gemini => builder.header("x-goog-api-key", api_key),
            Provider::Openai => builder.bearer_auth(api_key),
        };
        let response = builder.send().await.map_err(|e| LlmError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| LlmError::Network(e.to_string()))?;
        if !(200..300).contains(&status) {
            let json_detail = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string));
            let detail = match json_detail {
                Some(detail) => detail,
                // An HTML error page instead of the API's JSON: the service's front door refused this address
                // (11-11: Google's "Error 403 (Forbidden)!!1" page for every Gemini request sent without the VPN).
                None if status == 403 && looks_like_html(&text) => BLOCKED_PAGE.to_string(),
                None => text.chars().take(300).collect(),
            };
            // Some providers echo the key back in error messages; it must never reach the UI or logs.
            let detail = if api_key.len() >= 8 { detail.replace(api_key, "***") } else { detail };
            return Err(LlmError::Http { status, detail });
        }
        serde_json::from_str(text.trim()).map_err(|_| LlmError::Answer("The endpoint returned invalid JSON.".into()))
    }
}

/// The detail of a refusal by the service's front door (an HTML 403 page): it is the network, not the key.
pub const BLOCKED_PAGE: &str = "the service refused this address (its 403 page, not an API answer): the location is not supported without a VPN";

fn looks_like_html(text: &str) -> bool {
    let start = text.trim_start().get(..15).unwrap_or("").to_ascii_lowercase();
    start.starts_with("<!doctype html") || start.starts_with("<html")
}

fn short(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 120 { flat } else { format!("{}…", flat.chars().take(120).collect::<String>()) }
}

/// A rate-limit answer that asks to wait much longer than a short pause: a per-day quota, or "retry in" hours,
/// minutes, or more than a minute of seconds.
pub(crate) fn long_wait(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    if lower.contains("per day") || lower.contains("perday") {
        return true;
    }
    // "You exceeded your current quota, please check your plan and billing details" with no short wait in it: the key
    // has no (more) quota for this model, typically Flash or Pro on a free key (11-11: gemini-3.8-flash, while
    // gemini-3.5-flash-lite worked on the same key). Asking again in a few seconds cannot help.
    let no_quota = lower.contains("exceeded your current quota") || lower.contains("check your plan and billing");
    let Some(at) = lower.find("retry in ") else { return no_quota };
    let after = &lower[at + 9..];
    let number: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let unit = after[number.len()..].chars().next();
    match (number.parse::<f64>(), unit) {
        (Ok(_), Some('h' | 'm')) => true,
        (Ok(seconds), Some('s')) => seconds > 60.0,
        _ => false,
    }
}

/// A Gemini 400 that is plainly not about the thinking settings (a bad key, a blocked location): asking again with
/// other settings only delays the error.
fn names_another_cause(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    ["api key", "location is not supported", "billing", "permission"].iter().any(|cause| lower.contains(cause))
}

/// The major version in a Gemini model id ("gemini-3.5-flash-lite" is 3). None when it cannot be read.
fn gemini_major(model_id: &str) -> Option<u32> {
    let id = model_id.trim().to_ascii_lowercase();
    let rest = id.strip_prefix("gemini-")?;
    rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

/// The thinking settings to try for a Gemini request, best first; each 400 moves on to the next one. The last one
/// is always None (no thinking settings). Gemini 3 and later take a level; older models (and ids without a readable
/// version) a token budget.
fn gemini_thinking_forms(model_id: &str, thinking: Thinking) -> Vec<Option<ThinkingConfig>> {
    use ThinkingConfig::{Budget, Level};
    let mut forms = if gemini_major(model_id).is_some_and(|major| major >= 3) {
        let flash = model_id.to_ascii_lowercase().contains("flash");
        match thinking {
            // Briefly, as with the other providers: left out, Gemini 3 thinks at its own default ("high"), slowly.
            Thinking::Default => vec![Level("low"), Budget(512)],
            Thinking::Low => vec![Level(if flash { "minimal" } else { "low" }), Level("low"), Budget(0), Budget(128)],
            Thinking::Medium => vec![Level(if flash { "medium" } else { "high" }), Budget(2048)],
            Thinking::High => vec![Level("high"), Budget(8192)],
        }
    } else {
        match thinking {
            Thinking::Low => vec![Budget(0), Budget(128)],
            Thinking::Medium => vec![Budget(2048)],
            Thinking::High => vec![Budget(8192)],
            Thinking::Default => vec![Budget(512)],
        }
    };
    forms.dedup();
    forms.into_iter().map(Some).chain([None]).collect()
}

/// `thinking` is the Gemini thinking form; OpenAI requests take their effort from the level and `quirks`.
fn build(config: &ModelConfig, request: &GenerateRequest, quirks: Quirks, thinking: Option<ThinkingConfig>) -> (String, Value) {
    let base = config.base_url.trim_end_matches('/');
    match config.provider {
        Provider::Gemini => {
            let url = format!("{base}/models/{}:generateContent", config.model_id);
            (url, gemini::build_body(request, thinking))
        }
        Provider::Openai => {
            let effort = match request.thinking.unwrap_or(config.thinking) {
                Thinking::Medium => "medium",
                Thinking::High => "high",
                Thinking::Low | Thinking::Default => "low",
            };
            let url = if base.ends_with("/chat/completions") { base.to_string() } else { format!("{base}/chat/completions") };
            let options = openai::Options { effort: (!quirks.no_effort).then_some(effort), completion_tokens_field: quirks.completion_tokens };
            (url, openai::build_body(request, &config.model_id, options))
        }
    }
}

/// A configured model with its key, ready to answer.
pub struct ConfiguredModel {
    pub client: Arc<LlmClient>,
    pub config: ModelConfig,
    pub api_key: String,
}

#[async_trait]
impl Generator for ConfiguredModel {
    async fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
        self.client.generate(&self.config, &self.api_key, request).await
    }

    fn label(&self) -> String {
        format!("{} ({})", self.config.name, self.config.model_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn request() -> GenerateRequest {
        GenerateRequest { system: "s".into(), messages: vec![Message::user("hi")], tools: vec![], allow_tools: true, max_output_tokens: 100, thinking: None }
    }

    fn model(provider: Provider, base: &str) -> ModelConfig {
        ModelConfig { id: "m".into(), name: "Test".into(), provider, base_url: base.into(), model_id: "test-model".into(), thinking: Thinking::Default }
    }

    /// Matches requests whose JSON body has (or lacks) a value at a pointer.
    struct BodyHas(&'static str, bool);
    impl wiremock::Match for BodyHas {
        fn matches(&self, request: &Request) -> bool {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
            body.pointer(self.0).is_some() == self.1
        }
    }

    /// Matches requests whose JSON body has this value at a pointer.
    struct BodyAt(&'static str, Value);
    impl wiremock::Match for BodyAt {
        fn matches(&self, request: &Request) -> bool {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
            body.pointer(self.0) == Some(&self.1)
        }
    }

    const BUDGET: &str = "/generationConfig/thinkingConfig/thinkingBudget";
    const LEVEL: &str = "/generationConfig/thinkingConfig/thinkingLevel";

    fn gemini(base: &str, model_id: &str) -> ModelConfig {
        ModelConfig { model_id: model_id.into(), ..model(Provider::Gemini, base) }
    }

    fn with_thinking(thinking: Thinking) -> GenerateRequest {
        GenerateRequest { thinking: Some(thinking), ..request() }
    }

    fn answer(text: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "candidates": [{ "content": { "parts": [{ "text": text }] } }] }))
    }

    fn invalid_argument() -> ResponseTemplate {
        ResponseTemplate::new(400).set_body_json(json!({ "error": { "message": "Request contains an invalid argument." } }))
    }

    fn log_of(client: &LlmClient) -> Arc<Mutex<Vec<String>>> {
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = lines.clone();
        client.set_log(move |line| seen.lock().unwrap().push(line));
        lines
    }

    #[test]
    fn gemini_3_and_later_take_a_thinking_level() {
        use ThinkingConfig::{Budget, Level};
        assert_eq!(gemini_thinking_forms("gemini-3.5-flash-lite", Thinking::Low), [Some(Level("minimal")), Some(Level("low")), Some(Budget(0)), Some(Budget(128)), None]);
        assert_eq!(gemini_thinking_forms("gemini-3-pro-preview", Thinking::Low), [Some(Level("low")), Some(Budget(0)), Some(Budget(128)), None]);
        assert_eq!(gemini_thinking_forms("gemini-3.5-flash", Thinking::Medium), [Some(Level("medium")), Some(Budget(2048)), None]);
        assert_eq!(gemini_thinking_forms("gemini-3-pro", Thinking::Medium), [Some(Level("high")), Some(Budget(2048)), None]);
        assert_eq!(gemini_thinking_forms("gemini-4-flash", Thinking::High), [Some(Level("high")), Some(Budget(8192)), None]);
        assert_eq!(gemini_thinking_forms("Gemini-3-Flash", Thinking::Low)[0], Some(Level("minimal")));
        // Default leaves the model's own default.
        assert_eq!(gemini_thinking_forms("gemini-3.5-flash-lite", Thinking::Default), [Some(Level("low")), Some(Budget(512)), None], "briefly, never the model's slow default");
        assert!(names_another_cause("API key not valid. Please pass a valid API key."));
        assert!(names_another_cause("User location is not supported for the API use."));
        assert!(!names_another_cause("Request contains an invalid argument."));
        // Older models, and ids without a readable version, keep the budgets.
        assert_eq!(gemini_thinking_forms("gemini-2.5-flash", Thinking::Low), [Some(Budget(0)), Some(Budget(128)), None]);
        assert_eq!(gemini_thinking_forms("gemini-2.5-flash", Thinking::Medium), [Some(Budget(2048)), None]);
        assert_eq!(gemini_thinking_forms("gemini-2.5-pro", Thinking::High), [Some(Budget(8192)), None]);
        assert_eq!(gemini_thinking_forms("gemini-2.5-flash", Thinking::Default), [Some(Budget(512)), None]);
        assert_eq!(gemini_thinking_forms("gemini-flash-latest", Thinking::Low), [Some(Budget(0)), Some(Budget(128)), None]);
        assert_eq!(gemini_thinking_forms("test-model", Thinking::Default), [Some(Budget(512)), None]);
    }

    #[test]
    fn flash_lite_3_5_asks_for_minimal_thinking_on_low() {
        let config = gemini("https://example.test/v1beta/", "gemini-3.5-flash-lite");
        let request = with_thinking(Thinking::Low);
        let (url, body) = build(&config, &request, Quirks::default(), gemini_thinking_forms(&config.model_id, Thinking::Low)[0]);
        assert_eq!(url, "https://example.test/v1beta/models/gemini-3.5-flash-lite:generateContent");
        assert_eq!(body["generationConfig"]["thinkingConfig"], json!({ "thinkingLevel": "minimal" }));
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 612);
    }

    #[tokio::test]
    async fn a_refused_budget_0_tries_128_and_keeps_it_for_low_only() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(BodyAt(BUDGET, json!(0))).respond_with(invalid_argument()).expect(1).mount(&server).await;
        Mock::given(method("POST")).and(BodyAt(BUDGET, json!(128))).respond_with(answer("small")).expect(2).mount(&server).await;
        Mock::given(method("POST")).and(BodyAt(BUDGET, json!(8192))).respond_with(answer("deep")).expect(1).mount(&server).await;
        let client = LlmClient::new();
        let lines = log_of(&client);
        let config = gemini(&server.uri(), "gemini-2.5-flash");
        assert_eq!(client.generate(&config, "K", &with_thinking(Thinking::Low)).await.unwrap().text(), "small");
        // The second Low request goes straight to the budget that worked.
        assert_eq!(client.generate(&config, "K", &with_thinking(Thinking::Low)).await.unwrap().text(), "small");
        // A refusal for Low does not change what High sends.
        assert_eq!(client.generate(&config, "K", &with_thinking(Thinking::High)).await.unwrap().text(), "deep");
        assert_eq!(
            *lines.lock().unwrap(),
            ["Test (gemini-2.5-flash): HTTP 400 (Request contains an invalid argument.) for thinkingBudget 0; asking again with thinkingBudget 128"]
        );
    }

    #[tokio::test]
    async fn flash_lite_3_5_falls_back_from_minimal_to_low_and_keeps_high() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(BodyAt(LEVEL, json!("minimal"))).respond_with(invalid_argument()).expect(1).mount(&server).await;
        Mock::given(method("POST")).and(BodyAt(LEVEL, json!("low"))).respond_with(answer("low")).expect(2).mount(&server).await;
        Mock::given(method("POST")).and(BodyAt(LEVEL, json!("high"))).respond_with(answer("high")).expect(1).mount(&server).await;
        let client = LlmClient::new();
        let lines = log_of(&client);
        let config = gemini(&server.uri(), "gemini-3.5-flash-lite");
        assert_eq!(client.generate(&config, "K", &with_thinking(Thinking::Low)).await.unwrap().text(), "low");
        assert_eq!(client.generate(&config, "K", &with_thinking(Thinking::Low)).await.unwrap().text(), "low");
        assert_eq!(client.generate(&config, "K", &with_thinking(Thinking::High)).await.unwrap().text(), "high");
        assert_eq!(client.thinking_form(&format!("{}|gemini-3.5-flash-lite|Low", server.uri())), 1, "\"low\" is remembered for Low");
        assert_eq!(
            *lines.lock().unwrap(),
            ["Test (gemini-3.5-flash-lite): HTTP 400 (Request contains an invalid argument.) for thinkingLevel \"minimal\"; asking again with thinkingLevel \"low\""]
        );
    }

    #[tokio::test]
    async fn nothing_is_remembered_when_every_form_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(invalid_argument()).up_to_n_times(3).expect(3).mount(&server).await;
        Mock::given(method("POST")).and(BodyAt(BUDGET, json!(0))).respond_with(answer("back")).expect(1).mount(&server).await;
        let client = LlmClient::new();
        let lines = log_of(&client);
        let config = gemini(&server.uri(), "gemini-2.5-flash");
        let err = client.generate(&config, "K", &with_thinking(Thinking::Low)).await.unwrap_err();
        assert_eq!(err, LlmError::Http { status: 400, detail: "Request contains an invalid argument.".into() });
        let sent: Vec<Option<Value>> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap().pointer("/generationConfig/thinkingConfig").cloned())
            .collect();
        assert_eq!(sent, [Some(json!({ "thinkingBudget": 0 })), Some(json!({ "thinkingBudget": 128 })), None]);
        let notes = lines.lock().unwrap().clone();
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[1].ends_with("for thinkingBudget 128; asking again without thinking settings"), "{notes:?}");
        // The next request starts from the best form again.
        assert_eq!(client.generate(&config, "K", &with_thinking(Thinking::Low)).await.unwrap().text(), "back");
    }

    #[tokio::test]
    async fn gemini_retries_without_thinking_and_remembers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/models/test-model:generateContent"))
            .and(header("x-goog-api-key", "KEY"))
            .and(BodyHas("/generationConfig/thinkingConfig", true))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": { "message": "Thinking is not supported" } })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(BodyHas("/generationConfig/thinkingConfig", false))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "candidates": [{ "content": { "parts": [{ "text": "salam" }] } }] })))
            .expect(2)
            .mount(&server)
            .await;
        let client = LlmClient::new();
        let config = model(Provider::Gemini, &server.uri());
        assert_eq!(client.generate(&config, "KEY", &request()).await.unwrap().text(), "salam");
        // Second call goes straight to the working form.
        assert_eq!(client.generate(&config, "KEY", &request()).await.unwrap().text(), "salam");
    }

    #[tokio::test]
    async fn openai_switches_token_field_and_drops_effort() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(BodyHas("/reasoning_effort", true))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": { "message": "Unsupported parameter: reasoning_effort" } })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(BodyHas("/max_tokens", true))
            .and(BodyHas("/reasoning_effort", false))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": { "message": "Use max_completion_tokens instead of max_tokens" } })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(header("authorization", "Bearer sk-test"))
            .and(BodyHas("/max_completion_tokens", true))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "choices": [{ "message": { "content": "ok" } }] })))
            .mount(&server)
            .await;
        let config = model(Provider::Openai, &format!("{}/v1", server.uri()));
        let answer = LlmClient::new().generate(&config, "sk-test", &request()).await.unwrap();
        assert_eq!(answer.text(), "ok");
    }

    #[tokio::test]
    async fn other_errors_are_reported_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": { "message": "Invalid API key" } })))
            .expect(1)
            .mount(&server)
            .await;
        let err = LlmClient::new().generate(&model(Provider::Openai, &server.uri()), "bad", &request()).await.unwrap_err();
        assert_eq!(err, LlmError::Http { status: 401, detail: "Invalid API key".into() });
    }

    #[tokio::test]
    async fn echoed_key_is_hidden_in_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": { "message": "Incorrect API key provided: sk-secret-123456" } })))
            .mount(&server)
            .await;
        let err = LlmClient::new().generate(&model(Provider::Openai, &server.uri()), "sk-secret-123456", &request()).await.unwrap_err();
        assert_eq!(err, LlmError::Http { status: 401, detail: "Incorrect API key provided: ***".into() });
    }

    const GOOGLE_403: &str = "<!DOCTYPE html>\n<html lang=en>\n  <meta charset=utf-8>\n  <title>Error 403 (Forbidden)!!1</title>";

    #[tokio::test]
    async fn a_way_out_the_service_refuses_is_left_for_one_it_answers_and_never_kept() {
        // The proxy is another server that answers with Google's 403 page (a way out from a refused address).
        let refusing = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(403).set_body_string(GOOGLE_403)).mount(&refusing).await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "candidates": [{ "content": { "parts": [{ "text": "hi" }] } }] })))
            .mount(&server)
            .await;
        let client = LlmClient::new();
        let config = model(Provider::Gemini, &server.uri());
        client.set_proxy(Some(&refusing.uri()), std::slice::from_ref(&config.id)).unwrap();
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = lines.clone();
        client.set_log(move |line| seen.lock().unwrap().push(line));
        assert_eq!(client.generate(&config, "K", &request()).await.unwrap().text(), "hi");
        assert_eq!(client.generate(&config, "K", &request()).await.unwrap().text(), "hi");
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "the second request went the way that answered: {lines:?}");
        assert!(lines[0].contains("through the proxy: refused from this address"), "{}", lines[0]);
    }

    #[tokio::test]
    async fn refused_on_every_way_says_so_plainly() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(403).set_body_string(GOOGLE_403)).mount(&server).await;
        let err = LlmClient::new().generate(&model(Provider::Gemini, &server.uri()), "K", &request()).await.unwrap_err();
        assert!(err.is_blocked_here(), "{err}");
        assert!(err.to_string().contains("location is not supported without a VPN"), "{err}");
        assert!(!err.to_string().contains("<!DOCTYPE"), "no HTML page in the message: {err}");
    }

    #[tokio::test]
    async fn a_way_out_that_cannot_connect_is_left_for_one_that_can() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "candidates": [{ "content": { "parts": [{ "text": "hi" }] } }] })))
            .mount(&server)
            .await;
        let client = LlmClient::new();
        let config = model(Provider::Gemini, &server.uri());
        // The model goes through a proxy first, and nothing listens there (a VPN that is off).
        client.set_proxy(Some("http://127.0.0.1:9"), std::slice::from_ref(&config.id)).unwrap();
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = lines.clone();
        client.set_log(move |line| seen.lock().unwrap().push(line));
        assert_eq!(client.generate(&config, "K", &request()).await.unwrap().text(), "hi");
        assert_eq!(client.generate(&config, "K", &request()).await.unwrap().text(), "hi");
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "the second request went the way that worked: {lines:?}");
        assert!(lines[0].starts_with("Test (test-model): could not connect (through the proxy: "), "{}", lines[0]);
        assert!(lines[0].ends_with("reached it through the Windows proxy settings"), "{}", lines[0]);
    }

    #[tokio::test]
    async fn server_error_gets_one_retry() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "candidates": [{ "content": { "parts": [{ "text": "back" }] } }] })))
            .mount(&server)
            .await;
        let client = LlmClient::new();
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = lines.clone();
        client.set_log(move |line| seen.lock().unwrap().push(line));
        let text = client.generate(&model(Provider::Gemini, &server.uri()), "K", &request()).await.unwrap().text();
        assert_eq!(text, "back");
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("Test (test-model): busy (HTTP 503"), "the retry is in the log: {}", lines[0]);
    }

    #[tokio::test]
    async fn a_busy_server_is_retried_a_few_times_then_reported() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_json(json!({ "error": { "message": "high demand" } })))
            .expect(4)
            .mount(&server)
            .await;
        let err = LlmClient::new().generate(&model(Provider::Gemini, &server.uri()), "K", &request()).await.unwrap_err();
        assert_eq!(err, LlmError::Http { status: 503, detail: "high demand".into() });
    }

    #[test]
    fn long_rate_limit_waits_are_recognised() {
        assert!(long_wait("Quota exceeded for metric: ... requests per day limit exceeded on Free Tier. Please retry in 14h9m8s."));
        assert!(long_wait("Please retry in 120.5s."));
        assert!(!long_wait("Please retry in 2.3s."));
        assert!(!long_wait("Resource has been exhausted (e.g. check quota)."));
    }

    #[test]
    fn no_quota_on_the_key_is_not_waited_for() {
        assert!(long_wait("You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://aistudio.google.com/docs/rate-limits."));
        assert!(!long_wait("You exceeded your current quota, please check your plan and billing details. Please retry in 21.5s."), "a short wait is waited for");
        assert!(!long_wait("Resource has been exhausted (e.g. check quota)."));
    }

    #[tokio::test]
    async fn a_daily_quota_is_reported_without_retrying() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "error": { "message": "requests per day limit exceeded. Please retry in 14h9m8s." } })))
            .expect(1)
            .mount(&server)
            .await;
        let err = LlmClient::new().generate(&model(Provider::Gemini, &server.uri()), "K", &request()).await.unwrap_err();
        assert!(matches!(err, LlmError::Http { status: 429, .. }));
    }

    #[tokio::test]
    async fn missing_key_fails_fast() {
        let err = LlmClient::new().generate(&model(Provider::Gemini, "http://127.0.0.1:9"), " ", &request()).await.unwrap_err();
        assert!(matches!(err, LlmError::MissingKey(_)));
    }
}
