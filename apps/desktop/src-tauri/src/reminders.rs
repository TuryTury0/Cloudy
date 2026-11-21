//! Gives out due reminders: says them aloud, shows them on the island and writes them to the log. The reminders
//! themselves (set, list, cancel, kept in reminders.json) live in anytask_core::reminders; this only watches the clock.

use crate::brain::Brain;
use crate::island::ISLAND;
use anytask_core::CoreEvent;
use anytask_core::reminders::Reminder;
use chrono::{DateTime, Local};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Checked every second: a 5 s tick made reminders up to 5 s late (final test: 4 s).
const EVERY: Duration = Duration::from_secs(1);
/// A reminder given out later than this was missed (the app was closed or the PC asleep) and says so.
const LATE_MS: i64 = 2 * 60_000;

pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let now = Local::now().timestamp_millis();
            for reminder in app.state::<Brain>().reminders.take_due(now) {
                fire(&app, &reminder, now);
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// What the user hears and sees.
pub fn message(reminder: &Reminder, now_ms: i64) -> String {
    if now_ms - reminder.at_ms > LATE_MS {
        let at = DateTime::from_timestamp_millis(reminder.at_ms).map(|t| t.with_timezone(&Local).format("%H:%M").to_string()).unwrap_or_default();
        let label = anytask_core::lang::for_user(&reminder.text, "Reminder (for {at}, late):", "یادآوری (برای ساعت {at}، دیر شد):").replace("{at}", &at);
        format!("{label} {}", reminder.text)
    } else {
        format!("{} {}", anytask_core::lang::for_user(&reminder.text, "Reminder:", "یادآوری:"), reminder.text)
    }
}

fn fire(app: &AppHandle, reminder: &Reminder, now_ms: i64) {
    let text = message(reminder, now_ms);
    app.state::<Brain>().agent.events().emit(CoreEvent::Log { turn_id: None, source: "reminder".into(), text: text.clone() });
    let _ = app.emit_to(ISLAND, "reminder", &text);
    match &reminder.task {
        // A job for this time ("play Enemy at 12:05"): the assistant does it now, as if the user asked for it; its
        // answer is spoken. Before, such a reminder only said "time to play Enemy" (user report, 11-05 12:05).
        Some(task) => {
            app.state::<Brain>().agent.events().emit(CoreEvent::Log { turn_id: None, source: "reminder".into(), text: format!("doing: {task}") });
            // Said in Persian for a Persian reminder, so the answer comes in Persian too (training round 4: an English
            // task got an English reply).
            let now = anytask_core::lang::for_user(&reminder.text, "It is time. Do this now:", "زمانش رسید، این کار رو الان انجام بده:");
            crate::voice::ask(app, format!("{now} {task}"));
        }
        None => crate::speak::say_important(app, text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missed_reminder_says_it_is_late() {
        let reminder = Reminder { id: 1, at_ms: 1_800_000_000_000, text: "آب بخور".into(), task: None };
        assert_eq!(message(&reminder, reminder.at_ms + 3_000), "یادآوری: آب بخور");
        let late = message(&reminder, reminder.at_ms + 3_600_000);
        assert!(late.starts_with("یادآوری (برای ساعت ") && late.ends_with("دیر شد): آب بخور"), "{late}");
    }
}
