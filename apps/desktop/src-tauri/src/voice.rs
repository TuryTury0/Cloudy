//! Voice: the talk shortcut opens a Gemini Live conversation with the microphone and the speaker; pressing it
//! again (or Stop) closes it. The microphone opens before the connection, so nothing said meanwhile is lost.
//!
//! Live answers first, always: a typed message also goes to Live (a typed conversation: speaker on, no
//! microphone) and Live hands jobs with several steps to the brain. When Live cannot be reached (no Gemini key,
//! Google not reachable from this network), the brain answers in text and the island says why.

use crate::brain::Brain;
use crate::commands::{SettingsStore, active_generator, current};
use crate::island::ISLAND;
use crate::wake::WakeService;
use anytask_audio::{Mic, Speaker};
use anytask_core::{AssistantState, CoreEvent, Identity, LiveLink, Reconnect, StateUpdate, VoiceIo, VoiceOut};
use anytask_llm::{LiveConfig, LiveEvent, LiveSession, Provider};
use anytask_storage::model_key_name;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio_util::sync::CancellationToken;

/// Island animation updates per second while talking.
const LEVEL_EVERY: Duration = Duration::from_millis(60);
/// A typed conversation stays open this long after the last message (follow-ups keep the context).
const TYPED_QUIET: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Microphone and speaker.
    Voice,
    /// Typed messages, spoken answers.
    Typed,
}

struct Open {
    id: u64,
    cancel: CancellationToken,
    mode: Mode,
    texts: mpsc::UnboundedSender<String>,
}

/// The running conversation, if any (with its number, so an old one never clears a newer one).
#[derive(Default)]
pub struct VoiceSlot {
    next: AtomicU64,
    current: Mutex<Option<Open>>,
}

impl VoiceSlot {
    pub fn is_open(&self) -> bool {
        self.current.lock().map(|s| s.is_some()).unwrap_or(false)
    }

    fn mode(&self) -> Option<Mode> {
        self.current.lock().ok().and_then(|s| s.as_ref().map(|o| o.mode))
    }

    /// Closes the conversation. Returns false when none was open.
    pub fn stop(&self) -> bool {
        self.current.lock().ok().and_then(|mut s| s.take()).map(|o| o.cancel.cancel()).is_some()
    }

    /// Hands a typed message to the open conversation; false when none is open.
    fn send_text(&self, text: &str) -> bool {
        self.current.lock().ok().and_then(|s| s.as_ref().map(|o| o.texts.send(text.to_string()).is_ok())).unwrap_or(false)
    }

