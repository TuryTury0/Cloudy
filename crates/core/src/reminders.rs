//! Reminders and timers ("remind me in 10 minutes to drink water", "at 17:00 tell me to call mom"). They are kept
//! in a small JSON file so they survive a restart; the desktop app asks for due ones every few seconds and says them
//! aloud. Reminders that came due while the app was closed are given out at the next start, marked late.

use crate::lang::tr;
use crate::tools::{Tool, ToolCtx, arg_str, short};
use anytask_llm::ToolSpec;
use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveDate, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// At most this many reminders wait at once (a model in a loop must not fill the file).
const MAX_WAITING: usize = 50;
/// The furthest ahead a reminder may be set.
const MAX_AHEAD_DAYS: i64 = 366;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: u64,
    /// When it is due (ms since the epoch).
    pub at_ms: i64,
    pub text: String,
    /// A job to DO at that time ("play Enemy by Imagine Dragons in Spotify"), run as a request to the assistant.
    /// None: only say the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Book {
    next_id: u64,
    waiting: Vec<Reminder>,
}

/// The waiting reminders, saved to `path` after every change (none for tests).
pub struct Reminders {
    path: Option<PathBuf>,
    book: Mutex<Book>,
}

impl Reminders {
    /// Loads the reminders from `path`; a missing or broken file starts empty.
    pub fn open(path: PathBuf) -> Self {
        let book = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        Self { path: Some(path), book: Mutex::new(book) }
    }

    pub fn in_memory() -> Self {
        Self { path: None, book: Mutex::default() }
    }

    fn change<T>(&self, f: impl FnOnce(&mut Book) -> T) -> T {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        let out = f(&mut book);
        if let Some(path) = &self.path
            && let Ok(text) = serde_json::to_string_pretty(&*book)
        {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
        out
    }

    pub fn add(&self, at_ms: i64, text: &str) -> Result<Reminder, String> {
        self.add_task(at_ms, text, None)
    }

    /// A reminder that also runs `task` when it is due.
    pub fn add_task(&self, at_ms: i64, text: &str, task: Option<&str>) -> Result<Reminder, String> {
        self.change(|book| {
            if book.waiting.len() >= MAX_WAITING {
                return Err(format!("{MAX_WAITING} reminders are already waiting; cancel some first."));
            }
            book.next_id += 1;
            let task = task.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
            let reminder = Reminder { id: book.next_id, at_ms, text: text.trim().to_string(), task };
            book.waiting.push(reminder.clone());
            book.waiting.sort_by_key(|r| r.at_ms);
            Ok(reminder)
        })
    }

    /// The waiting reminders, soonest first.
    pub fn list(&self) -> Vec<Reminder> {
        self.book.lock().unwrap_or_else(|e| e.into_inner()).waiting.clone()
    }

    pub fn cancel(&self, id: u64) -> Option<Reminder> {
        self.change(|book| {
            let at = book.waiting.iter().position(|r| r.id == id)?;
            Some(book.waiting.remove(at))
        })
    }

    /// Takes out the reminders that are due at `now_ms` (each is given out once).
    pub fn take_due(&self, now_ms: i64) -> Vec<Reminder> {
        if !self.book.lock().unwrap_or_else(|e| e.into_inner()).waiting.iter().any(|r| r.at_ms <= now_ms) {
            return Vec::new();
        }
        self.change(|book| {
            let (due, waiting) = book.waiting.drain(..).partition(|r| r.at_ms <= now_ms);
            book.waiting = waiting;
            due
        })
    }
}

/// "HH:MM" (the next time it comes: today, or tomorrow when it has passed) or "YYYY-MM-DD HH:MM", local time.
pub fn parse_at<Tz: TimeZone>(text: &str, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    let text = to_latin_digits(text.trim());
    let zone = now.timezone();
    if let Ok(time) = NaiveTime::parse_from_str(&text, "%H:%M") {
        let today = zone.from_local_datetime(&now.date_naive().and_time(time)).earliest()?;
        return Some(if today > *now { today } else { today + ChronoDuration::days(1) });
    }
    let (date, time) = text.split_once([' ', 'T'])?;
    let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
    let time = NaiveTime::parse_from_str(time.trim(), "%H:%M").ok()?;
    zone.from_local_datetime(&date.and_time(time)).earliest()
}

/// Persian and Arabic digits (۱۷:۳۰) as ASCII.
fn to_latin_digits(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '۰'..='۹' => char::from(b'0' + (c as u32 - '۰' as u32) as u8),
            '٠'..='٩' => char::from(b'0' + (c as u32 - '٠' as u32) as u8),
            _ => c,
        })
        .collect()
}

