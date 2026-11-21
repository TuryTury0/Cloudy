//! Asks the brain one question with a real model and the background tools (web, files read-only, memory), and
//! prints every step. For checking a model end to end without the desktop app.
//!
//!   GEMINI_API_KEY=... cargo run -p anytask-core --example ask -- "قیمت دلار امروز چنده؟"
//!   (optional) ANYTASK_MODEL=gemini-2.5-flash
//!
//! The key is read from the environment only; it is never written anywhere.

use anytask_core::tools::files::{ListDirectory, ReadTextFile};
use anytask_core::tools::memory::{RecallMemory, RememberFact};
use anytask_core::tools::web::{ReadWebPage, SearchApi, WebSearch};
use anytask_core::{Agent, CoreEvent, DenyAll, EventBus, Identity, Registry, UserFolders};
use anytask_llm::{ConfiguredModel, LlmClient, ModelConfig, Provider, Thinking};
use anytask_storage::Memory;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let prompt = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let prompt = if prompt.trim().is_empty() { "سلام! خودت رو در یک جمله معرفی کن.".to_string() } else { prompt };
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        eprintln!("Set GEMINI_API_KEY first.");
        std::process::exit(2);
    };
    let model_id = std::env::var("ANYTASK_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());

    let net = anytask_web::Net::new();
    let folders = Arc::new(UserFolders::detect());
    let memory = Arc::new(Memory::open_in_memory().expect("memory"));
    let api: SearchApi = Arc::new(|| None);
    let mut tools = Registry::default();
    tools.add(Arc::new(WebSearch { net: net.clone(), api, google: None }));
    tools.add(Arc::new(ReadWebPage { net }));
    tools.add(Arc::new(ListDirectory { folders: folders.clone() }));
    tools.add(Arc::new(ReadTextFile { folders }));
    tools.add(Arc::new(RememberFact { memory: memory.clone() }));
    tools.add(Arc::new(RecallMemory { memory: memory.clone() }));

    let events = EventBus::default();
    let mut rx = events.subscribe();
    let printer = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            match event {
                CoreEvent::ToolStarted { label, .. } => println!("  > {label}"),
                CoreEvent::ToolFinished { ok, summary, .. } => println!("    {} {}", if ok { "ok" } else { "FAILED" }, summary),
                CoreEvent::ModelUsed { label, ok, reason } => println!("  model {label}: {}", if ok { "ok".into() } else { reason.unwrap_or_default() }),
                _ => {}
            }
        }
    });

    let agent = Agent::new(tools, memory, events, Arc::new(DenyAll));
    let model: Arc<dyn anytask_llm::Generator> = Arc::new(ConfiguredModel {
        client: Arc::new(LlmClient::new()),
        config: ModelConfig {
            id: "example".into(),
            name: "Gemini".into(),
            provider: Provider::Gemini,
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            model_id,
            thinking: Thinking::Default,
        },
        api_key,
    });
    println!("? {prompt}");
    let started = std::time::Instant::now();
    let outcome = agent.run(model, &Identity::default(), &prompt).await;
    printer.abort();
    println!("= [{:?} in {:.1}s] {}", outcome.status, started.elapsed().as_secs_f32(), outcome.text);
}
