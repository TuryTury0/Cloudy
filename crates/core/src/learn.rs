//! Learning from use ("train mode"). No model is retrained; the assistant gets better because what happened before
//! is fed back into its prompts:
//!
//! - playbooks: the steps of desktop tasks that worked, offered again for a similar goal (fewer looks, no detours),
//! - experiences: which tools answered similar requests before, and whether it worked,
//! - lessons: one-line rules written after a failure or a correction from the user ("na, to app play kon"), shown
//!   whenever a request looks like the one that went wrong.
//!
//! Everything stays on this PC in learning.json; Settings > Learning shows it and can remove any of it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

const KEEP_PLAYBOOKS: usize = 300;
const KEEP_EXPERIENCES: usize = 500;
const KEEP_LESSONS: usize = 200;
/// How alike two requests must be (shared words / all words) to count as similar.
const SIMILAR: f64 = 0.34;
/// A new playbook replaces an old one for practically the same goal.
const SAME_GOAL: f64 = 0.8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playbook {
    pub goal: String,
    pub steps: Vec<String>,
    /// The window the task ended in.
    pub window: String,
    pub seconds: f32,
    pub uses: u32,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Experience {
    pub request: String,
    pub actions: Vec<String>,
    pub ok: bool,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lesson {
    pub id: u64,
    pub text: String,
    /// The request it came from (its words decide when the lesson is shown).
    pub about: String,
    pub uses: u32,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Data {
    enabled: bool,
    playbooks: Vec<Playbook>,
    experiences: Vec<Experience>,
    lessons: Vec<Lesson>,
    next_id: u64,
}

impl Default for Data {
    fn default() -> Self {
        Self { enabled: true, playbooks: Vec::new(), experiences: Vec::new(), lessons: Vec::new(), next_id: 1 }
    }
}

/// What Settings > Learning shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnSummary {
    pub enabled: bool,
    pub requests: usize,
    pub worked: usize,
    pub playbooks: Vec<Playbook>,
    pub lessons: Vec<Lesson>,
}

pub struct Learner {
    path: Option<PathBuf>,
    data: Mutex<Data>,
}

/// Words that carry no meaning for matching (English, Persian and Persian in Latin letters).
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "to", "in", "on", "and", "of", "for", "it", "me", "my", "please", "can", "you", "is", "with", "ro", "ra", "be", "ba", "az", "ke",
    "dar", "va", "baram", "kon", "bokon", "lotfan", "man", "to", "ye", "yek", "in", "un", "را", "به", "با", "از", "که", "در", "و", "برام", "کن",
    "بکن", "لطفا", "لطفاً", "من", "تو", "یه", "یک", "این", "اون", "آن",
];

/// The meaningful words of a text, lower case.
pub fn words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2 && !STOPWORDS.contains(w))
        .map(str::to_string)
        .collect()
}

/// Shared words / all words, 0..1.
pub fn similarity(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 { 0.0 } else { a.intersection(b).count() as f64 / union as f64 }
}

/// A follow-up that says the last answer was wrong ("na", "eshtebah", "dobare", "hanuz ... nakardi").
pub fn is_correction(text: &str) -> bool {
    const CUES: &[&str] = &[
        "na", "nah", "eshtebah", "eshtebahi", "dobare", "hanuz", "nakardi", "nashod", "wrong", "again", "didn't", "didnt", "not", "نه", "اشتباه", "دوباره",
        "هنوز", "نکردی", "نشد", "غلط",
    ];
    let lower = text.to_lowercase();
    let first = lower.split(|c: char| !c.is_alphanumeric() && c != '\'').find(|w| !w.is_empty()).unwrap_or("");
    CUES.contains(&first) || lower.split(|c: char| !c.is_alphanumeric() && c != '\'').any(|w| ["eshtebah", "nakardi", "nashod", "اشتباه", "نکردی", "نشد", "wrong"].contains(&w))
}

impl Learner {
    pub fn load(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { path: Some(path), data: Mutex::new(data) }
    }

