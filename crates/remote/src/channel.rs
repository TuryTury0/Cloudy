//! Encrypted messages over a WebSocket: each JSON message is split into Noise messages (at most 64 KB each);
//! the first byte of every piece says whether more pieces follow.

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use snow::TransportState;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as Ws;

const PIECE: usize = 65_000;
/// Bigger messages are refused (a screenshot is well under this).
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
pub(crate) const HANDSHAKE_BUF: usize = 65_535;

pub(crate) type Noise = Arc<Mutex<TransportState>>;

pub(crate) fn lock(noise: &Noise) -> std::sync::MutexGuard<'_, TransportState> {
    noise.lock().unwrap_or_else(|e| e.into_inner())
}

/// Encrypts one message into WebSocket frames.
pub(crate) fn seal(noise: &Noise, message: &Value) -> Result<Vec<Ws>, String> {
    let text = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    let mut frames = Vec::new();
    let pieces: Vec<&[u8]> = if text.is_empty() { vec![&[][..]] } else { text.chunks(PIECE).collect() };
    let count = pieces.len();
    let mut state = lock(noise);
    for (i, piece) in pieces.into_iter().enumerate() {
        let mut plain = Vec::with_capacity(piece.len() + 1);
        plain.push(u8::from(i + 1 < count));
        plain.extend_from_slice(piece);
        let mut out = vec![0u8; plain.len() + 16];
        let n = state.write_message(&plain, &mut out).map_err(|e| format!("encrypt: {e}"))?;
        out.truncate(n);
        frames.push(Ws::binary(out));
    }
    Ok(frames)
}

/// Collects decrypted pieces into whole messages.
#[derive(Default)]
pub(crate) struct Assembler {
    buf: Vec<u8>,
}

impl Assembler {
    /// Some(message) when the last piece arrived.
    pub(crate) fn push(&mut self, noise: &Noise, frame: &[u8]) -> Result<Option<Value>, String> {
        let mut plain = vec![0u8; frame.len()];
        let n = lock(noise).read_message(frame, &mut plain).map_err(|_| "a message could not be decrypted".to_string())?;
        let (&more, data) = plain[..n].split_first().ok_or("empty message")?;
        if self.buf.len() + data.len() > MAX_MESSAGE {
            return Err("message too large".into());
        }
        self.buf.extend_from_slice(data);
        if more == 1 {
            return Ok(None);
        }
        let message = serde_json::from_slice(&self.buf).map_err(|e| format!("bad message: {e}"));
        self.buf.clear();
        message.map(Some)
    }
}

/// Sends queued messages until the queue closes or the socket fails.
pub(crate) async fn write_loop<S>(mut sink: SplitSink<WebSocketStream<S>, Ws>, noise: Noise, mut queue: tokio::sync::mpsc::UnboundedReceiver<Value>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    while let Some(message) = queue.recv().await {
        let Ok(frames) = seal(&noise, &message) else { continue };
        for frame in frames {
            if sink.send(frame).await.is_err() {
                return;
            }
        }
    }
    let _ = sink.close().await;
}

/// Reads whole messages; ends on close, error or a message that does not decrypt.
pub(crate) async fn next_message<S>(stream: &mut SplitStream<WebSocketStream<S>>, noise: &Noise, assembler: &mut Assembler) -> Option<Value>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    while let Some(frame) = stream.next().await {
        match frame {
            Ok(Ws::Binary(data)) => match assembler.push(noise, &data) {
                Ok(Some(message)) => return Some(message),
                Ok(None) => continue,
                Err(_) => return None,
            },
            Ok(Ws::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
    None
}

/// The next binary frame (handshake), or None.
pub(crate) async fn next_binary<S>(stream: &mut SplitStream<WebSocketStream<S>>) -> Option<Vec<u8>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    while let Some(frame) = stream.next().await {
        match frame {
            Ok(Ws::Binary(data)) => return Some(data.to_vec()),
            Ok(Ws::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
    None
}
