//! Voice conversations through Gemini Live: the microphone streams up, the model's voice plays back, and tool calls
//! run through the same tools as typed requests (with the same events, so the island and chat show every step).
//!
//! Lessons from v1 kept here:
//! - the speaker's sound reaches the microphone on PCs without headphones, so while the assistant talks (and a moment
//!   after) the microphone sends silence; otherwise the model hears itself and answers itself,
//! - transcripts arrive in small pieces and a little late, so an exchange is finished only after its sound has played,
//! - the session closes by itself after a quiet while, so the microphone is never left open.

use crate::agent::{Agent, Metered, clip, now_ms};
use crate::events::CoreEvent;
use crate::prompts::{Identity, system_prompt};
use crate::state::{AssistantState, StateUpdate};
use anytask_llm::live::{LiveConfig, LiveEvent, LiveSession, OUTPUT_RATE};
use anytask_llm::{Generator, Usage};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// How long the microphone stays muted after the assistant's voice stops (room echo).
const ECHO_TAIL: Duration = Duration::from_millis(400);
/// How long to wait for the model to speak after tool results before the exchange is closed.
const ANSWER_WAIT: Duration = Duration::from_secs(10);
/// Tool calls for one question. Every call sends the whole conversation to Live again (5,000–8,000 tokens), so a
/// model that keeps searching gets stopped; jobs with many steps go to the brain (ask_brain counts as one).
const MAX_TOOLS_PER_QUESTION: u32 = 6;
const TOO_MANY_TOOLS: &str = "Too many tool calls for this question. Do not call more tools now: answer with what you found, or say \
                              briefly that you could not find it.";
/// A tool result the voice model keeps. Live keeps every result for the rest of the session and resends it on each
/// call, so a whole web page (15,000 characters) made every later call about 3,000 tokens bigger (11-04 19:52).
const VOICE_RESULT_CHARS: usize = 8_000;
/// While a job runs, silence goes up to Live this often so the server does not close the session for inactivity.
const KEEP_ALIVE: Duration = Duration::from_secs(25);
/// 100 ms of silence at 16 kHz.
const KEEP_ALIVE_SAMPLES: usize = 1600;
/// Train mode lessons a voice session starts with.
/// Lessons are one line each; a short list forgot older ones within one training round (11-06: the "avoid isna/irna"
/// lesson of 23:54 was gone from the prompt by 00:31, when 17 lessons existed).
const VOICE_LESSONS: usize = 40;
/// Turns of the typed/spoken history the voice model sees.
const HISTORY_TURNS: usize = 6;

/// The voice rules. The language lines follow the app's language (Settings > General): an English message got a
/// Persian answer on 11-11, because these rules said "the user speaks Persian" whatever the app was set to; and a
/// Persian price page turned an English question into a Persian answer.
fn voice_rules() -> String {
    let language = if crate::lang::persian() {
        "- Answer in the language of the user's last message: Persian for Persian or Finglish (Persian in Latin letters), English\n\
  for English. Say numbers, prices and times the way people say them.\n\
- The user speaks Persian. A transcript in German, Italian, Chinese or another language is almost always misheard\n\
  Persian or someone else talking nearby: do not act on it; ask briefly in Persian what they said.\n"
    } else {
        "- Answer in the language of the user's last message: English for English, Persian for Persian or Finglish (Persian in\n\
  Latin letters), even when a tool result or a page is in another language. Say numbers, prices and times the way\n\
  people say them.\n\
- The user speaks English or Persian. A transcript in another language is almost always misheard or someone else\n\
  talking nearby: do not act on it; ask briefly what they said.\n"
    };
    format!("\n\nVOICE:\n- You are talking out loud. Answer in one or two short spoken sentences, like a person; no lists, markdown, links or emoji.\n{language}{VOICE_RULES}")
}

const VOICE_RULES: &str = "\
- When a tool is needed, call it right away; do not say what you are going to do first.\n\
- Typed messages from the user arrive as text: answer them by voice the same way.\n\
- Simple things (a quick answer, one search, play or pause, one tool) you do yourself right away.\n\
- Weather: call weather. Search at most twice for one question; if that does not answer it, say so briefly.\n\
- For a job with more than one step (find something, then write it, then save it), work across apps on the screen, or\n\
  anything that needs care, call ask_brain with the user's full request and every detail (exact text, names, files).\n\
  While it works say nothing; then tell the user the result in one or two sentences.\n\
- When a tool result says success false, FAILED or an error, say plainly that it did not work. Never say it was done\n\
  and never give a value the tool did not return.\n\
- If the user asks to stop, cancel or leave the job (stop, bas kon, ghat kon, بسه, نکن), call stop_task at once.\n\
  Never say a job stopped unless stop_task said so.";

/// A typed message as the voice model gets it: the answer's language when it is clear, and the date and time now.
/// The session's setup holds the date too, but the model still called yesterday "today" (11-11: "today, November
/// 10" at 02:29 on the 11th; a price row of 19 Aban called today on the 20th).
fn typed_for_live(text: &str, now: chrono::DateTime<chrono::Local>) -> String {
    format!(
        "{}\n(Now: {} {}, {}.)",
        crate::lang::with_answer_language(text),
        now.format("%A %Y-%m-%d %H:%M"),
        crate::prompts::persian_date(now),
        if crate::lang::persian() { "Persian app" } else { "English app" }
    )
}

/// The tool that hands a job to the brain (the text model with every tool, in many steps).
const ASK_BRAIN: &str = "ask_brain";
/// The tool that stops the running job.
const STOP_TASK: &str = "stop_task";
/// What the model hears about a job the user stopped.
const STOPPED: &str = "The user stopped this job before it finished.";

/// The user asks to stop the running job ("stop", "bas kon", "قطعش کن"). Only short messages count, so a new
/// request that mentions stopping something else does not end the work; longer ones reach the model's stop_task.
pub(crate) fn is_stop_request(text: &str) -> bool {
    const STOPS: &[&str] = &[
        "stop", "cancel", "abort", "halt", "never mind", "nevermind", "forget it", "bas", "basse", "bese", "besse", "ghat kon",
        "ghatesh", "gat kon", "vaysa", "vaisa", "vaysta", "nakon", "nakonid", "velesh", "laghv", "laghvesh", "tamomesh",
        "tamumesh", "edame nade", "motevaghef", "motevaghefesh", "dast negah dar", "estop", "بس", "بسه", "قطع کن", "قطعش",
        "وایسا", "واستا", "نکن", "نکنید", "ولش", "لغو", "لغوش", "کنسل", "کنسلش", "تمومش", "ادامه نده", "متوقف", "متوقفش",
        "دست نگه دار", "استاپ",
    ];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() || words.len() > 8 {
        return false;
    }
    STOPS.iter().any(|stop| {
        let phrase: Vec<&str> = stop.split(' ').collect();
        words.windows(phrase.len()).any(|w| w == phrase.as_slice())
    })
}

