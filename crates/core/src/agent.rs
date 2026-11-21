//! The brain: runs one request through the chosen model and the tools, and reports every step on the event bus.
//!
//! Ported from v1 BrainAgent, with its lessons:
//! - only the model the user chose answers; a failure is reported with its reason, never hidden by switching,
//! - at most `max_rounds` model calls and `max_tool_calls` tool calls; the last round must answer in text,
//! - the history carries what was done ("[actions: ...]") so follow-ups like "that song" resolve,
//! - a new request or Stop cancels the running one at once, even in the middle of a model call or a tool.

use crate::events::{CoreEvent, EventBus};
use crate::prompts::{Identity, system_prompt};
use crate::state::{AssistantState, StateUpdate};
use crate::tools::{Confirmer, Registry, ToolCtx, short};
use crate::learn::Learner;
use anytask_llm::{GenerateRequest, GenerateResponse, Generator, LlmError, Message, Part, Role, Thinking, Usage};
use anytask_storage::Memory;
use anytask_storage::memory::with_actions;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_rounds: usize,
    pub max_tool_calls: usize,
    /// Enough for a tool call or a short answer; keeps replies fast.
    pub max_output_tokens: u32,
    pub history_turns: usize,
    pub history_chars: usize,
    pub tool_result_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        // Long jobs (look something up, then write it, then save it) need room; the last round still has to answer.
        Self { max_rounds: 12, max_tool_calls: 24, max_output_tokens: 1500, history_turns: 8, history_chars: 600, tool_result_chars: 40_000 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Answered,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnOutcome {
    pub turn_id: u64,
    pub status: TurnStatus,
    /// The answer, or what went wrong.
    pub text: String,
    /// What was done, as short labels ("Searching: gold price").
    pub actions: Vec<String>,
    /// Tokens of every model call in this request.
    pub usage: Usage,
}

/// Added to a model's refusal of the user's location.
fn blocked_here() -> &'static str {
    crate::lang::tr(
        " — this service does not answer from here: turn the VPN on, or choose a backup model (such as GapGPT) in Settings > Models.",
        " — این سرویس از اینجا جواب نمی‌دهد: VPN را روشن کنید، یا در تنظیمات > مدل‌ها یک مدل پشتیبان (مثل GapGPT) انتخاب کنید.",
    )
}

/// Wraps the request's model to count the tokens of every call (the brain's rounds and tools that ask the model,
/// such as screen steps) and report each one.
pub(crate) struct Metered {
    pub(crate) inner: Arc<dyn Generator>,
    pub(crate) events: EventBus,
    pub(crate) turn_id: u64,
    pub(crate) total: Mutex<(u32, Usage)>,
}

#[async_trait::async_trait]
impl Generator for Metered {
    async fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
        let answer = self.inner.generate(request).await;
        if let Ok(response) = &answer {
            let usage = response.usage.unwrap_or_default();
            if let Ok(mut total) = self.total.lock() {
                total.0 += 1;
                total.1.add(usage);
            }
            if response.usage.is_some() {
                self.events.emit(CoreEvent::Usage { turn_id: self.turn_id, label: self.inner.label(), input: usage.input, output: usage.output, thinking: usage.thinking, cached: usage.cached });
            }
        }
        answer
    }

    fn label(&self) -> String {
        self.inner.label()
    }
}

pub(crate) enum Ending {
    Answer(String),
    Failed(String),
    Cancelled,
}

pub struct Agent {
    pub(crate) tools: RwLock<Registry>,
    /// What this PC has (default browser, apps by kind), for the prompt.
    pc_profile: RwLock<String>,
    memory: Arc<Memory>,
    events: EventBus,
    confirm: Arc<dyn Confirmer>,
    pub(crate) limits: Limits,
    pub(crate) next_turn: AtomicU64,
    current: Mutex<Option<(u64, CancellationToken)>>,
    /// Train mode: what worked and what went wrong, fed back into later prompts.
    pub(crate) learner: Option<Arc<Learner>>,
    /// The last finished request, to learn from it when the next message corrects it.
    last_turn: Mutex<Option<LastTurn>>,
    /// The job a voice conversation is running (its tools or a job handed to the brain), so Stop can end it
    /// and keep the conversation open.
    pub(crate) work_stop: Mutex<Option<CancellationToken>>,
}

struct LastTurn {
    at: i64,
    prompt: String,
    actions: Vec<String>,
    result: String,
}

/// A correction must come soon after the answer it corrects.
const CORRECTION_WINDOW_MS: i64 = 5 * 60 * 1000;

impl Agent {
    pub fn new(tools: Registry, memory: Arc<Memory>, events: EventBus, confirm: Arc<dyn Confirmer>) -> Self {
        Self {
            tools: RwLock::new(tools),
            pc_profile: RwLock::new(String::new()),
            memory,
            events,
            confirm,
            limits: Limits::default(),
            next_turn: AtomicU64::new(0),
            current: Mutex::new(None),
            learner: None,
            last_turn: Mutex::new(None),
            work_stop: Mutex::new(None),
        }
    }

