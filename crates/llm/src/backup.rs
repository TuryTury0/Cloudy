//! A backup model the user picked for when the chosen one is busy (overloaded, rate limited, out of quota,
//! unreachable, or refusing the user's location, as Google does from Iran when the VPN is off). Only those errors
//! switch: a wrong key or a wrong model id are the user's to fix and are reported as they are. A used-up quota
//! switches too (11-11: gemini-3.8-flash had no quota on the user's free key; with a backup picked, the assistant
//! should keep answering, and the label says why the backup answered). Once switched, the rest of the request stays on the backup (no long retries on
//! every round), and the label says which model answered and why.

use crate::client::{Generator, LlmError};
use crate::types::{GenerateRequest, GenerateResponse};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A model that refused the user's location is not asked again for this long: the refusal and its retries cost
/// 3-5 s at the start of every request (11-08, VPN not reaching Google), and the location does not change that fast.
const REFUSED_FOR: Duration = Duration::from_secs(10 * 60);

/// Models (by label) that refused the user's location, and until when they are skipped.
static REFUSED: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn refused_now(label: &str) -> bool {
    let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
    let map = refused.get_or_insert_with(HashMap::new);
    map.retain(|_, until| *until > Instant::now());
    map.contains_key(label)
}

fn remember_refusal(label: &str) {
    let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
    refused.get_or_insert_with(HashMap::new).insert(label.to_string(), Instant::now() + REFUSED_FOR);
}

impl LlmError {
    /// The model is busy or unreachable for now; another model may well answer.
    pub fn is_busy(&self) -> bool {
        match self {
            LlmError::Http { status, .. } => *status >= 500 || *status == 429 || self.is_blocked_here(),
            LlmError::Network(_) => true,
            _ => false,
        }
    }

    /// The service refuses requests from the user's country (Gemini from Iran without a VPN, 11-08: "HTTP 400: User
    /// location is not supported for the API use."; OpenAI: "unsupported_country_region_territory").
    pub fn is_blocked_here(&self) -> bool {
        let LlmError::Http { status: 400 | 403, detail } = self else { return false };
        let detail = detail.to_lowercase();
        [
            "location is not supported",
            "unsupported_country",
            "territory not supported",
            "not available in your country",
            "not supported in your country",
            "region is not supported",
            crate::client::BLOCKED_PAGE,
        ]
            .iter()
            .any(|text| detail.contains(text))
    }
}

pub struct WithBackup {
    pub primary: Arc<dyn Generator>,
    pub backup: Arc<dyn Generator>,
    switched: AtomicBool,
}

impl WithBackup {
    pub fn new(primary: Arc<dyn Generator>, backup: Arc<dyn Generator>) -> Self {
        Self { primary, backup, switched: AtomicBool::new(false) }
    }
}

