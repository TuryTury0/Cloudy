//! A live picture of what the assistant is doing and did since the app started, built from the event bus. The
//! get_activity tool reads it, so the user can ask "what are you doing?", "what happened?" or "why did it fail?".
//! Ported from v1 activity.js (jobs are not separate here yet; a running request plays that part).

use crate::events::{CoreEvent, EventBus};
use crate::lang::tr;
use crate::tools::{Tool, ToolCtx, short};
use anytask_llm::ToolSpec;
use async_trait::async_trait;
use chrono::{DateTime, Local};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const MAX_REQUESTS: usize = 12;
const MAX_MODEL_PROBLEMS: usize = 6;
const MAX_NOTES: usize = 8;
const TEXT_CHARS: usize = 160;

#[derive(Debug, Clone)]
struct Step {
    call_id: String,
    label: String,
    /// None while running.
    ok: Option<bool>,
    detail: String,
}

#[derive(Debug, Clone)]
struct Request {
    turn_id: u64,
    user: String,
    started: DateTime<Local>,
    steps: Vec<Step>,
    /// None while running.
    outcome: Option<String>,
    tokens: u64,
}

#[derive(Default)]
struct State {
    requests: VecDeque<Request>,
    model_problems: VecDeque<String>,
    notes: VecDeque<String>,
    voice_open: bool,
}

#[derive(Default)]
pub struct Activity {
    state: Mutex<State>,
}