    /// Turns on learning from use (train mode); the learner's own switch can still turn it off.
    pub fn with_learner(mut self, learner: Arc<Learner>) -> Self {
        self.learner = Some(learner);
        self
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Replaces the tool set (for example when MCP servers connect); the next request uses it.
    pub fn set_tools(&self, tools: Registry) {
        *self.tools.write().unwrap_or_else(|e| e.into_inner()) = tools;
    }

    /// Sets what this PC has (from the app scan); the next request uses it.
    pub fn set_pc_profile(&self, profile: String) {
        *self.pc_profile.write().unwrap_or_else(|e| e.into_inner()) = profile;
    }

    pub fn pc_profile(&self) -> String {
        self.pc_profile.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn events(&self) -> &EventBus {
        &self.events
    }

    pub fn memory(&self) -> &Arc<Memory> {
        &self.memory
    }

    /// Cancels the running request. Returns false when nothing was running.
    pub fn stop(&self) -> bool {
        match self.slot().as_ref() {
            Some((_, token)) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Stops the job a voice conversation is running; the conversation stays open. False when none runs.
    pub fn stop_work(&self) -> bool {
        match self.work_stop.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            Some(token) if !token.is_cancelled() => {
                token.cancel();
                true
            }
            _ => false,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.slot().is_some()
    }

    /// Runs one request to the end. A request that is still running is cancelled first.
    pub async fn run(&self, model: Arc<dyn Generator>, identity: &Identity, prompt: &str) -> TurnOutcome {
        let turn_id = self.next_turn.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = CancellationToken::new();
        if let Some((_, previous)) = self.slot().replace((turn_id, cancel.clone())) {
            previous.cancel();
        }
        self.events.emit(CoreEvent::TurnStarted { turn_id, user: prompt.to_string() });
        self.state(StateUpdate::new(AssistantState::Thinking).with_detail(short(prompt, 80)));

        let mut actions = Vec::new();
        let metered = Arc::new(Metered { inner: model, events: self.events.clone(), turn_id, total: Mutex::new((0, Usage::default())) });
        let model: Arc<dyn Generator> = metered.clone();
        let ending = match crate::quick::parse(prompt) {
            Some(quick) => self.quick(turn_id, &model, identity, prompt, quick, &cancel, &mut actions).await,
            None => self.work(turn_id, &model, identity, prompt, prompt, &cancel, &mut actions).await,
        };
        let (calls, usage) = metered.total.lock().map(|t| *t).unwrap_or_default();
        if calls > 0 {
            self.events.emit(CoreEvent::TurnUsage { turn_id, calls, input: usage.input, output: usage.output, thinking: usage.thinking });
        }
        let (status, text) = match ending {
            Ending::Answer(text) => {
                let _ = self.memory.add_turn(now_ms(), prompt, &text, &actions);
                self.events.emit(CoreEvent::Answer { turn_id, text: text.clone() });
                self.state(StateUpdate::new(AssistantState::Answer).with_detail(text.clone()));
                (TurnStatus::Answered, text)
            }
            Ending::Failed(message) => {
                if !actions.is_empty() {
                    let _ = self.memory.add_turn(now_ms(), prompt, &format!("(failed: {message})"), &actions);
                }
                self.events.emit(CoreEvent::Error { turn_id: Some(turn_id), message: message.clone() });
                self.state(StateUpdate::new(AssistantState::Error).with_detail(message.clone()));
                (TurnStatus::Failed, message)
            }
            Ending::Cancelled => {
                if !actions.is_empty() {
                    let _ = self.memory.add_turn(now_ms(), prompt, "(stopped by the user)", &actions);
                }
                self.events.emit(CoreEvent::Cancelled { turn_id });
                // A newer request owns the island now; only a plain Stop returns it to rest.
                if self.is_current(turn_id) {
                    self.state(StateUpdate::new(AssistantState::Idle));
                }
                (TurnStatus::Cancelled, String::new())
            }
        };
        let mut slot = self.slot();
        if slot.as_ref().is_some_and(|(id, _)| *id == turn_id) {
            *slot = None;
        }
        self.learn(metered.inner.clone(), turn_id, prompt, &actions, status, &text);
        TurnOutcome { turn_id, status, text, actions, usage }
    }

    /// A short command known without the model (quick.rs): its tool runs at once and the answer comes from the
    /// result. When the tool is missing, fails or says something the short answers do not cover, the model gets the
    /// message with what was already done, so it words the answer or tries another way without doing it twice.
    #[allow(clippy::too_many_arguments)]
    async fn quick(
        &self,
        turn_id: u64,
        model: &Arc<dyn Generator>,
        identity: &Identity,
        prompt: &str,
        quick: crate::quick::Quick,
        cancel: &CancellationToken,
        actions: &mut Vec<String>,
    ) -> Ending {
        if quick.tool == crate::quick::CLOCK {
            return Ending::Answer(crate::quick::clock(&quick, chrono::Local::now()));
        }
        let tools = self.tools.read().unwrap_or_else(|e| e.into_inner()).clone();
        if tools.get(quick.tool).is_none() {
            return self.work(turn_id, model, identity, prompt, prompt, cancel, actions).await;
        }
        let started = std::time::Instant::now();
        let call_id = format!("quick-{turn_id}");
        let Some(result) = self.call_tool(turn_id, &tools, (&call_id, quick.tool, quick.args.clone()), prompt, cancel, model, actions).await else {
            return Ending::Cancelled;
        };
        self.events.emit(CoreEvent::Log {
            turn_id: Some(turn_id),
            source: "quick".into(),
            text: format!("{} {} without the model in {:.1} s", quick.tool, quick.args, started.elapsed().as_secs_f32()),
        });
        match crate::quick::answer(&quick, &result) {
            Some(text) => Ending::Answer(text),
            None => {
                let done = format!(
                    "{prompt}\n\n[Already done for this message: {} {} -> {}. Answer from this result; use another tool only if it failed.]",
                    quick.tool,
                    quick.args,
                    short(&result.to_string(), 600)
                );
                self.work(turn_id, model, identity, &done, prompt, cancel, actions).await
            }
        }
    }

    /// `said` is the user's own words (the prompt itself, or the voice model's question it rewrote into `prompt`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn work(
        &self,
        turn_id: u64,
        model: &Arc<dyn Generator>,
        identity: &Identity,
        prompt: &str,
        said: &str,
        cancel: &CancellationToken,
        actions: &mut Vec<String>,
    ) -> Ending {
        let tools = self.tools.read().unwrap_or_else(|e| e.into_inner()).clone();
        let specs = tools.specs();
        let background = self.memory.context_summary(20).unwrap_or_default();
        let mut system = system_prompt(identity, chrono::Local::now(), &background, &tools.names(), &self.pc_profile());
        if let Some(hints) = self.learner.as_ref().map(|l| l.request_hints(prompt)).filter(|h| !h.is_empty()) {
            system.push_str("\n\nLEARNED ON THIS PC (from earlier requests; follow it unless the user asks otherwise):\n");
            system.push_str(&hints);
        }
        let mut messages = self.history();
        messages.push(Message::user(prompt));

        let mut tool_calls = 0;
        let mut model_reported = false;
        for round in 1..=self.limits.max_rounds {
            let last = round == self.limits.max_rounds || tool_calls >= self.limits.max_tool_calls;
            let request = GenerateRequest {
                system: system.clone(),
                messages: messages.clone(),
                tools: specs.clone(),
                allow_tools: !last && !specs.is_empty(),
                max_output_tokens: self.limits.max_output_tokens,
                thinking: None,
            };
            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ending::Cancelled,
                response = model.generate(&request) => response,
            };
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    let reason = error.to_string();
                    self.events.emit(CoreEvent::ModelUsed { label: model.label(), ok: false, reason: Some(reason.clone()) });
                    // Said in the user's language what to do: the raw refusal reads like a bug (11-08, VPN off).
                    let hint = if error.is_blocked_here() { blocked_here() } else { "" };
                    return Ending::Failed(format!("{}: {reason}{hint}", model.label()));
                }
            };
            if !model_reported {
                model_reported = true;
                self.events.emit(CoreEvent::ModelUsed { label: model.label(), ok: true, reason: None });
            }

            let calls: Vec<(String, String, Value)> =
                response.tool_calls().into_iter().map(|(id, name, args)| (id.to_string(), name.to_string(), args.clone())).collect();
            if calls.is_empty() || last {
                let text = response.text();
                return if !text.is_empty() {
                    Ending::Answer(text)
                } else if actions.last().is_some_and(|a| a.ends_with("(failed)")) {
                    Ending::Answer(not_done(said).into())
                } else if !actions.is_empty() {
                    Ending::Answer(done(said).into())
                } else {
                    Ending::Failed(match response.finish_reason {
                        Some(reason) => format!("The model returned an empty answer ({reason})."),
                        None => "The model returned an empty answer.".into(),
                    })
                };
            }

            messages.push(Message { role: Role::Model, parts: response.parts });
            let mut results = Vec::with_capacity(calls.len());
            for (id, name, args) in calls {
                let result = if tool_calls >= self.limits.max_tool_calls {
                    json!({ "success": false, "error": "The tool call limit for this request was reached. Answer with what you already have." })
                } else {
                    tool_calls += 1;
                    match self.call_tool(turn_id, &tools, (&id, &name, args), said, cancel, model, actions).await {
                        Some(result) => result,
                        None => return Ending::Cancelled,
                    }
                };
                results.push(Part::ToolResult { id, name, result });
            }
            messages.push(Message { role: Role::User, parts: results });
            self.state(StateUpdate::new(AssistantState::Thinking));
        }
        Ending::Failed("No rounds are allowed (max_rounds is 0).".into())
    }