#[async_trait]
impl Generator for WithBackup {
    async fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
        if !self.switched.load(Ordering::Relaxed) && refused_now(&self.primary.label()) {
            self.switched.store(true, Ordering::Relaxed);
        }
        if !self.switched.load(Ordering::Relaxed) {
            match self.primary.generate(request).await {
                Err(e) if e.is_busy() => {
                    if e.is_blocked_here() {
                        remember_refusal(&self.primary.label());
                    }
                    self.switched.store(true, Ordering::Relaxed);
                }
                other => return other,
            }
        }
        self.backup.generate(request).await
    }

    fn label(&self) -> String {
        if self.switched.load(Ordering::Relaxed) {
            format!("{} (backup: {} did not answer)", self.backup.label(), self.primary.label())
        } else {
            self.primary.label()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, Part};
    use std::sync::Mutex;

    struct Fixed {
        name: &'static str,
        answers: Mutex<Vec<Result<GenerateResponse, LlmError>>>,
        calls: Mutex<u32>,
    }

    impl Fixed {
        fn new(name: &'static str, answers: Vec<Result<GenerateResponse, LlmError>>) -> Arc<Self> {
            Arc::new(Self { name, answers: Mutex::new(answers), calls: Mutex::new(0) })
        }
        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl Generator for Fixed {
        async fn generate(&self, _: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
            *self.calls.lock().unwrap() += 1;
            let mut answers = self.answers.lock().unwrap();
            if answers.is_empty() { Ok(text(self.name)) } else { answers.remove(0) }
        }
        fn label(&self) -> String {
            self.name.into()
        }
    }

    fn text(t: &str) -> GenerateResponse {
        GenerateResponse { parts: vec![Part::text(t)], finish_reason: None, usage: None }
    }

    #[tokio::test]
    async fn a_location_refusal_switches_to_the_backup() {
        let refused = LlmError::Http { status: 400, detail: "User location is not supported for the API use.".into() };
        assert!(refused.is_blocked_here() && refused.is_busy());
        assert!(!LlmError::Http { status: 400, detail: "Invalid argument".into() }.is_busy(), "a bad request is not switched");
        // OpenAI's refusal, as post_with keeps it (only /error/message).
        assert!(LlmError::Http { status: 403, detail: "Country, region, or territory not supported".into() }.is_blocked_here());
        let primary = Fixed::new("gemini-refusing", vec![Err(refused)]);
        let backup = Fixed::new("gapgpt", vec![]);
        let model = WithBackup::new(primary.clone(), backup.clone());
        assert_eq!(model.generate(&request()).await.unwrap().parts, text("gapgpt").parts);
        assert_eq!((primary.calls(), backup.calls()), (1, 1));
        // The next request (a new WithBackup, as each request builds one) does not ask the refusing model again.
        let next = WithBackup::new(primary.clone(), backup.clone());
        assert_eq!(next.generate(&request()).await.unwrap().parts, text("gapgpt").parts);
        assert_eq!((primary.calls(), backup.calls()), (1, 2));
        assert!(next.label().contains("backup: gemini-refusing"));
    }

    fn request() -> GenerateRequest {
        GenerateRequest { system: String::new(), messages: vec![Message::user("hi")], tools: vec![], allow_tools: false, max_output_tokens: 10, thinking: None }
    }

    fn busy() -> LlmError {
        LlmError::Http { status: 503, detail: "This model is currently experiencing high demand.".into() }
    }

    #[tokio::test]
    async fn a_busy_model_hands_the_request_to_the_backup_for_good() {
        let primary = Fixed::new("Gemini", vec![Err(busy())]);
        let backup = Fixed::new("GapGPT", vec![]);
        let model = WithBackup::new(primary.clone(), backup.clone());
        assert_eq!(model.generate(&request()).await.unwrap().text(), "GapGPT");
        assert_eq!(model.label(), "GapGPT (backup: Gemini did not answer)");
        model.generate(&request()).await.unwrap();
        assert_eq!((primary.calls(), backup.calls()), (1, 2), "later rounds go straight to the backup");
    }

    #[tokio::test]
    async fn key_and_model_errors_are_not_hidden() {
        for error in [LlmError::Http { status: 401, detail: "API key not valid".into() }, LlmError::Http { status: 404, detail: "model not found".into() }] {
            let primary = Fixed::new("Gemini", vec![Err(error)]);
            let backup = Fixed::new("GapGPT", vec![]);
            let model = WithBackup::new(primary, backup.clone());
            assert!(model.generate(&request()).await.is_err());
            assert_eq!(backup.calls(), 0);
            assert_eq!(model.label(), "Gemini");
        }
        assert!(LlmError::Network("timed out".into()).is_busy());
        assert!(LlmError::Http { status: 429, detail: "Please retry in 2.3s.".into() }.is_busy());
        assert!(LlmError::Http { status: 429, detail: "Quota exceeded: requests per day".into() }.is_busy(), "a used-up quota switches to the backup");
        assert!(LlmError::Http { status: 429, detail: "You exceeded your current quota, please check your plan and billing details.".into() }.is_busy());
    }
}