/// "Thu 05 Nov 17:30 (in 2 h 5 min)" for the model.
pub fn describe(at_ms: i64, now_ms: i64) -> String {
    let when = DateTime::from_timestamp_millis(at_ms).map(|t| t.with_timezone(&Local).format("%a %d %b %H:%M").to_string()).unwrap_or_default();
    let minutes = ((at_ms - now_ms).max(0) + 59_999) / 60_000;
    let left = match minutes {
        0..=59 => format!("{minutes} min"),
        60..=1439 if minutes % 60 == 0 => format!("{} h", minutes / 60),
        60..=1439 => format!("{} h {} min", minutes / 60, minutes % 60),
        _ => {
            // "1 day 5 h", not "1 days" (11-11: a reminder 29 h ahead read "in 1 days").
            let (days, hours) = (minutes / 1440, minutes % 1440 / 60);
            let days = if days == 1 { "1 day".to_string() } else { format!("{days} days") };
            if hours > 0 { format!("{days} {hours} h") } else { days }
        }
    };
    format!("{when} (in {left})")
}

fn view(r: &Reminder, now_ms: i64) -> Value {
    let mut out = json!({
        "id": r.id,
        "text": r.text,
        "when": describe(r.at_ms, now_ms),
        "at": DateTime::from_timestamp_millis(r.at_ms).map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string()),
        "seconds_left": ((r.at_ms - now_ms).max(0) + 999) / 1000,
    });
    if let Some(task) = &r.task {
        out["task"] = json!(task);
    }
    out
}

/// Words that say which part of the day a clock time is (Persian, Finglish, English).
const DAY_PARTS: &[&str] = &[
    "صبح", "ظهر", "عصر", "شب", "بامداد", "قبل از ظهر", "بعد از ظهر", "بعدازظهر", "نیمه شب", "نصف شب", "sobh", "zohr", "asr", "shab", "bamdad",
    "morning", "noon", "afternoon", "evening", "night", "midnight",
];

/// The user named a clock time without saying which half of the day ("saat 5", "saat 12"): both 05:00 and
/// 17:00 fit, so guessing can be 12 hours wrong. A 24-hour number (13-23, 00) or a part of the day settles it.
fn ambiguous_time(words: &str) -> bool {
    let folded: String = words
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '۰'..='۹' => char::from(b'0' + (c as u32 - '۰' as u32) as u8),
            '٠'..='٩' => char::from(b'0' + (c as u32 - '٠' as u32) as u8),
            _ => c,
        })
        .collect();
    // "10 daghighe dige", "ye saat dige": a span from now, not a clock hour.
    const FROM_NOW: &[&str] = &["دقیقه", "دیگه", "دیگر", "بعد", "daghighe", "dagighe", "dige", "digar", "minute", "later"];
    if folded.trim().is_empty() || DAY_PARTS.iter().any(|p| folded.contains(p)) || FROM_NOW.iter().any(|p| folded.contains(p)) {
        return false;
    }
    let tokens: Vec<&str> = folded.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    if tokens.iter().any(|t| matches!(*t, "am" | "pm")) {
        return false;
    }
    let numbers: Vec<u32> = tokens.iter().filter_map(|t| t.parse().ok()).collect();
    let says_24h = numbers.iter().any(|n| (13..=23).contains(n)) || tokens.contains(&"00");
    // Some hour 1-12 was said and nothing tells the half of the day.
    !says_24h && numbers.iter().any(|n| (1..=12).contains(n))
}

pub struct SetReminder {
    pub reminders: Arc<Reminders>,
}

