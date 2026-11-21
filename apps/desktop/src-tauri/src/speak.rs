//! Reads written answers aloud with the voice model (Gemini Live), so a typed request is answered by voice too;
//! the text stays in the chat. Voice conversations speak for themselves and are skipped. A new request or Stop
//! cuts the reading off. Settings > General > Voice can turn it off.

use crate::brain::Brain;
use crate::commands::{SettingsStore, current};
use crate::voice::VoiceSlot;
use anytask_audio::Speaker;
use anytask_core::CoreEvent;
use anytask_llm::live::OUTPUT_RATE;
use anytask_llm::{LiveConfig, LiveEvent};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

/// Long answers are read up to here (the full text is in the chat).
const MAX_CHARS: usize = 700;

const READ_ALOUD: &str = "You read text aloud. When the user sends text, say it exactly as written, word for word, in its own \
language, naturally and warmly. Do not answer it, comment on it, translate it, or add anything.";

/// The reading in progress, if any.
#[derive(Default)]
pub struct Speech(Mutex<Option<CancellationToken>>);

impl Speech {
    pub fn stop(&self) {
        if let Some(token) = self.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
            token.cancel();
        }
    }

    fn begin(&self) -> CancellationToken {
        self.stop();
        let token = CancellationToken::new();
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(token.clone());
        token
    }
}

/// Listens for answers and reads them out.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    let mut rx = app.state::<Brain>().agent.events().subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(CoreEvent::TurnStarted { .. }) => app.state::<Speech>().stop(),
                Ok(CoreEvent::Answer { turn_id, text }) => {
                    // A request from the phone is answered on the phone (in text, and by voice when it asked);
                    // the PC stays quiet: nobody may be sitting at it.
                    let from_phone = app.try_state::<std::sync::Arc<crate::phone::Phone>>().is_some_and(|p| p.is_phone_turn(turn_id));
                    if !from_phone && current(&app.state::<SettingsStore>()).voice.speak_answers {
                        say(&app, text);
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// Reads a text aloud now (an answer, a reminder), cutting off any reading in progress. Skipped while a voice
/// conversation is open: it speaks for itself and a second voice would talk over it.
pub fn say(app: &AppHandle, text: String) {
    say_with(app, text, false);
}

/// Says something the user must not miss (a reminder): when it cannot be spoken, Windows' notification sound plays.
pub fn say_important(app: &AppHandle, text: String) {
    say_with(app, text, true);
}

fn say_with(app: &AppHandle, text: String, important: bool) {
    if app.state::<VoiceSlot>().is_open() {
        return;
    }
    let cancel = app.state::<Speech>().begin();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = speak(&app, &text, cancel).await {
            app.state::<Brain>().agent.events().emit(CoreEvent::Log { turn_id: None, source: "speak".into(), text: format!("FAILED: {e}") });
            // A reminder that cannot be spoken still makes a sound (11-08: Gemini refused the location, and the
            // 03:04 reminder only showed on the island).
            if important {
                anytask_control::chime();
            }
        }
    });
}

async fn speak(app: &AppHandle, text: &str, cancel: CancellationToken) -> Result<(), String> {
    let text = speakable(text);
    if text.is_empty() {
        return Ok(());
    }
    let brain = app.state::<Brain>();
    let settings = current(&app.state::<SettingsStore>());
    let key = crate::voice::live_key(&brain, &settings)?;
    let config = LiveConfig { model: settings.voice.model.clone(), voice: settings.voice.voice.clone(), system: READ_ALOUD.into(), tools: Vec::new(), proxy: settings.proxy.voice_url() };
    let connected = tokio::select! {
        _ = cancel.cancelled() => return Ok(()),
        connected = crate::voice::connect_live(&config, &key, settings.proxy.url()) => connected,
    };
    let (session, mut events) = connected?;
    let speaker = Speaker::start()?;
    session.send_text(&text);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => match event {
                Some(LiveEvent::Audio(pcm)) => speaker.play(&pcm, OUTPUT_RATE),
                Some(LiveEvent::TurnComplete | LiveEvent::Closed(_)) | None => break,
                Some(_) => {}
            },
        }
    }
    while speaker.is_playing() && !cancel.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    speaker.clear();
    speaker.stop();
    session.close();
    Ok(())
}

