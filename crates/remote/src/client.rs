//! The phone side: connects to the PC from the pairing info (or the saved PC key), proves who it is, then sends
//! and receives JSON messages.

use crate::channel::{Assembler, HANDSHAKE_BUF, Noise, next_binary, next_message, write_loop};
use crate::server::Outbox;
use crate::{Keys, NOISE};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Ws;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

pub struct Connection {
    pub outbox: Outbox,
    pub inbox: mpsc::UnboundedReceiver<Value>,
    /// The PC's name.
    pub pc: String,
}

/// `address` is "host:port". `token` only for the first connection (pairing).
pub async fn connect(address: &str, pc_key: &[u8], keys: &Keys, device: &str, token: Option<&str>) -> Result<Connection, String> {
    let url = format!("ws://{address}/");
    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url.as_str()))
        .await
        .map_err(|_| format!("{address} did not answer"))?
        .map_err(|e| format!("Could not reach {address}: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    let mut hs = snow::Builder::new(NOISE.parse().map_err(|_| "pattern")?)
        .local_private_key(&keys.private)
        .and_then(|b| b.remote_public_key(pc_key))
        .and_then(|b| b.build_initiator())
        .map_err(|e| e.to_string())?;
    let hello = json!({ "device": device, "token": token });
    let mut first = vec![0u8; HANDSHAKE_BUF];
    let n = hs.write_message(&serde_json::to_vec(&hello).unwrap_or_default(), &mut first).map_err(|e| e.to_string())?;
    sink.send(Ws::binary(first[..n].to_vec())).await.map_err(|e| e.to_string())?;

    let second = tokio::time::timeout(CONNECT_TIMEOUT, next_binary(&mut stream))
        .await
        .map_err(|_| "The PC did not answer.".to_string())?
        .ok_or("The PC closed the connection (is this the right PC?).")?;
    let mut reply = vec![0u8; HANDSHAKE_BUF];
    let n = hs.read_message(&second, &mut reply).map_err(|_| "The PC's key does not match; pair again.")?;
    let reply: Value = serde_json::from_slice(&reply[..n]).unwrap_or_default();
    if reply["ok"].as_bool() != Some(true) {
        return Err(reply["error"].as_str().unwrap_or("The PC refused the connection.").to_string());
    }
    let noise: Noise = Arc::new(Mutex::new(hs.into_transport_mode().map_err(|e| e.to_string())?));

    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(write_loop(sink, noise.clone(), rx));
    let (in_tx, inbox) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut assembler = Assembler::default();
        while let Some(message) = next_message(&mut stream, &noise, &mut assembler).await {
            if in_tx.send(message).is_err() {
                break;
            }
        }
    });
    Ok(Connection { outbox: Outbox(tx), inbox, pc: reply["pc"].as_str().unwrap_or_default().to_string() })
}