    /// Claims the slot; None when a conversation is already open.
    fn open(&self, mode: Mode) -> Option<(u64, CancellationToken, mpsc::UnboundedReceiver<String>)> {
        let mut s = self.current.lock().ok()?;
        if s.is_some() {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = CancellationToken::new();
        let (texts, rx) = mpsc::unbounded_channel();
        *s = Some(Open { id, cancel: cancel.clone(), mode, texts });
        Some((id, cancel, rx))
    }

    fn closed(&self, id: u64) {
        if let Ok(mut s) = self.current.lock()
            && s.as_ref().is_some_and(|o| o.id == id)
        {
            *s = None;
        }
    }
}

#[derive(Clone, Serialize)]
struct Levels {
    mic: f32,
    speaker: f32,
}

struct Out(Speaker);

impl VoiceOut for Out {
    fn play(&self, pcm: &[i16], rate: u32) {
        self.0.play(pcm, rate);
    }
    fn clear(&self) {
        self.0.clear();
    }
    fn is_playing(&self) -> bool {
        self.0.is_playing()
    }
}

/// Keychain name of the voice's own key (optional).
pub const VOICE_KEY: &str = "voice:live";

/// The voice's own key if one was saved, else the key of a Gemini model (the chosen one, else the first).
pub(crate) fn live_key(brain: &Brain, settings: &crate::settings::Settings) -> Result<String, String> {
    if let Some(key) = brain.secrets.get(VOICE_KEY).filter(|k| !k.trim().is_empty()) {
        return Ok(key);
    }
    let gemini = settings.active_model().filter(|m| m.provider == Provider::Gemini).or_else(|| settings.models.iter().find(|m| m.provider == Provider::Gemini));
    let Some(gemini) = gemini else {
        return Err("Voice needs a Gemini key: save one in Settings > General > Voice, or add a Gemini model.".into());
    };
    brain.secrets.get(&model_key_name(&gemini.id)).filter(|k| !k.trim().is_empty()).ok_or_else(|| "No API key for the Gemini model; add it in Settings > Models or save a voice key.".to_string())
}

/// Opens a spoken conversation, or closes the open one. A typed conversation becomes a spoken one.
pub fn toggle<R: Runtime>(app: &AppHandle<R>) {
    let slot = app.state::<VoiceSlot>();
    match slot.mode() {
        Some(Mode::Voice) => {
            slot.stop();
        }
        Some(Mode::Typed) => {
            slot.stop();
            open(app);
        }
        None => open(app),
    }
}

/// Opens a spoken conversation unless one is open (the wake word). The wake word pauses while it runs.
pub fn open<R: Runtime>(app: &AppHandle<R>) {
    start(app, Mode::Voice, None);
}

/// A typed message: into the open conversation, or a new typed one. Live answers first, always.
pub fn ask<R: Runtime>(app: &AppHandle<R>, text: String) {
    if !app.state::<VoiceSlot>().send_text(&text) {
        start(app, Mode::Typed, Some(text));
    }
}

/// After Live could not be reached, typed messages go straight to the brain for a while instead of waiting for the
/// same failure again (11-08, VPN not reaching Google: a location refusal cost 2-3 s per message, a connection that
/// timed out both ways 48 s). Spoken conversations still try Live.
const LIVE_REFUSED_FOR: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const LIVE_UNREACHABLE_FOR: std::time::Duration = std::time::Duration::from_secs(3 * 60);
static LIVE_DOWN_UNTIL: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

fn live_down() -> bool {
    LIVE_DOWN_UNTIL.lock().ok().and_then(|until| *until).is_some_and(|until| std::time::Instant::now() < until)
}

fn mark_live_down(message: &str) {
    let wait = if message.contains("location is not supported") { LIVE_REFUSED_FOR } else { LIVE_UNREACHABLE_FOR };
    if let Ok(mut until) = LIVE_DOWN_UNTIL.lock() {
        *until = Some(std::time::Instant::now() + wait);
    }
}

/// The brain answers a typed message in text when Live cannot.
async fn brain_answers<R: Runtime>(app: &AppHandle<R>, text: &str) {
    let brain = app.state::<Brain>();
    let settings = current(&app.state::<SettingsStore>());
    let identity = Identity { assistant_name: settings.assistant_name.clone(), user_name: settings.user_name.clone() };
    brain.agent.run(active_generator(&brain, &settings), &identity, text).await;
}

fn start<R: Runtime>(app: &AppHandle<R>, mode: Mode, first: Option<String>) {
    if let Some(text) = first.clone().filter(|_| live_down()) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            app.state::<Brain>().agent.events().emit(CoreEvent::Log {
                turn_id: None,
                source: "voice".into(),
                text: "Live could not be reached a few minutes ago; the brain answers in text".into(),
            });
            brain_answers(&app, &text).await;
        });
        return;
    }
    let Some((id, cancel, texts)) = app.state::<VoiceSlot>().open(mode) else {
        // Opened by someone else a moment ago: the message joins it.
        if let Some(text) = first {
            app.state::<VoiceSlot>().send_text(&text);
        }
        return;
    };
    // The message that opened this conversation goes into it too; it waits in the queue until Live is connected.
    if let Some(text) = &first {
        app.state::<VoiceSlot>().send_text(text);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = app.emit_to(ISLAND, "voice-session", true);
        app.state::<WakeService>().set_busy(&app, true);
        let mut texts = Some(texts);
        let result = converse(&app, cancel, mode, &mut texts, first.clone()).await;
        app.state::<VoiceSlot>().closed(id);
        app.state::<WakeService>().set_busy(&app, false);
        let _ = app.emit_to(ISLAND, "voice-session", false);
        let Err(message) = result else { return };
        mark_live_down(&message);
        // Everything typed while Live was connecting waits in the queue (the opening message too): the brain answers
        // each, in order. Before, only the opening message was answered and the rest were lost (11-08 03:26).
        let mut queued = Vec::new();
        if let Some(mut rx) = texts {
            while let Ok(text) = rx.try_recv() {
                queued.push(text);
            }
        }
        let brain = app.state::<Brain>();
        if !queued.is_empty() {
            brain.agent.events().emit(CoreEvent::Log { turn_id: None, source: "voice".into(), text: format!("Live not reachable ({message}); the brain answers in text") });
            let notice = anytask_core::lang::tr("Voice (Gemini Live) is not reachable; the answer comes in writing.", "صدا (Gemini Live) در دسترس نیست؛ جواب نوشتاری می‌آید.");
            let _ = app.emit_to(ISLAND, "notice", format!("{notice} ({})", short(&message)));
            for text in queued {
                brain_answers(&app, &text).await;
            }
            return;
        }
        match first {
            Some(text) => brain_answers(&app, &text).await,
            None => {
                // Spoken requests cannot fall back to the brain: say what to do about a location refusal.
                let message = if message.contains("location is not supported") {
                    format!("{message} — {}", anytask_core::lang::tr("Gemini does not answer from here: turn the VPN on.", "Gemini از اینجا جواب نمی‌دهد: VPN را روشن کنید."))
                } else {
                    message
                };
                brain.agent.events().emit(CoreEvent::Error { turn_id: None, message: message.clone() });
                brain.agent.events().emit(CoreEvent::State(StateUpdate::new(AssistantState::Error).with_detail(message)));
            }
        }
    });
}

