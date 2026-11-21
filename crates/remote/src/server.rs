//! The PC side: accepts phones, checks they are paired (or carry the current pairing token), then passes their
//! messages to the app and sends them whatever the app broadcasts.

use crate::channel::{Assembler, HANDSHAKE_BUF, Noise, next_binary, next_message, write_loop};
use crate::devices::{Device, DeviceStore};
use crate::{Keys, NOISE, PairingInfo, b64};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as Ws;

/// How long a pairing QR code stays valid.
const PAIRING_TTL: Duration = Duration::from_secs(600);
/// Wrong pairing codes after which the pairing window closes (a 6-digit code cannot be guessed in 5 tries).
const MAX_WRONG_CODES: u32 = 5;

/// An open pairing window: the QR code's token and the short code shown for typing, used once.
struct Pairing {
    token: String,
    code: String,
    since: Instant,
    wrong: u32,
}

impl Pairing {
    /// The QR code's token: until the window times out. Wrong guesses never close it (a stranger on the network
    /// could otherwise keep the owner from pairing).
    fn token_ok(&self) -> bool {
        self.since.elapsed() < PAIRING_TTL
    }

    /// The typed code: until it times out or was guessed wrong MAX_WRONG_CODES times.
    fn code_ok(&self) -> bool {
        self.token_ok() && self.wrong < MAX_WRONG_CODES
    }
}
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Messages to one phone.
#[derive(Clone)]
pub struct Outbox(pub(crate) mpsc::UnboundedSender<Value>);

impl Outbox {
    pub fn send(&self, message: Value) -> bool {
        self.0.send(message).is_ok()
    }
}

/// What the app does with phones.
#[async_trait::async_trait]
pub trait Handler: Send + Sync {
    /// A phone connected (send it the current state).
    async fn connected(&self, device: &Device, out: &Outbox);
    async fn message(&self, device: &Device, message: Value, out: &Outbox);
    /// The phone's connection ended.
    async fn disconnected(&self, _device: &Device) {}
}

struct Session {
    id: u64,
    key: String,
    out: Outbox,
    /// Dropping the session (forget, stop) ends its connection at once.
    _kick: oneshot::Sender<()>,
}

struct Inner {
    keys: Keys,
    name: String,
    devices: Mutex<DeviceStore>,
    pairing: Mutex<Option<Pairing>>,
    /// The port phones connect to (told to phones that ask on the network).
    port: std::sync::atomic::AtomicU16,
    sessions: Mutex<Vec<Session>>,
    handler: Arc<dyn Handler>,
    next: AtomicU64,
}

