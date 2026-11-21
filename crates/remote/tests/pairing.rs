//! A real server on 127.0.0.1 and real clients: pairing, messages both ways, big messages, refusals.

use anytask_remote::{Device, DeviceStore, Handler, Keys, Outbox, PairingInfo, RemoteServer, connect};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct Echo;

#[async_trait::async_trait]
impl Handler for Echo {
    async fn connected(&self, device: &Device, out: &Outbox) {
        out.send(json!({ "type": "welcome", "device": device.name }));
    }
    async fn message(&self, device: &Device, message: Value, out: &Outbox) {
        out.send(json!({ "type": "echo", "from": device.name, "message": message }));
    }
}

async fn next(inbox: &mut tokio::sync::mpsc::UnboundedReceiver<Value>) -> Value {
    tokio::time::timeout(Duration::from_secs(5), inbox.recv()).await.expect("a message in time").expect("open")
}

fn key_of(info: &PairingInfo) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(&info.k).unwrap()
}

#[tokio::test]
async fn pair_talk_reconnect_and_refuse_strangers() {
    let pc = Keys::generate();
    let server = RemoteServer::start("127.0.0.1:0", "Desk PC", pc.clone(), DeviceStore::in_memory(), Arc::new(Echo)).await.unwrap();
    let address = format!("127.0.0.1:{}", server.port());
    let info = server.new_pairing(vec!["127.0.0.1".into()]);
    let parsed = PairingInfo::parse(&info.qr_text()).expect("QR text parses back");
    assert_eq!(parsed, info);
    let pc_key = key_of(&info);

    // A stranger without the token is refused with a reason.
    let stranger = Keys::generate();
    let refused = connect(&address, &pc_key, &stranger, "Stranger", None).await.err().unwrap();
    assert!(refused.contains("not paired"), "{refused}");

    // Pairing with the token.
    let phone = Keys::generate();
    let mut c = connect(&address, &pc_key, &phone, "Pixel", Some(&info.t)).await.unwrap();
    assert_eq!(c.pc, "Desk PC");
    assert_eq!(next(&mut c.inbox).await, json!({ "type": "welcome", "device": "Pixel" }));
    c.outbox.send(json!({ "type": "prompt", "text": "سلام" }));
    assert_eq!(next(&mut c.inbox).await["message"]["text"], "سلام");

    // Big messages are split and joined again.
    let big = "x".repeat(300_000);
    c.outbox.send(json!({ "blob": big }));
    assert_eq!(next(&mut c.inbox).await["message"]["blob"].as_str().unwrap().len(), 300_000);

    // Broadcasts reach the phone.
    server.broadcast(json!({ "type": "event", "n": 1 }));
    assert_eq!(next(&mut c.inbox).await["n"], 1);
    assert_eq!(server.devices().len(), 1);
    assert_eq!(server.connected().len(), 1);

    // The token works only once.
    let other = Keys::generate();
    assert!(connect(&address, &pc_key, &other, "Other", Some(&info.t)).await.is_err());

    // The paired phone reconnects without a token.
    drop(c);
    let mut again = connect(&address, &pc_key, &phone, "Pixel", None).await.unwrap();
    assert_eq!(next(&mut again.inbox).await["type"], "welcome");

    // A wrong PC key fails the handshake.
    let wrong = Keys::generate();
    assert!(connect(&address, &wrong.public, &phone, "Pixel", None).await.is_err());

    // Forgetting the phone ends its session and refuses it later.
    assert!(server.forget(&phone.public_b64()));
    let closed = tokio::time::timeout(Duration::from_secs(5), again.inbox.recv()).await.expect("ends in time");
    assert!(closed.is_none(), "{closed:?}");
    assert!(connect(&address, &pc_key, &phone, "Pixel", None).await.is_err());
}

