//! The second check (Settings > Models): a decision model (Liquid's d1 family) looks again before the screen agent's
//! "done" is believed and before a click that is hard to undo. Optional and off by default; any number of decision
//! models can be added, one is used. A built-in test asks the model questions in English, Persian and Finglish and
//! about screenshots, so the user can see whether it understands before turning it on.

use crate::brain::Brain;
use crate::commands::{SettingsStore, apply_settings, current};
use crate::settings::{DecisionModel, Settings};
use anytask_core::{CoreEvent, Judge};
use anytask_web::Decider;
use anytask_web::decide::DecisionTarget;
use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Manager, State};

/// Name of a decision model's key in the keychain.
pub fn key_name(id: &str) -> String {
    format!("decision:{id}")
}

/// The screen agent's second opinion: the decision model chosen in Settings, when the second check is on.
pub struct DecisionJudge {
    pub app: AppHandle,
    pub decider: Arc<Decider>,
}

#[async_trait]
impl Judge for DecisionJudge {
    async fn yes_no(&self, state: &str, question: &str, jpeg: Option<&[u8]>) -> Option<f64> {
        let settings = current(self.app.try_state::<SettingsStore>()?.inner());
        let model = active(&settings)?;
        let brain = self.app.try_state::<Brain>()?;
        let log = |text: String| brain.agent.events().emit(CoreEvent::Log { turn_id: None, source: "second check".into(), text });
        let Some(key) = brain.secrets.get(&key_name(&model.id)) else {
            log(format!("{}: no key saved; skipped", model.name));
            return None;
        };
        let target = DecisionTarget { url: &model.base_url, model: &model.model_id, key: &key };
        match self.decider.yes_no(&target, state, question, jpeg).await {
            Ok(probability) => Some(probability),
            Err(problem) => {
                log(format!("{} could not answer, the job goes on without it: {problem}", model.name));
                None
            }
        }
    }
}

/// The decision model in use: the chosen one (or the first) while the second check is on.
fn active(settings: &Settings) -> Option<&DecisionModel> {
    let decision = &settings.decision;
    if !decision.enabled {
        return None;
    }
    decision.models.iter().find(|m| m.id == decision.active).or_else(|| decision.models.first())
}

fn validate(model: &DecisionModel) -> Result<(), String> {
    if model.name.trim().is_empty() || model.model_id.trim().is_empty() {
        return Err("A name and a model id are needed.".into());
    }
    if !model.base_url.trim().starts_with("https://") && !model.base_url.trim().starts_with("http://") {
        return Err("The address must start with https:// (or http:// for a model on this PC).".into());
    }
    Ok(())
}

/// Adds or updates a decision model (and its key, when given).
#[tauri::command]
pub fn save_decision_model(
    model: DecisionModel,
    api_key: Option<String>,
    app: AppHandle,
    store: State<'_, SettingsStore>,
    brain: State<'_, Brain>,
) -> Result<Settings, String> {
    validate(&model)?;
    let mut settings = current(&store);
    let mut model = DecisionModel {
        id: model.id.trim().to_string(),
        name: model.name.trim().to_string(),
        base_url: model.base_url.trim().trim_end_matches('/').to_string(),
        model_id: model.model_id.trim().to_string(),
    };
    if model.id.is_empty() {
        model.id = format!("d{:x}", chrono::Utc::now().timestamp_millis());
    }
    if let Some(key) = api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        brain.secrets.set(&key_name(&model.id), key).map_err(|e| format!("Could not save the key in the keychain: {e}"))?;
    }
    let decision = &mut settings.decision;
    match decision.models.iter_mut().find(|m| m.id == model.id) {
        Some(existing) => *existing = model.clone(),
        None => decision.models.push(model.clone()),
    }
    if decision.models.iter().all(|m| m.id != decision.active) {
        decision.active = model.id;
    }
    apply_settings(&app, &store, settings)
}

#[tauri::command]
pub fn delete_decision_model(id: String, app: AppHandle, store: State<'_, SettingsStore>, brain: State<'_, Brain>) -> Result<Settings, String> {
    let mut settings = current(&store);
    let decision = &mut settings.decision;
    decision.models.retain(|m| m.id != id);
    if decision.active == id {
        decision.active = decision.models.first().map(|m| m.id.clone()).unwrap_or_default();
    }
    if decision.models.is_empty() {
        decision.enabled = false;
    }
    let _ = brain.secrets.delete(&key_name(&id));
    apply_settings(&app, &store, settings)
}