    /// Runs one tool call (id, name, arguments); `said` is the user's own words. None means the request was cancelled
    /// meanwhile.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call_tool(
        &self,
        turn_id: u64,
        tools: &Registry,
        (call_id, name, args): (&str, &str, Value),
        said: &str,
        cancel: &CancellationToken,
        model: &Arc<dyn Generator>,
        actions: &mut Vec<String>,
    ) -> Option<Value> {
        let Some(tool) = tools.get(name) else {
            return Some(json!({ "success": false, "error": format!("Tool {name} is not available.") }));
        };
        let label = tool.label(&args);
        self.events.emit(CoreEvent::ToolStarted { turn_id, call_id: call_id.into(), name: name.into(), label: label.clone(), args: args.clone() });
        self.state(StateUpdate::new(AssistantState::Working).with_detail(label.clone()));
        let events = self.events.clone();
        let progress_id = call_id.to_string();
        let progress = Arc::new(move |text: String| events.emit(CoreEvent::ToolProgress { turn_id, call_id: progress_id.clone(), text }));
        let log_events = self.events.clone();
        let source = name.to_string();
        let log = Arc::new(move |text: String| log_events.emit(CoreEvent::Log { turn_id: Some(turn_id), source: source.clone(), text }));
        let ctx = ToolCtx { cancel: cancel.clone(), confirm: self.confirm.clone(), now_ms: now_ms(), model: model.clone(), progress, log, request: said.trim().to_string() };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            result = tool.run(args, &ctx) => Some(result),
        };
        let finished = |ok: bool, summary: String| CoreEvent::ToolFinished { turn_id, call_id: call_id.into(), name: name.into(), ok, summary };
        match result {
            None => {
                self.events.emit(finished(false, "Stopped".into()));
                None
            }
            Some(Ok(value)) => {
                // A tool that ran but reports it did not reach its goal (desktop_task "success": false) failed: the
                // log says so, and train mode must not keep it as a way that worked.
                let failed = value.get("success") == Some(&json!(false));
                self.events.emit(finished(!failed, short(&value.to_string(), 160)));
                actions.push(if failed { format!("{label} (failed)") } else { label });
                Some(bound(value, self.limits.tool_result_chars))
            }
            Some(Err(error)) => {
                self.events.emit(finished(false, error.clone()));
                actions.push(format!("{label} (failed)"));
                Some(json!({ "success": false, "error": error }))
            }
        }
    }

    /// Train mode: records the request, and in the background writes a lesson when it failed or when this message
    /// corrects the previous answer ("na, to app play kon").
    fn learn(&self, model: Arc<dyn Generator>, turn_id: u64, prompt: &str, actions: &[String], status: TurnStatus, text: &str) {
        if status == TurnStatus::Cancelled {
            // A stopped job neither worked nor failed, but the next message may correct it ("na, ..."): that must be
            // read as about this job, not the one before (training round 4: a correction right after a stopped job
            // wrote no lesson).
            if self.learner.as_ref().is_some_and(|l| l.enabled()) {
                let stopped = LastTurn { at: now_ms(), prompt: prompt.to_string(), actions: actions.to_vec(), result: "(stopped by the user)".into() };
                *self.last_turn.lock().unwrap_or_else(|e| e.into_inner()) = Some(stopped);
            }
            return;
        }
        let stumbled = actions.iter().any(|a| a.ends_with("(failed)"));
        self.learn_from(model, turn_id, prompt, actions, status == TurnStatus::Answered && !stumbled, status == TurnStatus::Failed || stumbled, text);
    }

    /// Train mode for any finished request (typed to the brain, or a voice exchange): records it, and in the
    /// background writes a lesson when `review` (it went wrong) or when this message corrects the previous answer.
    /// Voice exchanges are most requests, so they must teach too: a correction there wrote no lesson before.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn learn_from(&self, model: Arc<dyn Generator>, turn_id: u64, prompt: &str, actions: &[String], ok: bool, review: bool, text: &str) {
        let Some(learner) = self.learner.clone().filter(|l| l.enabled()) else { return };
        let now = now_ms();
        learner.record_request(prompt, actions, ok, now);
        let current = LastTurn { at: now, prompt: prompt.to_string(), actions: actions.to_vec(), result: text.to_string() };
        let previous = self.last_turn.lock().unwrap_or_else(|e| e.into_inner()).replace(current);

        let mut reviews: Vec<(String, Vec<String>, String, Option<String>)> = Vec::new();
        if review {
            reviews.push((prompt.to_string(), actions.to_vec(), text.to_string(), None));
        }
        if let Some(previous) = previous.filter(|p| now - p.at < CORRECTION_WINDOW_MS && crate::learn::is_correction(prompt)) {
            reviews.push((previous.prompt, previous.actions, previous.result, Some(prompt.to_string())));
        }
        for (request, done, result, correction) in reviews {
            let (learner, model, events) = (learner.clone(), model.clone(), self.events.clone());
            tokio::spawn(async move {
                let ask = crate::learn::lesson_prompt(&request, &done, &result, correction.as_deref());
                let req = GenerateRequest { system: String::new(), messages: vec![Message::user(ask)], tools: Vec::new(), allow_tools: false, max_output_tokens: 300, thinking: Some(Thinking::Low) };
                match model.generate(&req).await {
                    Ok(response) => {
                        if let Some(u) = response.usage {
                            events.emit(CoreEvent::Usage { turn_id, label: format!("{} (lesson)", model.label()), input: u.input, output: u.output, thinking: u.thinking, cached: u.cached });
                        }
                        let lesson = response.text();
                        if !lesson.trim().is_empty() && !lesson.trim().eq_ignore_ascii_case("none") {
                            learner.add_lesson(&lesson, &request, now_ms());
                            events.emit(CoreEvent::Log { turn_id: Some(turn_id), source: "learn".into(), text: format!("lesson: {}", lesson.trim()) });
                        }
                    }
                    Err(e) => events.emit(CoreEvent::Log { turn_id: Some(turn_id), source: "learn".into(), text: format!("lesson FAILED: {e}") }),
                }
            });
        }
    }

    /// The last turns as conversation, oldest first, each side clipped.
    fn history(&self) -> Vec<Message> {
        let turns = self.memory.recent_turns(self.limits.history_turns).unwrap_or_default();
        let mut messages = Vec::with_capacity(turns.len() * 2);
        let now = now_ms();
        for turn in turns {
            let user = clip(&turn.user, self.limits.history_chars);
            let reply = clip(&with_actions(&turn.assistant, &turn.actions), self.limits.history_chars);
            if !user.is_empty() && !reply.is_empty() {
                // When it was sent, so a request from yesterday is not taken as the job in hand today.
                messages.push(Message::user(format!("[sent {}] {user}", crate::prompts::when(turn.at, now))));
                messages.push(Message::model(reply));
            }
        }
        messages
    }

    pub(crate) fn state(&self, update: StateUpdate) {
        self.events.emit(CoreEvent::State(update));
    }

    fn slot(&self) -> MutexGuard<'_, Option<(u64, CancellationToken)>> {
        self.current.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_current(&self, turn_id: u64) -> bool {
        self.slot().as_ref().is_some_and(|(id, _)| *id == turn_id)
    }
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max { text.to_string() } else { format!("{}…", text.chars().take(max).collect::<String>()) }
}

