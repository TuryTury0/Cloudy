//! App tips ("skills"): short notes about one app or site, given to the screen agent only in jobs about that app,
//! so they cost tokens only where they help. A few come built in (crates/core/skills/*.md); the user can add their
//! own as Markdown files in the app's skills folder, and one with the same name replaces a built-in one.
//!
//! A skill file starts with a line naming the words that bring it in, then the tips:
//!
//! ```text
//! keys: telegram, تلگرام
//! # Telegram
//! - In Telegram, Enter sends the message.
//! ```
//!
//! The words are looked for in the job's goal and in the title of the window in front.

use serde::Serialize;
use std::path::PathBuf;

/// More built-in skills in one file (about a hundred apps and sites), separated by lines of "---"; each is named
/// after its "# Title" (Google Maps -> google-maps).
const CATALOG: &str = include_str!("../skills/catalog.md");

const BUILTIN: &[(&str, &str)] = &[
    ("browser", include_str!("../skills/browser.md")),
    ("youtube", include_str!("../skills/youtube.md")),
    ("explorer", include_str!("../skills/explorer.md")),
    ("settings", include_str!("../skills/settings.md")),
    ("chat-apps", include_str!("../skills/chat-apps.md")),
    ("notepad", include_str!("../skills/notepad.md")),
];

/// At most this many skills join one job, the best matches first.
const MAX_SKILLS: usize = 3;
/// And at most this much text, so a long user file cannot crowd out the screen.
const MAX_CHARS: usize = 2500;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skill {
    pub name: String,
    pub keys: Vec<String>,
    pub text: String,
    pub builtin: bool,
}

pub struct Skills {
    /// The user's own skills folder; None: built-in skills only.
    dir: Option<PathBuf>,
}

impl Skills {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> Option<&PathBuf> {
        self.dir.as_ref()
    }

    /// Built-in skills and the user's (read on every call, so an edited file counts in the next job).
    pub fn all(&self) -> Vec<Skill> {
        let mut skills: Vec<Skill> = BUILTIN.iter().filter_map(|(name, text)| parse(name, text, true)).collect();
        skills.extend(catalog());
        let Some(dir) = &self.dir else { return skills };
        let Ok(entries) = std::fs::read_dir(dir) else { return skills };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))).collect();
        files.sort();
        for path in files {
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let Some(skill) = std::fs::read_to_string(&path).ok().and_then(|text| parse(&name, &text, false)) else { continue };
            skills.retain(|s| s.name != skill.name);
            skills.push(skill);
        }
        skills
    }

    /// The tips for a job: the skills whose words appear in the goal or the window title.
    pub fn for_job(&self, goal: &str, title: &str) -> String {
        pick(&self.all(), goal, title)
    }
}

fn pick(skills: &[Skill], goal: &str, title: &str) -> String {
    let haystack = format!("{} {}", goal.to_lowercase(), title.to_lowercase());
    // The longest matching word counts most ("file explorer" before "explorer"); a skill matched in the window title
    // comes first because that app is on screen now.
    let mut matched: Vec<(usize, bool, &Skill)> = skills
        .iter()
        .filter_map(|s| {
            let best = s.keys.iter().filter(|k| contains_word(&haystack, k)).map(|k| k.chars().count()).max()?;
            let on_screen = s.keys.iter().any(|k| contains_word(&title.to_lowercase(), k));
            Some((best, on_screen, s))
        })
        .collect();
    matched.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    let mut out = String::new();
    for (_, _, skill) in matched.into_iter().take(MAX_SKILLS) {
        if out.chars().count() + skill.text.chars().count() > MAX_CHARS {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&skill.text);
    }
    out
}

/// A key is found as a whole word: "edge" must not match "knowledge". Persian keys match anywhere (words there take
/// attached suffixes such as ‌ها).
fn contains_word(haystack: &str, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    if !key.is_ascii() {
        return haystack.contains(key);
    }
    haystack.match_indices(key).any(|(i, _)| {
        let before = haystack[..i].chars().next_back();
        let after = haystack[i + key.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphanumeric())
    })
}

