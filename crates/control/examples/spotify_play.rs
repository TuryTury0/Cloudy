//! Plays a song in the real Spotify the way the assistant does (play_music), and prints what Spotify's window offers
//! to press, so the button names can be checked on this PC:
//!   cargo run -p anytask-control --example spotify_play -- "Believer Imagine Dragons"
//! With no song it resumes (or opens Spotify and presses Play). BUTTONS=1 only lists the "Play ..." buttons.

use anytask_core::tools::music::PlayMusic;
use anytask_core::tools::system::SystemOps;
use anytask_core::{DenyAll, Tool, ToolCtx};
use anytask_llm::{GenerateRequest, GenerateResponse, Generator, LlmError};
use async_trait::async_trait;
use std::sync::Arc;

struct NoModel;

#[async_trait]
impl Generator for NoModel {
    async fn generate(&self, _: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
        Err(LlmError::Answer("no model here".into()))
    }
    fn label(&self) -> String {
        "none".into()
    }
}

#[tokio::main]
async fn main() {
    let query = std::env::args().nth(1).unwrap_or_default();
    let system = Arc::new(anytask_control::NativeSystem);
    if std::env::var("BUTTONS").is_ok() {
        let names = system.app_buttons("Spotify").unwrap_or_default();
        println!("{} buttons; the \"Play ...\" ones:", names.len());
        for name in names.iter().filter(|n| n.starts_with("Play")) {
            println!("  {name}");
        }
        return;
    }
    let ctx = ToolCtx {
        cancel: tokio_util::sync::CancellationToken::new(),
        confirm: Arc::new(DenyAll),
        now_ms: 0,
        model: Arc::new(NoModel),
        progress: Arc::new(|text| println!("step: {text}")),
        log: Arc::new(|text| println!("log:  {text}")),
        request: query.clone(),
    };
    let args = if query.is_empty() { serde_json::json!({}) } else { serde_json::json!({ "query": query }) };
    let started = std::time::Instant::now();
    let result = PlayMusic { ops: system }.run(args, &ctx).await;
    println!("{:.1} s: {result:?}", started.elapsed().as_secs_f32());
}