#[async_trait]
impl Tool for SetReminder {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_reminder".into(),
            description: "Sets a reminder, a timer or a job for later: at that time the assistant says the text aloud and shows it, \
                          and when task is given it also DOES that task then (\"saat 12:05 Enemy ro play kon\" -> task \"In Spotify \
                          play Enemy by Imagine Dragons\"). Give in_minutes for \"in 10 minutes\" / a timer, or at for a clock time \
                          (\"17:30\" = the next 17:30, or \"2026-11-06 08:00\"). The text is what to tell the user then, in their \
                          language (e.g. \"آب بخور\"). If the user gave an hour without the part of the day (\"saat 5\"), ask once \
                          whether morning or evening (for 12: noon or midnight) before setting it."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "What to remind the user of." },
                    "in_minutes": { "type": "number", "description": "Minutes from now (a timer)." },
                    "at": { "type": "string", "description": "Local clock time HH:MM (24 h), or YYYY-MM-DD HH:MM." },
                    "task": { "type": "string", "description": "Optional: what to DO at that time, as a full request in the user's own language (Persian), e.g. «آهنگ Enemy از Imagine Dragons رو تو اسپاتیفای پخش کن»." }
                },
                "required": ["text"]
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("Reminder:", "یادآوری:"), short(arg_str(args, "text").unwrap_or(""), 40))
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let text = arg_str(&args, "text").ok_or("text is required: what should the reminder say?")?;
        let now = DateTime::from_timestamp_millis(ctx.now_ms).ok_or("The clock is unreadable.")?.with_timezone(&Local);
        let at = match (args.get("in_minutes").and_then(Value::as_f64), arg_str(&args, "at")) {
            (Some(minutes), _) if minutes > 0.0 => now + ChronoDuration::milliseconds((minutes * 60_000.0).round() as i64),
            (Some(_), None) => return Err("in_minutes must be more than 0.".into()),
            (_, Some(at)) => {
                // ctx.request is what the user said for this request; a later "sobh" or "shab" answer settles it.
                if ambiguous_time(&ctx.request) {
                    return Err("The user gave an hour without the part of the day. Ask them once before setting it: for example \
                                \"saat 5 sobh ya 5 asr?\" (for 12: \"12 zohr ya 12 shab?\"). Never guess; set it after they answer."
                        .into());
                }
                parse_at(at, &now).ok_or(format!("\"{at}\" is not a time I can read: use HH:MM or YYYY-MM-DD HH:MM."))?
            }
            (None, None) => return Err("Give in_minutes or at: when should I remind the user?".into()),
        };
        if at <= now {
            return Err(format!("{} has already passed.", at.format("%Y-%m-%d %H:%M")));
        }
        if at - now > ChronoDuration::days(MAX_AHEAD_DAYS) {
            return Err("That is more than a year away.".into());
        }
        let reminder = self.reminders.add_task(at.timestamp_millis(), text, arg_str(&args, "task"))?;
        Ok(json!({ "set": true, "reminder": view(&reminder, ctx.now_ms) }))
    }
}

pub struct ListReminders {
    pub reminders: Arc<Reminders>,
}

#[async_trait]
impl Tool for ListReminders {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_reminders".into(),
            description: "The reminders and timers that are still waiting, soonest first, with their ids.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    fn label(&self, _: &Value) -> String {
        tr("Reminders", "یادآوری‌ها").into()
    }

    async fn run(&self, _: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let waiting: Vec<Value> = self.reminders.list().iter().map(|r| view(r, ctx.now_ms)).collect();
        Ok(json!({ "count": waiting.len(), "reminders": waiting }))
    }
}

pub struct CancelReminder {
    pub reminders: Arc<Reminders>,
}