#[tokio::test]
async fn a_typed_code_pairs_once_and_wrong_codes_close_the_window() {
    let pc = Keys::generate();
    let server = RemoteServer::start("127.0.0.1:0", "Desk PC", pc.clone(), DeviceStore::in_memory(), Arc::new(Echo)).await.unwrap();
    let address = format!("127.0.0.1:{}", server.port());
    assert_eq!(server.pairing_code(), None, "no code before pairing is opened");
    server.new_pairing(vec![]);
    let code = server.pairing_code().expect("a code while pairing");
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));

    let phone = Keys::generate();
    let mut c = connect(&address, &pc.public, &phone, "Galaxy", Some(&code)).await.expect("the code pairs");
    assert_eq!(next(&mut c.inbox).await["type"], "welcome");
    assert_eq!(server.pairing_code(), None, "used once");

    // Five wrong codes close the code; then even the right one is refused, but the QR code still pairs (a stranger
    // guessing cannot keep the owner out).
    let info = server.new_pairing(vec![]);
    let right = server.pairing_code().unwrap();
    for i in 0..5 {
        let guess = format!("{:06}", (right.parse::<u32>().unwrap() + 1 + i) % 1_000_000);
        assert!(connect(&address, &pc.public, &Keys::generate(), "Guess", Some(&guess)).await.is_err());
    }
    assert_eq!(server.pairing_code(), None);
    assert!(connect(&address, &pc.public, &Keys::generate(), "Late", Some(&right)).await.is_err());
    let mut owner = connect(&address, &pc.public, &Keys::generate(), "Owner", Some(&info.t)).await.expect("the QR token still pairs");
    assert_eq!(next(&mut owner.inbox).await["type"], "welcome");
}

#[tokio::test]
async fn a_phone_finds_the_pc_on_the_network_by_its_key() {
    use anytask_remote::discovery;
    let pc = Keys::generate();
    let server = RemoteServer::start("127.0.0.1:0", "Desk PC", pc.clone(), DeviceStore::in_memory(), Arc::new(Echo)).await.unwrap();
    let udp = server.start_discovery("127.0.0.1:0").await.unwrap();
    let here: Vec<std::net::SocketAddr> = vec![format!("127.0.0.1:{udp}").parse().unwrap()];
    let wait = Duration::from_millis(600);

    let found = discovery::ask(&format!("ANYTASK1 FIND {}", discovery::tag(&pc.public)), &here, wait).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].address, format!("127.0.0.1:{}", server.port()), "the TCP port phones connect to");
    assert!(discovery::ask(&format!("ANYTASK1 FIND {}", discovery::tag(&Keys::generate().public)), &here, wait).await.is_empty(), "another PC's key");

    // A PC waiting for a code tells its key and name, only while the code is valid.
    assert!(discovery::ask("ANYTASK1 PAIR", &here, wait).await.is_empty());
    server.new_pairing(vec![]);
    let open = discovery::ask("ANYTASK1 PAIR", &here, wait).await;
    assert_eq!(open[0].key.as_deref(), Some(pc.public_b64().as_str()));
    assert_eq!(open[0].name.as_deref(), Some("Desk PC"));
    let code = server.pairing_code().unwrap();
    let key = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(open[0].key.as_ref().unwrap()).unwrap()
    };
    let mut c = connect(&open[0].address, &key, &Keys::generate(), "Galaxy", Some(&code)).await.expect("pairs with what discovery told");
    assert_eq!(next(&mut c.inbox).await["type"], "welcome");
}

#[test]
fn fingerprints_are_short_and_readable() {
    assert_eq!(anytask_remote::fingerprint(&[0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x01]), "A1B2-C3D4-E5F6");
}

#[test]
fn keys_survive_the_keychain_format() {
    let keys = Keys::generate();
    assert_eq!(Keys::from_secret(&keys.to_secret()), Some(keys.clone()));
    assert!(Keys::from_secret("nonsense").is_none());
    assert!(!format!("{keys:?}").contains(&anytask_remote_private(&keys)), "Debug never prints the private key");
}

fn anytask_remote_private(keys: &Keys) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(&keys.private)
}
