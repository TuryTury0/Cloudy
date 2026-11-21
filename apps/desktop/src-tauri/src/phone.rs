//! The phone connection in the desktop app: when turned on, phones on the same network can connect (after pairing
//! with the QR code in Settings > Phone), see what the assistant does and send it requests.
//!
//! Off by default: listening on the network makes Windows ask once for firewall permission.

use crate::brain::Brain;
use crate::commands::{SettingsStore, active_generator, current};
use anytask_core::{CoreEvent, Identity};
use anytask_remote::{DEFAULT_PORT, Device, DeviceStore, Handler, Keys, Outbox, RemoteServer};
use anytask_storage::SecretStore;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast::error::RecvError;

const KEY_NAME: &str = "remote:key";
/// How long a call to the phone may take.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneDevice {
    #[serde(flatten)]
    pub device: Device,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneStatus {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub address: String,
    pub devices: Vec<PhoneDevice>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingView {
    /// The QR code as an SVG image.
    pub svg: String,
    /// The same text, for typing or pasting on the phone.
    pub text: String,
    /// A 6-digit code to type on a phone on the same network (when the camera cannot scan).
    pub code: String,
    /// This PC's key in short ("A1B2-C3D4-E5F6"): the phone shows it before sending the code, to compare.
    pub fingerprint: String,
    pub minutes: u32,
}

/// The addresses phones can reach this PC at (local network and overlays such as Tailscale), the route's first.
pub fn addresses() -> Vec<String> {
    anytask_control::net::reachable(&anytask_control::net::adapters(), anytask_remote::lan_address().as_deref())
}

pub struct Phone {
    app: AppHandle,
    keys: Keys,
    devices_path: PathBuf,
    flag_path: PathBuf,
    server: tokio::sync::Mutex<Option<RemoteServer>>,
    error: std::sync::Mutex<Option<String>>,
    /// Calls sent to the phone, waiting for its answer.
    pending: std::sync::Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>>,
    next_call: std::sync::atomic::AtomicU64,
    /// A request from the phone is about to start: the next turn is the phone's.
    from_phone: std::sync::atomic::AtomicBool,
    /// Turns the phone asked for (answered there; the PC does not read them aloud).
    phone_turns: std::sync::Mutex<std::collections::VecDeque<u64>>,
}

impl Phone {
    /// The PC's key lives in the OS keychain (made once).
    pub fn new(app: AppHandle, data_dir: &std::path::Path, secrets: &Arc<dyn SecretStore>) -> Arc<Self> {
        let keys = secrets.get(KEY_NAME).and_then(|s| Keys::from_secret(&s)).unwrap_or_else(|| {
            let keys = Keys::generate();
            let _ = secrets.set(KEY_NAME, &keys.to_secret());
            keys
        });
        Arc::new(Self {
            app,
            keys,
            devices_path: data_dir.join("phones.json"),
            flag_path: data_dir.join("phone.json"),
            server: tokio::sync::Mutex::new(None),
            error: std::sync::Mutex::new(None),
            pending: Default::default(),
            next_call: Default::default(),
            from_phone: Default::default(),
            phone_turns: Default::default(),
        })
    }

    fn wanted(&self) -> bool {
        std::fs::read_to_string(&self.flag_path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["enabled"].as_bool()).unwrap_or(false)
    }

    /// Starts listening if it was on last time, and forwards the assistant's events to connected phones.
    pub fn start(self: &Arc<Self>) {
        // Files sent to a phone from the Phone page during the last run were downloaded by now.
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("anytask-to-phone"));
        let phone = self.clone();
        tauri::async_runtime::spawn(async move {
            if phone.wanted() {
                let _ = phone.set_enabled(true).await;
            }
            let mut rx = phone.app.state::<Brain>().agent.events().subscribe();
            loop {
                match rx.recv().await {
                    Ok(CoreEvent::Log { .. }) => {}
                    Ok(event) => {
                        if let CoreEvent::TurnStarted { turn_id, .. } = &event {
                            phone.claim_turn(*turn_id);
                        }
                        if let Some(server) = phone.server.lock().await.as_ref() {
                            server.broadcast(json!({ "type": "event", "event": event }));
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    pub async fn set_enabled(self: &Arc<Self>, enabled: bool) -> PhoneStatus {
        let _ = std::fs::write(&self.flag_path, json!({ "enabled": enabled }).to_string());
        let mut server = self.server.lock().await;
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if enabled && server.is_none() {
            let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into());
            let handler = Arc::new(AppHandler { app: self.app.clone(), uploads: Default::default(), pressed: Default::default(), pending_power: Default::default() });
            match RemoteServer::start(&format!("0.0.0.0:{DEFAULT_PORT}"), &name, self.keys.clone(), DeviceStore::load(self.devices_path.clone()), handler).await {
                Ok(started) => {
                    // Phones whose saved address went stale find the PC again by asking on the network.
                    if let Err(e) = started.start_discovery(&format!("0.0.0.0:{DEFAULT_PORT}")).await {
                        log(&self.app, format!("finding by phones is off: {e}"));
                    }
                    *server = Some(started);
                }
                Err(e) => *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("Could not listen on port {DEFAULT_PORT}: {e}")),
            }
        } else if !enabled {
            server.take();
        }
        drop(server);
        self.update_tools().await;
        self.changed().await
    }

    pub async fn status(&self) -> PhoneStatus {
        let server = self.server.lock().await;
        let devices = match server.as_ref() {
            Some(s) => {
                let connected = s.connected();
                s.devices().into_iter().map(|d| PhoneDevice { connected: connected.contains(&d.key), device: d }).collect()
            }
            None => DeviceStore::load(self.devices_path.clone()).list().into_iter().map(|device| PhoneDevice { device, connected: false }).collect(),
        };
        PhoneStatus {
            enabled: server.is_some(),
            error: self.error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            address: addresses().first().map(|ip| format!("{ip}:{DEFAULT_PORT}")).unwrap_or_default(),
            devices,
        }
    }

    async fn changed(&self) -> PhoneStatus {
        let status = self.status().await;
        let _ = self.app.emit("phone-changed", &status);
        status
    }

    /// A fresh QR code (valid 10 minutes, one phone).
    pub async fn pairing(&self) -> Result<PairingView, String> {
        let server = self.server.lock().await;
        let server = server.as_ref().ok_or("Turn on the phone connection first.")?;
        // Every address the phone may reach: with a VPN on, the route's own address is the VPN's.
        let hosts = addresses();
        if hosts.is_empty() {
            return Err("This PC is not on a network.".into());
        }
        let text = server.new_pairing(hosts).qr_text();
        let qr = qrcode::QrCode::new(text.as_bytes()).map_err(|e| e.to_string())?;
        let svg = qr.render::<qrcode::render::svg::Color>().min_dimensions(240, 240).quiet_zone(true).build();
        Ok(PairingView { svg, text, code: server.pairing_code().unwrap_or_default(), fingerprint: anytask_remote::fingerprint(&self.keys.public), minutes: 10 })
    }

    pub async fn close_pairing(&self) {
        if let Some(server) = self.server.lock().await.as_ref() {
            server.close_pairing();
        }
    }

    /// Runs a command on the connected phone and waits for its answer.
    pub async fn call_device(&self, command: &str, args: Value) -> Result<Value, String> {
        self.ask_phone(|id| json!({ "type": "call", "id": id, "name": command, "args": args }), "The phone did not answer (is Cloudy open on it?).").await
    }

    /// Sends what `make` builds with a fresh id to the connected phones and waits for their {"type": "result"}.
    async fn ask_phone(&self, make: impl FnOnce(u64) -> Value, silent: &str) -> Result<Value, String> {
        let id = self.next_call.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let server = self.server.lock().await;
            let server = server.as_ref().ok_or("The phone connection is off (Settings > Phone).")?;
            if server.connected().is_empty() {
                return Err("No phone is connected; open Cloudy on the phone.".into());
            }
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
            server.broadcast(make(id));
        }
        let answer = tokio::time::timeout(CALL_TIMEOUT, rx).await;
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        match answer {
            Ok(Ok(result)) => result,
            _ => Err(silent.to_string()),
        }
    }

    /// The turn that just started is the phone's when a phone request was about to start.
    fn claim_turn(&self, turn_id: u64) {
        if self.from_phone.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let mut turns = self.phone_turns.lock().unwrap_or_else(|e| e.into_inner());
            turns.push_back(turn_id);
            while turns.len() > 50 {
                turns.pop_front();
            }
        }
    }

    /// Whether a turn came from the phone.
    pub fn is_phone_turn(&self, turn_id: u64) -> bool {
        self.phone_turns.lock().unwrap_or_else(|e| e.into_inner()).contains(&turn_id)
    }

    /// Offers a PC file to the connected phones; the phone then fetches it piece by piece and saves it in its
    /// Downloads.
    pub async fn offer_file(&self, path: &str) -> Result<Value, String> {
        let file = std::path::Path::new(path);
        let meta = std::fs::metadata(file).map_err(|e| format!("Could not open {path}: {e}"))?;
        if meta.is_dir() {
            return Err("That is a folder; send one file at a time.".into());
        }
        let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        // The phone says it took the offer before this reports anything (an older phone app would ignore it).
        let size = meta.len();
        self.ask_phone(
            |id| json!({ "type": "offer", "id": id, "path": path, "name": name, "size": size }),
            "The phone did not take the file (update the Cloudy app on the phone, and keep it open).",
        )
        .await?;
        Ok(json!({ "sending": name, "size": size, "note": "The phone is downloading it into Downloads now." }))
    }

    fn resolve(&self, message: &Value) {
        let Some(id) = message["id"].as_u64() else { return };
        if let Some(tx) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
            let result = if message["ok"].as_bool() == Some(true) { Ok(message["value"].clone()) } else { Err(message["error"].as_str().unwrap_or("The phone could not do it.").to_string()) };
            let _ = tx.send(result);
        }
    }

    /// The assistant gets the phone tools while a phone is connected.
    async fn update_tools(self: &Arc<Self>) {
        // A paired phone keeps its tools even while it is away: a conversation opened before it connects can still
        // use it, and a call while it is away says "open Cloudy on the phone" instead of "I have no access".
        let paired = self.server.lock().await.as_ref().is_some_and(|s| !s.devices().is_empty());
        let tools = if paired { crate::phone_tools::tools(self.clone()) } else { Vec::new() };
        self.app.state::<Brain>().mcp.set_extra(&self.app, tools);
    }

    pub async fn forget(self: &Arc<Self>, key: &str) -> PhoneStatus {
        match self.server.lock().await.as_ref() {
            Some(server) => {
                server.forget(key);
            }
            None => {
                DeviceStore::load(self.devices_path.clone()).remove(key);
            }
        }
        self.update_tools().await;
        self.changed().await
    }
}

/// What phones can do: see the state and history, send a request, stop; use the PC as with a touchpad and keyboard,
/// see its screen, move files both ways, and turn it off (phone_remote.rs).
struct AppHandler {
    app: AppHandle,
    uploads: Arc<crate::phone_remote::Uploads>,
    /// Mouse buttons a phone holds down (a drag), released if it goes away mid-drag.
    pressed: std::sync::Mutex<std::collections::HashSet<(String, String)>>,
    /// A shutdown or restart waiting its delay (cancel aborts it).
    pending_power: std::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl AppHandler {
    fn welcome(&self) -> Value {
        let brain = self.app.state::<Brain>();
        let settings = current(&self.app.state::<SettingsStore>());
        let turns = brain.agent.memory().recent_turns(30).unwrap_or_default();
        let adapters = anytask_control::net::adapters();
        json!({
            "type": "welcome",
            "assistant": settings.assistant_name,
            "busy": brain.agent.is_busy(),
            "turns": turns,
            // Kept by the phone: other ways to reach this PC, and what wakes it (Wake-on-LAN).
            "addresses": anytask_control::net::reachable(&adapters, anytask_remote::lan_address().as_deref()).iter().map(|ip| format!("{ip}:{DEFAULT_PORT}")).collect::<Vec<_>>(),
            "macs": anytask_control::net::wake_addresses(&adapters),
        })
    }
}

#[async_trait::async_trait]
impl Handler for AppHandler {
    async fn connected(&self, device: &Device, out: &Outbox) {
        log(&self.app, format!("{} connected", device.name));
        out.send(self.welcome());
        if let Some(phone) = self.app.try_state::<Arc<Phone>>() {
            phone.changed().await;
            phone.update_tools().await;
        }
    }

    async fn disconnected(&self, device: &Device) {
        log(&self.app, format!("{} disconnected", device.name));
        // The phone may still be connected another way (it tries Wi-Fi and Tailscale at once; an old session can end a
        // minute after a new one began): then its drag and its files belong to that session and stay.
        let still_here = match self.app.try_state::<Arc<Phone>>() {
            Some(phone) => phone.server.lock().await.as_ref().is_some_and(|s| s.connected().contains(&device.key)),
            None => false,
        };
        if still_here {
            if let Some(phone) = self.app.try_state::<Arc<Phone>>() {
                phone.changed().await;
            }
            return;
        }
        // A drag that was going on is ended; half-sent files are removed.
        let held: Vec<String> = {
            let mut pressed = self.pressed.lock().unwrap_or_else(|e| e.into_inner());
            let held = pressed.iter().filter(|(k, _)| *k == device.key).map(|(_, b)| b.clone()).collect();
            pressed.retain(|(k, _)| *k != device.key);
            held
        };
        for button in held {
            let _ = crate::phone_remote::input(&json!({ "op": "button", "button": button, "down": false }));
        }
        self.uploads.forget(&device.key);
        if let Some(phone) = self.app.try_state::<Arc<Phone>>() {
            phone.changed().await;
            phone.update_tools().await;
        }
    }

    async fn message(&self, device: &Device, message: Value, out: &Outbox) {
        match message["type"].as_str().unwrap_or_default() {
            "prompt" => {
                let text = message["text"].as_str().unwrap_or_default().trim().to_string();
                if text.is_empty() {
                    out.send(json!({ "type": "error", "message": "The request is empty." }));
                    return;
                }
                log(&self.app, format!("request from {}: {}", device.name, text.chars().take(80).collect::<String>()));
                let app = self.app.clone();
                let wants_voice = message["voice"].as_bool().unwrap_or(false);
                let out = out.clone();
                if let Some(phone) = self.app.try_state::<Arc<Phone>>() {
                    phone.from_phone.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                // Runs like a typed request; progress reaches the phone as events. The answer is spoken here (the PC
                // has the VPN the voice model needs) and the sound goes to the phone.
                tauri::async_runtime::spawn(async move {
                    let brain = app.state::<Brain>();
                    let settings = current(&app.state::<SettingsStore>());
                    let model = active_generator(&brain, &settings);
                    let identity = Identity { assistant_name: settings.assistant_name, user_name: settings.user_name };
                    let outcome = brain.agent.run(model, &identity, &text).await;
                    if wants_voice && outcome.status == anytask_core::TurnStatus::Answered && !outcome.text.trim().is_empty() {
                        use base64::Engine;
                        match crate::speak::render(&app, &outcome.text).await {
                            Ok(wav) => {
                                out.send(json!({ "type": "voice", "turnId": outcome.turn_id, "wav": base64::engine::general_purpose::STANDARD.encode(wav) }));
                            }
                            Err(e) => {
                                log(&app, format!("voice for the phone failed: {e}"));
                                out.send(json!({ "type": "voice-error", "turnId": outcome.turn_id, "message": e }));
                            }
                        }
                    }
                });
            }
            "stop" => {
                self.app.state::<Brain>().agent.stop();
                self.app.state::<crate::voice::VoiceSlot>().stop();
            }
            "hello" | "history" => {
                out.send(self.welcome());
            }
            "result" => {
                if let Some(phone) = self.app.try_state::<Arc<Phone>>() {
                    phone.resolve(&message);
                }
            }
            "input" => {
                if message["op"] == "button" {
                    let held = (device.key.clone(), message["button"].as_str().unwrap_or("left").to_string());
                    let mut pressed = self.pressed.lock().unwrap_or_else(|e| e.into_inner());
                    if message["down"] == true {
                        pressed.insert(held);
                    } else {
                        pressed.remove(&held);
                    }
                }
                // Clicks wait a little between presses and long text is pasted: off the async workers. The next
                // message is read only after this one ran, so the order (press, move, release) holds.
                let result = tauri::async_runtime::spawn_blocking(move || crate::phone_remote::input(&message)).await.map_err(|e| e.to_string()).and_then(|r| r);
                if let Err(e) = result {
                    out.send(json!({ "type": "error", "about": "input", "message": e }));
                }
            }
            "screen" => {
                let width = message["width"].as_u64();
                let reply = tauri::async_runtime::spawn_blocking(move || crate::phone_remote::screen(width)).await.map_err(|e| e.to_string()).and_then(|r| r);
                out.send(reply.unwrap_or_else(|e| json!({ "type": "error", "about": "screen", "message": e })));
            }
            "files" => {
                let path = message["path"].as_str().map(str::to_string);
                let reply = tauri::async_runtime::spawn_blocking(move || crate::phone_remote::list(path.as_deref())).await.map_err(|e| e.to_string()).and_then(|r| r);
                out.send(reply.unwrap_or_else(|e| json!({ "type": "error", "about": "files", "message": e })));
            }
            "fetch" => {
                let path = message["path"].as_str().unwrap_or_default().to_string();
                let offset = message["offset"].as_u64().unwrap_or(0);
                if offset == 0 {
                    log(&self.app, format!("{} takes {path}", device.name));
                }
                let asked = path.clone();
                let reply = tauri::async_runtime::spawn_blocking(move || crate::phone_remote::piece(&path, offset)).await.map_err(|e| e.to_string()).and_then(|r| r);
                out.send(reply.unwrap_or_else(|e| json!({ "type": "error", "about": "fetch", "path": asked, "message": e })));
            }
            "upload" => {
                let folder = anytask_core::paths::UserFolders::detect().downloads.join("AnyTask");
                let (uploads, key) = (self.uploads.clone(), device.key.clone());
                let reply = tauri::async_runtime::spawn_blocking(move || uploads.piece(&key, &message, &folder))
                    .await
                    .unwrap_or_else(|e| json!({ "type": "uploaded", "error": e.to_string() }));
                if reply["done"] == true {
                    log(&self.app, format!("{} sent {}", device.name, reply["path"].as_str().unwrap_or_default()));
                }
                out.send(reply);
            }
            "crash" => {
                // Why the phone app crashed last time: kept in the log so it can be fixed.
                log(&self.app, format!("{}'s app crashed last time: {}", device.name, message["text"].as_str().unwrap_or_default().chars().take(4000).collect::<String>()));
            }
            "power" => {
                use crate::phone_remote::{POWER_DELAY, power};
                let action = message["action"].as_str().unwrap_or_default().to_string();
                log(&self.app, format!("{} asks to {action} the PC", device.name));
                let reply = |ok: bool, text: String| json!({ "type": "power", "action": action, "ok": ok, "message": text });
                match action.as_str() {
                    "shutdown" | "restart" => {
                        // Waits here, cancellable from the phone, then asks Windows without a timeout of its own, so apps
                        // with unsaved work can still ask to save instead of being forced closed.
                        let (due, app) = (action.clone(), self.app.clone());
                        let task = tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(POWER_DELAY)).await;
                            let label = due.clone();
                            let result = tauri::async_runtime::spawn_blocking(move || power(&due)).await.map_err(|e| e.to_string()).and_then(|r| r);
                            if let Err(e) = result {
                                log(&app, format!("{label} asked from the phone failed: {e}"));
                            }
                        });
                        if let Some(old) = self.pending_power.lock().unwrap_or_else(|e| e.into_inner()).replace(task) {
                            old.abort();
                        }
                        let what = if action == "shutdown" { "shuts down" } else { "restarts" };
                        out.send(reply(true, format!("The PC {what} in {POWER_DELAY} s; cancel is still possible. Apps with unsaved work may ask to save first.")));
                        // Whoever sits at the PC sees it coming too (Windows showed nothing: the wait is the app's).
                        notice(
                            &self.app,
                            anytask_core::lang::tr(
                                if action == "shutdown" { "The phone asked to shut down this PC in 20 s. Save your work." } else { "The phone asked to restart this PC in 20 s. Save your work." },
                                if action == "shutdown" { "گوشی خواست این کامپیوتر ۲۰ ثانیه دیگر خاموش شود. کارهایت را ذخیره کن." } else { "گوشی خواست این کامپیوتر ۲۰ ثانیه دیگر ری‌استارت شود. کارهایت را ذخیره کن." },
                            ),
                        );
                    }
                    "cancel" => {
                        // A task that already ran has asked Windows: only `shutdown /a` below can still stop that.
                        let waiting = self.pending_power.lock().unwrap_or_else(|e| e.into_inner()).take().is_some_and(|t| {
                            let pending = !t.inner().is_finished();
                            t.abort();
                            pending
                        });
                        // A shutdown started some other way is cancelled too.
                        let _ = tauri::async_runtime::spawn_blocking(|| power("cancel")).await;
                        if waiting {
                            notice(&self.app, anytask_core::lang::tr("Shutting down was cancelled from the phone.", "خاموش شدن از گوشی لغو شد."));
                        }
                        out.send(reply(true, if waiting { "Cancelled.".into() } else { "Nothing was waiting.".into() }));
                    }
                    "sleep" => {
                        // The answer goes out first: the call returns only after the PC wakes again.
                        out.send(reply(true, "The PC goes to sleep.".into()));
                        tauri::async_runtime::spawn(async {
                            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                            let _ = tauri::async_runtime::spawn_blocking(|| power("sleep")).await;
                        });
                    }
                    _ => {
                        let asked = action.clone();
                        let result = tauri::async_runtime::spawn_blocking(move || power(&asked)).await.map_err(|e| e.to_string()).and_then(|r| r);
                        out.send(match result {
                            Ok(text) => reply(true, text),
                            Err(e) => reply(false, e),
                        });
                    }
                }
            }
            other => {
                out.send(json!({ "type": "error", "message": format!("Unknown request type {other}.") }));
            }
        }
    }
}

/// A line on the island's answer card, for things the phone does to this PC that someone at the PC should see.
fn notice(app: &AppHandle, text: &str) {
    if let Some(brain) = app.try_state::<Brain>() {
        brain.agent.events().emit(CoreEvent::State(anytask_core::StateUpdate::new(anytask_core::AssistantState::Answer).with_detail(text.to_string())));
    }
}

fn log(app: &AppHandle, text: String) {
    if let Some(brain) = app.try_state::<Brain>() {
        brain.agent.events().emit(CoreEvent::Log { turn_id: None, source: "phone".into(), text });
    }
}
