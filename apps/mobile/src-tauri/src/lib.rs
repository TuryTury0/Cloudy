//! Cloudy on the phone: pairs with the PC once (the QR code, or a 6-digit code on the same network), then keeps an
//! encrypted connection open, reconnecting by itself and finding the PC again on the network when its address
//! changed, and passes messages between the PC and the screen. Besides the assistant: the PC as a touchpad and
//! keyboard, its screen, files both ways, power and Wake-on-LAN.
//!
//! Saved in the app's private folder: the PC (name, addresses, public key, wake addresses) and this phone's key pair.

use anytask_remote::{Connection, DEFAULT_PORT, Keys, Outbox, PairingInfo, connect, discovery};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// One piece of a file on the wire (before base64); the PC uses the same size.
const PIECE: usize = 384 * 1024;
/// How long to ask the network for the PC when none of its saved addresses answers.
const FIND_WAIT: Duration = Duration::from_millis(1500);
/// Saved addresses kept per PC (newest first).
const MAX_ADDRESSES: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedPc {
    name: String,
    addresses: Vec<String>,
    key: String,
    /// Hardware addresses of the PC's wired and Wi-Fi adapters, for Wake-on-LAN.
    #[serde(default)]
    macs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkStatus {
    pub paired: bool,
    pub connected: bool,
    pub pc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The PC can be woken from here (its wake addresses are known).
    pub can_wake: bool,
}

/// A request waiting for the PC's answer: which message answers it, and where to hand it.
struct Waiter {
    id: u64,
    matches: Box<dyn Fn(&Value) -> bool + Send>,
    reply: tokio::sync::oneshot::Sender<Value>,
}

#[derive(Default)]
struct Link {
    outbox: Mutex<Option<Outbox>>,
    status: Mutex<LinkStatus>,
    /// Bumped on pair/unpair so an old reconnect loop stops.
    generation: Mutex<u64>,
    waiters: Mutex<Vec<Waiter>>,
    next_waiter: std::sync::atomic::AtomicU64,
    /// The last crash's reason waiting to be sent to the PC's log (once).
    crash: Mutex<Option<String>>,
    /// The last crash's reason for the screen.
    last_crash: Mutex<Option<String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn folder(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn load_pc(app: &AppHandle) -> Option<SavedPc> {
    let text = std::fs::read_to_string(folder(app).ok()?.join("pc.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_pc(app: &AppHandle, pc: &SavedPc) -> Result<(), String> {
    std::fs::write(folder(app)?.join("pc.json"), serde_json::to_string(pc).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Puts addresses that just worked or that the PC reported first, keeping older ones after them.
fn merge_addresses(old: &[String], new: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in new.iter().chain(old.iter()) {
        if !a.is_empty() && !out.contains(a) {
            out.push(a.clone());
        }
    }
    out.truncate(MAX_ADDRESSES);
    out
}

/// Saves what the PC told about itself (other addresses, wake addresses) or where it was found.
fn remember(app: &AppHandle, addresses: &[String], macs: Option<Vec<String>>) {
    if let Some(mut pc) = load_pc(app) {
        pc.addresses = merge_addresses(&pc.addresses, addresses);
        if let Some(macs) = macs.filter(|m| !m.is_empty()) {
            pc.macs = macs;
        }
        let can_wake = !pc.macs.is_empty();
        let _ = save_pc(app, &pc);
        set_status(app, |s| s.can_wake = can_wake);
    }
}

/// This phone's key pair, made on first use.
fn keys(app: &AppHandle) -> Result<Keys, String> {
    let path = folder(app)?.join("device.key");
    if let Some(keys) = std::fs::read_to_string(&path).ok().and_then(|t| Keys::from_secret(&t)) {
        return Ok(keys);
    }
    let keys = Keys::generate();
    std::fs::write(&path, keys.to_secret()).map_err(|e| e.to_string())?;
    Ok(keys)
}

fn set_status(app: &AppHandle, change: impl FnOnce(&mut LinkStatus)) {
    let link = app.state::<Link>();
    let status = {
        let mut status = lock(&link.status);
        change(&mut status);
        status.clone()
    };
    let _ = app.emit("link", status);
}

fn key_bytes(b64: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD.decode(b64).map_err(|_| "The code is damaged.".to_string())
}

/// Hands a message to the request waiting for it; false when none is.
fn deliver(link: &Link, message: &Value) -> bool {
    let mut waiters = lock(&link.waiters);
    let Some(i) = waiters.iter().position(|w| (w.matches)(message)) else { return false };
    let waiter = waiters.remove(i);
    let _ = waiter.reply.send(message.clone());
    true
}

/// Sends `message` and waits for the PC's answer that `matches` picks; an {"type": "error"} answer is the error.
async fn request(app: &AppHandle, message: Value, matches: impl Fn(&Value) -> bool + Send + 'static, wait: Duration) -> Result<Value, String> {
    let link = app.state::<Link>();
    let (reply, answer) = tokio::sync::oneshot::channel();
    let id = link.next_waiter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    lock(&link.waiters).push(Waiter { id, matches: Box::new(matches), reply });
    let sent = lock(&link.outbox).as_ref().is_some_and(|o| o.send(message));
    let result = if sent {
        match tokio::time::timeout(wait, answer).await {
            Ok(Ok(value)) if value["type"] == "error" => Err(value["message"].as_str().unwrap_or("The PC could not do it.").to_string()),
            Ok(Ok(value)) => Ok(value),
            _ => Err("The PC did not answer.".to_string()),
        }
    } else {
        Err("Not connected to the PC.".to_string())
    };
    lock(&link.waiters).retain(|w| w.id != id);
    result
}

/// Keeps the connection up: uses `first` if given, then reconnects with growing pauses.
fn run_link(app: AppHandle, pc: SavedPc, mut first: Option<Connection>, generation: u64) {
    tauri::async_runtime::spawn(async move {
        let link = app.state::<Link>();
        let current = || *lock(&link.generation) == generation;
        let mut pause = Duration::from_secs(1);
        while current() {
            let connection = match first.take() {
                Some(c) => Ok(c),
                // The saved file has the newest addresses (the PC reports them, discovery finds them).
                None => try_connect(&app, &load_pc(&app).unwrap_or_else(|| pc.clone()), None).await,
            };
            match connection {
                Ok(mut c) => {
                    pause = Duration::from_secs(1);
                    *lock(&link.outbox) = Some(c.outbox.clone());
                    set_status(&app, |s| {
                        s.connected = true;
                        s.pc = if c.pc.is_empty() { pc.name.clone() } else { c.pc.clone() };
                        s.error = None;
                    });
                    // The last crash's reason goes to the PC's log once, so it can be fixed (the screen keeps its own copy).
                    if let Some(text) = lock(&link.crash).take() {
                        c.outbox.send(json!({ "type": "crash", "text": text }));
                    }
                    while let Some(message) = c.inbox.recv().await {
                        if !current() {
                            return;
                        }
                        if deliver(&link, &message) {
                            continue;
                        }
                        match message["type"].as_str().unwrap_or_default() {
                            "call" => {
                                let (app, out) = (app.clone(), c.outbox.clone());
                                tauri::async_runtime::spawn(async move { answer_call(&app, &out, &message).await });
                                continue;
                            }
                            "welcome" => {
                                let addresses: Vec<String> = message["addresses"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                                let macs = message["macs"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect());
                                remember(&app, &addresses, macs);
                            }
                            "voice" => {
                                // The answer spoken on the PC: played with the phone's media player.
                                let (app, wav) = (app.clone(), message["wav"].as_str().unwrap_or_default().to_string());
                                tauri::async_runtime::spawn(async move {
                                    if let Err(e) = play_voice(&app, &wav).await {
                                        let _ = app.emit("remote", json!({ "type": "voice-error", "message": e }));
                                    }
                                });
                                continue;
                            }
                            "offer" => {
                                // The PC sent a file: the phone says it takes it, then fetches it into Downloads.
                                c.outbox.send(json!({ "type": "result", "id": message["id"], "ok": true, "value": { "accepted": true } }));
                                let (app, path, name, size) = (app.clone(), message["path"].as_str().unwrap_or_default().to_string(), message["name"].as_str().unwrap_or("file").to_string(), message["size"].as_u64().unwrap_or(0));
                                tauri::async_runtime::spawn(async move { download_file(&app, path, name, size).await });
                            }
                            _ => {}
                        }
                        let _ = app.emit("remote", message);
                    }
                    lock(&link.outbox).take();
                    set_status(&app, |s| s.connected = false);
                }
                Err(error) => set_status(&app, |s| {
                    s.connected = false;
                    s.error = Some(error);
                }),
            }
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(Duration::from_secs(30));
        }
    });
}

/// Connects to whichever of `addresses` answers first (all are tried at once: an address that does not answer
/// would otherwise cost its whole timeout before the next one is tried).
async fn connect_any(addresses: &[String], pc_key: &[u8], keys: &Keys, token: Option<&str>) -> Result<(Connection, String), String> {
    let mut tries = tokio::task::JoinSet::new();
    for address in addresses {
        let (address, pc_key, keys, token) = (address.clone(), pc_key.to_vec(), keys.clone(), token.map(str::to_string));
        tries.spawn(async move { connect(&address, &pc_key, &keys, &device_name(), token.as_deref()).await.map(|c| (c, address)) });
    }
    let mut last = "The PC is not reachable.".to_string();
    while let Some(done) = tries.join_next().await {
        match done {
            Ok(Ok(found)) => {
                tries.abort_all();
                return Ok(found);
            }
            Ok(Err(e)) => last = e,
            Err(_) => {}
        }
    }
    Err(last)
}

/// Tries the saved addresses; when none answers (the PC got another address), asks the network for the PC with
/// this key and remembers where it answered.
async fn try_connect(app: &AppHandle, pc: &SavedPc, token: Option<&str>) -> Result<Connection, String> {
    let keys = keys(app)?;
    let pc_key = key_bytes(&pc.key)?;
    let saved = connect_any(&pc.addresses, &pc_key, &keys, token).await;
    let last = match saved {
        Ok((c, address)) => {
            remember(app, std::slice::from_ref(&address), None);
            return Ok(c);
        }
        Err(e) => e,
    };
    let found: Vec<String> = discovery::find(&pc_key, DEFAULT_PORT, FIND_WAIT).await.into_iter().filter(|a| !pc.addresses.contains(a)).collect();
    if found.is_empty() {
        return Err(last);
    }
    let (c, address) = connect_any(&found, &pc_key, &keys, token).await?;
    remember(app, std::slice::from_ref(&address), None);
    Ok(c)
}

fn device_name() -> String {
    if cfg!(target_os = "android") { "Android phone".into() } else if cfg!(target_os = "ios") { "iPhone".into() } else { "Test device".into() }
}

/// What the PC may ask the phone: reading, opening apps and links, ringing it to find it, alarms and timers, and
/// opening the dialer, a new SMS or the maps app filled in (the user presses call or send: nothing goes out by itself).
const PC_CALLS: &[&str] = &[
    "status", "notifications", "messages", "apps", "openApp", "openUrl", "ring", "stopRing", "setAlarm", "setTimer", "dial", "composeSms", "navigate",
];

/// Runs the PC's request on the phone and sends the answer back; the screen is told what was read.
async fn answer_call(app: &AppHandle, out: &Outbox, message: &Value) {
    use tauri_plugin_anytask_phone::PhoneExt;
    let name = message["name"].as_str().unwrap_or_default();
    let result = if PC_CALLS.contains(&name) { app.phone().call(name, message["args"].clone()).await } else { Err(format!("{name} is not allowed from the PC.")) };
    let reply = match result {
        Ok(value) => json!({ "type": "result", "id": message["id"], "ok": true, "value": value }),
        Err(error) => json!({ "type": "result", "id": message["id"], "ok": false, "error": error }),
    };
    out.send(reply);
    let _ = app.emit("phone-used", name);
}

/// Battery, device and which access the phone allows (for the screen).
#[tauri::command]
async fn access(app: AppHandle) -> Result<Value, String> {
    use tauri_plugin_anytask_phone::PhoneExt;
    app.phone().call("status", json!({})).await
}

/// Asks for an access: "notifications" opens Android's notification access screen, "sms" asks for SMS.
#[tauri::command]
async fn grant(kind: String, app: AppHandle) -> Result<Value, String> {
    use tauri_plugin_anytask_phone::PhoneExt;
    match kind.as_str() {
        "notifications" => app.phone().call("openNotificationAccess", json!({})).await,
        "sms" => app.phone().call("requestPermissions", json!({})).await,
        _ => Err("Unknown access.".into()),
    }
}

#[tauri::command]
fn status(link: State<'_, Link>) -> LinkStatus {
    lock(&link.status).clone()
}

/// Starts using a PC the phone just paired with.
fn start_with(app: &AppHandle, pc: SavedPc, connection: Connection) -> Result<LinkStatus, String> {
    save_pc(app, &pc)?;
    let generation = {
        let link = app.state::<Link>();
        let mut g = lock(&link.generation);
        *g += 1;
        *g
    };
    set_status(app, |s| {
        s.paired = true;
        s.pc = pc.name.clone();
        s.can_wake = false;
    });
    run_link(app.clone(), pc, Some(connection), generation);
    Ok(lock(&app.state::<Link>().status).clone())
}

/// Pairs with the PC from the QR code text.
#[tauri::command]
async fn pair(code: String, app: AppHandle) -> Result<LinkStatus, String> {
    let info = PairingInfo::parse(code.trim()).ok_or("This is not a Cloudy pairing code.")?;
    let pc = SavedPc { name: info.n.clone(), addresses: info.h.iter().map(|h| format!("{h}:{}", info.p)).collect(), key: info.k.clone(), macs: Vec::new() };
    let connection = try_connect(&app, &pc, Some(&info.t)).await?;
    start_with(&app, pc, connection)
}

/// Computers on this network waiting to be paired with a code, one per key, with the fingerprint the PC shows next
/// to its code. The screen asks the user to compare it before the code is sent: the code goes only to the computer
/// the user confirmed, and more than one answer means "use the QR code" (someone else may be answering).
#[tauri::command]
async fn find_code_pc() -> Result<Vec<Value>, String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for pc in discovery::find_open(DEFAULT_PORT, Duration::from_secs(2)).await {
        let Some(key) = pc.key.clone() else { continue };
        if seen.contains(&key) {
            continue;
        }
        let Ok(bytes) = key_bytes(&key) else { continue };
        seen.push(key.clone());
        out.push(json!({ "name": pc.name.unwrap_or_else(|| "PC".into()), "address": pc.address, "key": key, "fingerprint": anytask_remote::fingerprint(&bytes) }));
    }
    if out.is_empty() {
        return Err("No computer on this network is waiting for a code. On the computer: Cloudy Settings > Phone > Add a phone.".into());
    }
    Ok(out)
}

/// Pairs with the 6-digit code, with the computer the user confirmed (from find_code_pc).
#[tauri::command]
async fn pair_code(code: String, address: String, key: String, name: Option<String>, app: AppHandle) -> Result<LinkStatus, String> {
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    if code.len() != 6 {
        return Err("The code has 6 digits.".into());
    }
    let keys = keys(&app)?;
    match connect(&address, &key_bytes(&key)?, &keys, &device_name(), Some(&code)).await {
        Ok(connection) => start_with(&app, SavedPc { name: name.unwrap_or_else(|| "PC".into()), addresses: vec![address], key, macs: Vec::new() }, connection),
        Err(e) if e.contains("not paired") => Err("The code is not right (or it expired); check it on the computer.".into()),
        Err(e) => Err(e),
    }
}

/// Sends a message to the PC ({"type": "prompt", "text": ...}, {"type": "stop"}, {"type": "input", ...}).
#[tauri::command]
fn send(message: Value, link: State<'_, Link>) -> Result<(), String> {
    match lock(&link.outbox).as_ref() {
        Some(outbox) if outbox.send(message) => Ok(()),
        _ => Err("Not connected to the PC.".into()),
    }
}

/// Forgets the PC (pairing is needed again).
#[tauri::command]
fn unpair(app: AppHandle) -> Result<LinkStatus, String> {
    let link = app.state::<Link>();
    *lock(&link.generation) += 1;
    lock(&link.outbox).take();
    let _ = std::fs::remove_file(folder(&app)?.join("pc.json"));
    set_status(&app, |s| *s = LinkStatus::default());
    let _ = app.emit("remote", json!({ "type": "unpaired" }));
    Ok(LinkStatus::default())
}

/// The PC's screen as a JPEG (base64) with the pointer's place.
#[tauri::command]
async fn screen(width: Option<u32>, app: AppHandle) -> Result<Value, String> {
    request(&app, json!({ "type": "screen", "width": width }), |m| m["type"] == "screen" || (m["type"] == "error" && m["about"] == "screen"), Duration::from_secs(10)).await
}

/// A folder on the PC (no path: the starting places).
#[tauri::command]
async fn files(path: Option<String>, app: AppHandle) -> Result<Value, String> {
    request(&app, json!({ "type": "files", "path": path }), |m| m["type"] == "files" || (m["type"] == "error" && m["about"] == "files"), Duration::from_secs(15)).await
}

/// Shuts down, restarts, sleeps or locks the PC, or cancels a shutdown.
#[tauri::command]
async fn power(action: String, app: AppHandle) -> Result<Value, String> {
    let reply = request(&app, json!({ "type": "power", "action": action }), |m| m["type"] == "power", Duration::from_secs(15)).await?;
    if reply["ok"] == true { Ok(reply) } else { Err(reply["message"].as_str().unwrap_or("The PC refused.").to_string()) }
}

/// Wakes the PC with Wake-on-LAN (works on the same network, when the PC allows it in its settings).
#[tauri::command]
async fn wake(app: AppHandle) -> Result<Value, String> {
    let pc = load_pc(&app).ok_or("No computer is paired.")?;
    if pc.macs.is_empty() {
        return Err("Connect to the computer once first, so the phone learns how to wake it.".into());
    }
    let sent = discovery::wake(&pc.macs, &pc.addresses).await;
    Ok(json!({ "sent": sent }))
}

fn emit_transfer(app: &AppHandle, value: Value) {
    let _ = app.emit("transfer", value);
}

fn transfer_id() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
}

/// Lets the user pick a file and sends it to the PC (it lands in the PC's Downloads). Progress arrives as
/// "transfer" events.
#[tauri::command]
async fn send_file(app: AppHandle) -> Result<Value, String> {
    use tauri_plugin_anytask_phone::PhoneExt;
    if lock(&app.state::<Link>().outbox).is_none() {
        return Err("Not connected to the PC.".into());
    }
    let picked = app.phone().call("pickFile", json!({})).await?;
    let path = picked["path"].as_str().unwrap_or_default().to_string();
    let name = picked["name"].as_str().unwrap_or("file").to_string();
    let size = picked["size"].as_u64().unwrap_or(0);
    let id = transfer_id();
    let info = json!({ "id": id, "direction": "up", "name": name, "size": size });
    let task = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = upload_file(&task, &id, &path, &name, size).await;
        let _ = std::fs::remove_file(&path);
        match result {
            Ok(saved) => emit_transfer(&task, json!({ "id": id, "direction": "up", "name": name, "size": size, "done": size, "finished": true, "path": saved })),
            Err(e) => emit_transfer(&task, json!({ "id": id, "direction": "up", "name": name, "size": size, "error": e })),
        }
    });
    Ok(info)
}

async fn upload_file(app: &AppHandle, id: &str, path: &str, name: &str, size: u64) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("Could not read the file: {e}"))?;
    let mut offset: u64 = 0;
    loop {
        let mut buf = vec![0u8; PIECE];
        let mut filled = 0;
        while filled < PIECE {
            let n = file.read(&mut buf[filled..]).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        buf.truncate(filled);
        let last = offset + filled as u64 >= size || filled < PIECE;
        let wanted = id.to_string();
        let message = json!({ "type": "upload", "id": id, "name": name, "size": size, "offset": offset, "data": base64::engine::general_purpose::STANDARD.encode(&buf), "last": last });
        let reply = request(app, message, move |m| m["type"] == "uploaded" && m["id"] == wanted.as_str(), Duration::from_secs(60)).await?;
        if let Some(e) = reply["error"].as_str() {
            return Err(e.to_string());
        }
        offset += filled as u64;
        emit_transfer(app, json!({ "id": id, "direction": "up", "name": name, "size": size, "done": offset }));
        if last {
            return Ok(reply["path"].as_str().unwrap_or_default().to_string());
        }
    }
}

/// Saves the PC's spoken answer (a WAV in base64) in the app's cache and plays it.
async fn play_voice(app: &AppHandle, wav: &str) -> Result<(), String> {
    use tauri_plugin_anytask_phone::PhoneExt;
    let bytes = base64::engine::general_purpose::STANDARD.decode(wav).map_err(|_| "A damaged sound.".to_string())?;
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("voice");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.wav", transfer_id()));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    app.phone().call("playAudio", json!({ "path": path.to_string_lossy(), "name": "" })).await.map(|_| ())
}

/// Fetches a PC file piece by piece and saves it in the phone's Downloads.
#[tauri::command]
async fn download(path: String, name: String, size: u64, app: AppHandle) -> Result<(), String> {
    if lock(&app.state::<Link>().outbox).is_none() {
        return Err("Not connected to the PC.".into());
    }
    tauri::async_runtime::spawn(async move { download_file(&app, path, name, size).await });
    Ok(())
}

async fn download_file(app: &AppHandle, path: String, name: String, size: u64) {
    let id = transfer_id();
    let result = fetch_and_save(app, &id, &path, &name, size).await;
    // Whatever went wrong, no half file stays in the phone's storage.
    if let Ok(cache) = app.path().app_cache_dir() {
        let _ = std::fs::remove_file(cache.join("download").join(format!("{id}.part")));
    }
    match result {
        Ok(uri) => emit_transfer(app, json!({ "id": id, "direction": "down", "name": name, "size": size, "done": size, "finished": true, "uri": uri })),
        Err(e) => emit_transfer(app, json!({ "id": id, "direction": "down", "name": name, "size": size, "error": e })),
    }
}

async fn fetch_and_save(app: &AppHandle, id: &str, path: &str, name: &str, size: u64) -> Result<String, String> {
    use tauri_plugin_anytask_phone::PhoneExt;
    let cache = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("download");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let local = cache.join(format!("{id}.part"));
    let mut file = std::fs::File::create(&local).map_err(|e| e.to_string())?;
    let mut offset: u64 = 0;
    emit_transfer(app, json!({ "id": id, "direction": "down", "name": name, "size": size, "done": 0 }));
    loop {
        let (p, o) = (path.to_string(), offset);
        let reply = request(
            app,
            json!({ "type": "fetch", "path": path, "offset": offset }),
            move |m| (m["type"] == "chunk" && m["path"] == p.as_str() && m["offset"].as_u64() == Some(o)) || (m["type"] == "error" && m["about"] == "fetch" && m["path"] == p.as_str()),
            Duration::from_secs(60),
        )
        .await
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&local);
        })?;
        let data = base64::engine::general_purpose::STANDARD.decode(reply["data"].as_str().unwrap_or_default()).map_err(|_| "A damaged piece.".to_string())?;
        file.write_all(&data).map_err(|e| format!("The phone could not store it: {e}"))?;
        offset += data.len() as u64;
        emit_transfer(app, json!({ "id": id, "direction": "down", "name": name, "size": reply["size"].as_u64().unwrap_or(size), "done": offset }));
        if reply["last"] == true {
            break;
        }
        if data.is_empty() {
            return Err("The file stopped short.".into());
        }
    }
    drop(file);
    let saved = app.phone().call("saveFile", json!({ "path": local.to_string_lossy(), "name": name })).await;
    let _ = std::fs::remove_file(&local);
    Ok(saved?["uri"].as_str().unwrap_or_default().to_string())
}

