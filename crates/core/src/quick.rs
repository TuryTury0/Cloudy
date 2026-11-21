//! Common short commands done at once, without asking the model: pause, next song, what is playing, volume, a
//! screenshot to the phone, play some music, open an app. Each of these took two model calls before (one to pick the
//! tool, one to word the answer: 6,000-12,000 tokens and 2-4 s) for a request whose tool and answer are fixed. Only
//! whole messages that match a known phrase are taken; anything else, or a tool that fails, goes to the model as
//! before.

use serde_json::{Value, json};

/// A command recognised without the model.
#[derive(Debug, Clone, PartialEq)]
pub struct Quick {
    pub tool: &'static str,
    pub args: Value,
    /// Answer in Persian (the message was Persian or Finglish).
    pub persian: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Pause,
    Play,
    Next,
    Previous,
    Status,
    Louder,
    Quieter,
    Mute,
    Unmute,
    Time,
    Date,
}

/// The clock and calendar need no tool: answered from the PC's time.
pub const CLOCK: &str = "clock";

/// Whole messages (after `normalize`) and what they ask. Finglish and Persian answer in Persian, English in English.
const PHRASES: &[(&str, Kind)] = &[
    // pause
    ("pause", Kind::Pause),
    ("pause kon", Kind::Pause),
    ("ahang ro pause kon", Kind::Pause),
    ("ahang ro negah dar", Kind::Pause),
    ("ahang ro ghat kon", Kind::Pause),
    ("ahang ro stop kon", Kind::Pause),
    ("pause the music", Kind::Pause),
    ("pause music", Kind::Pause),
    ("pause the song", Kind::Pause),
    ("stop the music", Kind::Pause),
    ("stop music", Kind::Pause),
    ("مکث", Kind::Pause),
    ("پاز کن", Kind::Pause),
    ("آهنگ رو پاز کن", Kind::Pause),
    ("آهنگ رو قطع کن", Kind::Pause),
    ("آهنگ رو نگه دار", Kind::Pause),
    ("آهنگ رو متوقف کن", Kind::Pause),
    // play something (no song named)
    ("play kon", Kind::Play),
    ("ahang play kon", Kind::Play),
    ("ye ahang play kon", Kind::Play),
    ("ye ahang bezar", Kind::Play),
    ("ahang bezar", Kind::Play),
    ("ye ahang pakhsh kon", Kind::Play),
    ("ahang pakhsh kon", Kind::Play),
    ("dobare play kon", Kind::Play),
    ("ahang ro play kon", Kind::Play),
    ("spotify baz kon ye ahang play kon", Kind::Play),
    ("spotify ro baz kon ye ahang play kon", Kind::Play),
    ("spotify baz kon ye ahang bezar", Kind::Play),
    ("spotify ro baz kon ye ahang bezar", Kind::Play),
    ("spotify ro baz kon va ye ahang play kon", Kind::Play),
    ("spotify baz kon va ye ahang play kon", Kind::Play),
    ("play music", Kind::Play),
    ("play some music", Kind::Play),
    ("play a song", Kind::Play),
    ("play something", Kind::Play),
    ("resume", Kind::Play),
    ("resume the music", Kind::Play),
    ("resume music", Kind::Play),
    ("open spotify and play a song", Kind::Play),
    ("open spotify and play some music", Kind::Play),
    ("یه آهنگ پخش کن", Kind::Play),
    ("آهنگ پخش کن", Kind::Play),
    ("یه آهنگ بذار", Kind::Play),
    ("آهنگ بذار", Kind::Play),
    ("یه آهنگ پلی کن", Kind::Play),
    ("آهنگ رو پخش کن", Kind::Play),
    ("دوباره پخش کن", Kind::Play),
    ("اسپاتیفای رو باز کن یه آهنگ پخش کن", Kind::Play),
    ("اسپاتیفای رو باز کن و یه آهنگ پخش کن", Kind::Play),
    ("اسپاتیفای رو باز کن یه آهنگ بذار", Kind::Play),
    // next / previous
    ("next", Kind::Next),
    ("next kon", Kind::Next),
    ("badi", Kind::Next),
    ("ahang badi", Kind::Next),
    ("ahang badi ro bezar", Kind::Next),
    ("ahang badi ro play kon", Kind::Next),
    ("boro ahang badi", Kind::Next),
    ("next song", Kind::Next),
    ("skip", Kind::Next),
    ("skip this song", Kind::Next),
    ("play the next song", Kind::Next),
    ("بعدی", Kind::Next),
    ("آهنگ بعدی", Kind::Next),
    ("آهنگ بعدی رو بذار", Kind::Next),
    ("برو آهنگ بعدی", Kind::Next),
    ("ghabli", Kind::Previous),
    ("ahang ghabli", Kind::Previous),
    ("ahang ghabli ro bezar", Kind::Previous),
    ("previous", Kind::Previous),
    ("previous song", Kind::Previous),
    ("play the previous song", Kind::Previous),
    ("قبلی", Kind::Previous),
    ("آهنگ قبلی", Kind::Previous),
    ("آهنگ قبلی رو بذار", Kind::Previous),
    // what is playing
    ("chi dare play mishe", Kind::Status),
    ("chi dare pakhsh mishe", Kind::Status),
    ("alan chi dare play mishe", Kind::Status),
    ("che ahangi dare play mishe", Kind::Status),
    ("che ahangi dare pakhsh mishe", Kind::Status),
    ("in ahang chie", Kind::Status),
    ("in chi ahangie", Kind::Status),
    ("esm in ahang chie", Kind::Status),
    ("esme in ahang chie", Kind::Status),
    ("what is playing", Kind::Status),
    ("whats playing", Kind::Status),
    ("what song is this", Kind::Status),
    ("whats this song", Kind::Status),
    ("what is this song", Kind::Status),
    ("چی داره پخش میشه", Kind::Status),
    ("چی داره پخش می شه", Kind::Status),
    ("الان چی داره پخش میشه", Kind::Status),
    ("چه آهنگی داره پخش میشه", Kind::Status),
    ("این آهنگ چیه", Kind::Status),
    ("اسم این آهنگ چیه", Kind::Status),
    // volume
    ("seda ro ziad kon", Kind::Louder),
    ("seda ro bishtar kon", Kind::Louder),
    ("seda ro bala bebar", Kind::Louder),
    ("seda ro boland kon", Kind::Louder),
    ("volume up", Kind::Louder),
    ("louder", Kind::Louder),
    ("turn it up", Kind::Louder),
    ("turn the volume up", Kind::Louder),
    ("صدا رو زیاد کن", Kind::Louder),
    ("صدا رو بلند کن", Kind::Louder),
    ("صدا رو بیشتر کن", Kind::Louder),
    ("seda ro kam kon", Kind::Quieter),
    ("seda ro paeen bebar", Kind::Quieter),
    ("seda ro payin bebar", Kind::Quieter),
    ("volume down", Kind::Quieter),
    ("quieter", Kind::Quieter),
    ("turn it down", Kind::Quieter),
    ("turn the volume down", Kind::Quieter),
    ("صدا رو کم کن", Kind::Quieter),
    ("صدا رو پایین بیار", Kind::Quieter),
    ("mute", Kind::Mute),
    ("mute kon", Kind::Mute),
    ("bi seda kon", Kind::Mute),
    ("bisedash kon", Kind::Mute),
    ("بی صدا کن", Kind::Mute),
    ("میوت کن", Kind::Mute),
    ("unmute", Kind::Unmute),
    ("unmute kon", Kind::Unmute),
    ("seda ro vasl kon", Kind::Unmute),
    ("seda ro bargardoon", Kind::Unmute),
    ("صدا رو وصل کن", Kind::Unmute),
    ("صدا رو برگردون", Kind::Unmute),
    // the clock and the date
    ("saat chande", Kind::Time),
    ("saat chand e", Kind::Time),
    ("alan saat chande", Kind::Time),
    ("saat chand ast", Kind::Time),
    ("what time is it", Kind::Time),
    ("whats the time", Kind::Time),
    ("what is the time", Kind::Time),
    ("ساعت چنده", Kind::Time),
    ("الان ساعت چنده", Kind::Time),
    ("ساعت چند است", Kind::Time),
    ("emruz chandome", Kind::Date),
    ("emrooz chandome", Kind::Date),
    ("tarikh emruz chie", Kind::Date),
    ("tarikh emrooz chie", Kind::Date),
    ("emruz chand shanbas", Kind::Date),
    ("emrooz chand shanbas", Kind::Date),
    ("whats the date", Kind::Date),
    ("what is the date today", Kind::Date),
    ("whats the date today", Kind::Date),
    ("what day is it", Kind::Date),
    ("what day is it today", Kind::Date),
    ("امروز چندمه", Kind::Date),
    ("تاریخ امروز چیه", Kind::Date),
    ("امروز چند شنبه است", Kind::Date),
    ("امروز چندشنبه است", Kind::Date),
    ("امروز چه روزیه", Kind::Date),
];

