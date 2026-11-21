//! Wake word in the desktop app: while it is on (and no conversation is open, and no training runs) a microphone
//! stream feeds the detector; saying the phrase opens a voice conversation. Training runs here too, with a short
//! tone before each step. The trained word and the on/off switch are kept in wake-word.json in the app data folder.

use crate::island::ISLAND;
use anytask_audio::{Mic, Speaker};
use anytask_core::CoreEvent;
use anytask_wake::{EnrollEvent, Enrollment, Listener, Stage, WakeModel};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// No audio for this long means the microphone went away (unplugged, taken by another app).
const STALL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeStatus {
    pub has_model: bool,
    pub enabled: bool,
    pub listening: bool,
    pub training: bool,
    pub label: String,
    pub quality: String,
    pub tested: bool,
    pub created_at: i64,
}

#[derive(Default)]
struct Saved {
    enabled: bool,
    model: Option<WakeModel>,
}

pub struct WakeService {
    path: PathBuf,
    saved: Mutex<Saved>,
    mic: Mutex<Option<Mic>>,
    /// A voice conversation is open (it has the microphone).
    busy: AtomicBool,
    /// Training runs; cleared to abort it.
    training: Arc<AtomicBool>,
}

impl WakeService {
    pub fn load(path: PathBuf) -> Self {
        let saved = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| Saved { enabled: v["enabled"].as_bool().unwrap_or(false), model: WakeModel::from_json(&v["model"]) })
            .unwrap_or_default();
        Self { path, saved: Mutex::new(saved), mic: Mutex::new(None), busy: AtomicBool::new(false), training: Arc::new(AtomicBool::new(false)) }
    }

    pub fn status(&self) -> WakeStatus {
        let saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        let model = saved.model.as_ref();
        WakeStatus {
            has_model: model.is_some(),
            enabled: saved.enabled,
            listening: self.mic.lock().map(|m| m.is_some()).unwrap_or(false),
            training: self.training.load(Ordering::Relaxed),
            label: model.map(|m| m.label.clone()).unwrap_or_default(),
            quality: model.map(|m| m.quality.clone()).unwrap_or_default(),
            tested: model.is_some_and(|m| m.tested),
            created_at: model.map(|m| m.created_at).unwrap_or(0),
        }
    }

    fn save(&self) -> Result<(), String> {
        let saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        let data = json!({ "enabled": saved.enabled, "model": saved.model.as_ref().map(WakeModel::to_json) });
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, data.to_string()).and_then(|_| std::fs::rename(&tmp, &self.path)).map_err(|e| format!("Could not save the wake word: {e}"))
    }

    pub fn set_enabled<R: Runtime>(&self, app: &AppHandle<R>, enabled: bool) -> Result<WakeStatus, String> {
        self.saved.lock().unwrap_or_else(|e| e.into_inner()).enabled = enabled;
        self.save()?;
        self.sync(app);
        Ok(self.status())
    }

    /// Uses a wake word trained elsewhere (v1).
    pub fn set_model<R: Runtime>(&self, app: &AppHandle<R>, model: WakeModel, enabled: bool) -> Result<WakeStatus, String> {
        *self.saved.lock().unwrap_or_else(|e| e.into_inner()) = Saved { enabled, model: Some(model) };
        self.save()?;
        self.sync(app);
        Ok(self.status())
    }

    pub fn remove<R: Runtime>(&self, app: &AppHandle<R>) -> Result<WakeStatus, String> {
        *self.saved.lock().unwrap_or_else(|e| e.into_inner()) = Saved::default();
        self.save()?;
        self.sync(app);
        Ok(self.status())
    }

    /// A conversation opened or closed; the wake word pauses meanwhile.
    pub fn set_busy<R: Runtime>(&self, app: &AppHandle<R>, busy: bool) {
        self.busy.store(busy, Ordering::Relaxed);
        self.sync(app);
    }

    /// Starts or stops listening to match the current state, and tells the windows.
    pub fn sync<R: Runtime>(&self, app: &AppHandle<R>) {
        let model = {
            let saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
            saved.model.clone().filter(|_| saved.enabled && !self.busy.load(Ordering::Relaxed) && !self.training.load(Ordering::Relaxed))
        };
        let mut mic = self.mic.lock().unwrap_or_else(|e| e.into_inner());
        match (model, mic.is_some()) {
            (Some(model), false) => match listen(app.clone(), model) {
                Ok(started) => *mic = Some(started),
                Err(e) => log(app, format!("FAILED to start listening: {e}")),
            },
            (None, true) => *mic = None,
            _ => {}
        }
        drop(mic);
        let _ = app.emit("wake-changed", self.status());
    }

    /// Records a new wake word. Progress arrives in the windows as "wake-training" events.
    pub fn train<R: Runtime>(&self, app: &AppHandle<R>, label: String) -> Result<(), String> {
        if self.training.swap(true, Ordering::Relaxed) {
            return Err("Training is already running.".into());
        }
        self.sync(app);
        let (tx, rx) = mpsc::channel::<Vec<i16>>();
        let mic = Mic::start(move |chunk| {
            let _ = tx.send(chunk);
        });
        let started = mic.and_then(|mic| Speaker::start().map(|speaker| (mic, speaker)));
        let (mic, speaker) = match started {
            Ok(both) => both,
            Err(e) => {
                self.training.store(false, Ordering::Relaxed);
                self.sync(app);
                return Err(e);
            }
        };
        let (app, running) = (app.clone(), self.training.clone());
        std::thread::spawn(move || {
            let outcome = run_training(&app, &rx, &speaker, &running, &label);
            drop(mic);
            speaker.stop();
            let wake = app.state::<WakeService>();
            if let Some(model) = outcome {
                let mut saved = wake.saved.lock().unwrap_or_else(|e| e.into_inner());
                saved.model = Some(model);
                saved.enabled = true;
                drop(saved);
                if let Err(e) = wake.save() {
                    let _ = app.emit("wake-training", json!({ "kind": "failed", "reason": e }));
                }
            }
            wake.training.store(false, Ordering::Relaxed);
            wake.sync(&app);
        });
        Ok(())
    }

    /// Aborts training (the new word is not kept).
    pub fn cancel_training(&self) {
        self.training.store(false, Ordering::Relaxed);
    }
}