/// The answer when the work was done but the model said nothing. It is shown, read aloud and kept in the history,
/// so it is Persian for a Persian request even in an English app: an English "Done." (11-03 16:06) pulled later
/// answers towards English.
pub(crate) fn done(user: &str) -> &'static str {
    crate::lang::for_user(user, "Done.", "انجام شد.")
}
/// The same, when the last step failed.
pub(crate) fn not_done(user: &str) -> &'static str {
    crate::lang::for_user(user, "That did not work.", "انجام نشد.")
}

/// Keeps a tool result under `max` characters so one big page cannot fill the model's context.
pub(crate) fn bound(value: Value, max: usize) -> Value {
    let text = value.to_string();
    if text.chars().count() <= max {
        value
    } else {
        json!({ "truncated": true, "text": text.chars().take(max).collect::<String>() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{DenyAll, Tool};
    use anytask_llm::{GenerateResponse, LlmError, ToolSpec};
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::time::Duration;

    /// A model that answers from a script and remembers what it was asked.
    #[derive(Default)]
    struct Script {
        replies: Mutex<VecDeque<Result<GenerateResponse, LlmError>>>,
        seen: Mutex<Vec<GenerateRequest>>,
    }

    impl Script {
        fn new(replies: Vec<Result<GenerateResponse, LlmError>>) -> Self {
            Self { replies: Mutex::new(replies.into()), seen: Mutex::default() }
        }
        fn seen(&self) -> Vec<GenerateRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Generator for Script {
        async fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
            self.seen.lock().unwrap().push(request.clone());
            self.replies.lock().unwrap().pop_front().unwrap_or_else(|| Ok(text("(script ended)")))
        }
        fn label(&self) -> String {
            "Script".into()
        }
    }

    fn text(t: &str) -> GenerateResponse {
        GenerateResponse { parts: vec![Part::text(t)], finish_reason: None, usage: Some(Usage { input: 100, output: 10, thinking: 0, cached: 0 }) }
    }

    fn call(name: &str, args: Value) -> GenerateResponse {
        GenerateResponse { parts: vec![Part::ToolCall { id: format!("call-{name}"), name: name.into(), args, signature: None }], finish_reason: None, usage: None }
    }

    struct Echo;
    #[async_trait]
    impl Tool for Echo {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "echo".into(), description: "Echo".into(), parameters: json!({ "type": "object", "properties": {} }) }
        }
        fn label(&self, args: &Value) -> String {
            format!("Echo {}", args["word"].as_str().unwrap_or(""))
        }
        async fn run(&self, args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
            match args["word"].as_str() {
                Some("fail") => Err("broken".into()),
                Some("big") => Ok(json!("x".repeat(500))),
                Some("flop") => Ok(json!({ "success": false, "message": "Not completed." })),
                Some(word) => Ok(json!({ "echo": word })),
                None => Err("word is required.".into()),
            }
        }
    }

    struct Slow;
    #[async_trait]
    impl Tool for Slow {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "slow".into(), description: "Slow".into(), parameters: json!({ "type": "object", "properties": {} }) }
        }
        fn label(&self, _: &Value) -> String {
            "Waiting".into()
        }
        async fn run(&self, _args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(json!("late"))
        }
    }

    fn agent(limits: Limits) -> Agent {
        let mut tools = Registry::default();
        tools.add(Arc::new(Echo));
        tools.add(Arc::new(Slow));
        let memory = Arc::new(Memory::open_in_memory().unwrap());
        Agent::new(tools, memory, EventBus::default(), Arc::new(DenyAll)).with_limits(limits)
    }

    fn drain(rx: &mut tokio::sync::broadcast::Receiver<CoreEvent>) -> Vec<CoreEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    fn states(events: &[CoreEvent]) -> Vec<AssistantState> {
        events
            .iter()
            .filter_map(|e| match e {
                CoreEvent::State(update) => Some(update.state),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn train_mode_reuses_what_worked_and_learns_from_a_correction() {
        let learner = Arc::new(Learner::in_memory());
        let agent = agent(Limits::default()).with_learner(learner.clone());
        let model = Arc::new(Script::new(vec![Ok(call("echo", json!({ "word": "tgju" }))), Ok(text("Dollar is 100."))]));
        agent.run(model.clone(), &Identity::default(), "gheimat dollar ro bego").await;

        // A similar request sees how the first one was done.
        let next = Arc::new(Script::new(vec![
            Ok(text("Dollar is 101.")),
            // The correction's reply, then the lesson written in the background.
            Ok(text("Ok, from the site.")),
            Ok(text("Read prices from tgju.org, not search snippets.")),
        ]));
        agent.run(next.clone(), &Identity::default(), "gheimat dollar chande").await;
        let system = &next.seen()[0].system;
        assert!(system.contains("LEARNED ON THIS PC"), "{system}");
        assert!(system.contains("Echo tgju (worked)"), "{system}");

        agent.run(next.clone(), &Identity::default(), "na, az site begir").await;
        for _ in 0..50 {
            if !learner.summary().lessons.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let lessons = learner.summary().lessons;
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].text, "Read prices from tgju.org, not search snippets.");
        assert_eq!(lessons[0].about, "gheimat dollar chande", "the lesson is about the request that was corrected");
    }

    #[tokio::test]
    async fn a_correction_after_a_stopped_job_is_about_that_job() {
        let learner = Arc::new(Learner::in_memory());
        let agent = Arc::new(agent(Limits::default()).with_learner(learner.clone()));
        let model = Arc::new(Script::new(vec![Ok(text("Dollar is 100."))]));
        agent.run(model, &Identity::default(), "gheimat dollar ro bego").await;
        // The next job is stopped while its tool runs.
        let mut rx = agent.events().subscribe();
        let runner = agent.clone();
        let stopped = tokio::spawn(async move {
            let model = Arc::new(Script::new(vec![Ok(call("slow", json!({})))]));
            runner.run(model, &Identity::default(), "to calculator 123 zarb 45 ro hesab kon").await
        });
        while !matches!(rx.recv().await, Ok(CoreEvent::ToolStarted { .. })) {}
        agent.stop();
        assert_eq!(stopped.await.unwrap().status, TurnStatus::Cancelled);

        let next = Arc::new(Script::new(vec![Ok(text("Ok.")), Ok(text("Type the whole sum at once, then read the result."))]));
        agent.run(next, &Identity::default(), "na, kol-e adad ro yekja type kon").await;
        for _ in 0..50 {
            if !learner.summary().lessons.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let lessons = learner.summary().lessons;
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].about, "to calculator 123 zarb 45 ro hesab kon", "the stopped job, not the one before it");
    }

    /// A player that says what plays, or fails when asked to.
    struct FakeMedia(Result<Value, String>);
    #[async_trait]
    impl Tool for FakeMedia {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "media_control".into(), description: "Media".into(), parameters: json!({ "type": "object", "properties": {} }) }
        }
        fn label(&self, _: &Value) -> String {
            "What is playing".into()
        }
        async fn run(&self, _args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
            self.0.clone()
        }
    }

    fn agent_with_media(result: Result<Value, String>) -> Agent {
        let mut tools = Registry::default();
        tools.add(Arc::new(FakeMedia(result)));
        Agent::new(tools, Arc::new(Memory::open_in_memory().unwrap()), EventBus::default(), Arc::new(DenyAll))
    }

    #[tokio::test]
    async fn a_short_command_runs_its_tool_without_the_model() {
        let agent = agent_with_media(Ok(json!({ "playing": true, "now": "Ebi - Goriz", "app": "Spotify" })));
        let model = Arc::new(Script::new(vec![]));
        let outcome = agent.run(model.clone(), &Identity::default(), "chi dare play mishe?").await;
        assert_eq!(outcome.text, "الان داره پخش میشه: Ebi - Goriz");
        assert!(model.seen().is_empty(), "no model call");
        assert_eq!(outcome.actions, vec!["What is playing".to_string()]);
        assert_eq!(agent.memory().recent_turns(1).unwrap()[0].user, "chi dare play mishe?");
    }

    #[tokio::test]
    async fn a_short_command_that_fails_goes_to_the_model_with_what_was_done() {
        let agent = agent_with_media(Err("no player".into()));
        let model = Arc::new(Script::new(vec![Ok(text("Hich ahangi pakhsh nemishe."))]));
        let outcome = agent.run(model.clone(), &Identity::default(), "chi dare play mishe").await;
        assert_eq!(outcome.text, "Hich ahangi pakhsh nemishe.");
        let asked = model.seen();
        assert_eq!(asked.len(), 1);
        let Some(Part::Text { text: last, .. }) = asked[0].messages.last().and_then(|m| m.parts.first()) else { panic!("no text") };
        assert!(last.contains("chi dare play mishe") && last.contains("Already done for this message: media_control") && last.contains("no player"), "{last}");
        assert_eq!(agent.memory().recent_turns(1).unwrap()[0].user, "chi dare play mishe", "the user's own words are kept");
    }

    #[tokio::test]
    async fn plain_answer_is_shown_and_remembered() {
        let agent = agent(Limits::default());
        let mut rx = agent.events().subscribe();
        let model = Arc::new(Script::new(vec![Ok(text("Salam!"))]));
        let outcome = agent.run(model.clone(), &Identity::default(), "salam").await;

        assert_eq!(outcome.status, TurnStatus::Answered);
        assert_eq!(outcome.text, "Salam!");
        let events = drain(&mut rx);
        assert_eq!(events[0], CoreEvent::TurnStarted { turn_id: 1, user: "salam".into() });
        assert_eq!(states(&events), vec![AssistantState::Thinking, AssistantState::Answer]);
        assert!(events.contains(&CoreEvent::ModelUsed { label: "Script".into(), ok: true, reason: None }));
        assert!(events.contains(&CoreEvent::Answer { turn_id: 1, text: "Salam!".into() }));
        assert_eq!(agent.memory().recent_turns(5).unwrap()[0].assistant, "Salam!");
        assert!(!agent.is_busy());
    }

    #[tokio::test]
    async fn a_tool_that_reports_no_success_counts_as_a_failed_step() {
        let agent = agent(Limits::default());
        let mut rx = agent.events().subscribe();
        let model = Arc::new(Script::new(vec![Ok(call("echo", json!({ "word": "flop" }))), Ok(text("It did not work."))]));
        let outcome = agent.run(model, &Identity::default(), "do it").await;
        assert_eq!(outcome.actions, vec!["Echo flop (failed)".to_string()]);
        let events = drain(&mut rx);
        assert!(events.iter().any(|e| matches!(e, CoreEvent::ToolFinished { ok: false, name, .. } if name == "echo")), "the log says it failed");
    }

    #[tokio::test]
    async fn tool_results_go_back_to_the_model_and_history_carries_actions() {
        let agent = agent(Limits::default());
        let mut rx = agent.events().subscribe();
        let model = Arc::new(Script::new(vec![Ok(call("echo", json!({ "word": "gold" }))), Ok(text("Gold is gold."))]));
        let outcome = agent.run(model.clone(), &Identity::default(), "gold?").await;

        assert_eq!(outcome.text, "Gold is gold.");
        // Two model calls; only the answer reported usage (100 in + 10 out).
        assert_eq!(outcome.usage, Usage { input: 100, output: 10, thinking: 0, cached: 0 });
        assert_eq!(outcome.actions, vec!["Echo gold".to_string()]);
        let seen = model.seen();
        assert!(seen[0].allow_tools);
        let last = seen[1].messages.last().unwrap();
        assert_eq!(last.parts, vec![Part::ToolResult { id: "call-echo".into(), name: "echo".into(), result: json!({ "echo": "gold" }) }]);
        let events = drain(&mut rx);
        assert_eq!(states(&events), vec![AssistantState::Thinking, AssistantState::Working, AssistantState::Thinking, AssistantState::Answer]);
        assert!(events.iter().any(|e| matches!(e, CoreEvent::ToolFinished { ok: true, name, .. } if name == "echo")));
        assert!(events.contains(&CoreEvent::TurnUsage { turn_id: 1, calls: 2, input: 100, output: 10, thinking: 0 }));
        assert_eq!(events.iter().filter(|e| matches!(e, CoreEvent::Usage { .. })).count(), 1);

        // The next request sees the previous turn with what was done.
        let next = Arc::new(Script::new(vec![Ok(text("ok"))]));
        agent.run(next.clone(), &Identity::default(), "and that?").await;
        let messages = &next.seen()[0].messages;
        // Earlier messages carry when they were sent.
        let first = match &messages[0].parts[0] { Part::Text { text, .. } => text.clone(), other => panic!("{other:?}") };
        assert!(first.starts_with("[sent ") && first.ends_with("just now] gold?"), "{first}");
        assert_eq!(messages[1], Message::model("Gold is gold. [actions: Echo gold]"));
        assert_eq!(messages[2], Message::user("and that?"));
    }

    #[tokio::test]
    async fn last_round_must_answer_in_text() {
        let agent = agent(Limits { max_rounds: 2, ..Limits::default() });
        let model = Arc::new(Script::new(vec![Ok(call("echo", json!({ "word": "a" }))), Ok(call("echo", json!({ "word": "b" })))]));
        let outcome = agent.run(model.clone(), &Identity::default(), "loop").await;
        let seen = model.seen();
        assert_eq!(seen.len(), 2);
        assert!(!seen[1].allow_tools);
        // The model ignored the rule and gave no text: the actions were done, so it still ends well.
        assert_eq!(outcome.status, TurnStatus::Answered);
        assert_eq!(outcome.actions, vec!["Echo a".to_string()]);
    }

    #[tokio::test]
    async fn tool_call_limit_and_bad_tools_are_reported_to_the_model() {
        let agent = agent(Limits { max_tool_calls: 2, tool_result_chars: 100, ..Limits::default() });
        let many = GenerateResponse {
            parts: vec![
                Part::ToolCall { id: "1".into(), name: "echo".into(), args: json!({ "word": "big" }), signature: None },
                Part::ToolCall { id: "2".into(), name: "nope".into(), args: json!({}), signature: None },
                Part::ToolCall { id: "3".into(), name: "echo".into(), args: json!({ "word": "c" }), signature: None },
            ],
            finish_reason: None,
            usage: None,
        };
        let model = Arc::new(Script::new(vec![Ok(many), Ok(text("done"))]));
        agent.run(model.clone(), &Identity::default(), "go").await;
        let seen = model.seen();
        let Part::ToolResult { result: big, .. } = &seen[1].messages.last().unwrap().parts[0] else { panic!() };
        assert_eq!(big["truncated"], true);
        let Part::ToolResult { result: missing, .. } = &seen[1].messages.last().unwrap().parts[1] else { panic!() };
        assert_eq!(missing["error"], "Tool nope is not available.");
        let Part::ToolResult { result: limited, .. } = &seen[1].messages.last().unwrap().parts[2] else { panic!() };
        assert!(limited["error"].as_str().unwrap().contains("limit"));
        // The limit was reached, so the next round may not call tools.
        assert!(!seen[1].allow_tools);
    }

    #[tokio::test]
    async fn a_location_refusal_says_what_to_do() {
        let agent = agent(Limits::default());
        let model = Arc::new(Script::new(vec![Err(LlmError::Http { status: 400, detail: "User location is not supported for the API use.".into() })]));
        let outcome = agent.run(model, &Identity::default(), "hi").await;
        assert_eq!(outcome.status, TurnStatus::Failed);
        assert!(outcome.text.contains("VPN") && outcome.text.contains("backup model"), "{}", outcome.text);
    }

    #[tokio::test]
    async fn model_failure_is_reported_with_its_reason_and_not_stored() {
        let agent = agent(Limits::default());
        let mut rx = agent.events().subscribe();
        let model = Arc::new(Script::new(vec![Err(LlmError::Http { status: 401, detail: "Invalid API key".into() })]));
        let outcome = agent.run(model.clone(), &Identity::default(), "hi").await;

        assert_eq!(outcome.status, TurnStatus::Failed);
        assert_eq!(outcome.text, "Script: HTTP 401: Invalid API key");
        let events = drain(&mut rx);
        assert_eq!(states(&events).last(), Some(&AssistantState::Error));
        assert!(events.iter().any(|e| matches!(e, CoreEvent::ModelUsed { ok: false, .. })));
        assert!(agent.memory().recent_turns(5).unwrap().is_empty());
        assert_eq!(model.seen().len(), 1, "no other model or retry is tried");
    }

    #[tokio::test]
    async fn stop_cancels_a_running_tool_at_once() {
        let agent = Arc::new(agent(Limits::default()));
        let mut rx = agent.events().subscribe();
        let runner = agent.clone();
        let task = tokio::spawn(async move {
            let model = Arc::new(Script::new(vec![Ok(call("slow", json!({})))]));
            runner.run(model.clone(), &Identity::default(), "wait").await
        });
        // Wait until the tool runs, then stop.
        loop {
            if let Ok(CoreEvent::ToolStarted { .. }) = rx.recv().await {
                break;
            }
        }
        assert!(agent.stop());
        let outcome = tokio::time::timeout(Duration::from_secs(2), task).await.expect("stopped quickly").unwrap();
        assert_eq!(outcome.status, TurnStatus::Cancelled);
        let events = drain(&mut rx);
        assert!(events.contains(&CoreEvent::Cancelled { turn_id: 1 }));
        assert_eq!(states(&events).last(), Some(&AssistantState::Idle));
        assert!(!agent.stop(), "nothing left to stop");
    }

    #[tokio::test]
    async fn a_new_request_replaces_the_running_one() {
        let agent = Arc::new(agent(Limits::default()));
        let mut rx = agent.events().subscribe();
        let runner = agent.clone();
        let first = tokio::spawn(async move {
            let model = Arc::new(Script::new(vec![Ok(call("slow", json!({})))]));
            runner.run(model.clone(), &Identity::default(), "first").await
        });
        loop {
            if let Ok(CoreEvent::ToolStarted { .. }) = rx.recv().await {
                break;
            }
        }
        let model = Arc::new(Script::new(vec![Ok(text("second answer"))]));
        let second = agent.run(model.clone(), &Identity::default(), "second").await;
        let first = first.await.unwrap();
        assert_eq!(first.status, TurnStatus::Cancelled);
        assert_eq!(second.text, "second answer");
        let events = drain(&mut rx);
        // The cancelled request did not reset the island to idle under the new one.
        assert!(!states(&events).contains(&AssistantState::Idle));
    }
}
