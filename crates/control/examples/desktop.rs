//! Runs one desktop goal with a real Gemini model and the real screen, mouse and keyboard, printing each step.
//!   GEMINI_API_KEY=... cargo run -p anytask-control --example desktop -- "open Notepad and write: hello"
//! It takes over the mouse and keyboard while it runs. The key is read from the environment only.
//! With ANYTASK_SHOTS=<folder>, every screenshot the model saw is saved there for checking.

use anytask_control::NativeDesktop;
use anytask_core::tools::desktop::{DesktopOps, DesktopTask, Look, MouseButton};
use anytask_core::tools::{DenyAll, Tool, ToolCtx};
use anytask_llm::{ConfiguredModel, LlmClient, ModelConfig, Provider, Thinking};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let goal = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let api_key = std::env::var("GEMINI_API_KEY").expect("set GEMINI_API_KEY");
    let model_id = std::env::var("ANYTASK_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());
    let model = Arc::new(ConfiguredModel {
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
    let started = std::time::Instant::now();
    let ctx = ToolCtx {
        cancel: CancellationToken::new(),
        confirm: Arc::new(DenyAll),
        now_ms: 0,
        model,
        progress: Arc::new(move |s: String| println!("  [{:>5.1}s] {s}", started.elapsed().as_secs_f32())),
        log: Arc::new(move |s: String| println!("  [{:>5.1}s]     log: {s}", started.elapsed().as_secs_f32())),
        request: String::new(),
    };
    let task = DesktopTask { ops: Arc::new(Recorder { shots: std::env::var("ANYTASK_SHOTS").ok(), count: Default::default() }), learner: None, judge: None, skills: None };
    let result = task.run(serde_json::json!({ "goal": goal }), &ctx).await;
    println!("= {:.1}s {result:?}", started.elapsed().as_secs_f32());
}

/// The real desktop, saving each screenshot when asked.
struct Recorder {
    shots: Option<String>,
    count: std::sync::atomic::AtomicUsize,
}

impl DesktopOps for Recorder {
    fn look(&self) -> Result<Look, String> {
        let look = NativeDesktop.look()?;
        if let Some(dir) = &self.shots {
            let n = self.count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(format!("{dir}/step-{n:02}.jpg"), &look.jpeg);
            println!("           active window: {}", look.title);
        }
        Ok(look)
    }
    fn click(&self, x: u16, y: u16, b: MouseButton, c: u8) -> Result<(), String> {
        NativeDesktop.click(x, y, b, c)
    }
    fn scroll(&self, x: u16, y: u16, up: bool) -> Result<(), String> {
        NativeDesktop.scroll(x, y, up)
    }
    fn type_text(&self, text: &str) -> Result<(), String> {
        NativeDesktop.type_text(text)
    }
    fn hotkey(&self, combo: &str) -> Result<(), String> {
        NativeDesktop.hotkey(combo)
    }
    fn launch_app(&self, name: &str) -> Result<(), String> {
        NativeDesktop.launch_app(name)
    }
    fn open_url(&self, url: &str) -> Result<(), String> {
        NativeDesktop.open_url(url)
    }
    fn fingerprint(&self) -> Result<u64, String> {
        NativeDesktop.fingerprint()
    }
    fn active_title(&self) -> String {
        NativeDesktop.active_title()
    }
}