/// Words that only make a request polite, at either end ("lotfan pause kon mersi").
const POLITE: &[&str] = &["lotfan", "please", "pls", "plz", "mersi", "merci", "mamnoon", "thanks", "لطفا", "لطفاً", "مرسی", "ممنون"];

/// Different spellings of the same word, so the table needs one form (Finglish has no fixed spelling).
fn canonical(word: &str) -> &str {
    match word {
        "roo" | "ru" | "rro" | "ra" => "ro",
        "bokon" | "bekon" | "kn" => "kon",
        "music" | "mosighi" | "mosiqi" | "musighi" | "ahange" => "ahang",
        "bzar" | "bezaar" | "bezarid" => "bezar",
        "pley" | "plai" => "play",
        "spotifay" | "spatify" | "espotify" => "spotify",
        "paz" => "pause",
        "zyad" | "ziyad" => "ziad",
        "اهنگ" => "آهنگ",
        "بزار" => "بذار",
        "را" => "رو",
        "اسپاتیفای" | "اسپاتیفاي" => "اسپاتیفای",
        _ => word,
    }
}

/// Lower case, Persian letters in one form, no punctuation, polite words and fused "ro" split off
/// ("ahango" -> "ahang ro", "آهنگو" -> "آهنگ رو", "sedasho" -> "seda ro").
pub fn normalize(text: &str) -> String {
    let lower = text.to_lowercase().replace(['\u{200c}', '\u{200f}', '\u{200e}'], " ").replace('ي', "ی").replace('ك', "ک").replace('ة', "ه");
    // "what's" -> "whats"; other marks split words.
    let cleaned: String = lower.chars().filter(|c| !matches!(c, '\'' | '’')).map(|c| if c.is_alphanumeric() || c == ' ' { c } else { ' ' }).collect();
    let mut words: Vec<String> = Vec::new();
    for word in cleaned.split_whitespace() {
        let fused = [("ahango", "ahang"), ("sedasho", "seda"), ("sedaro", "seda"), ("ahangaro", "ahang")]
            .iter()
            .find(|(w, _)| *w == word)
            .map(|(_, stem)| *stem)
            .or(match word {
                "آهنگو" => Some("آهنگ"),
                "صداشو" | "صدارو" => Some("صدا"),
                _ => None,
            });
        match fused {
            Some(stem) => {
                words.push(stem.into());
                words.push(if crate::lang::has_persian(stem) { "رو".into() } else { "ro".into() });
            }
            None => words.push(canonical(word).to_string()),
        }
    }
    while words.first().is_some_and(|w| POLITE.contains(&w.as_str())) {
        words.remove(0);
    }
    while words.last().is_some_and(|w| POLITE.contains(&w.as_str())) {
        words.pop();
    }
    words.join(" ")
}

