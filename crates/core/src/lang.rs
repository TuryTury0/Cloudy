//! The app's language (Settings > General) for texts the core shows the user: step labels, notices, questions.
//! English by default. What the model is told (prompts) is not translated here.

use std::sync::atomic::{AtomicBool, Ordering};

static PERSIAN: AtomicBool = AtomicBool::new(false);

/// Sets the language from the settings value ("fa" is Persian; anything else is English).
pub fn set(language: &str) {
    PERSIAN.store(language == "fa", Ordering::Relaxed);
}

/// Is the app in Persian?
pub fn persian() -> bool {
    PERSIAN.load(Ordering::Relaxed)
}

/// The text in the app's language.
pub fn tr<'a>(en: &'a str, fa: &'a str) -> &'a str {
    pick(persian(), en, fa)
}

fn pick<'a>(persian: bool, en: &'a str, fa: &'a str) -> &'a str {
    if persian { fa } else { en }
}

/// Does the text have Persian (Arabic script) letters?
pub fn has_persian(text: &str) -> bool {
    text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c))
}

/// A short text said for the user (not chosen by a model) is Persian when the app is, or when the user wrote in
/// Persian: an English "Done." in the history pulled later answers towards English (11-03 16:06).
pub fn for_user<'a>(user_text: &str, en: &'a str, fa: &'a str) -> &'a str {
    pick(persian() || has_persian(user_text), en, fa)
}

/// Common Finglish words (Persian written in Latin letters) that are not English words.
const FINGLISH: &[&str] = &[
    "ro", "kon", "bokon", "chie", "chetore", "chande", "chand", "bezar", "baram", "mikham", "bede", "begu", "bebad", "emruz", "farda",
    "dirooz", "hava", "gheimat", "ahang", "hast", "nist", "shod", "mishe", "khob", "lotfan", "salam", "merci", "yadet", "bashe", "bekhon",
    "benevis", "pakhsh", "dige", "kojast", "kodoom", "alan", "yadavari", "daghighe", "saat", "va", "ye", "un", "oon", "inja", "chera",
    "ham", "faghat", "mitooni", "mituni", "chi", "kardam", "kardi", "mikoni", "bara", "baraye", "roye", "toye", "esm", "akharin",
    "negah", "dar", "ahango", "ahangi", "bezan", "bebin", "beband", "biar", "bia", "boro", "konam", "mikonam", "chetori", "khoobi",
    "khubi", "mersi", "ziad", "seda", "sedasho", "tamum", "bas", "vaysa", "ghat", "ghatesh", "velesh", "baz", "bazesh", "yeki",
    "chejoori", "chikar", "begir", "bekon", "nemikham", "mikhay", "midooni", "dorost", "eshtebah", "hanuz", "nakon", "nafrest",
];

/// Common English words: a message in Latin letters counts as English only with one of them, so Finglish words that
/// are missing from the list above ("ahango negah dar") do not make an English answer.
const ENGLISH: &[&str] = &[
    "the", "a", "an", "is", "are", "what", "whats", "how", "when", "where", "who", "why", "which", "please", "to", "of", "for", "my",
    "me", "you", "it", "this", "that", "and", "with", "on", "in", "at", "from", "can", "could", "would", "will", "do", "does", "open",
    "play", "show", "set", "find", "tell", "today", "tomorrow", "now", "stop", "pause", "next", "close", "write", "read", "search",
];

/// The language a typed message is written in, for the answer: Some("English") or Some("Persian") when it is clear
/// from the words, None when it is in Persian letters (the model sees that anyway) or has no words.
pub fn typed_language(text: &str) -> Option<&'static str> {
    if has_persian(text) {
        return None;
    }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return None;
    }
    if words.iter().any(|w| FINGLISH.contains(w)) {
        Some("Persian")
    } else if words.iter().any(|w| ENGLISH.contains(w)) {
        Some("English")
    } else {
        None
    }
}

/// A typed message as the voice model gets it: with a reminder of the answer's language when that is clear, because
/// the model kept answering English questions in Persian after Persian pages and a Persian history (11-11).
pub fn with_answer_language(text: &str) -> String {
    match typed_language(text) {
        Some(language) => format!("{}\n(Reply in {language}.)", text.trim()),
        None => text.to_string(),
    }
}

/// "30" in the digits of the app's language.
pub fn digits(n: u64) -> String {
    if !persian() {
        return n.to_string();
    }
    n.to_string().chars().map(|c| char::from_u32('۰' as u32 + c.to_digit(10).unwrap_or(0)).unwrap_or(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_of_a_typed_message() {
        assert_eq!(typed_language("what's the price of the dollar today?"), Some("English"));
        assert_eq!(typed_language("play Believer on Spotify"), Some("English"));
        assert_eq!(typed_language("hava-ye esfahan farda chetore?"), Some("Persian"));
        assert_eq!(typed_language("ahang Believer ro play kon"), Some("Persian"));
        assert_eq!(typed_language("قیمت دلار چنده؟"), None, "Persian letters speak for themselves");
        assert_eq!(typed_language("123"), None);
        assert_eq!(typed_language("ahango negah dar"), Some("Persian"), "11-11: this got an English answer");
        assert_eq!(typed_language("Believer Imagine Dragons"), None, "names alone: no reminder either way");
        assert_eq!(with_answer_language("what time is it?"), "what time is it?\n(Reply in English.)");
    }

    #[test]
    fn picks_by_language() {
        assert_eq!(pick(false, "Stop", "توقف"), "Stop");
        assert_eq!(pick(true, "Stop", "توقف"), "توقف");
    }

    #[test]
    fn finds_persian_letters() {
        assert!(has_persian("آهنگ بذار"));
        assert!(has_persian("play آهنگ"));
        assert!(!has_persian("ahang bezar"));
        assert!(!has_persian("play Enemy 2"));
    }

    #[test]
    fn a_persian_request_gets_a_persian_text_in_an_english_app() {
        // The app's language is English unless set; tests never change it (it is shared by all of them).
        assert_eq!(for_user("آهنگ بذار", "Done.", "انجام شد."), "انجام شد.");
        assert_eq!(for_user("play a song", "Done.", "انجام شد."), "Done.");
    }
}
