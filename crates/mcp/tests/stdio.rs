//! Starts a real MCP server process (tests/stub_server.js, plain Node, no packages) and uses its tools.

use anytask_mcp::{LogSink, McpConfig, McpManager, SecretLookup, Status};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::test]
async fn a_stdio_server_is_started_used_and_its_crash_is_noticed() {
    if rmcp_node_missing() {
        eprintln!("node is not installed; skipped");
        return;
    }
    let stub = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub_server.js");
    let config: McpConfig = serde_json::from_value(json!({ "mcpServers": {
        "stub": { "command": "node", "args": [stub], "env": { "STUB_TOKEN": "secret:STUB" } },
        "picky": { "command": "node", "args": [stub], "tools": ["fails"] },
        "off": { "command": "node", "args": [stub], "disabled": true },
        "missing": { "command": "definitely-not-a-program-xyz" },
        "nokey": { "command": "node", "args": [stub], "env": { "T": "secret:NOPE" } }
    } }))
    .unwrap();
    let secrets: SecretLookup = Arc::new(|name| (name == "STUB").then(|| "t0k".to_string()));
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = lines.clone();
    let log: LogSink = Arc::new(move |line| sink.lock().unwrap().push(line));
    let manager = McpManager::new(secrets, log);
    manager.start_all(&config).await;

    let status = manager.status(&config);
    let of = |name: &str| status.iter().find(|s| s.name == name).unwrap().clone();
    assert_eq!(of("stub").status, Status::Connected, "{:?}", of("stub"));
    assert_eq!(of("stub").tools, vec!["get-price", "fails", "crash"]);
    assert_eq!(of("picky").tools, vec!["fails"]);
    assert_eq!(of("off").status, Status::Disabled);
    assert_eq!(of("missing").status, Status::Error);
    assert!(of("missing").error.unwrap().contains("not found"));
    assert!(of("nokey").error.unwrap().contains("NOPE"));

    let mut names: Vec<String> = manager.tools().iter().map(|t| t.spec().name).collect();
    names.sort();
    assert_eq!(names, vec!["mcp__picky__fails", "mcp__stub__crash", "mcp__stub__fails", "mcp__stub__get_price"]);
    let price = manager.tools().into_iter().find(|t| t.spec().name == "mcp__stub__get_price").unwrap();
    assert!(price.spec().description.starts_with("[MCP stub] Returns the price"));

    let schema = price.spec().parameters;
    // Gemini sends free-form objects as JSON text; the server still gets an object.
    let answer = manager.call("stub", "get-price", json!({ "item": "gold", "options": "{\"fast\":true}" }), &schema).await.unwrap();
    assert_eq!(answer["text"], "gold = 262,073,000 | options={\"fast\":true} | env=t0k");
    assert_eq!(manager.call("stub", "fails", json!({}), &json!({})).await.unwrap_err(), "nothing here");
    assert!(lines.lock().unwrap().iter().any(|l| l == "stub: stub server ready"), "server output reaches the log");

    manager.call("stub", "crash", json!({}), &json!({})).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(manager.call("stub", "get-price", json!({ "item": "gold" }), &schema).await.is_err());
    assert_eq!(manager.status(&config).iter().find(|s| s.name == "stub").unwrap().status, Status::Error);
    assert!(!manager.tools().iter().any(|t| t.spec().name.starts_with("mcp__stub__")));

    // The other server is untouched and closes when removed.
    assert_eq!(manager.call("picky", "fails", json!({}), &json!({})).await.unwrap_err(), "nothing here");
    manager.disconnect("picky");
    assert!(manager.call("picky", "fails", json!({}), &json!({})).await.unwrap_err().contains("not running"));
}

fn rmcp_node_missing() -> bool {
    std::process::Command::new("node").arg("--version").output().is_err()
}