/// Listens once with the phone's speech recognition ("fa-IR" by default, "en-US" for English) and returns the words.
#[tauri::command]
async fn listen(lang: Option<String>, app: AppHandle) -> Result<String, String> {
    use tauri_plugin_anytask_phone::PhoneExt;
    let heard = app.phone().call("listen", json!({ "lang": lang.unwrap_or_else(|| "fa-IR".into()) })).await?;
    Ok(heard["text"].as_str().unwrap_or_default().to_string())
}

/// Reads a text aloud with the phone's own voice (the assistant's answers, when the user turned it on).
#[tauri::command]
async fn speak(text: String, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_anytask_phone::PhoneExt;
    app.phone().call(if text.trim().is_empty() { "stopSpeaking" } else { "speak" }, json!({ "text": text })).await.map(|_| ())
}

/// Opens a file saved from the PC.
#[tauri::command]
async fn open_file(uri: String, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_anytask_phone::PhoneExt;
    app.phone().call("openFile", json!({ "uri": uri })).await.map(|_| ())
}

/// Opens a new phone app's download from the releases on GitHub (nothing else: the address is checked).
#[tauri::command]
async fn open_release(url: String, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_anytask_phone::PhoneExt;
    if !url.starts_with("https://github.com/TuryTury0/AnyTask/releases/download/") {
        return Err("This is not a Cloudy release.".into());
    }
    app.phone().call("openUrl", json!({ "url": url })).await.map(|_| ())
}