/// Whether to answer in Persian: the message is in Persian letters or Finglish. A word both languages use ("pause",
/// "next") and anything unclear follow the app's language.
fn persian_answer(message: &str) -> bool {
    if crate::lang::has_persian(message) {
        return true;
    }
    match crate::lang::typed_language(message) {
        Some("Persian") => true,
        Some("English") if !shared(message) => false,
        _ => crate::lang::persian(),
    }
}

/// One or two words Finglish speakers use as they are ("pause", "next song").
fn shared(message: &str) -> bool {
    const SHARED: &[&str] = &["pause", "next", "skip", "mute", "unmute", "resume", "previous", "play", "stop", "song", "music", "volume", "up", "down"];
    let lower = message.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    words.len() <= 2 && words.iter().all(|w| SHARED.contains(w))
}

/// The level of "seda ro bezar ru 30", "volume 40", "صدا رو بذار رو ۳۰".
fn volume_level(text: &str) -> Option<u8> {
    let words: Vec<&str> = text.split(' ').collect();
    if !matches!(words.first(), Some(&("seda" | "volume" | "صدا" | "ولوم"))) || words.len() > 7 {
        return None;
    }
    let numbers: Vec<u32> = words
        .iter()
        .filter_map(|w| {
            let digits: String = w
                .chars()
                .map(|c| match c {
                    '۰'..='۹' => char::from_u32(c as u32 - 0x06f0 + '0' as u32).unwrap_or(c),
                    '٠'..='٩' => char::from_u32(c as u32 - 0x0660 + '0' as u32).unwrap_or(c),
                    _ => c,
                })
                .collect();
            digits.parse().ok()
        })
        .collect();
    let allowed = ["seda", "volume", "ro", "bezar", "kon", "ru", "roye", "rooye", "ruye", "dar", "sad", "darsad", "set", "to", "صدا", "ولوم", "رو", "بذار", "کن", "روی", "درصد", "set"];
    let only_volume_words = words.iter().all(|w| allowed.contains(w) || w.chars().all(|c| c.is_numeric()));
    match numbers.as_slice() {
        [n] if *n <= 100 && only_volume_words => Some(*n as u8),
        _ => None,
    }
}