pub struct RemoteServer {
    inner: Arc<Inner>,
    port: u16,
    accept: tokio::task::JoinHandle<()>,
    discovery: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl RemoteServer {
    /// Listens on `bind` ("0.0.0.0:47321" for the network, "127.0.0.1:0" in tests). `name` is the PC's name.
    pub async fn start(bind: &str, name: &str, keys: Keys, devices: DeviceStore, handler: Arc<dyn Handler>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(bind).await?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            keys,
            name: name.to_string(),
            devices: Mutex::new(devices),
            pairing: Mutex::new(None),
            port: std::sync::atomic::AtomicU16::new(port),
            sessions: Mutex::new(Vec::new()),
            handler,
            next: AtomicU64::new(0),
        });
        let accepting = inner.clone();
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let inner = accepting.clone();
                tokio::spawn(async move {
                    let _ = serve(inner, stream).await;
                });
            }
        });
        Ok(Self { inner, port, accept, discovery: Mutex::new(None) })
    }

    /// Answers phones looking for this PC on the network (UDP on `bind`, "0.0.0.0:47321" for the network): a phone
    /// whose saved address went stale finds the PC again, and a phone without a camera finds a PC waiting for a code.
    /// Returns the UDP port.
    pub async fn start_discovery(&self, bind: &str) -> std::io::Result<u16> {
        let socket = tokio::net::UdpSocket::bind(bind).await?;
        let port = socket.local_addr()?.port();
        let inner = self.inner.clone();
        let task = tokio::spawn(crate::discovery::respond(socket, move || {
            // Answered while the window is open, even after wrong codes closed the code: a stranger who guessed five
            // times must not leave the phone hearing only the stranger's own answer.
            let open = inner.pairing.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(Pairing::token_ok);
            crate::discovery::Answer {
                tag: crate::discovery::tag(&inner.keys.public),
                port: inner.port.load(Ordering::Relaxed),
                open: open.then(|| (inner.keys.public_b64(), inner.name.clone())),
            }
        }));
        if let Some(old) = self.discovery.lock().unwrap_or_else(|e| e.into_inner()).replace(task) {
            old.abort();
        }
        Ok(port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// A fresh one-time token for the QR code and a 6-digit code for typing (replaces any earlier ones).
    pub fn new_pairing(&self, hosts: Vec<String>) -> PairingInfo {
        let random = Keys::generate().public;
        let token = b64(&random)[..22].replace(['+', '/'], "x");
        let code = format!("{:06}", u32::from_le_bytes([random[24], random[25], random[26], random[27]]) % 1_000_000);
        *self.inner.pairing.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pairing { token: token.clone(), code, since: Instant::now(), wrong: 0 });
        PairingInfo { v: 1, k: self.inner.keys.public_b64(), t: token, p: self.port, h: hosts, n: self.inner.name.clone() }
    }

    /// The code to type on the phone while the pairing window is open.
    pub fn pairing_code(&self) -> Option<String> {
        self.inner.pairing.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|p| p.code_ok()).map(|p| p.code.clone())
    }

    /// Ends the pairing window early (the QR dialog closed).
    pub fn close_pairing(&self) {
        self.inner.pairing.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// Sends to every connected phone.
    pub fn broadcast(&self, message: Value) {
        let sessions = self.inner.sessions.lock().unwrap_or_else(|e| e.into_inner());
        for session in sessions.iter() {
            session.out.send(message.clone());
        }
    }

    pub fn devices(&self) -> Vec<Device> {
        self.inner.devices.lock().unwrap_or_else(|e| e.into_inner()).list()
    }

    /// Keys of the phones connected now.
    pub fn connected(&self) -> Vec<String> {
        self.inner.sessions.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|s| s.key.clone()).collect()
    }

    /// Unpairs a phone and drops its connections.
    pub fn forget(&self, key: &str) -> bool {
        let removed = self.inner.devices.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
        self.inner.sessions.lock().unwrap_or_else(|e| e.into_inner()).retain(|s| s.key != key);
        removed
    }

    pub fn stop(&self) {
        self.accept.abort();
        if let Some(task) = self.discovery.lock().unwrap_or_else(|e| e.into_inner()).take() {
            task.abort();
        }
        self.inner.sessions.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

async fn serve(inner: Arc<Inner>, stream: TcpStream) -> Result<(), String> {
    let _ = stream.set_nodelay(true);
    let ws = tokio::time::timeout(HANDSHAKE_TIMEOUT, tokio_tungstenite::accept_async(stream)).await.map_err(|_| "timeout")?.map_err(|e| e.to_string())?;
    let (mut sink, mut stream) = ws.split();

    // Noise IK, responder side: the first message carries the phone's key and its hello.
    let mut hs = snow::Builder::new(NOISE.parse().map_err(|_| "pattern")?)
        .local_private_key(&inner.keys.private)
        .and_then(|b| b.build_responder())
        .map_err(|e| e.to_string())?;
    let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, next_binary(&mut stream)).await.map_err(|_| "timeout")?.ok_or("closed")?;
    let mut hello = vec![0u8; HANDSHAKE_BUF];
    let n = hs.read_message(&first, &mut hello).map_err(|_| "not for this PC")?;
    let hello: Value = serde_json::from_slice(&hello[..n]).unwrap_or_default();
    let key = b64(hs.get_remote_static().ok_or("no key")?);
    let name = hello["device"].as_str().unwrap_or("Phone").to_string();

    let verdict = admit(&inner, &key, &name, hello["token"].as_str());
    let reply = match &verdict {
        Ok(_) => json!({ "ok": true, "pc": inner.name }),
        Err(reason) => json!({ "ok": false, "error": reason }),
    };
    let mut second = vec![0u8; HANDSHAKE_BUF];
    let n = hs.write_message(&serde_json::to_vec(&reply).unwrap_or_default(), &mut second).map_err(|e| e.to_string())?;
    sink.send(Ws::binary(second[..n].to_vec())).await.map_err(|e| e.to_string())?;
    let device = match verdict {
        Ok(device) => device,
        Err(reason) => {
            let _ = sink.close().await;
            return Err(reason);
        }
    };
    let noise: Noise = Arc::new(Mutex::new(hs.into_transport_mode().map_err(|e| e.to_string())?));

    let (tx, rx) = mpsc::unbounded_channel();
    let out = Outbox(tx);
    let id = inner.next.fetch_add(1, Ordering::Relaxed);
    let (kick, mut kicked) = oneshot::channel();
    inner.sessions.lock().unwrap_or_else(|e| e.into_inner()).push(Session { id, key: key.clone(), out: out.clone(), _kick: kick });
    let writer = tokio::spawn(write_loop(sink, noise.clone(), rx));
    inner.handler.connected(&device, &out).await;

    let mut assembler = Assembler::default();
    loop {
        let message = tokio::select! {
            _ = &mut kicked => break,
            message = next_message(&mut stream, &noise, &mut assembler) => message,
        };
        let Some(message) = message else { break };
        inner.handler.message(&device, message, &out).await;
    }
    inner.sessions.lock().unwrap_or_else(|e| e.into_inner()).retain(|s| s.id != id);
    inner.handler.disconnected(&device).await;
    drop(out);
    writer.abort();
    inner.devices.lock().unwrap_or_else(|e| e.into_inner()).seen(&key, now_ms());
    Ok(())
}

/// Paired phones come in; a new phone needs the current, unexpired token or code (used once). Wrong guesses count:
/// after MAX_WRONG_CODES the code stops working (the QR token still works until it times out).
fn admit(inner: &Inner, key: &str, name: &str, token: Option<&str>) -> Result<Device, String> {
    let mut devices = inner.devices.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(device) = devices.get(key) {
        return Ok(device.clone());
    }
    let mut pairing = inner.pairing.lock().unwrap_or_else(|e| e.into_inner());
    let valid = match (pairing.as_mut(), token.map(str::trim)) {
        (Some(open), Some(given)) if open.token_ok() => {
            let right = given == open.token || (open.code_ok() && given == open.code);
            if !right {
                open.wrong += 1;
            }
            right
        }
        _ => false,
    };
    if !valid {
        return Err("This phone is not paired. Scan the QR code or type the code from AnyTask Settings > Phone.".into());
    }
    pairing.take();
    devices.add(name, key, now_ms());
    Ok(devices.get(key).cloned().expect("just added"))
}