/// Turns the second check on or off and picks the model it uses.
#[tauri::command]
pub fn set_decision(enabled: bool, active: String, app: AppHandle, store: State<'_, SettingsStore>) -> Result<Settings, String> {
    let mut settings = current(&store);
    if enabled && settings.decision.models.is_empty() {
        return Err("Add a decision model first.".into());
    }
    if !active.is_empty() && settings.decision.models.iter().all(|m| m.id != active) {
        return Err("That decision model no longer exists.".into());
    }
    settings.decision.enabled = enabled;
    if !active.is_empty() {
        settings.decision.active = active;
    }
    apply_settings(&app, &store, settings)
}

/// Which decision models have a key saved (the keys themselves never leave the keychain).
#[tauri::command]
pub fn decision_keys(store: State<'_, SettingsStore>, brain: State<'_, Brain>) -> Vec<String> {
    current(&store).decision.models.into_iter().filter(|m| brain.secrets.get(&key_name(&m.id)).is_some()).map(|m| m.id).collect()
}

// ---------- The built-in test ----------

/// One question of the test and what the model answered.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCase {
    /// "words" (what the user means), "clicks" (is a click hard to undo) or "screen" (a question about a screenshot).
    pub group: &'static str,
    /// "en", "fa", "fin" (Finglish) or "image".
    pub lang: &'static str,
    /// What was shown: the user's words, the click, or the screenshot's name.
    pub text: String,
    pub want: bool,
    /// The model's probability of yes; None when it did not answer.
    pub probability: Option<f64>,
    pub ok: bool,
    pub ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReport {
    pub cases: Vec<TestCase>,
    pub passed: usize,
    pub total: usize,
    /// Every screenshot question got an answer: the model takes images.
    pub images_work: bool,
    pub median_ms: u64,
    /// "good" (ready to turn on), "weak" (it answers, but too many mistakes) or "failed" (no answers at all).
    pub verdict: &'static str,
    /// Why nothing worked (a wrong key, no connection), when the verdict is "failed".
    pub error: Option<String>,
}

struct Question {
    group: &'static str,
    lang: &'static str,
    text: &'static str,
    state: String,
    question: &'static str,
    image: Option<&'static [u8]>,
    want: bool,
}

const STOP: &str = "Is the user asking the assistant to stop or cancel the job it is doing right now (and nothing else)?";
const WRONG: &str = "Is the user saying that what the assistant just did was wrong or not what they wanted?";
const RISKY: &str = "Could this click delete, send, buy, post, close without saving, or shut something down (something hard to undo)?";
const SONG: &str = "Is the song \"Believer\" playing right now (not paused)?";
const SAVED: &str = "Has the text been saved to a file named dollar.txt (no dialog still open)?";
const DELETE: &str = "Is the screen asking to confirm deleting a file?";

/// Synthetic screenshots (made by scripts/decision-test-images.mjs; no real screen content).
const PLAYING: &[u8] = include_bytes!("../decision-tests/music-believer-playing.jpg");
const PAUSED: &[u8] = include_bytes!("../decision-tests/music-believer-paused.jpg");
const SAVE_AS_OPEN: &[u8] = include_bytes!("../decision-tests/notepad-saveas-open.jpg");
const SAVED_FILE: &[u8] = include_bytes!("../decision-tests/notepad-saved.jpg");
const DELETE_FA: &[u8] = include_bytes!("../decision-tests/delete-confirm-fa.jpg");

