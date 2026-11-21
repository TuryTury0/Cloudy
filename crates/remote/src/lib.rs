//! The phone's channel to the PC: a WebSocket on the local network, encrypted and authenticated with Noise IK
//! (the phone knows the PC's key from the pairing QR code; the PC knows each paired phone's key). Messages are JSON.
//!
//! Pairing: the PC shows a QR code with its address, its public key and a one-time token (valid 10 minutes). The
//! phone connects with that token once; from then on its own key is enough. Unknown phones are refused.

mod channel;
mod client;
mod devices;
pub mod discovery;
mod server;

pub use client::{Connection, connect};
pub use devices::{Device, DeviceStore};
pub use server::{Handler, Outbox, RemoteServer};

use base64::Engine;
use serde::{Deserialize, Serialize};

pub const NOISE: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
pub const DEFAULT_PORT: u16 = 47_321;

pub(crate) fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(crate) fn unb64(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(text.trim()).ok()
}

/// A static key pair (X25519).
#[derive(Clone, PartialEq, Eq)]
pub struct Keys {
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys {{ public: {} }}", b64(&self.public))
    }
}

impl Keys {
    pub fn generate() -> Self {
        let builder = snow::Builder::new(NOISE.parse().expect("valid Noise pattern"));
        let pair = builder.generate_keypair().expect("system randomness");
        Self { private: pair.private, public: pair.public }
    }

    /// "private:public" in base64, for the keychain.
    pub fn to_secret(&self) -> String {
        format!("{}:{}", b64(&self.private), b64(&self.public))
    }

    pub fn from_secret(text: &str) -> Option<Self> {
        let (private, public) = text.split_once(':')?;
        let (private, public) = (unb64(private)?, unb64(public)?);
        (private.len() == 32 && public.len() == 32).then_some(Self { private, public })
    }

    pub fn public_b64(&self) -> String {
        b64(&self.public)
    }
}

/// What the pairing QR code carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairingInfo {
    /// Format version.
    pub v: u32,
    /// The PC's public key (base64).
    pub k: String,
    /// One-time pairing token.
    pub t: String,
    pub p: u16,
    /// The PC's addresses on the local network.
    pub h: Vec<String>,
    /// The PC's name, shown on the phone.
    pub n: String,
}

impl PairingInfo {
    pub fn qr_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str::<Self>(text).ok().filter(|p| p.v == 1 && unb64(&p.k).is_some_and(|k| k.len() == 32))
    }
}

/// A short, readable form of a public key ("A1B2-C3D4-E5F6"), shown on the PC next to the pairing code and on the
/// phone before it sends the code: a computer pretending to be the PC on the network would need a key with the same
/// first 48 bits, which cannot be made in the minutes a code is valid.
pub fn fingerprint(public: &[u8]) -> String {
    public.iter().take(6).map(|b| format!("{b:02X}")).collect::<Vec<_>>().chunks(2).map(|pair| pair.concat()).collect::<Vec<_>>().join("-")
}

/// The address the PC uses on the local network (no packet is sent: a UDP socket only picks its route).
pub fn lan_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}