/// The open conversation: what goes up to the model.
pub trait LiveLink: Send + Sync {
    fn send_audio(&self, pcm: &[i16]) -> bool;
    /// A typed message in the same conversation.
    fn send_text(&self, text: &str) -> bool;
    fn send_tool_results(&self, results: &[(String, String, Value)]) -> bool;
    fn close(&self);
}

impl LiveLink for LiveSession {
    fn send_audio(&self, pcm: &[i16]) -> bool {
        LiveSession::send_audio(self, pcm)
    }
    fn send_tool_results(&self, results: &[(String, String, Value)]) -> bool {
        LiveSession::send_tool_results(self, results)
    }
    fn send_text(&self, text: &str) -> bool {
        LiveSession::send_text(self, text)
    }
    fn close(&self) {
        LiveSession::close(self)
    }
}

/// Where the model's voice plays.
pub trait VoiceOut: Send + Sync {
    fn play(&self, pcm: &[i16], rate: u32);
    /// Stops at once.
    fn clear(&self);
    fn is_playing(&self) -> bool;
}

/// A connection to Live: what goes up, and what comes back.
pub type LiveConnection = (Arc<dyn LiveLink>, mpsc::UnboundedReceiver<LiveEvent>);

/// Opens a new connection for the same conversation (its prompt carries the recent history from memory).
pub type Reconnect = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<LiveConnection, String>> + Send>> + Send + Sync>;

/// A dropped connection is opened again at most once in this time (no reconnect loops).
const RECONNECT_EVERY: Duration = Duration::from_secs(60);

pub struct VoiceIo {
    pub link: Arc<dyn LiveLink>,
    pub events: mpsc::UnboundedReceiver<LiveEvent>,
    /// Opens the connection again when it drops; None: a drop ends the conversation.
    pub reconnect: Option<Reconnect>,
    /// 16 kHz mono microphone chunks.
    pub mic: mpsc::UnboundedReceiver<Vec<i16>>,
    pub speaker: Arc<dyn VoiceOut>,
    /// The voice model's name for usage lines ("gemini-3.8-live").
    pub label: String,
    /// Typed messages for this conversation.
    pub texts: mpsc::UnboundedReceiver<String>,
    /// Who is talking to whom, for jobs handed to the brain.
    pub identity: Identity,
    /// The microphone is on (a spoken conversation); off for a typed one.
    pub mic_open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEnd {
    /// The user stopped it.
    Stopped,
    /// Nobody spoke for a while.
    Quiet,
    /// The connection or the microphone ended; the text says why.
    Closed(String),
}

/// One question and its spoken answer.
struct Exchange {
    turn_id: u64,
    user: String,
    said: String,
    actions: Vec<String>,
    live_calls: u32,
    /// Tools the model called for this question.
    tool_calls: u32,
    live: Usage,
    tools_model: Arc<Metered>,
    /// The model finished; the exchange closes once its sound has played.
    done: bool,
    /// The model called a tool and has not spoken about the result yet. Live ends the model's turn together with
    /// the tool call; that turn end must not close the exchange, or the answer after the tool becomes a new one.
    waiting: bool,
    /// When the tool results went back (to stop waiting if the model never speaks again).
    results_at: Option<Instant>,
    /// A screen job failed for this question: it is not started again in the same question (final test: the loop
    /// guard stopped one and the model at once started another that ran until the user pressed Stop).
    screen_failed: bool,
}

type ToolRun<'a> = Pin<Box<dyn Future<Output = Option<(Vec<(String, String, Value)>, Vec<String>)>> + Send + 'a>>;

impl Agent {
    /// The session setup for a voice conversation: the brain's prompt, voice rules, recent history and all tools.
    pub fn live_config(&self, identity: &Identity, model: &str, voice: &str) -> LiveConfig {
        let tools = self.tools.read().unwrap_or_else(|e| e.into_inner()).clone();
        let background = self.memory().context_summary(20).unwrap_or_default();
        let mut system = system_prompt(identity, chrono::Local::now(), &background, &tools.names(), &self.pc_profile());
        system.push_str(&voice_rules());
        // Train mode: what earlier mistakes taught. The brain gets the lessons that fit each request; a voice
        // session gets the latest ones up front, since most requests come through it.
        if let Some(lessons) = self.learner.as_ref().filter(|l| l.enabled()).map(|l| l.recent_lessons(VOICE_LESSONS)).filter(|l| !l.is_empty()) {
            system.push_str("\n\nLEARNED ON THIS PC (lessons from earlier mistakes; follow them unless the user asks otherwise):\n");
            system.push_str(&lessons);
        }
        let turns = self.memory().recent_turns(HISTORY_TURNS).unwrap_or_default();
        if !turns.is_empty() {
            system.push_str("\n\nRECENT CONVERSATION (oldest first):");
            for turn in turns {
                let chars = self.limits.history_chars;
                let reply = anytask_storage::memory::with_actions(&turn.assistant, &turn.actions);
                let sent = crate::prompts::when(turn.at, now_ms());
                system.push_str(&format!("\nUser [{sent}]: {}\nYou: {}", clip(&turn.user, chars), clip(&reply, chars)));
            }
        }
        let mut specs = tools.specs();
        specs.push(anytask_llm::ToolSpec {
            name: ASK_BRAIN.into(),
            description: "Hands a job to the assistant's planner, which uses every tool in as many steps as needed (look things up, \
                          write, save, work in apps on the screen) and returns the result. Use it for anything with more than one \
                          step or that needs care; give the user's full request with every detail."
                .into(),
            parameters: json!({ "type": "object", "properties": { "task": { "type": "string", "description": "The whole job in the user's words, with every detail." } }, "required": ["task"] }),
        });
        specs.push(anytask_llm::ToolSpec {
            name: STOP_TASK.into(),
            description: "Stops the job that is running now (a screen task, ask_brain or any other tool). Call it as soon as the user \
                          asks to stop or cancel."
                .into(),
            parameters: json!({ "type": "object", "properties": { "reason": { "type": "string", "description": "What the user said, short." } } }),
        });
        LiveConfig { model: model.to_string(), voice: voice.to_string(), system, tools: specs, proxy: None }
    }