#[async_trait]
impl Tool for CancelReminder {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "cancel_reminder".into(),
            description: "Cancels a waiting reminder or timer by its id (find it with list_reminders first).".into(),
            parameters: json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }),
        }
    }

    fn label(&self, _: &Value) -> String {
        tr("Cancelling a reminder", "لغو یادآوری").into()
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_u64).ok_or("id is required (see list_reminders).")?;
        let reminder = self.reminders.cancel(id).ok_or(format!("No waiting reminder has id {id}."))?;
        Ok(json!({ "cancelled": view(&reminder, ctx.now_ms) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::DenyAll;
    use crate::tools::test_support::ctx;
    use chrono::FixedOffset;

    const MIN: i64 = 60_000;

    fn tehran(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(3 * 3600 + 1800).unwrap().with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn far_reminders_say_days_and_hours() {
        let now = 1_800_000_000_000;
        let hour = 3_600_000;
        assert!(describe(now + 29 * hour, now).ends_with("(in 1 day 5 h)"), "{}", describe(now + 29 * hour, now));
        assert!(describe(now + 48 * hour, now).ends_with("(in 2 days)"));
        assert!(describe(now + 90 * 60_000, now).ends_with("(in 1 h 30 min)"));
    }

    #[test]
    fn clock_times_mean_the_next_time_it_comes() {
        let now = tehran(2026, 11, 5, 16, 0);
        assert_eq!(parse_at("17:30", &now), Some(tehran(2026, 11, 5, 17, 30)));
        assert_eq!(parse_at("08:00", &now), Some(tehran(2026, 11, 6, 8, 0)), "passed today: tomorrow");
        assert_eq!(parse_at("۱۷:۳۰", &now), Some(tehran(2026, 11, 5, 17, 30)), "Persian digits");
        assert_eq!(parse_at("2026-11-07 09:15", &now), Some(tehran(2026, 11, 7, 9, 15)));
        assert_eq!(parse_at("tomorrow", &now), None);
    }

    #[test]
    fn due_reminders_are_given_out_once_and_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("anytask-reminders-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reminders.json");
        let _ = std::fs::remove_file(&path);
        let book = Reminders::open(path.clone());
        let water = book.add(10 * MIN, "آب بخور").unwrap();
        book.add(5 * MIN, "call mom").unwrap();
        assert_eq!(book.list().iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), vec!["call mom", "آب بخور"], "soonest first");
        let reopened = Reminders::open(path.clone());
        assert_eq!(reopened.list().len(), 2, "kept in the file");
        assert_eq!(reopened.take_due(6 * MIN).iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), vec!["call mom"]);
        assert!(reopened.take_due(6 * MIN).is_empty(), "given out once");
        assert_eq!(reopened.cancel(water.id).map(|r| r.text), Some("آب بخور".into()));
        assert!(Reminders::open(path.clone()).list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_timer_is_set_from_minutes_and_listed() {
        let reminders = Arc::new(Reminders::in_memory());
        let mut ctx = ctx(Arc::new(DenyAll));
        ctx.now_ms = 1_800_000_000_000;
        let set = SetReminder { reminders: reminders.clone() };
        let out = set.run(json!({ "text": "آب بخور", "in_minutes": 10 }), &ctx).await.unwrap();
        assert_eq!(out["set"], true);
        assert!(out["reminder"]["when"].as_str().unwrap().ends_with("(in 10 min)"), "{out}");
        assert_eq!(reminders.list()[0].at_ms, ctx.now_ms + 10 * MIN);
        let listed = ListReminders { reminders: reminders.clone() }.run(json!({}), &ctx).await.unwrap();
        assert_eq!(listed["count"], 1);
        let id = listed["reminders"][0]["id"].as_u64().unwrap();
        CancelReminder { reminders: reminders.clone() }.run(json!({ "id": id }), &ctx).await.unwrap();
        assert!(reminders.list().is_empty());
    }

    #[test]
    fn an_hour_without_the_part_of_the_day_is_ambiguous() {
        assert!(ambiguous_time("saat 5 yadam bendaz"));
        assert!(ambiguous_time("ساعت ۱۲ بگو"));
        assert!(!ambiguous_time("ساعت ۵ صبح بیدارم کن"));
        assert!(!ambiguous_time("saat 12 zohr"));
        assert!(!ambiguous_time("saat 17:30 begu"));
        assert!(!ambiguous_time("at 5 pm"));
        assert!(!ambiguous_time("10 daghighe dige"), "no clock hour");
        assert!(!ambiguous_time("یک ساعت دیگه یادم بنداز"), "a span from now");
        assert!(!ambiguous_time(""), "no words to judge by");
    }

    #[tokio::test]
    async fn a_reminder_can_do_a_job_and_an_unclear_hour_is_asked_about() {
        let reminders = Arc::new(Reminders::in_memory());
        let set = SetReminder { reminders: reminders.clone() };
        let mut ctx = ctx(Arc::new(DenyAll));
        ctx.now_ms = 1_800_000_000_000;
        ctx.request = "saat 5 ahang enemy ro play kon".into();
        let err = set.run(json!({ "text": "x", "at": "17:00", "task": "play Enemy" }), &ctx).await.unwrap_err();
        assert!(err.contains("Ask them once"), "{err}");
        assert!(reminders.list().is_empty(), "nothing set before the user answers");

        ctx.request = "5 asr".into();
        let out = set.run(json!({ "text": "وقت آهنگ", "at": "17:00", "task": "In Spotify play Enemy by Imagine Dragons" }), &ctx).await.unwrap();
        assert_eq!(out["reminder"]["task"], "In Spotify play Enemy by Imagine Dragons");
        assert_eq!(reminders.list()[0].task.as_deref(), Some("In Spotify play Enemy by Imagine Dragons"));
        assert!(out["reminder"]["seconds_left"].as_i64().unwrap() > 0, "{out}");
    }

    #[tokio::test]
    async fn unreadable_or_past_times_are_refused() {
        let set = SetReminder { reminders: Arc::new(Reminders::in_memory()) };
        let mut ctx = ctx(Arc::new(DenyAll));
        ctx.now_ms = 1_800_000_000_000;
        assert!(set.run(json!({ "text": "x" }), &ctx).await.unwrap_err().contains("in_minutes or at"));
        assert!(set.run(json!({ "text": "x", "at": "soon" }), &ctx).await.unwrap_err().contains("not a time"));
        assert!(set.run(json!({ "text": "x", "at": "2020-01-01 10:00" }), &ctx).await.unwrap_err().contains("already passed"));
        assert!(set.run(json!({ "text": "x", "in_minutes": 0 }), &ctx).await.unwrap_err().contains("more than 0"));
    }
}