/// The voice got out through the proxy last time because the direct connection failed: try that way first.
/// Test 3 (05:20): 19 s to connect, 15 s of it a direct attempt timing out before the proxy worked.
static PROXY_FIRST: AtomicBool = AtomicBool::new(false);

/// Connects to Live. When that fails (Google's location check answers 403, a VPN's proxy is broken, a reset) and
/// the user has a proxy the voice does not use yet, once more through it (11-04 19:51: two 403s in a row cost 38 s).
/// The way that worked is tried first next time.
pub(crate) async fn connect_live(config: &LiveConfig, key: &str, proxy: Option<String>) -> Result<(LiveSession, mpsc::UnboundedReceiver<LiveEvent>), String> {
    let Some(through) = proxy.filter(|_| config.proxy.is_none()).map(|proxy| LiveConfig { proxy: Some(proxy), ..config.clone() }) else {
        return LiveSession::connect(config, key).await.map_err(|e| e.to_string());
    };
    let proxy_first = PROXY_FIRST.load(Ordering::Relaxed);
    let (first, second) = if proxy_first { (&through, config) } else { (config, &through) };
    match LiveSession::connect(first, key).await {
        Ok(connected) => Ok(connected),
        Err(e) => match LiveSession::connect(second, key).await {
            Ok(connected) => {
                PROXY_FIRST.store(!proxy_first, Ordering::Relaxed);
                Ok(connected)
            }
            Err(again) => Err(format!("{e}; the other way ({}) too: {again}", if proxy_first { "direct" } else { "through the proxy" })),
        },
    }
}

/// Opens the conversation again after a drop, with a fresh prompt (it carries the recent turns from memory).
fn reconnector<R: Runtime>(app: &AppHandle<R>, key: String, identity: Identity) -> Reconnect {
    let app = app.clone();
    Arc::new(move || {
        let (app, key, identity) = (app.clone(), key.clone(), identity.clone());
        Box::pin(async move {
            let (config, proxy) = {
                let brain = app.state::<Brain>();
                let settings = current(&app.state::<SettingsStore>());
                let mut config = brain.agent.live_config(&identity, &settings.voice.model, &settings.voice.voice);
                config.proxy = settings.proxy.voice_url();
                (config, settings.proxy.url())
            };
            let (session, events) = connect_live(&config, &key, proxy).await?;
            Ok((Arc::new(session) as Arc<dyn LiveLink>, events))
        })
    })
}