    /// Runs a voice conversation until the user stops it, it goes quiet for `quiet_close`, or the connection ends.
    /// `model` answers tools that ask a model (screen steps).
    pub async fn converse(&self, model: Arc<dyn Generator>, io: VoiceIo, cancel: CancellationToken, quiet_close: Duration) -> VoiceEnd {
        let VoiceIo { mut link, mut events, reconnect, mut mic, speaker, label, mut texts, identity, mic_open } = io;
        let mut reconnected: Option<Instant> = None;
        // Between exchanges: listening with the microphone on, resting in a typed conversation.
        let resting = if mic_open { AssistantState::Listening } else { AssistantState::Idle };
        let mut texts_open = true;
        let tools = self.tools.read().unwrap_or_else(|e| e.into_inner()).clone();
        let mut heard = String::new();
        let mut current: Option<Exchange> = None;
        let mut running: Option<ToolRun<'_>> = None;
        // The calls the running tools will answer, what was heard since they started, and their own stop.
        let mut pending: Vec<(String, String)> = Vec::new();
        // stop_task's answers while a job runs: sent together with the stopped job's results. Live answers every
        // tool response, so two separate responses made it say "stopped" twice (the second time as a turn without
        // words, logged as "🎤").
        let mut held_stops: Vec<(String, String, Value)> = Vec::new();
        let mut heard_in_run = String::new();
        let mut work = CancellationToken::new();
        let mut last_activity = Instant::now();
        let mut last_sound: Option<Instant> = None;
        // When something last went up to Live (audio, text, results): a typed conversation sends nothing while a long
        // job runs, and the server closes a silent session after about two minutes.
        let mut last_up = tokio::time::Instant::now();
        // The last typed message the model has not started answering: a connection that drops right after it was
        // sent loses it, so it goes again over the new connection (training round 3, 01:35: a message vanished).
        let mut unanswered: Option<String> = None;
        let mut shown = AssistantState::Listening;
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        self.state(StateUpdate::new(if mic_open { AssistantState::Listening } else { AssistantState::Thinking }));
        self.log(None, "voice session open".into());

        let end = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break VoiceEnd::Stopped,
                finished = async {
                    match running.as_mut() {
                        Some(run) => run.await,
                        None => std::future::pending().await,
                    }
                } => {
                    running = None;
                    *self.work_stop.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    let Some((results, actions)) = finished else {
                        if cancel.is_cancelled() {
                            break VoiceEnd::Stopped;
                        }
                        // The user stopped the job: the model hears it and the conversation goes on.
                        let mut stopped: Vec<(String, String, Value)> =
                            pending.drain(..).map(|(id, name)| (id, name, json!({ "success": false, "stopped": true, "error": STOPPED }))).collect();
                        if let Some(ex) = current.as_mut() {
                            ex.results_at = Some(Instant::now());
                            for (id, name, _) in &stopped {
                                self.events().emit(CoreEvent::ToolFinished { turn_id: ex.turn_id, call_id: id.clone(), name: name.clone(), ok: false, summary: STOPPED.into() });
                            }
                            self.events().emit(CoreEvent::Cancelled { turn_id: ex.turn_id });
                            self.log(Some(ex.turn_id), "the user stopped the job".into());
                        }
                        stopped.append(&mut held_stops);
                        link.send_tool_results(&stopped);
                        last_activity = Instant::now();
                        shown = self.show(AssistantState::Thinking, None);
                        continue;
                    };
                    pending.clear();
                    if let Some(ex) = current.as_mut() {
                        ex.actions.extend(actions);
                        ex.results_at = Some(Instant::now());
                        ex.screen_failed |= results.iter().any(|(_, name, value)| name == "desktop_task" && value.get("success") == Some(&json!(false)));
                    }
                    let mut results = results;
                    results.append(&mut held_stops);
                    link.send_tool_results(&results);
                    last_activity = Instant::now();
                    shown = self.show(AssistantState::Thinking, None);
                }
                text = texts.recv(), if texts_open => {
                    let Some(text) = text else {
                        texts_open = false;
                        continue;
                    };
                    if let Some(ex) = current.take_if(|ex| ex.done) {
                        self.finish(ex);
                    }
                    if !heard.is_empty() {
                        heard.push(' ');
                    }
                    heard.push_str(text.trim());
                    if running.is_some() && is_stop_request(&text) {
                        work.cancel();
                    }
                    let sent = link.send_text(&typed_for_live(&text, chrono::Local::now()));
                    last_up = tokio::time::Instant::now();
                    unanswered = Some(text.clone());
                    self.log(None, format!("typed message {}: {}", if sent { "sent" } else { "NOT sent" }, clip(text.trim(), 120)));
                    last_activity = Instant::now();
                    shown = self.show(AssistantState::Thinking, Some(text.trim()));
                }
                chunk = mic.recv() => {
                    let Some(chunk) = chunk else { break VoiceEnd::Closed("The microphone stopped.".into()) };
                    let echo = speaker.is_playing() || last_sound.is_some_and(|t| t.elapsed() < ECHO_TAIL);
                    if echo {
                        link.send_audio(&vec![0; chunk.len()]);
                    } else {
                        link.send_audio(&chunk);
                    }
                    last_up = tokio::time::Instant::now();
                }
                event = events.recv() => {
                    // A connection that ends without a word is a drop like any other.
                    let event = event.unwrap_or_else(|| LiveEvent::Closed("The connection ended.".into()));
                    last_activity = Instant::now();
                    match event {
                        LiveEvent::Heard(text) => {
                            if running.is_some() {
                                heard_in_run.push_str(&text);
                                if is_stop_request(&heard_in_run) {
                                    work.cancel();
                                }
                            }
                            match current.as_mut() {
                                // Late pieces of the question the model is already answering.
                                Some(ex) if !ex.done => ex.user.push_str(&text),
                                _ => {
                                    if let Some(ex) = current.take() {
                                        self.finish(ex);
                                    }
                                    heard.push_str(&text);
                                    shown = self.show(AssistantState::Listening, Some(heard.trim()));
                                }
                            }
                        }
                        LiveEvent::Audio(pcm) => {
                            unanswered = None;
                            let ex = self.begin(&mut current, &mut heard, &model);
                            if ex.results_at.is_some() {
                                ex.waiting = false;
                            }
                            speaker.play(&pcm, OUTPUT_RATE);
                            last_sound = Some(Instant::now());
                            if shown != AssistantState::Speaking {
                                shown = self.show(AssistantState::Speaking, None);
                            }
                        }
                        LiveEvent::Said(text) => {
                            unanswered = None;
                            let ex = self.begin(&mut current, &mut heard, &model);
                            if ex.results_at.is_some() {
                                ex.waiting = false;
                            }
                            ex.said.push_str(&text);
                            let said = ex.said.trim().to_string();
                            shown = self.show(AssistantState::Speaking, Some(&said));
                        }
                        LiveEvent::ToolCalls(calls) => {
                            unanswered = None;
                            let ex = self.begin(&mut current, &mut heard, &model);
                            ex.waiting = true;
                            ex.results_at = None;
                            let (stops, calls): (Vec<_>, Vec<_>) = calls.into_iter().partition(|(_, name, _)| name == STOP_TASK);
                            if !stops.is_empty() {
                                let was_running = running.is_some();
                                work.cancel();
                                if was_running {
                                    held_stops.extend(stops.into_iter().map(|(id, name, _)| (id, name, json!({ "stopped": true }))));
                                } else {
                                    let reply = json!({ "stopped": false, "note": "Nothing was running." });
                                    link.send_tool_results(&stops.into_iter().map(|(id, name, _)| (id, name, reply.clone())).collect::<Vec<_>>());
                                    ex.results_at = Some(Instant::now());
                                }
                            }
                            ex.tool_calls += calls.len() as u32;
                            if calls.is_empty() {
                                // Only stop_task: its answer already went back.
                            } else if ex.tool_calls > MAX_TOOLS_PER_QUESTION {
                                self.log(Some(ex.turn_id), format!("{} tool calls for one question: the model must answer now", ex.tool_calls));
                                let refused: Vec<(String, String, Value)> =
                                    calls.into_iter().map(|(id, name, _)| (id, name, json!({ "success": false, "error": TOO_MANY_TOOLS }))).collect();
                                link.send_tool_results(&refused);
                                ex.results_at = Some(Instant::now());
                            } else if ex.screen_failed && calls.iter().any(|(_, name, _)| name == "desktop_task") {
                                let refused: Vec<(String, String, Value)> = calls
                                    .into_iter()
                                    .map(|(id, name, _)| (id, name, json!({ "success": false, "error": "The screen job already failed for this request; do not start it again now. Tell the user in one sentence what did not work." })))
                                    .collect();
                                link.send_tool_results(&refused);
                                ex.results_at = Some(Instant::now());
                            } else if running.is_some() {
                                let busy: Vec<(String, String, Value)> =
                                    calls.into_iter().map(|(id, name, _)| (id, name, json!({ "success": false, "error": "Another tool is still running; wait for it." }))).collect();
                                link.send_tool_results(&busy);
                            } else {
                                work = cancel.child_token();
                                *self.work_stop.lock().unwrap_or_else(|e| e.into_inner()) = Some(work.clone());
                                pending = calls.iter().map(|(id, name, _)| (id.clone(), name.clone())).collect();
                                heard_in_run.clear();
                                let said = ex.user.trim().to_string();
                                running = Some(self.run_tools(ex.turn_id, &tools, &identity, calls, said, work.clone(), ex.tools_model.clone()));
                            }
                        }
                        LiveEvent::Usage(usage) => {
                            if let Some(ex) = current.as_mut() {
                                ex.live_calls += 1;
                                ex.live.add(usage);
                                self.events().emit(CoreEvent::Usage { turn_id: ex.turn_id, label: label.clone(), input: usage.input, output: usage.output, thinking: usage.thinking, cached: usage.cached });
                            }
                        }
                        LiveEvent::Interrupted => {
                            speaker.clear();
                            if let Some(ex) = current.as_mut() {
                                ex.done = true;
                            }
                        }
                        LiveEvent::TurnComplete => {
                            if let Some(ex) = current.as_mut().filter(|ex| !ex.waiting) {
                                ex.done = true;
                            }
                        }
                        // The server ends a session nobody used for a while: that is a quiet end, not an error.
                        LiveEvent::Closed(reason) if running.is_none() && reason.to_lowercase().contains("inactivity") => break VoiceEnd::Quiet,
                        LiveEvent::GoingAway | LiveEvent::Closed(_) => {
                            let reason = match event {
                                LiveEvent::Closed(reason) => reason,
                                _ => "The voice session reached its time limit.".to_string(),
                            };
                            match self.reopen(reconnect.as_ref(), &mut reconnected, running.is_some(), &reason, &cancel).await {
                                Some((new_link, new_events)) => {
                                    (link, events) = (new_link, new_events);
                                    if let Some(text) = unanswered.take() {
                                        let sent = link.send_text(&typed_for_live(&text, chrono::Local::now()));
                                        self.log(None, format!("typed message sent again after the reconnect: {} ({})", clip(text.trim(), 80), if sent { "sent" } else { "NOT sent" }));
                                    }
                                    if let Some(ex) = current.take() {
                                        self.finish(ex);
                                    }
                                    heard.clear();
                                    last_activity = Instant::now();
                                    shown = self.show(resting, None);
                                }
                                None => break VoiceEnd::Closed(reason),
                            }
                        }
                    }
                }
                _ = tick.tick() => {
                    // A job is running and nothing went up for a while: a moment of silence keeps the session open,
                    // or its answer is lost (training round 3: screen jobs over 120 s were cut off by "Session
                    // closed due to client inactivity" and the user got "done" or nothing).
                    if running.is_some() && last_up.elapsed() >= KEEP_ALIVE {
                        link.send_audio(&[0; KEEP_ALIVE_SAMPLES]);
                        last_up = tokio::time::Instant::now();
                    }
                    // The model got the tool results but stayed silent: close the exchange anyway.
                    if let Some(ex) = current.as_mut().filter(|ex| ex.waiting && ex.results_at.is_some_and(|t| t.elapsed() >= ANSWER_WAIT)) {
                        ex.waiting = false;
                        ex.done = true;
                    }
                    if speaker.is_playing() {
                        last_sound = Some(Instant::now());
                        last_activity = Instant::now();
                    } else if current.as_ref().is_some_and(|ex| ex.done) && last_sound.is_none_or(|t| t.elapsed() >= ECHO_TAIL) {
                        if let Some(ex) = current.take() {
                            self.finish(ex);
                        }
                        shown = self.show(resting, None);
                    }
                    if running.is_none() && !speaker.is_playing() && last_activity.elapsed() >= quiet_close {
                        break VoiceEnd::Quiet;
                    }
                }
            }
        };