/// The catalog's skills, named after their titles.
fn catalog() -> Vec<Skill> {
    CATALOG
        .split("\n---")
        .filter_map(|block| {
            let title = block.lines().find_map(|l| l.strip_prefix("# "))?;
            let name: String = title
                .to_lowercase()
                .replace('+', "-plus")
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '-' })
                .collect::<String>()
                .split('-')
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("-");
            parse(&name, block, true)
        })
        .collect()
}

/// A skill file: "keys: a, b" first (other blank or # lines may come before it), then the tips.
fn parse(name: &str, text: &str, builtin: bool) -> Option<Skill> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    let keys_line = lines.by_ref().find(|l| !l.trim().is_empty())?;
    let (head, keys) = keys_line.split_once(':')?;
    if !head.trim().eq_ignore_ascii_case("keys") {
        return None;
    }
    let keys: Vec<String> = keys.split([',', '،']).map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty()).collect();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    (!keys.is_empty() && !body.is_empty()).then(|| Skill { name: name.to_string(), keys, text: body, builtin })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_skills_all_parse() {
        let all = Skills::new(None).all();
        let blocks = CATALOG.split("\n---").filter(|b| !b.trim().is_empty()).count();
        assert_eq!(all.len(), BUILTIN.len() + blocks, "every built-in skill starts with a keys line");
        assert!(all.len() >= 100, "about a hundred apps and sites: {}", all.len());
        assert!(all.iter().all(|s| s.builtin && s.text.starts_with('#')));
        let mut names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "names are unique");
        assert!(all.iter().any(|s| s.name == "google-maps"));
    }

    #[test]
    fn a_job_gets_only_the_skills_of_its_apps() {
        let skills = Skills::new(None);
        let telegram = skills.for_job("Open Telegram and write hi to Ali, do not send", "");
        assert!(telegram.contains("Enter SENDS"), "{telegram}");
        assert!(!telegram.contains("YouTube"));
        assert!(skills.for_job("پیام رو تو تلگرام بنویس", "").contains("Telegram"), "Persian words count");
        assert!(skills.for_job("write a note", "dollar.txt - Notepad").contains("Notepad"), "the window title counts");
        assert!(skills.for_job("Calculate 2+2", "Calculator").starts_with("# Calculator"), "the window in front brings its app's tips");
        assert_eq!(skills.for_job("Write hello", "Untitled - Something"), "", "no app named, no extra text");
        assert!(skills.for_job("ye mahsool to digikala peyda kon", "").contains("# Digikala"), "Finglish app names count");
        assert!(!skills.for_job("اجازه بده وارد شوم", "").contains("Edge"), "a Persian key must not hide inside another word");
        assert!(!skills.for_job("show my knowledge base", "").contains("Web browser"), "whole words only: edge is not in knowledge");
    }

    #[test]
    fn user_skills_are_read_and_replace_built_in_ones() {
        let dir = std::env::temp_dir().join(format!("anytask-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("spotify.md"), "keys: spotify, اسپاتیفای\n# Spotify\n- Liked Songs is in Your Library.").unwrap();
        std::fs::write(dir.join("notepad.md"), "keys: notepad\n# My Notepad\n- Use tab 2.").unwrap();
        std::fs::write(dir.join("README.md"), "How to write skills").unwrap();
        let skills = Skills::new(Some(dir.clone()));
        let all = skills.all();
        assert!(all.iter().any(|s| s.name == "spotify" && !s.builtin));
        assert_eq!(all.iter().filter(|s| s.name == "notepad").count(), 1, "the user's notepad replaces the built-in one");
        assert!(!all.iter().any(|s| s.name == "README"), "a file without a keys line is not a skill");
        assert!(skills.for_job("play a song in Spotify", "").contains("Liked Songs"));
        assert!(skills.for_job("write", "x - Notepad").contains("Use tab 2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn at_most_three_skills_and_the_one_on_screen_first() {
        let all = Skills::new(None).all();
        let text = pick(&all, "open youtube in chrome, then telegram, then notepad and settings", "Telegram");
        assert!(text.starts_with("# Telegram"), "{text}");
        assert_eq!(text.matches("\n# ").count() + 1, 3);
    }
}
