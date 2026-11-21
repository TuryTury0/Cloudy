//! Asks a model the app has saved once per thinking level, through the same client, routes and proxy the app uses,
//! and prints what came back (never the key).
//!
//!   cargo run -p anytask-core --example model_check -- <saved model id> <model id to ask> [base url]
//!   e.g. cargo run -p anytask-core --example model_check -- m1a18e67ca62 gemini-3.8-flash
//!
//! The key is read from the Windows Credential Manager entry the app saved for the first id.

use anytask_llm::{GenerateRequest, LlmClient, Message, ModelConfig, Provider, Thinking};
use anytask_storage::{Keychain, SecretStore, model_key_name};
use std::time::Instant;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let saved = args.first().cloned().unwrap_or_else(|| "m1a18e67ca62".into());
    let model_id = args.get(1).cloned().unwrap_or_else(|| "gemini-3.8-flash".into());
    let base_url = args.get(2).cloned().unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".into());
    let key = Keychain::new("AnyTask Next").get(&model_key_name(&saved)).expect("no key saved for that model in the app");
    // CHECK_SEARCH="question" (CHECK_DEPTH=deep): one Google search through this Gemini model, as web_search does it.
    if let Ok(question) = std::env::var("CHECK_SEARCH") {
        let google = anytask_web::GoogleSearch { base_url: base_url.clone(), models: vec![model_id.clone(), "gemini-2.5-flash-lite".into()], key: key.clone() };
        let depth = if std::env::var("CHECK_DEPTH").as_deref() == Ok("deep") { anytask_web::Depth::Deep } else { anytask_web::Depth::Quick };
        let started = std::time::Instant::now();
        let net = anytask_web::Net::new();
        let found = net
            .try_both_if(
                |client| {
                    let (google, q) = (google.clone(), question.clone());
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    async move { anytask_web::google_search(&client, &google, &q, depth, &today).await }
                },
                anytask_web::google_route_retry,
                anytask_web::google_route_retry,
            )
            .await;
        match found {
            Ok(answer) => {
                println!("{} ms: {}", started.elapsed().as_millis(), answer.answer.unwrap_or_default());
                for r in answer.results {
                    println!("  source: {}", r.title);
                }
            }
            Err(e) => println!("{} ms: FAILED {e}", started.elapsed().as_millis()),
        }
        return;
    }
    // CHECK_LIST=1: list the Gemini models this key can call (through the proxy) and stop.
    if std::env::var("CHECK_LIST").is_ok() {
        let http = reqwest::Client::builder().proxy(reqwest::Proxy::all("socks5h://127.0.0.1:10808").unwrap()).build().unwrap();
        let body: serde_json::Value = http
            .get(format!("{base_url}/models?pageSize=200"))
            .header("x-goog-api-key", &key)
            .send()
            .await
            .expect("list")
            .json()
            .await
            .expect("json");
        for m in body["models"].as_array().into_iter().flatten() {
            let methods = m["supportedGenerationMethods"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default();
            if methods.contains("generateContent") {
                println!("{} | thinking: {}", m["name"].as_str().unwrap_or(""), m["thinking"].as_bool().map(|b| b.to_string()).unwrap_or_default());
            }
        }
        return;
    }
    let client = LlmClient::new();
    // As the app does with the default settings: the VPN's local proxy, no model sent through it first.
    // As the app does with the default settings: the VPN's local proxy, no model sent through it first. With
    // CHECK_PROXY_FIRST=1 the model goes through the proxy first (Settings > Models > Proxy, the model switched on).
    let first: Vec<String> = if std::env::var("CHECK_PROXY_FIRST").is_ok() { vec![format!("check-{model_id}")] } else { Vec::new() };
    client.set_proxy(anytask_llm::proxy::proxy_url("127.0.0.1:10808").as_deref(), &first).expect("proxy");
    client.set_log(|line| println!("   log: {line}"));
    // CHECK_LEVELS=low,high picks levels; each request is given at most CHECK_SECONDS (default 60).
    let levels: Vec<Thinking> = std::env::var("CHECK_LEVELS")
        .unwrap_or_else(|_| "default,low,medium,high".into())
        .split(',')
        .filter_map(|l| match l.trim() {
            "default" => Some(Thinking::Default),
            "low" => Some(Thinking::Low),
            "medium" => Some(Thinking::Medium),
            "high" => Some(Thinking::High),
            _ => None,
        })
        .collect();
    let limit = std::time::Duration::from_secs(std::env::var("CHECK_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(60));
    for thinking in levels {
        let config = ModelConfig {
            id: format!("check-{model_id}"),
            name: "Check".into(),
            provider: if base_url.contains("generativelanguage") { Provider::Gemini } else { Provider::Openai },
            base_url: base_url.clone(),
            model_id: model_id.clone(),
            thinking,
        };
        let request = GenerateRequest {
            system: "Reply with exactly: OK".into(),
            messages: vec![Message::user("ping")],
            tools: Vec::new(),
            allow_tools: false,
            max_output_tokens: 1500,
            thinking: None,
        };
        let started = Instant::now();
        let answer = match tokio::time::timeout(limit, client.generate(&config, &key, &request)).await {
            Ok(answer) => answer,
            Err(_) => Err(anytask_llm::LlmError::Answer(format!("no answer within {} s", limit.as_secs()))),
        };
        match answer {
            Ok(response) => {
                let usage = response.usage.unwrap_or_default();
                println!(
                    "{model_id} {thinking:?}: OK in {} ms, \"{}\" ({} in, {} out, {} thinking)",
                    started.elapsed().as_millis(),
                    response.text().trim(),
                    usage.input,
                    usage.output,
                    usage.thinking
                );
            }
            Err(e) => println!("{model_id} {thinking:?}: FAILED in {} ms: {}", started.elapsed().as_millis(), e.to_string().chars().take(220).collect::<String>()),
        }
    }
}