impl Activity {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Keeps the picture up to date from now on: listens at once, and the returned task (for the app's runtime)
    /// reads the events.
    pub fn follow(self: &Arc<Self>, events: &EventBus) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut rx = events.subscribe();
        let activity = self.clone();
        async move {
            loop {
                match rx.recv().await {
                    Ok(event) => activity.record(&event),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    pub fn record(&self, event: &CoreEvent) {
        self.record_at(event, Local::now());
    }

    fn record_at(&self, event: &CoreEvent, now: DateTime<Local>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let time = now.format("%H:%M:%S");
        match event {
            CoreEvent::TurnStarted { turn_id, user } => {
                state.requests.push_back(Request { turn_id: *turn_id, user: short(user, TEXT_CHARS), started: now, steps: Vec::new(), outcome: None, tokens: 0 });
                while state.requests.len() > MAX_REQUESTS {
                    state.requests.pop_front();
                }
            }
            CoreEvent::ToolStarted { turn_id, call_id, label, .. } => {
                if let Some(request) = state.request(*turn_id) {
                    request.steps.push(Step { call_id: call_id.clone(), label: label.clone(), ok: None, detail: String::new() });
                }
            }
            CoreEvent::ToolProgress { turn_id, call_id, text } => {
                if let Some(step) = state.request(*turn_id).and_then(|r| r.steps.iter_mut().rev().find(|s| s.call_id == *call_id)) {
                    step.detail = short(text, TEXT_CHARS);
                }
            }
            CoreEvent::ToolFinished { turn_id, call_id, ok, summary, .. } => {
                if let Some(step) = state.request(*turn_id).and_then(|r| r.steps.iter_mut().rev().find(|s| s.call_id == *call_id)) {
                    step.ok = Some(*ok);
                    if !*ok {
                        step.detail = short(summary, TEXT_CHARS);
                    }
                }
            }
            CoreEvent::Answer { turn_id, text } => state.finish(*turn_id, format!("answered: \"{}\"", short(text, TEXT_CHARS))),
            CoreEvent::Error { turn_id: Some(turn_id), message } => state.finish(*turn_id, format!("failed: {}", short(message, TEXT_CHARS))),
            CoreEvent::Error { turn_id: None, message } => state.note(format!("{time} problem: {}", short(message, TEXT_CHARS))),
            CoreEvent::Cancelled { turn_id } => state.finish(*turn_id, "stopped by the user".into()),
            CoreEvent::TurnUsage { turn_id, input, output, thinking, .. } => {
                if let Some(request) = state.request(*turn_id) {
                    request.tokens = input + output + thinking;
                }
            }
            CoreEvent::ModelUsed { label, ok: false, reason } => {
                state.model_problems.push_back(format!("{time} {label} failed: {}", short(reason.as_deref().unwrap_or("unknown reason"), TEXT_CHARS)));
                while state.model_problems.len() > MAX_MODEL_PROBLEMS {
                    state.model_problems.pop_front();
                }
            }
            CoreEvent::Log { source, text, .. } if source == "voice" || source == "mcp" => {
                if source == "voice" && text.starts_with("voice session open") {
                    state.voice_open = true;
                } else if source == "voice" && text.starts_with("voice session closed") {
                    state.voice_open = false;
                }
                // Server chatter is for the log file; only connections and failures are worth telling.
                if source == "voice" || text.contains("connected") || text.contains("FAILED") || text.contains("stopped") {
                    state.note(format!("{time} {source}: {}", short(text, TEXT_CHARS)));
                }
            }
            _ => {}
        }
    }

    /// A short report for the model. The newest running request is the one asking, so it is left out.
    pub fn report(&self) -> String {
        self.report_at(Local::now())
    }

    fn report_at(&self, now: DateTime<Local>) -> String {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let asking = state.requests.iter().rev().find(|r| r.outcome.is_none()).map(|r| r.turn_id);
        let mut lines = vec![format!("Now: {}. Voice conversation: {}.", now.format("%H:%M:%S"), if state.voice_open { "open" } else { "closed" })];

        let running: Vec<&Request> = state.requests.iter().filter(|r| r.outcome.is_none() && Some(r.turn_id) != asking).collect();
        if running.is_empty() {
            lines.push("Running: nothing else.".into());
        } else {
            for r in running {
                lines.push(format!("Running for {} s: \"{}\"{}", (now - r.started).num_seconds().max(0), r.user, steps(r)));
            }
        }

        let finished: Vec<&Request> = state.requests.iter().rev().filter(|r| r.outcome.is_some()).collect();
        if !finished.is_empty() {
            lines.push("Recent requests (newest first):".into());
            for r in finished {
                let tokens = if r.tokens > 0 { format!(" [{} tokens]", r.tokens) } else { String::new() };
                lines.push(format!("- {} \"{}\" -> {}{}{tokens}", r.started.format("%H:%M"), r.user, r.outcome.as_deref().unwrap_or_default(), steps(r)));
            }
        }
        if !state.model_problems.is_empty() {
            lines.push("Model problems:".into());
            lines.extend(state.model_problems.iter().map(|p| format!("- {p}")));
        }
        if !state.notes.is_empty() {
            lines.push("Other events:".into());
            lines.extend(state.notes.iter().map(|n| format!("- {n}")));
        }
        lines.join("\n")
    }
}

impl State {
    fn request(&mut self, turn_id: u64) -> Option<&mut Request> {
        self.requests.iter_mut().rev().find(|r| r.turn_id == turn_id)
    }

    fn finish(&mut self, turn_id: u64, outcome: String) {
        if let Some(request) = self.request(turn_id) {
            request.outcome = Some(outcome);
            // Steps still marked running ended with the request.
            for step in request.steps.iter_mut().filter(|s| s.ok.is_none()) {
                step.ok = Some(false);
            }
        }
    }

    fn note(&mut self, note: String) {
        self.notes.push_back(note);
        while self.notes.len() > MAX_NOTES {
            self.notes.pop_front();
        }
    }
}

fn steps(request: &Request) -> String {
    if request.steps.is_empty() {
        return String::new();
    }
    let list: Vec<String> = request
        .steps
        .iter()
        .map(|s| {
            let status = match s.ok {
                None => "running",
                Some(true) => "ok",
                Some(false) => "failed",
            };
            if s.detail.is_empty() { format!("{} ({status})", s.label) } else { format!("{} ({status}: {})", s.label, s.detail) }
        })
        .collect();
    format!("; steps: {}", list.join(", "))
}

pub struct GetActivity {
    pub activity: Arc<Activity>,
}

#[async_trait]
impl Tool for GetActivity {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_activity".into(),
            description: "What the assistant is doing now and did since the app started: running requests and their steps, recent requests with results and errors, model failures with reasons, voice and MCP events. Use when the user asks what you are doing, what happened, or why something failed.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    fn label(&self, _args: &Value) -> String {
        tr("Looking through recent work", "مرور کارهای اخیر").into()
    }

    async fn run(&self, _args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        Ok(json!({ "activity": self.activity.report() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 30, 14, 0, s).unwrap()
    }

    #[test]
    fn report_shows_running_work_results_and_failures() {
        let a = Activity::default();
        let started = |turn_id, user: &str| CoreEvent::TurnStarted { turn_id, user: user.into() };
        a.record_at(&started(1, "gold price"), at(0));
        a.record_at(&CoreEvent::ToolStarted { turn_id: 1, call_id: "c1".into(), name: "web_search".into(), label: "جستجو: gold".into(), args: json!({}) }, at(1));
        a.record_at(&CoreEvent::ToolFinished { turn_id: 1, call_id: "c1".into(), name: "web_search".into(), ok: true, summary: "{}".into() }, at(2));
        a.record_at(&CoreEvent::TurnUsage { turn_id: 1, calls: 2, input: 900, output: 40, thinking: 0 }, at(3));
        a.record_at(&CoreEvent::Answer { turn_id: 1, text: "262 million".into() }, at(3));

        a.record_at(&started(2, "open spotify"), at(10));
        a.record_at(&CoreEvent::ToolStarted { turn_id: 2, call_id: "d".into(), name: "desktop_task".into(), label: "Spotify".into(), args: json!({}) }, at(11));
        a.record_at(&CoreEvent::ToolProgress { turn_id: 2, call_id: "d".into(), text: "clicking #12 Play".into() }, at(12));

        a.record_at(&started(3, "what are you doing?"), at(20));
        a.record_at(&CoreEvent::ModelUsed { label: "Gemini".into(), ok: false, reason: Some("HTTP 429 quota".into()) }, at(21));
        a.record_at(&CoreEvent::Log { turn_id: None, source: "mcp".into(), text: "github: FAILED token missing".into() }, at(22));
        a.record_at(&CoreEvent::Log { turn_id: None, source: "mcp".into(), text: "github: some server chatter".into() }, at(22));

        let report = a.report_at(at(25));
        assert!(report.contains("Running for 15 s: \"open spotify\"; steps: Spotify (running: clicking #12 Play)"), "{report}");
        assert!(!report.contains("what are you doing"), "the asking request is left out: {report}");
        assert!(report.contains("- 14:00 \"gold price\" -> answered: \"262 million\"; steps: جستجو: gold (ok) [940 tokens]"), "{report}");
        assert!(report.contains("Gemini failed: HTTP 429 quota"));
        assert!(report.contains("github: FAILED token missing"));
        assert!(!report.contains("chatter"));
    }

    #[test]
    fn stopped_requests_close_their_steps_and_voice_state_is_tracked() {
        let a = Activity::default();
        a.record_at(&CoreEvent::Log { turn_id: None, source: "voice".into(), text: "voice session open".into() }, at(0));
        a.record_at(&CoreEvent::TurnStarted { turn_id: 1, user: "notepad".into() }, at(1));
        a.record_at(&CoreEvent::ToolStarted { turn_id: 1, call_id: "d".into(), name: "desktop_task".into(), label: "Notepad".into(), args: json!({}) }, at(2));
        a.record_at(&CoreEvent::Cancelled { turn_id: 1 }, at(3));
        let report = a.report_at(at(4));
        assert!(report.contains("Voice conversation: open."));
        assert!(report.contains("-> stopped by the user; steps: Notepad (failed)"), "{report}");
        a.record_at(&CoreEvent::Log { turn_id: None, source: "voice".into(), text: "voice session closed: quiet for 20 s".into() }, at(5));
        assert!(a.report_at(at(6)).contains("Voice conversation: closed."));
    }
}