        drop(running);
        *self.work_stop.lock().unwrap_or_else(|e| e.into_inner()) = None;
        speaker.clear();
        link.close();
        if let Some(ex) = current.take() {
            self.finish(ex);
        }
        let why = match &end {
            VoiceEnd::Stopped => "stopped".to_string(),
            VoiceEnd::Quiet => format!("quiet for {} s", quiet_close.as_secs()),
            VoiceEnd::Closed(reason) => reason.clone(),
        };
        self.log(None, format!("voice session closed: {why}"));
        match &end {
            VoiceEnd::Closed(reason) => {
                self.events().emit(CoreEvent::Error { turn_id: None, message: reason.clone() });
                self.state(StateUpdate::new(AssistantState::Error).with_detail(reason.clone()));
            }
            _ => self.state(StateUpdate::new(AssistantState::Idle)),
        }
        end
    }

    /// After the connection to Live dropped: opens a new one, unless a job is running (its answer belongs to the old
    /// connection), it was reopened less than a minute ago, or nothing can reconnect. Drops ended the conversation
    /// (11-03 16:49 and 16:51 "Connection reset", 11-04 19:51 os error 10054); the user had to start again.
    async fn reopen(
        &self,
        reconnect: Option<&Reconnect>,
        last: &mut Option<Instant>,
        busy: bool,
        reason: &str,
        cancel: &CancellationToken,
    ) -> Option<LiveConnection> {
        let reconnect = reconnect?;
        if busy || last.is_some_and(|t| t.elapsed() < RECONNECT_EVERY) {
            return None;
        }
        *last = Some(Instant::now());
        let started = Instant::now();
        let opened = tokio::select! {
            _ = cancel.cancelled() => return None,
            opened = reconnect() => opened,
        };
        match opened {
            Ok(connection) => {
                self.log(None, format!("voice connection dropped ({}); reconnected in {:.1} s", clip(reason, 120), started.elapsed().as_secs_f32()));
                Some(connection)
            }
            Err(e) => {
                self.log(None, format!("voice connection dropped ({}); could not reconnect: {}", clip(reason, 120), clip(&e, 120)));
                None
            }
        }
    }

    /// The exchange the model is answering now; starts one (with what was heard) on its first output.
    fn begin<'a>(&self, current: &'a mut Option<Exchange>, heard: &mut String, model: &Arc<dyn Generator>) -> &'a mut Exchange {
        if current.as_ref().is_some_and(|ex| ex.done)
            && let Some(ex) = current.take()
        {
            self.finish(ex);
        }
        current.get_or_insert_with(|| {
            let turn_id = self.next_turn.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let user = std::mem::take(heard).trim().to_string();
            self.events().emit(CoreEvent::TurnStarted { turn_id, user: if user.is_empty() { "🎤".into() } else { user.clone() } });
            let tools_model = Arc::new(Metered { inner: model.clone(), events: self.events().clone(), turn_id, total: Mutex::new((0, Usage::default())) });
            Exchange { turn_id, user, said: String::new(), actions: Vec::new(), live_calls: 0, tool_calls: 0, live: Usage::default(), tools_model, done: false, waiting: false, results_at: None, screen_failed: false }
        })
    }

    /// Reports the exchange's tokens and answer and remembers it.
    fn finish(&self, ex: Exchange) {
        let (tool_calls, tool_usage) = ex.tools_model.total.lock().map(|t| *t).unwrap_or_default();
        let mut usage = ex.live;
        usage.add(tool_usage);
        let calls = ex.live_calls + tool_calls;
        if calls > 0 {
            self.events().emit(CoreEvent::TurnUsage { turn_id: ex.turn_id, calls, input: usage.input, output: usage.output, thinking: usage.thinking });
        }
        let said = ex.said.trim();
        if said.is_empty() && ex.actions.is_empty() {
            // Nothing was answered (cut off at once); close the chat's waiting bubble.
            self.events().emit(CoreEvent::Cancelled { turn_id: ex.turn_id });
            return;
        }
        // It worked when the model answered and its last step did not fail (an answer can also be "sorry, it
        // failed"; a first try that failed and a second way that worked is still a request that worked). One that
        // did not work, or a correction of the last answer ("na, to spotify"), is turned into a lesson.
        if !ex.user.trim().is_empty() {
            let ok = !said.is_empty() && !ex.actions.last().is_some_and(|a| a.ends_with("(failed)"));
            self.learn_from(ex.tools_model.inner.clone(), ex.turn_id, ex.user.trim(), &ex.actions, ok, !ok && !ex.actions.is_empty(), said);
        }
        let text = if !said.is_empty() {
            // The voice model sometimes writes Bengali digits in a Persian answer (training round 4: "۳۰۲ هزار و ১৫০").
            said.chars().map(|c| if ('০'..='৯').contains(&c) { char::from_u32('۰' as u32 + (c as u32 - '০' as u32)).unwrap_or(c) } else { c }).collect()
        } else if ex.actions.last().is_some_and(|a| a.ends_with("(failed)")) {
            // Silent after a failed step: never "done" (training round 2: a write the user did not allow, then "انجام شد.").
            crate::agent::not_done(&ex.user).to_string()
        } else {
            crate::agent::done(&ex.user).to_string()
        };
        let _ = self.memory().add_turn(now_ms(), &ex.user, &text, &ex.actions);
        self.events().emit(CoreEvent::Answer { turn_id: ex.turn_id, text });
    }

    /// `said` is what the user said for this exchange; the tools and a job handed to the brain see it next to the
    /// voice model's rewrite of it.
    #[allow(clippy::too_many_arguments)]
    fn run_tools<'a>(
        &'a self,
        turn_id: u64,
        tools: &'a crate::tools::Registry,
        identity: &'a Identity,
        calls: Vec<(String, String, Value)>,
        said: String,
        cancel: CancellationToken,
        model: Arc<Metered>,
    ) -> ToolRun<'a> {
        Box::pin(async move {
            let model: Arc<dyn Generator> = model;
            let mut actions = Vec::new();
            let mut results = Vec::with_capacity(calls.len());
            for (id, name, args) in calls {
                let result = if name == ASK_BRAIN {
                    // The brain works under this exchange: its steps show on the island and in the chat.
                    let task = args["task"].as_str().unwrap_or_default().to_string();
                    self.log(Some(turn_id), format!("ask_brain: {}", clip(&task, 120)));
                    let said = if said.is_empty() { task.as_str() } else { said.as_str() };
                    match self.work(turn_id, &model, identity, &task, said, &cancel, &mut actions).await {
                        crate::agent::Ending::Answer(text) => json!({ "result": text }),
                        crate::agent::Ending::Failed(error) => json!({ "success": false, "error": error }),
                        crate::agent::Ending::Cancelled => return None,
                    }
                } else {
                    let result = self.call_tool(turn_id, tools, (&id, &name, args), &said, &cancel, &model, &mut actions).await?;
                    crate::agent::bound(result, VOICE_RESULT_CHARS)
                };
                results.push((id, name, result));
            }
            Some((results, actions))
        })
    }

    /// Shows a state on the island; returns it as the new shown state.
    fn show(&self, state: AssistantState, detail: Option<&str>) -> AssistantState {
        let mut update = StateUpdate::new(state);
        if let Some(detail) = detail.filter(|d| !d.is_empty()) {
            update = update.with_detail(detail.to_string());
        }
        self.state(update);
        state
    }

    fn log(&self, turn_id: Option<u64>, text: String) {
        self.events().emit(CoreEvent::Log { turn_id, source: "voice".into(), text });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{DenyAll, Registry, Tool, ToolCtx};
    use crate::tools::test_support::NoModel;
    use anytask_llm::ToolSpec;
    use anytask_storage::Memory;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Default)]
    struct Link {
        audio: Mutex<Vec<Vec<i16>>>,
        results: Mutex<Vec<(String, String, Value)>>,
        /// The call ids of each tool response, in order (Live answers each response once).
        batches: Mutex<Vec<Vec<String>>>,
        texts: Mutex<Vec<String>>,
        closed: AtomicBool,
    }

    impl LiveLink for Link {
        fn send_audio(&self, pcm: &[i16]) -> bool {
            self.audio.lock().unwrap().push(pcm.to_vec());
            true
        }
        fn send_tool_results(&self, results: &[(String, String, Value)]) -> bool {
            self.batches.lock().unwrap().push(results.iter().map(|(id, _, _)| id.clone()).collect());
            self.results.lock().unwrap().extend_from_slice(results);
            true
        }
        fn send_text(&self, text: &str) -> bool {
            self.texts.lock().unwrap().push(text.to_string());
            true
        }
        fn close(&self) {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    /// A brain that always answers the same.
    struct Brain(&'static str);
    #[async_trait]
    impl Generator for Brain {
        async fn generate(&self, _: &anytask_llm::GenerateRequest) -> Result<anytask_llm::GenerateResponse, anytask_llm::LlmError> {
            Ok(anytask_llm::GenerateResponse { parts: vec![anytask_llm::Part::text(self.0)], finish_reason: None, usage: None })
        }
        fn label(&self) -> String {
            "brain".into()
        }
    }

    #[derive(Default)]
    struct Out {
        played: Mutex<usize>,
        playing: AtomicBool,
    }

    impl VoiceOut for Out {
        fn play(&self, pcm: &[i16], _rate: u32) {
            *self.played.lock().unwrap() += pcm.len();
        }
        fn clear(&self) {
            self.playing.store(false, Ordering::Relaxed);
        }
        fn is_playing(&self) -> bool {
            self.playing.load(Ordering::Relaxed)
        }
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
            Ok(json!({ "echo": args["word"] }))
        }
    }

    /// A job that runs until it is stopped.
    struct Slow;
    #[async_trait]
    impl Tool for Slow {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "slow".into(), description: "Slow".into(), parameters: json!({ "type": "object", "properties": {} }) }
        }
        fn label(&self, _: &Value) -> String {
            "Slow".into()
        }
        async fn run(&self, _: Value, _ctx: &ToolCtx) -> Result<Value, String> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(json!({}))
        }
    }

    struct Rig {
        agent: Arc<Agent>,
        link: Arc<Link>,
        out: Arc<Out>,
        live: mpsc::UnboundedSender<LiveEvent>,
        mic: mpsc::UnboundedSender<Vec<i16>>,
        text: mpsc::UnboundedSender<String>,
        cancel: CancellationToken,
        session: tokio::task::JoinHandle<VoiceEnd>,
    }

    fn start(quiet: Duration) -> Rig {
        start_with(quiet, Arc::new(NoModel))
    }

    fn start_with(quiet: Duration, model: Arc<dyn Generator>) -> Rig {
        let mut tools = Registry::default();
        tools.add(Arc::new(Echo));
        tools.add(Arc::new(Slow));
        let agent = Arc::new(Agent::new(tools, Arc::new(Memory::open_in_memory().unwrap()), Default::default(), Arc::new(DenyAll)));
        let (link, out) = (Arc::new(Link::default()), Arc::new(Out::default()));
        let (live, events) = mpsc::unbounded_channel();
        let (mic, mic_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let (text, texts) = mpsc::unbounded_channel();
        let io = VoiceIo { link: link.clone(), events, reconnect: None, mic: mic_rx, speaker: out.clone(), label: "live".into(), texts, identity: Identity::default(), mic_open: true };
        let (a, c) = (agent.clone(), cancel.clone());
        let session = tokio::spawn(async move { a.converse(model, io, c, quiet).await });
        Rig { agent, link, out, live, mic, text, cancel, session }
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    #[tokio::test]
    async fn a_spoken_exchange_with_a_tool_is_shown_counted_and_remembered() {
        let rig = start(Duration::from_secs(30));
        let mut rx = rig.agent.events().subscribe();
        rig.live.send(LiveEvent::Heard("echo ".into())).unwrap();
        rig.live.send(LiveEvent::Heard("gold".into())).unwrap();
        rig.live.send(LiveEvent::ToolCalls(vec![("c1".into(), "echo".into(), json!({ "word": "gold" }))])).unwrap();
        // Live ends the turn together with the tool call; the answer after the tool still belongs to this exchange.
        rig.live.send(LiveEvent::TurnComplete).unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(*rig.link.results.lock().unwrap(), vec![("c1".to_string(), "echo".to_string(), json!({ "echo": "gold" }))]);

        rig.live.send(LiveEvent::Audio(vec![1; 2400])).unwrap();
        rig.live.send(LiveEvent::Said("Gold ".into())).unwrap();
        rig.live.send(LiveEvent::Said("is gold.".into())).unwrap();
        rig.live.send(LiveEvent::Usage(Usage { input: 500, output: 40, thinking: 0, cached: 0 })).unwrap();
        rig.live.send(LiveEvent::TurnComplete).unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;

        let events: Vec<CoreEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(events.iter().find(|e| matches!(e, CoreEvent::TurnStarted { .. })), Some(&CoreEvent::TurnStarted { turn_id: 1, user: "echo gold".into() }));
        assert!(events.contains(&CoreEvent::Answer { turn_id: 1, text: "Gold is gold.".into() }));
        assert!(events.contains(&CoreEvent::TurnUsage { turn_id: 1, calls: 1, input: 500, output: 40, thinking: 0 }));
        assert!(events.iter().any(|e| matches!(e, CoreEvent::ToolFinished { ok: true, name, .. } if name == "echo")));
        assert_eq!(events.iter().filter(|e| matches!(e, CoreEvent::TurnStarted { .. })).count(), 1, "one exchange: {events:?}");
        assert!(!events.iter().any(|e| matches!(e, CoreEvent::Cancelled { .. })));
        let turn = &rig.agent.memory().recent_turns(1).unwrap()[0];
        assert_eq!((turn.user.as_str(), turn.assistant.as_str()), ("echo gold", "Gold is gold."));
        assert_eq!(turn.actions, vec!["Echo gold".to_string()]);
        assert_eq!(*rig.out.played.lock().unwrap(), 2400);

        rig.cancel.cancel();
        assert_eq!(rig.session.await.unwrap(), VoiceEnd::Stopped);
        assert!(rig.link.closed.load(Ordering::Relaxed));
    }

    #[test]
    fn a_typed_message_carries_the_date() {
        use chrono::TimeZone;
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 11, 2, 29, 0).unwrap();
        let sent = typed_for_live("how many days until the new year?", now);
        assert_eq!(sent, "how many days until the new year?\n(Reply in English.)\n(Now: Wednesday 2026-11-11 02:29 چهارشنبه 20 آبان 1405, English app.)");
    }

    #[tokio::test]
    async fn typed_messages_go_to_live_and_ask_brain_hands_the_job_to_the_brain() {
        let rig = start_with(Duration::from_secs(30), Arc::new(Brain("Saved prices.txt on the Desktop.")));
        let mut rx = rig.agent.events().subscribe();
        rig.text.send("gheimat dollar ro save kon".into()).unwrap();
        settle().await;
        let texts = rig.link.texts.lock().unwrap().clone();
        assert_eq!(texts.len(), 1);
        assert!(texts[0].starts_with("gheimat dollar ro save kon\n(Reply in Persian.)\n(Now: "), "Finglish gets a Persian answer: {texts:?}");

        rig.live.send(LiveEvent::ToolCalls(vec![("b1".into(), "ask_brain".into(), json!({ "task": "save the dollar price on the Desktop" }))])).unwrap();
        settle().await;
        assert_eq!(*rig.link.results.lock().unwrap(), vec![("b1".to_string(), "ask_brain".to_string(), json!({ "result": "Saved prices.txt on the Desktop." }))]);
        let events: Vec<CoreEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.contains(&CoreEvent::TurnStarted { turn_id: 1, user: "gheimat dollar ro save kon".into() }), "the typed text is the exchange's question");
        assert!(rig.agent.live_config(&Identity::default(), "m", "v").tools.iter().any(|t| t.name == "ask_brain"));
        rig.cancel.cancel();
        rig.session.await.unwrap();
    }

    /// A model that answers every question with the same lesson.
    struct Teacher;

    #[async_trait]
    impl Generator for Teacher {
        async fn generate(&self, _: &anytask_llm::GenerateRequest) -> Result<anytask_llm::GenerateResponse, anytask_llm::LlmError> {
            Ok(anytask_llm::GenerateResponse { parts: vec![anytask_llm::Part::text("For music use the Spotify app.")], finish_reason: None, usage: None })
        }
        fn label(&self) -> String {
            "teacher".into()
        }
    }

    #[tokio::test]
    async fn a_spoken_correction_teaches_a_lesson_that_later_voice_sessions_know() {
        let learner = Arc::new(crate::learn::Learner::in_memory());
        let mut tools = Registry::default();
        tools.add(Arc::new(Echo));
        let agent = Arc::new(Agent::new(tools, Arc::new(Memory::open_in_memory().unwrap()), Default::default(), Arc::new(DenyAll)).with_learner(learner.clone()));
        let (live, events) = mpsc::unbounded_channel();
        let (_mic, mic) = mpsc::unbounded_channel();
        let (_text, texts) = mpsc::unbounded_channel();
        let io = VoiceIo { link: Arc::new(Link::default()), events, reconnect: None, mic, speaker: Arc::new(Out::default()), label: "live".into(), texts, identity: Identity::default(), mic_open: true };
        let cancel = CancellationToken::new();
        let (a, c) = (agent.clone(), cancel.clone());
        let session = tokio::spawn(async move { a.converse(Arc::new(Teacher), io, c, Duration::from_secs(30)).await });

        // "play a song" is answered from the browser; the user corrects it.
        for (heard, said) in [("ye ahang play kon", "Playing it in the browser."), ("na, to spotify play kon", "OK, in Spotify.")] {
            live.send(LiveEvent::Heard(heard.into())).unwrap();
            live.send(LiveEvent::Said(said.into())).unwrap();
            live.send(LiveEvent::TurnComplete).unwrap();
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
        live.send(LiveEvent::Heard("salam".into())).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let lessons = learner.summary().lessons;
        assert_eq!(lessons.len(), 1, "the correction wrote a lesson: {lessons:?}");
        assert_eq!(lessons[0].text, "For music use the Spotify app.");
        let system = agent.live_config(&Identity::default(), "m", "v").system;
        assert!(system.contains("LEARNED ON THIS PC") && system.contains("- For music use the Spotify app."), "the next voice session knows it");
        cancel.cancel();
        session.await.unwrap();
    }

    #[tokio::test]
    async fn a_whole_web_page_is_not_kept_in_the_voice_context() {
        let rig = start(Duration::from_secs(30));
        let page = "x".repeat(20_000);
        rig.live.send(LiveEvent::ToolCalls(vec![("c1".into(), "echo".into(), json!({ "word": page }))])).unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let results = rig.link.results.lock().unwrap().clone();
        let sent = results[0].2.to_string().chars().count();
        assert!(sent <= VOICE_RESULT_CHARS + 50, "{sent} characters went to Live");
        assert_eq!(results[0].2["truncated"], true);
    }

    #[tokio::test]
    async fn a_dropped_connection_is_opened_again_once() {
        let agent = Arc::new(Agent::new(Registry::default(), Arc::new(Memory::open_in_memory().unwrap()), Default::default(), Arc::new(DenyAll)));
        let (first, second) = (Arc::new(Link::default()), Arc::new(Link::default()));
        let (live, events) = mpsc::unbounded_channel();
        let (live2, events2) = mpsc::unbounded_channel();
        let parked = Arc::new(Mutex::new(Some(events2)));
        let next = second.clone();
        let reconnect: Reconnect = Arc::new(move || {
            let (next, parked) = (next.clone(), parked.clone());
            Box::pin(async move {
                let events = parked.lock().unwrap().take().ok_or("asked to reconnect twice")?;
                Ok((next as Arc<dyn LiveLink>, events))
            })
        });
        let (_mic, mic) = mpsc::unbounded_channel();
        let (text, texts) = mpsc::unbounded_channel();
        let io = VoiceIo { link: first.clone(), events, reconnect: Some(reconnect), mic, speaker: Arc::new(Out::default()), label: "live".into(), texts, identity: Identity::default(), mic_open: true };
        let cancel = CancellationToken::new();
        let (a, c) = (agent.clone(), cancel.clone());
        let session = tokio::spawn(async move { a.converse(Arc::new(NoModel), io, c, Duration::from_secs(30)).await });

        live.send(LiveEvent::Closed("IO error: An existing connection was forcibly closed by the remote host. (os error 10054)".into())).unwrap();
        settle().await;
        text.send("salam".into()).unwrap();
        settle().await;
        let texts = second.texts.lock().unwrap().clone();
        assert!(texts.len() == 1 && texts[0].starts_with("salam\n(Reply in Persian.)"), "the conversation goes on over the new connection: {texts:?}");
        assert!(!session.is_finished());

        // A second drop within a minute ends it (no reconnect loops).
        live2.send(LiveEvent::Closed("Connection reset without closing handshake".into())).unwrap();
        assert!(matches!(session.await.unwrap(), VoiceEnd::Closed(_)));
        drop(live);
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_job_keeps_the_session_alive_with_silence() {
        let rig = start(Duration::from_secs(300));
        rig.live.send(LiveEvent::ToolCalls(vec![("s1".into(), "slow".into(), json!({}))])).unwrap();
        tokio::time::sleep(Duration::from_secs(55)).await;
        let audio = rig.link.audio.lock().unwrap().clone();
        assert!(audio.iter().filter(|chunk| chunk.len() == KEEP_ALIVE_SAMPLES && chunk.iter().all(|s| *s == 0)).count() >= 2, "silence went up while the job ran: {} chunks", audio.len());
        rig.cancel.cancel();
    }

    #[tokio::test]
    async fn the_server_closing_an_idle_session_is_a_quiet_end() {
        let rig = start(Duration::from_secs(30));
        rig.live.send(LiveEvent::Closed("Session closed due to client inactivity.".into())).unwrap();
        assert_eq!(rig.session.await.unwrap(), VoiceEnd::Quiet, "no error on the island for an idle session");
        let rig = start(Duration::from_secs(30));
        rig.live.send(LiveEvent::Closed("Connection reset without closing handshake".into())).unwrap();
        assert!(matches!(rig.session.await.unwrap(), VoiceEnd::Closed(_)), "a real drop is still an error");
    }

    #[tokio::test]
    async fn stop_words_stop_task_and_the_stop_button_end_the_job_but_not_the_conversation() {
        let rig = start(Duration::from_secs(30));
        let mut rx = rig.agent.events().subscribe();
        let stopped = |id: &str| rig.link.results.lock().unwrap().iter().any(|(i, _, r)| i == id && r["stopped"] == true);

        // Spoken "bas kon" while a job runs.
        rig.live.send(LiveEvent::ToolCalls(vec![("s1".into(), "slow".into(), json!({}))])).unwrap();
        settle().await;
        rig.live.send(LiveEvent::Heard("bas ".into())).unwrap();
        rig.live.send(LiveEvent::Heard("kon".into())).unwrap();
        settle().await;
        assert!(stopped("s1"));

        // The model's stop_task.
        rig.live.send(LiveEvent::ToolCalls(vec![("s2".into(), "slow".into(), json!({}))])).unwrap();
        settle().await;
        rig.live.send(LiveEvent::ToolCalls(vec![("x".into(), "stop_task".into(), json!({}))])).unwrap();
        settle().await;
        assert!(stopped("s2"));
        assert!(rig.link.results.lock().unwrap().contains(&("x".to_string(), "stop_task".to_string(), json!({ "stopped": true }))));
        let batches = rig.link.batches.lock().unwrap().clone();
        assert!(batches.contains(&vec!["s2".to_string(), "x".to_string()]), "one response, so the model says it once: {batches:?}");
        assert!(!batches.contains(&vec!["x".to_string()]), "{batches:?}");

        // The Stop button.
        rig.live.send(LiveEvent::ToolCalls(vec![("s3".into(), "slow".into(), json!({}))])).unwrap();
        settle().await;
        assert!(rig.agent.stop_work());
        settle().await;
        assert!(stopped("s3"));
        assert!(!rig.agent.stop_work(), "nothing runs now");

        let events: Vec<CoreEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().any(|e| matches!(e, CoreEvent::ToolFinished { ok: false, name, .. } if name == "slow")));
        assert!(events.iter().any(|e| matches!(e, CoreEvent::Cancelled { .. })));
        assert!(!rig.session.is_finished(), "the conversation stays open");
        rig.cancel.cancel();
        assert_eq!(rig.session.await.unwrap(), VoiceEnd::Stopped);
    }

    /// Returns what the tool context says the user said.
    struct Said;
    #[async_trait]
    impl Tool for Said {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "said".into(), description: "Said".into(), parameters: json!({ "type": "object", "properties": {} }) }
        }
        fn label(&self, _: &Value) -> String {
            "Said".into()
        }
        async fn run(&self, _: Value, ctx: &ToolCtx) -> Result<Value, String> {
            Ok(json!({ "said": ctx.request }))
        }
    }

    #[tokio::test]
    async fn tools_get_the_users_own_words() {
        let mut tools = Registry::default();
        tools.add(Arc::new(Said));
        let agent = Arc::new(Agent::new(tools, Arc::new(Memory::open_in_memory().unwrap()), Default::default(), Arc::new(DenyAll)));
        let link = Arc::new(Link::default());
        let (live, events) = mpsc::unbounded_channel();
        let (_mic, mic) = mpsc::unbounded_channel();
        let (_text, texts) = mpsc::unbounded_channel();
        let io = VoiceIo { link: link.clone(), events, reconnect: None, mic, speaker: Arc::new(Out::default()), label: "live".into(), texts, identity: Identity::default(), mic_open: true };
        let cancel = CancellationToken::new();
        let (a, c) = (agent.clone(), cancel.clone());
        let session = tokio::spawn(async move { a.converse(Arc::new(NoModel), io, c, Duration::from_secs(30)).await });
        live.send(LiveEvent::Heard("tu gmail amadash kon vali nafrest".into())).unwrap();
        live.send(LiveEvent::ToolCalls(vec![("c1".into(), "said".into(), json!({}))])).unwrap();
        settle().await;
        assert_eq!(link.results.lock().unwrap()[0].2, json!({ "said": "tu gmail amadash kon vali nafrest" }));
        cancel.cancel();
        session.await.unwrap();
    }

    #[tokio::test]
    async fn a_model_that_keeps_calling_tools_must_answer() {
        let rig = start(Duration::from_secs(30));
        rig.live.send(LiveEvent::Heard("weather".into())).unwrap();
        for i in 0..MAX_TOOLS_PER_QUESTION + 1 {
            rig.live.send(LiveEvent::ToolCalls(vec![(format!("c{i}"), "echo".into(), json!({ "word": "x" }))])).unwrap();
            settle().await;
        }
        let results = rig.link.results.lock().unwrap().clone();
        assert_eq!(results.len() as u32, MAX_TOOLS_PER_QUESTION + 1);
        assert!(results[..MAX_TOOLS_PER_QUESTION as usize].iter().all(|(_, _, r)| r["echo"] == "x"));
        assert_eq!(results.last().unwrap().2["error"], TOO_MANY_TOOLS);
        rig.cancel.cancel();
        rig.session.await.unwrap();
    }

    #[test]
    fn only_short_stop_requests_count() {
        for text in ["stop", "Bas kon!", "ghat kon", "قطعش کن", "بسه دیگه", "نکن", "never mind", "lotfan velesh kon"] {
            assert!(is_stop_request(text), "{text}");
        }
        for text in ["", "stop the music and then open notepad and write a long letter please", "basket", "نکنه بارون بیاد", "edame bede"] {
            assert!(!is_stop_request(text), "{text}");
        }
    }

    #[tokio::test]
    async fn the_microphone_sends_silence_while_the_assistant_talks() {
        let rig = start(Duration::from_secs(30));
        rig.mic.send(vec![5; 640]).unwrap();
        settle().await;
        rig.out.playing.store(true, Ordering::Relaxed);
        rig.mic.send(vec![5; 640]).unwrap();
        settle().await;
        let audio = rig.link.audio.lock().unwrap().clone();
        assert_eq!(audio.len(), 2);
        assert!(audio[0].iter().all(|s| *s == 5));
        assert!(audio[1].iter().all(|s| *s == 0), "echo of the assistant must not reach the model");
        rig.cancel.cancel();
        rig.session.await.unwrap();
    }

    #[tokio::test]
    async fn a_quiet_session_closes_and_a_dropped_connection_is_reported() {
        let rig = start(Duration::from_millis(300));
        assert_eq!(rig.session.await.unwrap(), VoiceEnd::Quiet);

        let rig = start(Duration::from_secs(30));
        let mut rx = rig.agent.events().subscribe();
        rig.live.send(LiveEvent::Closed("quota".into())).unwrap();
        assert_eq!(rig.session.await.unwrap(), VoiceEnd::Closed("quota".into()));
        let events: Vec<CoreEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.contains(&CoreEvent::Error { turn_id: None, message: "quota".into() }));
    }

    #[test]
    fn the_voice_setup_has_tools_rules_and_recent_history() {
        let mut tools = Registry::default();
        tools.add(Arc::new(Echo));
        let memory = Arc::new(Memory::open_in_memory().unwrap());
        memory.add_turn(1, "play music", "Playing.", &["Spotify".into()]).unwrap();
        let agent = Agent::new(tools, memory, Default::default(), Arc::new(DenyAll));
        let config = agent.live_config(&Identity::default(), "gemini-3.8-live", "Aoede");
        assert_eq!(config.tools.len(), 3, "the tools plus ask_brain and stop_task");
        assert!(config.system.contains("VOICE:"));
        // The app is in English in tests (the default): an English message gets an English answer.
        assert!(config.system.contains("Answer in the language of the user's last message: English for English"), "{}", config.system);
        assert!(!config.system.contains("The user speaks Persian."));
        assert!(config.system.contains("]: play music") && config.system.contains("
User ["), "{}", config.system);
        assert!(config.system.contains("Spotify"));
    }
}
