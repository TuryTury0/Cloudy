//! A phone made of code, to try the PC side end to end without a real phone: finds a PC that shows a pairing code,
//! pairs with the code, then lists files, takes a small file, sends one, asks for the screen and moves the mouse a
//! little. Power actions are never sent.
//!   cargo run -p anytask-remote --example fake_phone -- 123456
//! Afterwards remove "Test device" in AnyTask Settings > Phone.

use anytask_remote::{Keys, connect, discovery};
use base64::Engine;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

async fn reply(inbox: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, kind: &str) -> Value {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, inbox.recv()).await {
            Ok(Some(m)) if m["type"] == kind || (m["type"] == "error") => return m,
            Ok(Some(_)) => continue,
            _ => return json!({ "type": "timeout" }),
        }
    }
}

#[tokio::main]
async fn main() {
    let code = std::env::args().nth(1).expect("the 6-digit code shown on the PC");
    let started = Instant::now();
    let mut targets = discovery::broadcast_targets(anytask_remote::DEFAULT_PORT);
    targets.push(format!("127.0.0.1:{}", anytask_remote::DEFAULT_PORT).parse().unwrap());
    let open: Vec<_> = discovery::ask("ANYTASK1 PAIR", &targets, Duration::from_secs(2)).await.into_iter().filter(|f| f.key.is_some()).collect();
    println!("{} ms: {} PC(s) waiting for a code: {:?}", started.elapsed().as_millis(), open.len(), open.iter().map(|f| (&f.name, &f.address)).collect::<Vec<_>>());
    let Some(pc) = open.first() else { return };
    let key = base64::engine::general_purpose::STANDARD.decode(pc.key.as_ref().unwrap()).unwrap();
    println!("fingerprint {}", anytask_remote::fingerprint(&key));
    let phone = Keys::generate();
    let mut c = match connect(&pc.address, &key, &phone, "Test device", Some(&code)).await {
        Ok(c) => c,
        Err(e) => return println!("pairing failed: {e}"),
    };
    let welcome = reply(&mut c.inbox, "welcome").await;
    println!("{} ms: paired with {}; addresses {}; wake {}", started.elapsed().as_millis(), c.pc, welcome["addresses"], welcome["macs"]);

    // Found again by key (as after an IP change).
    let found = discovery::ask(&format!("ANYTASK1 FIND {}", discovery::tag(&key)), &targets, Duration::from_secs(2)).await;
    println!("found by key at {:?}", found.iter().map(|f| &f.address).collect::<Vec<_>>());

    c.outbox.send(json!({ "type": "files" }));
    let roots = reply(&mut c.inbox, "files").await;
    let names: Vec<&str> = roots["entries"].as_array().map(|a| a.iter().filter_map(|e| e["name"].as_str()).collect()).unwrap_or_default();
    println!("starting places: {names:?}");

    let t = Instant::now();
    c.outbox.send(json!({ "type": "screen", "width": 960 }));
    let screen = reply(&mut c.inbox, "screen").await;
    println!("screen {}x{} jpeg {} KB in {} ms, pointer {}", screen["w"], screen["h"], screen["jpeg"].as_str().map_or(0, |s| s.len() * 3 / 4 / 1024), t.elapsed().as_millis(), screen["cursor"]);

    // A small file both ways (into Downloads\AnyTask on the PC, then back).
    let text = "AnyTask test file from the fake phone.\n".repeat(20_000);
    let data = text.as_bytes();
    let mut offset = 0usize;
    let t = Instant::now();
    let mut saved = String::new();
    while offset < data.len() {
        let end = (offset + 384 * 1024).min(data.len());
        let last = end == data.len();
        c.outbox.send(json!({ "type": "upload", "id": "fake1", "name": "fake-phone-test.txt", "offset": offset, "data": base64::engine::general_purpose::STANDARD.encode(&data[offset..end]), "last": last }));
        let r = reply(&mut c.inbox, "uploaded").await;
        if r["error"].is_string() || r["type"] != "uploaded" {
            println!("upload failed: {r}");
            break;
        }
        offset = end;
        if last {
            saved = r["path"].as_str().unwrap_or_default().to_string();
        }
    }
    println!("sent {} KB to the PC in {} ms: {saved}", data.len() / 1024, t.elapsed().as_millis());
    if !saved.is_empty() {
        let t = Instant::now();
        let mut back = Vec::new();
        loop {
            c.outbox.send(json!({ "type": "fetch", "path": saved, "offset": back.len() }));
            let r = reply(&mut c.inbox, "chunk").await;
            if r["type"] != "chunk" {
                println!("fetch failed: {r}");
                break;
            }
            back.extend(base64::engine::general_purpose::STANDARD.decode(r["data"].as_str().unwrap_or_default()).unwrap());
            if r["last"] == true {
                break;
            }
        }
        println!("took it back: {} KB in {} ms, same: {}", back.len() / 1024, t.elapsed().as_millis(), back == data);
        let _ = std::fs::remove_file(&saved);
    }

    // The touchpad: a small square and back to where it was.
    for (dx, dy) in [(40, 0), (0, 40), (-40, 0), (0, -40)] {
        c.outbox.send(json!({ "type": "input", "op": "move", "dx": dx, "dy": dy }));
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    println!("moved the mouse in a small square; total {} ms", started.elapsed().as_millis());
}
