//! Plays one spoken Gemini Live answer through the real speaker (checks the voice path end to end: 24 kHz audio,
//! resampling, playback).   GEMINI_API_KEY=... cargo run -p anytask-llm --example live_voice
use anytask_audio::Speaker;
use anytask_llm::live::OUTPUT_RATE;
use anytask_llm::{LiveConfig, LiveEvent, LiveSession};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let key = std::env::var("GEMINI_API_KEY").expect("set GEMINI_API_KEY");
    let config = LiveConfig { model: "gemini-3.8-live".into(), voice: "Kore".into(), system: "You are a friendly assistant. Answer in one short Persian sentence.".into(), tools: vec![], proxy: None };
    let speaker = Speaker::start().expect("speaker");
    let (session, mut events) = LiveSession::connect(&config, &key).await.expect("connect");
    let t = Instant::now();
    session.send_text("سلام! خودت رو در یک جمله کوتاه معرفی کن.");
    let (mut first, mut samples, mut said) = (None, 0usize, String::new());
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(20), events.recv()).await {
        match event {
            LiveEvent::Audio(pcm) => {
                first.get_or_insert(t.elapsed());
                samples += pcm.len();
                speaker.play(&pcm, OUTPUT_RATE);
            }
            LiveEvent::Said(text) => said.push_str(&text),
            LiveEvent::TurnComplete | LiveEvent::Closed(_) => break,
            _ => {}
        }
    }
    let mut loudest = 0.0f32;
    while speaker.is_playing() {
        loudest = loudest.max(speaker.level());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    session.close();
    speaker.stop();
    println!("first audio {:?}, {:.1} s of speech played (loudest level {loudest:.2}), said: {said}", first.unwrap_or_default(), samples as f32 / OUTPUT_RATE as f32);
}