fn questions() -> Vec<Question> {
    let words = |lang, text, question, want| Question {
        group: "words",
        lang,
        text,
        state: format!("The user said to the voice assistant: \"{text}\""),
        question,
        image: None,
        want,
    };
    let click = |lang, goal: &'static str, button: &'static str, want| Question {
        group: "clicks",
        lang,
        text: button,
        state: format!("A desktop agent is working on this goal for the user: {goal}\nIt is about to click the button named \"{button}\"."),
        question: RISKY,
        image: None,
        want,
    };
    let screen = |text, image, question, want| Question {
        group: "screen",
        lang: "image",
        text,
        state: "Screenshot of the computer screen after a desktop agent worked on a task.".into(),
        question,
        image: Some(image),
        want,
    };
    vec![
        words("en", "stop", STOP, true),
        words("en", "what is the weather today", STOP, false),
        words("en", "no, play it in the app not on the website", WRONG, true),
        words("en", "thanks, now open notepad", WRONG, false),
        words("fa", "قطعش کن", STOP, true),
        words("fa", "قیمت دلار امروز چنده", STOP, false),
        words("fa", "نه، توی اپ پخش کن", WRONG, true),
        words("fa", "مرسی عالی بود", WRONG, false),
        words("fin", "bas kon", STOP, true),
        words("fin", "hava chetore", STOP, false),
        words("fin", "na eshtebah shod", WRONG, true),
        words("fin", "merci kheili khob bod", WRONG, false),
        click("en", "Delete the old report file", "Delete", true),
        click("en", "Play Believer in Spotify", "Play", false),
        click("fa", "پیام رو برای علی بفرست", "ارسال", true),
        click("fa", "آهنگ Believer رو پخش کن", "پخش", false),
        click("fin", "file gozaresh ro pak kon", "Delete", true),
        click("fin", "ahang Believer ro play kon", "Play", false),
        screen("Spotify: Believer playing", PLAYING, SONG, true),
        screen("Spotify: Believer paused", PAUSED, SONG, false),
        screen("Notepad: Save as still open", SAVE_AS_OPEN, SAVED, false),
        screen("Notepad: dollar.txt saved", SAVED_FILE, SAVED, true),
        screen("A Persian delete question", DELETE_FA, DELETE, true),
        screen("Spotify: Believer playing", PLAYING, DELETE, false),
    ]
}

/// Asks every test question (four at a time) and sums up.
pub async fn run_test(decider: Arc<Decider>, url: String, model: String, key: String) -> TestReport {
    let mut cases = Vec::new();
    let all = questions();
    for chunk in all.chunks(4) {
        let mut running = Vec::new();
        for q in chunk {
            let (decider, url, model, key) = (decider.clone(), url.clone(), model.clone(), key.clone());
            let (state, question, image) = (q.state.clone(), q.question, q.image);
            running.push(tauri::async_runtime::spawn(async move {
                let started = Instant::now();
                let target = DecisionTarget { url: &url, model: &model, key: &key };
                let answer = decider.yes_no(&target, &state, question, image).await;
                (answer, started.elapsed().as_millis() as u64)
            }));
        }
        for (q, handle) in chunk.iter().zip(running) {
            let (answer, ms) = handle.await.unwrap_or_else(|e| (Err(e.to_string()), 0));
            let (probability, error) = match answer {
                Ok(p) => (Some(p), None),
                Err(e) => (None, Some(e)),
            };
            let ok = probability.is_some_and(|p| (p >= 0.5) == q.want);
            cases.push(TestCase { group: q.group, lang: q.lang, text: q.text.into(), want: q.want, probability, ok, ms, error });
        }
        // A wrong key or no way out fails every question the same way: no need to ask the rest.
        if cases.iter().all(|c| c.probability.is_none()) {
            break;
        }
    }
    summarize(cases, all.len())
}

fn summarize(cases: Vec<TestCase>, total: usize) -> TestReport {
    let passed = cases.iter().filter(|c| c.ok).count();
    let answered: Vec<&TestCase> = cases.iter().filter(|c| c.probability.is_some()).collect();
    let screen: Vec<&TestCase> = cases.iter().filter(|c| c.group == "screen").collect();
    let images_work = !screen.is_empty() && screen.iter().all(|c| c.probability.is_some());
    let mut times: Vec<u64> = answered.iter().map(|c| c.ms).collect();
    times.sort_unstable();
    let median_ms = times.get(times.len() / 2).copied().unwrap_or(0);
    let error = if answered.is_empty() { cases.iter().find_map(|c| c.error.clone()) } else { None };
    // Good: it takes images, gets every screenshot right (that is what the second check looks at) and at least 85%
    // of everything. Finglish is the hardest part; a miss there alone does not make it weak.
    let verdict = if answered.is_empty() {
        "failed"
    } else if images_work && screen.iter().all(|c| c.ok) && passed * 100 >= total * 85 {
        "good"
    } else {
        "weak"
    };
    TestReport { cases, passed, total, images_work, median_ms, verdict, error }
}

