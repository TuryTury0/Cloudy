//! Token totals: the last request, today and all time, kept in usage.json in the app data folder (90 days of
//! daily totals). Shown at the top of the Logs page.

use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

const KEEP_DAYS: usize = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Tally {
    pub requests: u64,
    pub calls: u64,
    pub input: u64,
    pub output: u64,
    pub thinking: u64,
}

impl Tally {
    fn add(&mut self, other: Tally) {
        self.requests += other.requests;
        self.calls += other.calls;
        self.input += other.input;
        self.output += other.output;
        self.thinking += other.thinking;
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Book {
    total: Tally,
    days: BTreeMap<String, Tally>,
    last: Option<Tally>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub last: Option<Tally>,
    pub today: Tally,
    pub total: Tally,
}

pub struct UsageBook {
    path: PathBuf,
    book: Mutex<Book>,
}

impl UsageBook {
    pub fn load(path: PathBuf) -> Self {
        let book = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { path, book: Mutex::new(book) }
    }

    /// Adds one finished request (all its model calls) and saves.
    pub fn record(&self, request: Tally) -> Summary {
        let today = Local::now().format("%Y-%m-%d").to_string();
        let Ok(mut book) = self.book.lock() else { return self.summary() };
        book.total.add(request);
        book.days.entry(today).or_default().add(request);
        while book.days.len() > KEEP_DAYS {
            let oldest = book.days.keys().next().cloned().unwrap_or_default();
            book.days.remove(&oldest);
        }
        book.last = Some(request);
        if let Ok(text) = serde_json::to_vec_pretty(&*book) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
        drop(book);
        self.summary()
    }

    pub fn summary(&self) -> Summary {
        let today = Local::now().format("%Y-%m-%d").to_string();
        self.book
            .lock()
            .map(|b| Summary { last: b.last, today: b.days.get(&today).copied().unwrap_or_default(), total: b.total })
            .unwrap_or(Summary { last: None, today: Tally::default(), total: Tally::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_add_up_and_survive_a_restart() {
        let path = std::env::temp_dir().join(format!("anytask-usage-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let book = UsageBook::load(path.clone());
        book.record(Tally { requests: 1, calls: 2, input: 1000, output: 50, thinking: 10 });
        let s = book.record(Tally { requests: 1, calls: 3, input: 2000, output: 70, thinking: 0 });
        let last = s.last.unwrap();
        assert_eq!(last.input + last.output + last.thinking, 2070);
        assert_eq!(s.today.requests, 2);
        assert_eq!(s.total.input + s.total.output + s.total.thinking, 3130);
        let again = UsageBook::load(path.clone()).summary();
        assert_eq!(again.total.calls, 5);
        let _ = std::fs::remove_file(path);
    }
}