/// "screenshot begir befrest baraye goshim", "یه عکس از صفحه بگیر بفرست برای گوشیم".
fn screenshot(text: &str) -> Option<bool> {
    let words: Vec<&str> = text.split(' ').collect();
    let has = |list: &[&str]| words.iter().any(|w| list.iter().any(|l| w.starts_with(l)));
    let shot = has(&["screenshot", "eskrin", "اسکرین"]) || (has(&["aks", "عکس"]) && has(&["safe", "safhe", "safam", "screen", "monitor", "desktop", "صفحه", "دسکتاپ", "مانیتور"]));
    // Taking one, not opening, finding or deleting one ("screenshot-am kojast?").
    let take = has(&["begir", "bgir", "bendaz", "take", "capture", "grab", "screenshot", "بگیر", "بنداز"]);
    let other = has(&["nafrest", "نفرست", "nagir", "نگیر", "dont", "baz", "باز", "open", "kojast", "کجاست", "where", "pak", "پاک", "delete", "show", "neshoon", "نشون", "bebin", "ببین", "chi", "چی"]);
    if !shot || !take || other || words.len() > 14 {
        return None;
    }
    Some(has(&["goshi", "gooshi", "gushi", "phone", "mobile", "mobail", "گوشی", "موبایل"]))
}

/// Well-known apps by the words people use for them. Only these are opened without the model: open_app hands an
/// unknown name to the Start menu's search, which can open something else (a web search for "youtube").
const KNOWN_APPS: &[(&[&str], &str)] = &[
    (&["spotify", "اسپاتیفای"], "Spotify"),
    (&["telegram", "تلگرام"], "Telegram"),
    (&["whatsapp", "واتساپ"], "WhatsApp"),
    (&["chrome", "google chrome", "کروم"], "Google Chrome"),
    (&["firefox", "فایرفاکس"], "Firefox"),
    (&["edge", "microsoft edge"], "Microsoft Edge"),
    (&["vivaldi"], "Vivaldi"),
    (&["notepad", "note pad", "نوت پد", "نوتپد"], "Notepad"),
    (&["calculator", "mashin hesab", "ماشین حساب"], "Calculator"),
    (&["settings", "tanzimat", "تنظیمات"], "Settings"),
    (&["file explorer", "explorer", "my computer", "this pc"], "File Explorer"),
    (&["word", "microsoft word", "ورد"], "Word"),
    (&["excel", "microsoft excel", "اکسل"], "Excel"),
    (&["powerpoint", "پاورپوینت"], "PowerPoint"),
    (&["vs code", "vscode", "visual studio code"], "Visual Studio Code"),
    (&["discord", "دیسکورد"], "Discord"),
    (&["steam", "استیم"], "Steam"),
    (&["vlc"], "VLC media player"),
    (&["paint", "پینت"], "Paint"),
];

/// "spotify ro baz kon", "open chrome", "تلگرام رو باز کن": a well-known app to open.
fn open_app(text: &str) -> Option<&'static str> {
    let name = text
        .strip_prefix("open ")
        .or_else(|| text.strip_suffix(" ro baz kon"))
        .or_else(|| text.strip_suffix(" baz kon"))
        .or_else(|| text.strip_suffix(" رو باز کن"))
        .or_else(|| text.strip_suffix(" باز کن"))?
        .trim();
    KNOWN_APPS.iter().find(|(words, _)| words.contains(&name)).map(|(_, app)| *app)
}