/// Runs the built-in test on a decision model (with the key typed in the form, or the saved one).
#[tauri::command]
pub async fn test_decision_model(model: DecisionModel, api_key: Option<String>, brain: State<'_, Brain>) -> Result<TestReport, String> {
    validate(&model)?;
    let key = api_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| brain.secrets.get(&key_name(&model.id)))
        .ok_or("Paste the model's API key first.")?;
    let decider = Arc::new(Decider::new(brain.net.clone()));
    let report = run_test(decider, model.base_url.trim().trim_end_matches('/').to_string(), model.model_id.trim().to_string(), key).await;
    let summary = match &report.error {
        Some(error) => format!("test {} ({}) FAILED: {error}", model.name, model.model_id),
        None => format!(
            "test {} ({}): {}/{} right, images {}, median {} ms, {}",
            model.name,
            model.model_id,
            report.passed,
            report.total,
            if report.images_work { "work" } else { "do NOT work" },
            report.median_ms,
            report.verdict
        ),
    };
    brain.agent.events().emit(CoreEvent::Log { turn_id: None, source: "second check".into(), text: summary });
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(group: &'static str, ok: bool, answered: bool) -> TestCase {
        TestCase {
            group,
            lang: "en",
            text: String::new(),
            want: true,
            probability: answered.then_some(if ok { 0.9 } else { 0.1 }),
            ok,
            ms: 1000,
            error: (!answered).then(|| "HTTP 401: Invalid API key provided.".into()),
        }
    }

    #[test]
    fn the_test_covers_three_languages_and_screenshots() {
        let all = questions();
        for lang in ["en", "fa", "fin", "image"] {
            assert!(all.iter().filter(|q| q.lang == lang).count() >= 6, "{lang}");
        }
        assert!(all.iter().filter(|q| q.image.is_some()).all(|q| q.image.unwrap().starts_with(&[0xFF, 0xD8])), "real JPEGs");
        assert!(all.iter().any(|q| q.want) && all.iter().any(|q| !q.want), "yes and no answers both count");
    }

    /// The whole test against the real service, through the local VPN proxy. Costs a fraction of a cent.
    #[test]
    #[ignore = "calls the real service: set LK=<key>, then cargo test -p anytask-desktop --lib real_service -- --ignored --nocapture"]
    fn real_service() {
        let Ok(key) = std::env::var("LK") else { return };
        let net = anytask_web::Net::new();
        net.set_proxy(anytask_llm::proxy::proxy_url("127.0.0.1:10808").as_deref()).unwrap();
        let decider = Arc::new(Decider::new(net));
        let url = anytask_web::decide::DECISION_URL.to_string();
        let report = tauri::async_runtime::block_on(run_test(decider, url, anytask_web::decide::DECISION_MODEL.into(), key));
        for c in &report.cases {
            println!("{} {:>5} {:<6} {:<45} want {:<5} got {:?} {} ms {}", if c.ok { "ok " } else { "BAD" }, c.group, c.lang, c.text, c.want, c.probability.map(|p| (p * 100.0).round()), c.ms, c.error.as_deref().unwrap_or(""));
        }
        println!("{}/{} right, images work: {}, median {} ms, verdict {}", report.passed, report.total, report.images_work, report.median_ms, report.verdict);
        assert!(report.images_work, "{report:?}");
    }

    #[test]
    fn the_verdict() {
        let mut all: Vec<TestCase> = (0..18).map(|_| case("words", true, true)).chain((0..6).map(|_| case("screen", true, true))).collect();
        assert_eq!(summarize(all.clone(), 24).verdict, "good");
        all[0] = case("words", false, true);
        all[1] = case("words", false, true);
        assert_eq!(summarize(all.clone(), 24).verdict, "good", "22/24 is enough");
        all[23] = case("screen", false, true);
        assert_eq!(summarize(all.clone(), 24).verdict, "weak", "a wrong screenshot answer is not");
        let no_images: Vec<TestCase> = (0..18).map(|_| case("words", true, true)).chain((0..6).map(|_| case("screen", false, false))).collect();
        let report = summarize(no_images, 24);
        assert!(!report.images_work);
        assert_eq!(report.verdict, "weak");
        let nothing = summarize(vec![case("words", false, false)], 24);
        assert_eq!(nothing.verdict, "failed");
        assert_eq!(nothing.error.as_deref(), Some("HTTP 401: Invalid API key provided."));
    }
}