fn log<R: Runtime>(app: &AppHandle<R>, text: String) {
    if let Some(brain) = app.try_state::<crate::brain::Brain>() {
        brain.agent.events().emit(CoreEvent::Log { turn_id: None, source: "wake".into(), text });
    }
}

fn to_f32(chunk: &[i16]) -> Vec<f32> {
    chunk.iter().map(|s| f32::from(*s) / 32768.0).collect()
}

/// A microphone stream into the detector; a match opens a conversation.
fn listen<R: Runtime>(app: AppHandle<R>, model: WakeModel) -> Result<Mic, String> {
    let mut listener = Listener::new(model);
    Mic::start(move |chunk| {
        if let Some(trigger) = listener.push(&to_f32(&chunk)) {
            log(&app, format!("heard the wake word (score {:.3}, threshold {:.3})", trigger.score, trigger.threshold));
            crate::actions::show_island(&app);
            crate::voice::open(&app);
        }
    })
}

/// A short soft beep: "now".
fn tone() -> Vec<i16> {
    let n = 16_000 * 12 / 100;
    (0..n)
        .map(|i| {
            let fade = (i.min(n - i) as f32 / 160.0).min(1.0);
            (8000.0 * fade * (2.0 * std::f32::consts::PI * 880.0 * i as f32 / 16_000.0).sin()) as i16
        })
        .collect()
}

fn run_training<R: Runtime>(app: &AppHandle<R>, rx: &mpsc::Receiver<Vec<i16>>, speaker: &Speaker, running: &AtomicBool, label: &str) -> Option<WakeModel> {
    let emit = |value: Value| {
        let _ = app.emit("wake-training", value);
    };
    let mut enrollment = Enrollment::new(label, chrono::Utc::now().timestamp_millis());
    emit(json!({ "kind": "started" }));
    loop {
        if !running.load(Ordering::Relaxed) {
            emit(json!({ "kind": "cancelled" }));
            return None;
        }
        let chunk = match rx.recv_timeout(STALL) {
            Ok(chunk) => chunk,
            Err(_) => {
                emit(json!({ "kind": "failed", "reason": "mic_unavailable" }));
                return None;
            }
        };
        for event in enrollment.push(&to_f32(&chunk)) {
            match event {
                EnrollEvent::Prompt(stage) => {
                    speaker.play(&tone(), 16_000);
                    let _ = app.emit_to(ISLAND, "wake-prompt", ());
                    emit(match stage {
                        Stage::Enroll { index, total } => json!({ "kind": "prompt", "stage": "enroll", "index": index, "total": total }),
                        Stage::Negatives { seconds } => json!({ "kind": "prompt", "stage": "negatives", "seconds": seconds }),
                        Stage::Test => json!({ "kind": "prompt", "stage": "test" }),
                        Stage::TestRetry => json!({ "kind": "prompt", "stage": "test_retry" }),
                    });
                }
                EnrollEvent::Accepted { index, total } => emit(json!({ "kind": "accepted", "index": index, "total": total })),
                EnrollEvent::Rejected(reason) => emit(json!({ "kind": "rejected", "reason": reason })),
                EnrollEvent::Tick(remaining) => emit(json!({ "kind": "tick", "remaining": remaining })),
                EnrollEvent::Failed(reason) => {
                    log(app, format!("training FAILED: {reason}"));
                    emit(json!({ "kind": "failed", "reason": reason }));
                    return None;
                }
                EnrollEvent::Finished { model, tested } => {
                    log(app, format!("trained \"{}\": quality {}, self-test {}", model.label, model.quality, if tested { "passed" } else { "missed" }));
                    emit(json!({ "kind": "finished", "quality": model.quality, "tested": tested }));
                    return Some(*model);
                }
            }
        }
    }
}
