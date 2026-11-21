//! Opens a Gemini Live session, sends one typed sentence and reports the spoken answer (audio is counted, not
//! played).   GEMINI_API_KEY=... cargo run -p anytask-llm --example live_ping -- gemini-3.8-live
use anytask_llm::{LiveConfig, LiveEvent, LiveSession};
use std::time::Instant;

#[tokio::main]
async fn main() {
    let model = std::env::args().nth(1).unwrap_or_else(|| "gemini-3.8-live".into());
    let key = std::env::var("GEMINI_API_KEY").expect("set GEMINI_API_KEY");
    let config = LiveConfig { model, voice: "Aoede".into(), system: "You are a friendly assistant. Answer in one short sentence.".into(), tools: vec![], proxy: None };
    let t = Instant::now();
    let (session, mut events) = match LiveSession::connect(&config, &key).await {
        Ok(s) => s,
        Err(e) => return println!("connect failed after {:?}: {e}", t.elapsed()),
    };
    println!("ready in {:?}", t.elapsed());
    for question in ["سلام! حالت چطوره؟", "یه عدد بین ۱ تا ۱۰ بگو."] {
        let t = Instant::now();
        session.send_text(question);
        let (mut samples, mut said, mut first_audio) = (0usize, String::new(), None);
        while let Ok(Some(event)) = tokio::time::timeout(std::time::Duration::from_secs(20), events.recv()).await {
            match event {
                LiveEvent::Audio(pcm) => {
                    first_audio.get_or_insert(t.elapsed());
                    samples += pcm.len();
                }
                LiveEvent::Said(text) => said.push_str(&text),
                LiveEvent::Usage(usage) => println!("usage at {:?}: {usage:?}", t.elapsed()),
                LiveEvent::TurnComplete => break,
                LiveEvent::Closed(reason) => {
                    println!("closed: {reason}");
                    break;
                }
                _ => {}
            }
        }
        println!("first audio after {:?}; {:.1} s of speech; said: {said}", first_audio.unwrap_or_default(), samples as f32 / 24_000.0);
    }
    session.close();
}