    pub fn in_memory() -> Self {
        Self { path: None, data: Mutex::new(Data::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, data: &Data) {
        if let (Some(path), Ok(text)) = (&self.path, serde_json::to_string(data)) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    fn change(&self, edit: impl FnOnce(&mut Data)) {
        let mut data = self.lock();
        edit(&mut data);
        self.save(&data);
    }

    pub fn enabled(&self) -> bool {
        self.lock().enabled
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.change(|d| d.enabled = enabled);
    }

    /// A finished request: what was asked, what was done, whether it worked.
    pub fn record_request(&self, request: &str, actions: &[String], ok: bool, now: i64) {
        if !self.enabled() || request.trim().is_empty() {
            return;
        }
        self.change(|d| {
            d.experiences.push(Experience { request: clip(request, 300), actions: actions.iter().map(|a| clip(a, 120)).collect(), ok, at: now });
            let extra = d.experiences.len().saturating_sub(KEEP_EXPERIENCES);
            d.experiences.drain(..extra);
        });
    }

    /// A finished desktop task. One that worked becomes (or replaces) the playbook for that goal.
    pub fn record_desktop(&self, goal: &str, steps: &[String], ok: bool, window: &str, seconds: f32, now: i64) {
        // A run that ended in the assistant's own window did its work there by mistake (training round 3: the
        // Calculator sum typed into "AnyTask", then reported as done and reused as a playbook).
        if !self.enabled() || !ok || steps.is_empty() || window.trim().starts_with("AnyTask") {
            return;
        }
        let goal_words = words(goal);
        self.change(|d| {
            d.playbooks.retain(|p| similarity(&words(&p.goal), &goal_words) < SAME_GOAL);
            d.playbooks.push(Playbook { goal: clip(goal, 300), steps: steps.iter().take(30).map(|s| clip(s, 160)).collect(), window: clip(window, 120), seconds, uses: 0, at: now });
            let extra = d.playbooks.len().saturating_sub(KEEP_PLAYBOOKS);
            d.playbooks.drain(..extra);
        });
    }

    pub fn add_lesson(&self, text: &str, about: &str, now: i64) {
        let text = text.trim().trim_start_matches(['-', '*', ' ']).trim();
        if !self.enabled() || text.is_empty() || text.eq_ignore_ascii_case("none") {
            return;
        }
        self.change(|d| {
            // The same lesson again (in other words) is not stored twice.
            let new_words = words(text);
            if d.lessons.iter().any(|l| similarity(&words(&l.text), &new_words) >= SAME_GOAL) {
                return;
            }
            let id = d.next_id;
            d.next_id += 1;
            d.lessons.push(Lesson { id, text: clip(text, 300), about: clip(about, 300), uses: 0, at: now });
            let extra = d.lessons.len().saturating_sub(KEEP_LESSONS);
            d.lessons.drain(..extra);
        });
    }

    /// Lessons about requests like this one, most alike first.
    fn lessons_for(data: &mut Data, request: &BTreeSet<String>, n: usize) -> Vec<String> {
        let mut scored: Vec<(f64, usize)> = data.lessons.iter().enumerate().map(|(i, l)| (similarity(&words(&l.about), request).max(similarity(&words(&l.text), request)), i)).filter(|(s, _)| *s >= SIMILAR).collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
            .into_iter()
            .take(n)
            .map(|(_, i)| {
                data.lessons[i].uses += 1;
                format!("- {}", data.lessons[i].text)
            })
            .collect()
    }

    /// The newest lessons as lines ("- ..."), most recent first; empty when there are none.
    pub fn recent_lessons(&self, n: usize) -> String {
        let data = self.lock();
        data.lessons.iter().rev().take(n).map(|l| format!("- {}", l.text)).collect::<Vec<_>>().join("\n")
    }

    /// For the brain: lessons and how similar requests were handled before. Empty when nothing fits.
    pub fn request_hints(&self, request: &str) -> String {
        if !self.enabled() {
            return String::new();
        }
        let asked = words(request);
        let mut data = self.lock();
        let lessons = Self::lessons_for(&mut data, &asked, 3);
        let mut similar: Vec<(f64, &Experience)> = data.experiences.iter().filter(|e| e.ok && !e.actions.is_empty()).map(|e| (similarity(&words(&e.request), &asked), e)).filter(|(s, _)| *s >= SIMILAR).collect();
        similar.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.at.cmp(&a.1.at)));
        let mut seen = BTreeSet::new();
        let worked: Vec<String> = similar
            .into_iter()
            .filter(|(_, e)| seen.insert(e.actions.join(" | ")))
            .take(3)
            .map(|(_, e)| format!("- \"{}\" -> {} (worked)", clip(&e.request, 100), e.actions.join(", ")))
            .collect();
        let mut parts = Vec::new();
        if !lessons.is_empty() {
            parts.push(format!("Lessons from earlier mistakes:\n{}", lessons.join("\n")));
        }
        if !worked.is_empty() {
            parts.push(format!("Similar requests that worked before (reuse the approach, adapt the details):\n{}", worked.join("\n")));
        }
        let hints = parts.join("\n");
        if !lessons.is_empty() {
            self.save(&data);
        }
        hints
    }

    /// For the planner of a desktop task (never the executor): the closest playbook and lessons. Empty when nothing
    /// fits.
    pub fn desktop_hints(&self, goal: &str) -> String {
        if !self.enabled() {
            return String::new();
        }
        let wanted = words(goal);
        let mut data = self.lock();
        let lessons = Self::lessons_for(&mut data, &wanted, 2);
        let best = data.playbooks.iter().enumerate().map(|(i, p)| (similarity(&words(&p.goal), &wanted), i)).filter(|(s, _)| *s >= SIMILAR).max_by(|a, b| a.0.total_cmp(&b.0));
        let mut parts = Vec::new();
        if let Some((_, i)) = best {
            let p = &mut data.playbooks[i];
            p.uses += 1;
            parts.push(format!(
                "A similar goal worked before (\"{}\", {:.0} s, ended in \"{}\") with these steps; reuse the way when it fits, with this goal's names and values:\n{}",
                p.goal,
                p.seconds,
                p.window,
                general_steps(&p.steps).iter().enumerate().map(|(n, s)| format!("{}. {s}", n + 1)).collect::<Vec<_>>().join("\n")
            ));
        }
        if !lessons.is_empty() {
            parts.push(format!("Lessons from earlier mistakes:\n{}", lessons.join("\n")));
        }
        if !parts.is_empty() {
            self.save(&data);
        }
        parts.join("\n")
    }

    pub fn summary(&self) -> LearnSummary {
        let data = self.lock();
        let mut playbooks = data.playbooks.clone();
        playbooks.reverse();
        let mut lessons = data.lessons.clone();
        lessons.reverse();
        LearnSummary { enabled: data.enabled, requests: data.experiences.len(), worked: data.experiences.iter().filter(|e| e.ok).count(), playbooks, lessons }
    }

    pub fn forget_lesson(&self, id: u64) {
        self.change(|d| d.lessons.retain(|l| l.id != id));
    }

    pub fn forget_playbook(&self, goal: &str) {
        self.change(|d| d.playbooks.retain(|p| p.goal != goal));
    }

    /// Forgets everything learned (the on/off choice stays).
    pub fn clear(&self) {
        self.change(|d| {
            d.playbooks.clear();
            d.experiences.clear();
            d.lessons.clear();
        });
    }
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { format!("{}…", flat.chars().take(max).collect::<String>()) }
}

/// The prompt for writing a lesson after a failure or a correction.
pub fn lesson_prompt(request: &str, actions: &[String], result: &str, correction: Option<&str>) -> String {
    let done = if actions.is_empty() { "nothing (answered directly)".to_string() } else { actions.join(", ") };
    let mut text = format!(
        "An assistant on the user's Windows PC handled this request. Learn from it.\nRequest: {}\nWhat it did: {done}\nResult: {}\n",
        clip(request, 400),
        clip(result, 400)
    );
    if let Some(correction) = correction {
        text.push_str(&format!("The user then said: {} (so the result was not what they wanted)\n", clip(correction, 300)));
    }
    text.push_str(
        "Write ONE short lesson (at most 30 words, English) that would make the assistant do this kind of request right the first \
         time: concrete (which app, tool, site or step to use or avoid). If there is nothing useful to learn, reply exactly NONE.",
    );
    text
}

/// Steps as advice for a new plan. Runs saved before 11-12 clicked controls by number, and numbers differ on every
/// look ("double_click_id : 41" was whatever had number 41 that time); controls have no numbers now, so an old step
/// says to click the matching control by its name. A step repeated back to back (play/pause pressed again,
/// as in a saved Spotify run with click_id 41 three times) is given once.
fn general_steps(steps: &[String]) -> Vec<String> {
    const BY_NAME: &str = "\"<the matching control's name on the current screen>\"";
    let mut out: Vec<String> = Vec::new();
    for step in steps {
        let step = if let Some((action, rest)) = step.split_once("_id : ") {
            let after = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            format!("{action} : {BY_NAME}{after}")
        } else if let Some((action, rest)) = step.split_once(" : ").filter(|(a, r)| a.starts_with("type_into") && r.starts_with(|c: char| c.is_ascii_digit())) {
            format!("{action} : {BY_NAME}{}", rest.trim_start_matches(|c: char| c.is_ascii_digit()))
        } else {
            step.clone()
        };
        if out.last() != Some(&step) {
            out.push(step);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similar_requests_match_across_details() {
        let a = words("ahang Dirty Cash ro to Spotify play kon");
        let b = words("ye ahang shakira to spotify play kon");
        assert!(similarity(&a, &b) >= SIMILAR, "{}", similarity(&a, &b));
        assert!(similarity(&a, &words("gheimat dollar ro bego")) < SIMILAR);
        assert!(is_correction("na, to app play kon"));
        assert!(is_correction("hanuz play nakardi"));
        assert!(is_correction("اشتباه شد"));
        assert!(!is_correction("ahang play kon"));
    }

    #[test]
    fn playbooks_lessons_and_experiences_come_back_for_similar_requests() {
        let l = Learner::in_memory();
        l.record_desktop("play Dirty Cash in the Spotify app", &["open_url : spotify:search:Dirty Cash".into(), "double_click_id : 41".into()], true, "PAWSA - Dirty Cash", 12.0, 1);
        l.record_desktop("open notepad", &["launch_app : notepad".into()], false, "", 3.0, 2);
        l.record_desktop("calculate 123*45 in the calculator", &["type_enter : 123*45".into()], true, "AnyTask", 5.0, 2);
        assert!(l.summary().playbooks.iter().all(|p| !p.goal.contains("calculator")), "a run that ended in AnyTask's own window is no playbook");
        let hints = l.desktop_hints("play Shakira in the Spotify app");
        assert!(hints.contains("1. open_url : spotify:search:Dirty Cash"), "{hints}");
        assert!(hints.contains("2. double_click : \"<the matching control's name on the current screen>\"") && !hints.contains("41"), "{hints}");
        assert_eq!(general_steps(&["click_id : 41".into(), "click_id : 41".into(), "hotkey : space".into()]).len(), 2, "a repeat is given once");
        let old = general_steps(&["double_click_id : 41".into(), "type_into_enter : 3 : Believer".into(), "scroll_id : 12 : down : 4".into(), "click : \"Play\"".into()]);
        assert_eq!(
            old,
            [
                "double_click : \"<the matching control's name on the current screen>\"",
                "type_into_enter : \"<the matching control's name on the current screen>\" : Believer",
                "scroll : \"<the matching control's name on the current screen>\" : down : 4",
                "click : \"Play\"",
            ],
            "saved numbers become names"
        );
        assert_eq!(l.desktop_hints("open the calculator"), "");

        // The same goal again replaces the old playbook.
        l.record_desktop("play Dirty Cash in the Spotify app", &["launch_app : Spotify".into()], true, "Spotify", 9.0, 3);
        assert_eq!(l.summary().playbooks.len(), 1);
        assert_eq!(l.summary().playbooks[0].steps, vec!["launch_app : Spotify".to_string()]);

        l.record_request("gheimat dollar va tala ro dar biar", &["read_web_page: tgju.org".into()], true, 4);
        l.record_request("gheimat dollar chande", &["web_search: dollar".into()], false, 5);
        l.add_lesson("For Iranian prices read tgju.org directly; search results are stale.", "gheimat dollar tala", 6);
        l.add_lesson("For Iranian prices read tgju.org directly; search results are often stale.", "gheimat dollar", 7);
        let hints = l.request_hints("gheimat dollar ro dar biar");
        assert!(hints.contains("Lessons from earlier mistakes:\n- For Iranian prices read tgju.org"), "{hints}");
        assert!(hints.contains("read_web_page: tgju.org (worked)"), "{hints}");
        assert!(!hints.contains("web_search: dollar"), "failed attempts are not offered as examples");
        assert_eq!(l.summary().lessons.len(), 1, "a lesson said twice is kept once");
        assert_eq!(l.summary().lessons[0].uses, 1);
    }

    #[test]
    fn off_means_nothing_is_recorded_or_offered_and_it_survives_a_restart() {
        let path = std::env::temp_dir().join(format!("anytask-learn-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let l = Learner::load(path.clone());
        l.add_lesson("Use the Spotify app, not the web player.", "play music spotify", 1);
        l.set_enabled(false);
        l.add_lesson("ignored", "x", 2);
        assert_eq!(l.request_hints("play music in spotify"), "");
        let again = Learner::load(path.clone());
        assert!(!again.enabled());
        assert_eq!(again.summary().lessons.len(), 1);
        again.clear();
        assert!(again.summary().lessons.is_empty());
        let _ = std::fs::remove_file(path);
    }
}