/// Why the app crashed last time, if it did (read at start; also given to the screen by the "crash" event).
#[tauri::command]
fn last_crash(link: State<'_, Link>) -> Option<String> {
    lock(&link.last_crash).clone()
}

/// Where a Rust panic's reason is written; read at the next start.
static PANIC_FILE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn record_panics(app: &AppHandle) {
    if let Ok(dir) = folder(app) {
        let _ = PANIC_FILE.set(dir.join("rust-crash.txt"));
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = PANIC_FILE.get() {
            // A panic inside a background task ends that task only, not the app: said so, to read it right.
            let thread = std::thread::current().name().unwrap_or("main").to_string();
            let note = if thread.contains("tokio") || thread.contains("async") { " (in a background task; the app may have kept running)" } else { "" };
            let _ = std::fs::write(path, format!("Rust panic on thread {thread}{note}: {info}"));
        }
        previous(info);
    }));
}

/// Reads (and removes) the reason of the last crash, from Rust or from Android.
async fn read_crash(app: &AppHandle) {
    use tauri_plugin_anytask_phone::PhoneExt;
    let mut reasons = Vec::new();
    if let Some(path) = PANIC_FILE.get()
        && let Ok(text) = std::fs::read_to_string(path)
    {
        let _ = std::fs::remove_file(path);
        reasons.push(text);
    }
    if let Ok(value) = app.phone().call("lastCrash", json!({})).await
        && let Some(text) = value["text"].as_str().filter(|t| !t.trim().is_empty())
    {
        reasons.push(text.to_string());
    }
    if !reasons.is_empty() {
        let text: String = reasons.join("\n---\n").chars().take(8000).collect();
        *lock(&app.state::<Link>().last_crash) = Some(text.clone());
        let _ = app.emit("crash", text.clone());
        let sent = lock(&app.state::<Link>().outbox).as_ref().is_some_and(|out| out.send(json!({ "type": "crash", "text": text })));
        // Not connected yet: sent once on the next connection.
        if !sent {
            *lock(&app.state::<Link>().crash) = Some(text);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_anytask_phone::init());
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    builder
        .manage(Link::default())
        .invoke_handler(tauri::generate_handler![
            status, pair, find_code_pc, pair_code, send, unpair, access, grant, screen, files, power, wake, send_file, download, open_file, open_release, last_crash, listen, speak
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            record_panics(&handle);
            let reader = handle.clone();
            tauri::async_runtime::spawn(async move { read_crash(&reader).await });
            if let Some(pc) = load_pc(&handle) {
                let can_wake = !pc.macs.is_empty();
                set_status(&handle, |s| {
                    s.paired = true;
                    s.pc = pc.name.clone();
                    s.can_wake = can_wake;
                });
                run_link(handle, pc, None, 0);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Cloudy mobile");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_that_worked_go_first_and_the_list_stays_short() {
        let old: Vec<String> = (1..=8).map(|i| format!("192.168.1.{i}:47321")).collect();
        let merged = merge_addresses(&old, &["10.221.116.223:47321".into(), "192.168.1.3:47321".into()]);
        assert_eq!(merged[0], "10.221.116.223:47321");
        assert_eq!(merged[1], "192.168.1.3:47321");
        assert_eq!(merged.len(), MAX_ADDRESSES);
        assert_eq!(merged.iter().filter(|a| *a == "192.168.1.3:47321").count(), 1);
    }
}