/// The command a whole message asks for, or None when the model should read it.
pub fn parse(message: &str) -> Option<Quick> {
    let text = normalize(message);
    if text.is_empty() || text.split(' ').count() > 14 {
        return None;
    }
    let persian = persian_answer(message);
    let quick = |tool: &'static str, args: Value| Some(Quick { tool, args, persian });
    if let Some((_, kind)) = PHRASES.iter().find(|(phrase, _)| normalize(phrase) == text) {
        return match kind {
            Kind::Pause => quick("media_control", json!({ "action": "pause" })),
            Kind::Play => quick("play_music", json!({})),
            Kind::Next => quick("media_control", json!({ "action": "next" })),
            Kind::Previous => quick("media_control", json!({ "action": "previous" })),
            Kind::Status => quick("media_control", json!({ "action": "status" })),
            Kind::Louder => quick("volume", json!({ "change": 10 })),
            Kind::Quieter => quick("volume", json!({ "change": -10 })),
            Kind::Mute => quick("volume", json!({ "mute": true })),
            Kind::Unmute => quick("volume", json!({ "mute": false })),
            Kind::Time => quick(CLOCK, json!({ "what": "time" })),
            Kind::Date => quick(CLOCK, json!({ "what": "date" })),
        };
    }
    if let Some(level) = volume_level(&text) {
        return quick("volume", json!({ "level": level }));
    }
    if let Some(phone) = screenshot(&text) {
        return quick("screenshot", json!({ "send_to_phone": phone }));
    }
    if let Some(app) = open_app(&text) {
        return quick("open_app", json!({ "app": app }));
    }
    None
}

/// Persian digits for a Persian answer.
fn num(n: u64, persian: bool) -> String {
    if !persian {
        return n.to_string();
    }
    n.to_string().chars().map(|c| char::from_u32('۰' as u32 + c.to_digit(10).unwrap_or(0)).unwrap_or(c)).collect()
}

/// The answer to a command from its tool's result, or None when the model should word it (the tool failed or said
/// something these short answers do not cover).
/// The time or the date now, in the message's language (the Persian calendar for a Persian answer, as the prompt
/// gives the model: never converted by guessing).
pub fn clock(quick: &Quick, now: chrono::DateTime<chrono::Local>) -> String {
    let fa = quick.persian;
    if quick.args["what"] == json!("time") {
        let time = now.format("%H:%M").to_string();
        return if fa { format!("ساعت {}", time.chars().map(|c| c.to_digit(10).map_or(c, |d| char::from_u32(0x06f0 + d).unwrap_or(c))).collect::<String>()) } else { format!("It's {time}.") };
    }
    if fa {
        let date = crate::prompts::persian_date(now);
        format!("امروز {}", date.chars().map(|c| c.to_digit(10).map_or(c, |d| char::from_u32(0x06f0 + d).unwrap_or(c))).collect::<String>())
    } else {
        format!("Today is {}.", now.format("%A, %-d %B %Y"))
    }
}

