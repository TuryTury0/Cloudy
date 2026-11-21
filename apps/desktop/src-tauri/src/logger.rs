//! The activity log: every request, tool step, model answer and error with its time, in a daily file
//! (app log folder, kept for 14 days) and in memory for the Logs page, which also gets new lines live.
//! API keys never reach it: core events do not carry them and model errors are redacted in the llm crate.

use crate::usage::{Tally, UsageBook};
use anytask_core::{CoreEvent, EventBus};
use chrono::Local;
use serde::Serialize;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast::error::RecvError;

const KEEP_LINES: usize = 2000;
const KEEP_DAYS: u64 = 14;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// Local time, e.g. "14:05:09.231".
    pub at: String,
    /// request, tool, step, detail, answer, model, error
    pub kind: &'static str,
    pub text: String,
}

pub struct Logger {
    dir: PathBuf,
    recent: Mutex<VecDeque<LogLine>>,
}

/// 12345 -> "12,345".
pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn short(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { format!("{}…", flat.chars().take(max).collect::<String>()) }
}

/// The log line for an event, or None for events that are only UI state.
pub fn line_for(event: &CoreEvent) -> Option<(&'static str, String)> {
    Some(match event {
        CoreEvent::State(_) => return None,
        CoreEvent::TurnStarted { turn_id, user } => ("request", format!("#{turn_id} \"{}\"", short(user, 300))),
        CoreEvent::ToolStarted { label, name, .. } => ("tool", format!("{name}: {label}")),
        CoreEvent::ToolProgress { text, .. } if text.ends_with('…') => return None,
        CoreEvent::ToolProgress { text, .. } => ("step", text.clone()),
        CoreEvent::Log { source, text, .. } => ("detail", format!("{source}: {text}")),
        CoreEvent::ToolFinished { name, ok, summary, .. } => ("tool", format!("{name} {}: {}", if *ok { "ok" } else { "FAILED" }, short(summary, 200))),
        CoreEvent::Answer { turn_id, text } => ("answer", format!("#{turn_id} {}", short(text, 400))),
        CoreEvent::Usage { label, input, output, thinking, cached, .. } => (
            "tokens",
            format!(
                "{label}: {} in{} + {} out{}",
                group(*input),
                if *cached > 0 { format!(" ({} cached)", group(*cached)) } else { String::new() },
                group(*output),
                if *thinking > 0 { format!(" + {} thinking", group(*thinking)) } else { String::new() }
            ),
        ),
        CoreEvent::TurnUsage { turn_id, calls, input, output, thinking } => (
            "tokens",
            format!("#{turn_id} total {} tokens in {calls} model call{} ({} in, {} out, {} thinking)", group(input + output + thinking), if *calls == 1 { "" } else { "s" }, group(*input), group(*output), group(*thinking)),
        ),
        CoreEvent::ModelUsed { label, ok, reason } => ("model", if *ok { format!("{label}: answered") } else { format!("{label}: {}", reason.clone().unwrap_or_default()) }),
        CoreEvent::Error { message, .. } => ("error", message.clone()),
        CoreEvent::Cancelled { turn_id } => ("request", format!("#{turn_id} stopped")),
    })
}

impl Logger {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        remove_old_files(&dir);
        Self { dir, recent: Mutex::new(VecDeque::new()) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn write(&self, kind: &'static str, text: String) -> LogLine {
        let now = Local::now();
        let line = LogLine { at: now.format("%H:%M:%S%.3f").to_string(), kind, text };
        let file = self.dir.join(format!("anytask-{}.log", now.format("%Y-%m-%d")));
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(file) {
            let _ = writeln!(f, "{} {:<7} {}", line.at, line.kind, line.text);
        }
        if let Ok(mut recent) = self.recent.lock() {
            recent.push_back(line.clone());
            while recent.len() > KEEP_LINES {
                recent.pop_front();
            }
        }
        line
    }

    pub fn recent(&self, limit: usize) -> Vec<LogLine> {
        self.recent.lock().map(|r| r.iter().rev().take(limit).rev().cloned().collect()).unwrap_or_default()
    }

    /// Writes every core event to the log and sends each new line to the windows as "log-line"; adds each
    /// request's tokens to the usage book and sends the new totals as "usage-changed".
    pub fn follow(self: &std::sync::Arc<Self>, app: AppHandle, events: &EventBus, usage: std::sync::Arc<UsageBook>) {
        let mut rx = events.subscribe();
        let logger = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let CoreEvent::TurnUsage { calls, input, output, thinking, .. } = &event {
                            let summary = usage.record(Tally { requests: 1, calls: u64::from(*calls), input: *input, output: *output, thinking: *thinking });
                            let _ = app.emit("usage-changed", &summary);
                        }
                        if let Some((kind, text)) = line_for(&event) {
                            let line = logger.write(kind, text);
                            let _ = app.emit("log-line", &line);
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        logger.write("error", format!("{n} log lines were dropped (too many at once)"));
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }
}

fn remove_old_files(dir: &Path) {
    let limit = std::time::Duration::from_secs(KEEP_DAYS * 24 * 3600);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > limit);
        if name.starts_with("anytask-") && name.ends_with(".log") && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_become_readable_lines() {
        assert_eq!(line_for(&CoreEvent::TurnStarted { turn_id: 3, user: "قیمت دلار".into() }), Some(("request", "#3 \"قیمت دلار\"".into())));
        assert_eq!(line_for(&CoreEvent::ToolProgress { turn_id: 3, call_id: "c".into(), text: "launch_app : notepad".into() }).unwrap().0, "step");
        assert_eq!(line_for(&CoreEvent::ToolProgress { turn_id: 3, call_id: "c".into(), text: "نگاه به صفحه…".into() }), None, "status lines are noise");
        assert_eq!(line_for(&CoreEvent::Log { turn_id: Some(3), source: "desktop_task".into(), text: "look 0.52s".into() }).unwrap().1, "desktop_task: look 0.52s");
        assert_eq!(line_for(&CoreEvent::TurnUsage { turn_id: 4, calls: 3, input: 5100, output: 180, thinking: 40 }).unwrap().1, "#4 total 5,320 tokens in 3 model calls (5,100 in, 180 out, 40 thinking)");
        assert_eq!(group(1234567), "1,234,567");
        assert!(line_for(&CoreEvent::State(anytask_core::StateUpdate::new(anytask_core::AssistantState::Idle))).is_none());
    }

    #[test]
    fn lines_go_to_a_daily_file_and_memory() {
        let dir = std::env::temp_dir().join(format!("anytask-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let logger = Logger::new(dir.clone());
        logger.write("request", "#1 \"salam\"".into());
        logger.write("answer", "#1 Salam!".into());
        assert_eq!(logger.recent(1)[0].text, "#1 Salam!");
        let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        let content = std::fs::read_to_string(file).unwrap();
        assert!(content.contains("request #1 \"salam\""));
        assert_eq!(content.lines().count(), 2);
    }
}