fn short(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 90 { flat } else { format!("{}…", flat.chars().take(90).collect::<String>()) }
}

/// `texts` is taken once Live is connected; when connecting fails it is left for the caller, who answers what waits in it.
async fn converse<R: Runtime>(
    app: &AppHandle<R>,
    cancel: CancellationToken,
    mode: Mode,
    texts: &mut Option<mpsc::UnboundedReceiver<String>>,
    first: Option<String>,
) -> Result<(), String> {
    let brain = app.state::<Brain>();
    let settings = current(&app.state::<SettingsStore>());
    let events = brain.agent.events().clone();
    let log = |text: String| events.emit(CoreEvent::Log { turn_id: None, source: "voice".into(), text });

    let key = live_key(&brain, &settings)?;

    let spoken = mode == Mode::Voice;
    let waiting = if spoken { StateUpdate::new(AssistantState::Listening).with_detail("…") } else { StateUpdate::new(AssistantState::Thinking).with_detail(first.clone().unwrap_or_default()) };
    events.emit(CoreEvent::State(waiting));
    let (mic_tx, mic_rx) = mpsc::unbounded_channel();
    // A typed conversation keeps the sender (no microphone), so the receiver simply waits.
    let (mic, _no_mic) = if spoken {
        (
            Some(Arc::new(Mic::start(move |chunk| {
                let _ = mic_tx.send(chunk);
            })?)),
            None,
        )
    } else {
        (None, Some(mic_tx))
    };
    let speaker = Speaker::start()?;

    let identity = Identity { assistant_name: settings.assistant_name.clone(), user_name: settings.user_name.clone() };
    let mut config = brain.agent.live_config(&identity, &settings.voice.model, &settings.voice.voice);
    config.proxy = settings.proxy.voice_url();
    let started = Instant::now();
    let connected = tokio::select! {
        _ = cancel.cancelled() => {
            speaker.stop();
            events.emit(CoreEvent::State(StateUpdate::new(AssistantState::Idle)));
            return Ok(());
        }
        connected = connect_live(&config, &key, settings.proxy.url()) => connected,
    };
    let (session, live_events) = connected.inspect_err(|_| speaker.stop())?;
    log(format!("voice ready in {:.1} s ({}, voice {})", started.elapsed().as_secs_f32(), settings.voice.model, settings.voice.voice));

    // Mouth and ears for the island's animation.
    let (meter_app, meter_mic, meter_speaker, meter_stop) = (app.clone(), mic.clone(), speaker.clone(), cancel.child_token());
    let meter_done = meter_stop.clone();
    tauri::async_runtime::spawn(async move {
        while !meter_done.is_cancelled() {
            let mic = meter_mic.as_ref().map(|m| m.level()).unwrap_or(0.0);
            let _ = meter_app.emit_to(ISLAND, "voice-level", Levels { mic, speaker: meter_speaker.level() });
            tokio::time::sleep(LEVEL_EVERY).await;
        }
        let _ = meter_app.emit_to(ISLAND, "voice-level", Levels { mic: 0.0, speaker: 0.0 });
    });

    let io = VoiceIo {
        link: Arc::new(session),
        events: live_events,
        reconnect: Some(reconnector(app, key.clone(), identity.clone())),
        mic: mic_rx,
        speaker: Arc::new(Out(speaker.clone())),
        label: settings.voice.model.clone(),
        texts: texts.take().unwrap_or_else(|| mpsc::unbounded_channel().1),
        identity: identity.clone(),
        mic_open: spoken,
    };
    let model = active_generator(&brain, &settings);
    let quiet = if spoken { Duration::from_secs(u64::from(settings.voice.quiet_seconds.clamp(5, 300))) } else { TYPED_QUIET };
    brain.agent.converse(model, io, cancel, quiet).await;
    meter_stop.cancel();
    speaker.stop();
    drop(mic);
    Ok(())
}