pub fn answer(quick: &Quick, result: &Value) -> Option<String> {
    let fa = quick.persian;
    let t = |en: &str, persian: &str| if fa { persian.to_string() } else { en.to_string() };
    if result.get("success") == Some(&json!(false)) || result.get("error").is_some() {
        return None;
    }
    let now = result["now"].as_str().filter(|s| !s.trim().is_empty());
    let playing = result["playing"].as_bool();
    let unlisted = result["note"].as_str().is_some_and(|n| n.contains("Windows lists no player"));
    let action = result.get("action").and_then(Value::as_str).or_else(|| quick.args["action"].as_str()).unwrap_or_default();
    Some(match (quick.tool, action) {
        ("media_control", "pause") if unlisted => t("Pressed pause.", "دکمه‌ی توقف رو زدم."),
        ("media_control", "pause") => match playing {
            Some(false) => t("Paused.", "متوقف شد."),
            _ => return None,
        },
        ("media_control", "next" | "previous") if unlisted => t("Pressed the key.", "دکمه‌اش رو زدم."),
        ("media_control", "next") => match now {
            Some(song) => format!("{}{song}", t("Next: ", "آهنگ بعدی: ")),
            None => t("Skipped to the next song.", "رفت آهنگ بعدی."),
        },
        ("media_control", "previous") => match now {
            Some(song) => format!("{}{song}", t("Back to: ", "برگشت به: ")),
            None => t("Went back a song.", "برگشت آهنگ قبلی."),
        },
        ("media_control", "status") => match (playing, now) {
            (Some(true), Some(song)) => format!("{}{song}", t("Playing now: ", "الان داره پخش میشه: ")),
            (Some(false), Some(song)) => format!("{song}{}", t(" (paused).", " (الان متوقفه).")),
            (Some(false), None) => t("Nothing is playing.", "الان چیزی پخش نمی‌شه."),
            _ => return None,
        },
        ("play_music", _) => match (playing, now) {
            (Some(true), Some(song)) => format!("{}{song}", t("Playing: ", "در حال پخش: ")),
            _ => return None,
        },
        ("volume", _) => {
            let level = result["now"]["level"].as_u64()?;
            let muted = result["now"]["muted"].as_bool() == Some(true);
            match (muted, fa) {
                (true, _) => t("Muted.", "بی‌صدا شد."),
                (false, true) => format!("صدا: {}٪", num(level, true)),
                (false, false) => format!("Volume: {level}%"),
            }
        }
        ("screenshot", _) => {
            let saved = result["saved"].as_str()?;
            if result.get("phone").is_some() {
                t("Took a screenshot and sent it to your phone.", "از صفحه عکس گرفتم و فرستادم برای گوشیت.")
            } else if let Some(why) = result["phone_error"].as_str() {
                format!("{}{saved}\n{why}", t("Took the screenshot but it did not reach the phone. It is saved here: ", "عکس رو گرفتم ولی به گوشی نرسید. اینجا ذخیره شد: "))
            } else {
                format!("{}{saved}", t("Screenshot saved: ", "عکس صفحه ذخیره شد: "))
            }
        }
        ("open_app", _) if result["opened"] == json!(true) => {
            let app = result["app"].as_str().or(quick.args["app"].as_str()).unwrap_or_default();
            format!("{app}{}", t(" is open.", " باز شد."))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(text: &str) -> Option<(&'static str, Value, bool)> {
        parse(text).map(|q| (q.tool, q.args, q.persian))
    }

    #[test]
    fn short_commands_are_recognised_in_finglish_persian_and_english() {
        assert_eq!(tool("Pause kon!"), Some(("media_control", json!({ "action": "pause" }), true)));
        assert_eq!(tool("ahango negah dar"), Some(("media_control", json!({ "action": "pause" }), true)));
        assert_eq!(tool("pause the music"), Some(("media_control", json!({ "action": "pause" }), false)));
        assert_eq!(tool("آهنگ بعدی"), Some(("media_control", json!({ "action": "next" }), true)));
        assert_eq!(tool("lotfan ahang badi ro bezar"), Some(("media_control", json!({ "action": "next" }), true)));
        assert_eq!(tool("chi dare play mishe?"), Some(("media_control", json!({ "action": "status" }), true)));
        assert_eq!(tool("What's playing?"), Some(("media_control", json!({ "action": "status" }), false)));
        assert_eq!(tool("این آهنگ چیه؟"), Some(("media_control", json!({ "action": "status" }), true)));
        assert_eq!(tool("spotify baz kon ye ahang play kon"), Some(("play_music", json!({}), true)));
        assert_eq!(tool("Spotify ro baz kon, ye music bezar"), Some(("play_music", json!({}), true)));
        assert_eq!(tool("play some music"), Some(("play_music", json!({}), false)));
        assert_eq!(tool("sedasho kam kon"), Some(("volume", json!({ "change": -10 }), true)));
        assert_eq!(tool("seda ro bezar ru 30"), Some(("volume", json!({ "level": 30 }), true)));
        assert_eq!(tool("صدا رو بذار رو ۴۰"), Some(("volume", json!({ "level": 40 }), true)));
        assert_eq!(tool("volume 25"), Some(("volume", json!({ "level": 25 }), false)));
    }

    #[test]
    fn screenshots_and_apps() {
        assert_eq!(tool("baram az safam aks begir befrest bara goshim"), Some(("screenshot", json!({ "send_to_phone": true }), true)));
        assert_eq!(tool("یه اسکرین شات بگیر"), Some(("screenshot", json!({ "send_to_phone": false }), true)));
        assert_eq!(tool("take a screenshot and send it to my phone"), Some(("screenshot", json!({ "send_to_phone": true }), false)));
        assert_eq!(tool("spotify ro baz kon"), Some(("open_app", json!({ "app": "Spotify" }), true)));
        assert_eq!(tool("open google chrome"), Some(("open_app", json!({ "app": "Google Chrome" }), false)));
        assert_eq!(tool("تلگرام رو باز کن"), Some(("open_app", json!({ "app": "Telegram" }), true)));
        assert_eq!(tool("youtube ro baz kon"), None, "not an app: the Start menu would search the web");
    }

    #[test]
    fn the_clock_and_the_date_need_no_tool() {
        use chrono::TimeZone;
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 12, 22, 5, 0).unwrap();
        let q = parse("saat chande?").unwrap();
        assert_eq!((q.tool, q.persian), (CLOCK, true));
        assert_eq!(clock(&q, now), "ساعت ۲۲:۰۵");
        assert_eq!(clock(&parse("what time is it").unwrap(), now), "It's 22:05.");
        assert_eq!(clock(&parse("امروز چندمه؟").unwrap(), now), "امروز پنجشنبه ۲۱ آبان ۱۴۰۵");
        assert_eq!(clock(&parse("what's the date today?").unwrap(), now), "Today is Thursday, 12 November 2026.");
    }

    #[test]
    fn anything_more_goes_to_the_model() {
        for text in [
            "spotify baz kon in ahango play kon",
            "ahang shadmehr ro play kon",
            "file report ro baz kon",
            "in ro baz kon",
            "pause kon va ye ahang dige bezar",
            "aks begir nafrest",
            "gheimat dollar chande",
            "seda ro bezar ru 30 va ye ahang bezar",
            "why did you pause",
            "",
        ] {
            assert_eq!(tool(text), None, "{text}");
        }
    }

    #[test]
    fn answers_come_from_the_result_or_go_to_the_model() {
        let q = |text: &str| parse(text).unwrap();
        assert_eq!(answer(&q("chi dare play mishe"), &json!({ "playing": true, "now": "Ebi - Goriz", "app": "Spotify" })).as_deref(), Some("الان داره پخش میشه: Ebi - Goriz"));
        assert_eq!(answer(&q("what's playing"), &json!({ "playing": false, "note": "No player is open: nothing plays." })).as_deref(), Some("Nothing is playing."));
        assert_eq!(answer(&q("pause kon"), &json!({ "playing": false, "now": "x" })).as_deref(), Some("متوقف شد."));
        // A word both languages use follows the app's language (English in tests).
        assert_eq!(answer(&q("pause"), &json!({ "playing": false, "now": "x" })).as_deref(), Some("Paused."));
        assert_eq!(answer(&q("seda ro kam kon"), &json!({ "now": { "level": 40, "muted": false } })).as_deref(), Some("صدا: ۴۰٪"));
        assert_eq!(answer(&q("play some music"), &json!({ "playing": true, "now": "Ebi - Goriz" })).as_deref(), Some("Playing: Ebi - Goriz"));
        // Not playing after all, or a failed tool: the model words it (and may try another way).
        assert_eq!(answer(&q("play some music"), &json!({ "playing": false, "note": "It did not start playing." })), None);
        assert_eq!(answer(&q("spotify ro baz kon"), &json!({ "success": false, "error": "no such app" })), None);
        assert_eq!(answer(&q("spotify ro baz kon"), &json!({ "opened": true, "app": "Spotify", "window": "Spotify Free" })).as_deref(), Some("Spotify باز شد."));
        assert_eq!(answer(&q("spotify ro baz kon"), &json!({ "opened": false, "app": "Spotify" })), None, "no window came: the model says what happened");
        let shot = answer(&q("aks az safe begir befrest baraye goshi"), &json!({ "saved": "C:/p.jpg", "phone": { "sending": "p.jpg" } })).unwrap();
        assert_eq!(shot, "از صفحه عکس گرفتم و فرستادم برای گوشیت.");
    }
}