/// Speaks `text` with the voice model into a WAV file's bytes (16-bit mono at the model's rate) instead of the
/// speakers: an answer sent to the phone, so the phone needs no VPN to hear the assistant.
pub async fn render(app: &AppHandle, text: &str) -> Result<Vec<u8>, String> {
    let text = speakable(text);
    if text.is_empty() {
        return Err("Nothing to say.".into());
    }
    let brain = app.state::<Brain>();
    let settings = current(&app.state::<SettingsStore>());
    let key = crate::voice::live_key(&brain, &settings)?;
    let config = LiveConfig { model: settings.voice.model.clone(), voice: settings.voice.voice.clone(), system: READ_ALOUD.into(), tools: Vec::new(), proxy: settings.proxy.voice_url() };
    let (session, mut events) = crate::voice::connect_live(&config, &key, settings.proxy.url()).await?;
    session.send_text(&text);
    let mut pcm: Vec<i16> = Vec::new();
    let collected = tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(event) = events.recv().await {
            match event {
                LiveEvent::Audio(chunk) => pcm.extend_from_slice(&chunk),
                LiveEvent::TurnComplete | LiveEvent::Closed(_) => break,
                _ => {}
            }
        }
    })
    .await;
    session.close();
    if collected.is_err() && pcm.is_empty() {
        return Err("The voice model did not answer in time.".into());
    }
    if pcm.is_empty() {
        return Err("The voice model gave no sound.".into());
    }
    Ok(wav(&pcm, OUTPUT_RATE))
}

/// A WAV file: 16-bit little-endian mono PCM.
fn wav(pcm: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Markdown and links out, length kept to a few sentences.
fn speakable(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches(['#', '>', '-', '*', '•']).trim();
        // [label](url) -> label; bare links are not read.
        let mut rest = line.to_string();
        while let (Some(open), Some(mid)) = (rest.find('['), rest.find("](")) {
            let Some(close) = rest[mid..].find(')').map(|c| mid + c) else { break };
            if open >= mid {
                break;
            }
            rest = format!("{}{}{}", &rest[..open], &rest[open + 1..mid], &rest[close + 1..]);
        }
        let words: Vec<&str> = rest.split_whitespace().filter(|w| !w.starts_with("http://") && !w.starts_with("https://")).collect();
        let clean = words.join(" ").replace(['*', '`', '_'], "");
        if !clean.is_empty() {
            out.push_str(&clean);
            out.push(' ');
        }
    }
    let out = out.trim().to_string();
    if out.chars().count() <= MAX_CHARS {
        return out;
    }
    let cut: String = out.chars().take(MAX_CHARS).collect();
    // End on a sentence when one ends in the last third.
    match cut.rfind(['.', '!', '?', '؟', '。']) {
        Some(at) if at > cut.len() * 2 / 3 => cut[..=at].to_string(),
        _ => format!("{cut}…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_links_and_long_text_are_made_speakable() {
        assert_eq!(speakable("## Price\n- **Dollar**: 100 [tgju](https://tgju.org)\nSee https://x.y"), "Price Dollar: 100 tgju See");
        let long = "Short sentence. ".repeat(80);
        let spoken = speakable(&long);
        assert!(spoken.chars().count() <= MAX_CHARS && spoken.ends_with('.'), "{spoken}");
        assert_eq!(speakable("   "), "");
    }

    #[test]
    fn a_wav_file_has_its_header_and_the_samples() {
        let file = wav(&[1, -2], 24_000);
        assert_eq!(&file[..4], b"RIFF");
        assert_eq!(&file[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(file[24..28].try_into().unwrap()), 24_000);
        assert_eq!(&file[36..40], b"data");
        assert_eq!(u32::from_le_bytes(file[40..44].try_into().unwrap()), 4);
        assert_eq!(&file[44..], &[1, 0, 0xFE, 0xFF]);
    }
}
