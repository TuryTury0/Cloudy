//! Conversation memory in SQLite: turns (with the actions taken) and durable facts.
//!
//! Recall follows the rules learned in v1: the newest turns are always returned (so "play that song again"
//! means the latest request), and keyword matches are ranked by recency, so something said minutes ago beats an
//! old saved fact.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

const HOUR_MS: f64 = 3_600_000.0;
const DAY_MS: f64 = 86_400_000.0;
const RECENT_IN_RECALL: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("memory lock poisoned")]
    Lock,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: i64,
    pub at: i64,
    pub user: String,
    pub assistant: String,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub key: String,
    pub value: String,
    pub updated: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "source")]
pub enum RecallMatch {
    Conversation { minutes_ago: i64, user: String, assistant: String, score: f64 },
    SavedFact { minutes_ago: i64, key: String, value: String, score: f64 },
}

impl RecallMatch {
    fn score(&self) -> f64 {
        match self {
            Self::Conversation { score, .. } | Self::SavedFact { score, .. } => *score,
        }
    }
    fn minutes_ago(&self) -> i64 {
        match self {
            Self::Conversation { minutes_ago, .. } | Self::SavedFact { minutes_ago, .. } => *minutes_ago,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recall {
    /// Newest turns first.
    pub recent: Vec<Turn>,
    pub matches: Vec<RecallMatch>,
}

pub struct Memory {
    conn: Mutex<Connection>,
}

impl Memory {
    pub fn open(path: &Path) -> Result<Self, MemoryError> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, MemoryError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, MemoryError> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS turns (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 at INTEGER NOT NULL,
                 user TEXT NOT NULL,
                 assistant TEXT NOT NULL,
                 actions TEXT NOT NULL DEFAULT '[]'
             );
             CREATE INDEX IF NOT EXISTS turns_at ON turns(at);
             CREATE TABLE IF NOT EXISTS facts (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL,
                 updated INTEGER NOT NULL,
                 hits INTEGER NOT NULL DEFAULT 1
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T, rusqlite::Error>) -> Result<T, MemoryError> {
        let conn = self.conn.lock().map_err(|_| MemoryError::Lock)?;
        Ok(f(&conn)?)
    }

    pub fn add_turn(&self, at: i64, user: &str, assistant: &str, actions: &[String]) -> Result<i64, MemoryError> {
        let actions = serde_json::to_string(actions).unwrap_or_else(|_| "[]".into());
        self.with(|c| {
            c.execute("INSERT INTO turns (at, user, assistant, actions) VALUES (?1, ?2, ?3, ?4)", params![at, user, assistant, actions])?;
            Ok(c.last_insert_rowid())
        })
    }

    /// The last `n` turns, oldest first (ready to be used as conversation history).
    pub fn recent_turns(&self, n: usize) -> Result<Vec<Turn>, MemoryError> {
        let mut turns = self.with(|c| {
            let mut stmt = c.prepare("SELECT id, at, user, assistant, actions FROM turns ORDER BY id DESC LIMIT ?1")?;
            let rows = stmt.query_map([n as i64], row_to_turn)?;
            rows.collect::<Result<Vec<_>, _>>()
        })?;
        turns.reverse();
        Ok(turns)
    }

    pub fn all_turns(&self) -> Result<Vec<Turn>, MemoryError> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id, at, user, assistant, actions FROM turns ORDER BY id")?;
            let rows = stmt.query_map([], row_to_turn)?;
            rows.collect()
        })
    }

    /// Keys are normalised ("Favorite Editor" -> "favorite_editor").
    pub fn set_fact(&self, at: i64, key: &str, value: &str) -> Result<(), MemoryError> {
        let key = normalize_key(key);
        if key.is_empty() {
            return Ok(());
        }
        self.with(|c| {
            c.execute(
                "INSERT INTO facts (key, value, updated, hits) VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated = excluded.updated, hits = hits + 1",
                params![key, value, at],
            )?;
            Ok(())
        })
    }

    pub fn facts(&self) -> Result<Vec<Fact>, MemoryError> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT key, value, updated FROM facts ORDER BY hits DESC, updated DESC")?;
            let rows = stmt.query_map([], |r| Ok(Fact { key: r.get(0)?, value: r.get(1)?, updated: r.get(2)? }))?;
            rows.collect()
        })
    }

    pub fn fact(&self, key: &str) -> Result<Option<String>, MemoryError> {
        let key = normalize_key(key);
        self.with(|c| c.query_row("SELECT value FROM facts WHERE key = ?1", [key], |r| r.get(0)).optional())
    }

    /// Removes a saved fact; true when it existed.
    pub fn forget_fact(&self, key: &str) -> Result<bool, MemoryError> {
        let key = normalize_key(key);
        self.with(|c| c.execute("DELETE FROM facts WHERE key = ?1", [key]).map(|n| n > 0))
    }

    pub fn recall(&self, query: &str, limit: usize, now: i64) -> Result<Recall, MemoryError> {
        let turns = self.all_turns()?;
        let facts = self.facts()?;
        let recent: Vec<Turn> = turns.iter().rev().take(RECENT_IN_RECALL).cloned().collect();

        let tokens: Vec<String> = query
            .to_lowercase()
            .split(|c: char| c.is_whitespace() || ",.;:!?'\"()،؟".contains(c))
            .filter(|t| t.chars().count() > 2)
            .map(str::to_string)
            .collect();
        if tokens.is_empty() {
            return Ok(Recall { recent, matches: Vec::new() });
        }
        let hits = |text: &str| {
            let lower = text.to_lowercase();
            tokens.iter().filter(|t| lower.contains(t.as_str())).count() as f64
        };
        let minutes = |at: i64| (now - at).max(0) / 60_000;

        let mut matches = Vec::new();
        for turn in &turns {
            let text = format!("{} {} {}", turn.user, turn.assistant, turn.actions.join(" "));
            let score = hits(&text);
            if score > 0.0 {
                let age = (now - turn.at).max(0) as f64;
                let weight = 1.0 + 3.0 * (-age / (6.0 * HOUR_MS)).exp();
                matches.push(RecallMatch::Conversation {
                    minutes_ago: minutes(turn.at),
                    user: turn.user.clone(),
                    assistant: with_actions(&turn.assistant, &turn.actions),
                    score: round2(score * weight),
                });
            }
        }
        for fact in &facts {
            let score = hits(&format!("{} {}", fact.key, fact.value));
            if score > 0.0 {
                let age = (now - fact.updated).max(0) as f64;
                let weight = 0.8 + 0.7 * (-age / (14.0 * DAY_MS)).exp();
                matches.push(RecallMatch::SavedFact {
                    minutes_ago: minutes(fact.updated),
                    key: fact.key.clone(),
                    value: fact.value.clone(),
                    score: round2(score * weight),
                });
            }
        }
        matches.sort_by(|a, b| b.score().total_cmp(&a.score()).then(a.minutes_ago().cmp(&b.minutes_ago())));
        matches.truncate(limit);
        Ok(Recall { recent, matches })
    }

    /// Short background for the system prompt: the most used facts.
    pub fn context_summary(&self, max_facts: usize) -> Result<String, MemoryError> {
        let facts = self.facts()?;
        Ok(facts.iter().take(max_facts).map(|f| format!("- {}: {}", f.key, f.value)).collect::<Vec<_>>().join("\n"))
    }

    pub fn clear(&self) -> Result<(), MemoryError> {
        self.with(|c| c.execute_batch("DELETE FROM turns; DELETE FROM facts;"))
    }
}

fn row_to_turn(r: &rusqlite::Row<'_>) -> rusqlite::Result<Turn> {
    let actions: String = r.get(4)?;
    Ok(Turn {
        id: r.get(0)?,
        at: r.get(1)?,
        user: r.get(2)?,
        assistant: r.get(3)?,
        actions: serde_json::from_str(&actions).unwrap_or_default(),
    })
}

/// The assistant reply with the actions taken, as stored history ("Done. [actions: ...]").
pub fn with_actions(reply: &str, actions: &[String]) -> String {
    if actions.is_empty() { reply.to_string() } else { format!("{reply} [actions: {}]", actions.join("; ")) }
}

fn normalize_key(key: &str) -> String {
    key.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join("_").chars().take(100).collect()
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const MIN: i64 = 60_000;

    #[test]
    fn facts_can_be_forgotten() {
        let m = Memory::open_in_memory().unwrap();
        m.set_fact(NOW, "Preferred Music App", "Spotify").unwrap();
        assert!(m.forget_fact("preferred_music_app").unwrap());
        assert_eq!(m.fact("preferred_music_app").unwrap(), None);
        assert!(!m.forget_fact("preferred_music_app").unwrap());
    }

    #[test]
    fn turns_round_trip_oldest_first() {
        let m = Memory::open_in_memory().unwrap();
        m.add_turn(NOW - 2 * MIN, "first", "a", &[]).unwrap();
        m.add_turn(NOW - MIN, "second", "b", &["web_search(query=\"x\") -> ok".into()]).unwrap();
        let turns = m.recent_turns(10).unwrap();
        assert_eq!(turns.iter().map(|t| t.user.as_str()).collect::<Vec<_>>(), ["first", "second"]);
        assert_eq!(turns[1].actions.len(), 1);
        assert_eq!(m.recent_turns(1).unwrap()[0].user, "second");
    }

    #[test]
    fn recent_request_beats_old_fact() {
        // The v1 bug: "play that song again" found an old saved favourite instead of the song just played.
        let m = Memory::open_in_memory().unwrap();
        m.set_fact(NOW - 30 * 24 * 60 * MIN, "favorite_song", "Love Me by JMSN").unwrap();
        m.add_turn(NOW - 5 * MIN, "ahang dirty cash ro play kon", "Done.", &["execute_gui_task(goal=\"play the song Dirty Cash\") -> ok".into()]).unwrap();
        m.add_turn(NOW - MIN, "brain ro chi set kardi?", "Gemini.", &[]).unwrap();
        let recall = m.recall("song", 5, NOW).unwrap();
        assert_eq!(recall.recent[0].user, "brain ro chi set kardi?");
        match &recall.matches[0] {
            RecallMatch::Conversation { user, assistant, .. } => {
                assert!(user.contains("dirty cash"));
                assert!(assistant.contains("[actions:"));
            }
            other => panic!("expected the recent conversation first, got {other:?}"),
        }
        assert!(matches!(recall.matches[1], RecallMatch::SavedFact { .. }));
    }

    #[test]
    fn facts_are_normalized_and_updated() {
        let m = Memory::open_in_memory().unwrap();
        m.set_fact(NOW, "Favorite Editor", "VS Code").unwrap();
        m.set_fact(NOW + 1, "favorite editor", "Zed").unwrap();
        assert_eq!(m.fact("FAVORITE_EDITOR").unwrap().as_deref(), Some("Zed"));
        assert_eq!(m.facts().unwrap().len(), 1);
        assert_eq!(m.context_summary(5).unwrap(), "- favorite_editor: Zed");
    }

    #[test]
    fn short_or_empty_queries_return_only_recent() {
        let m = Memory::open_in_memory().unwrap();
        m.add_turn(NOW, "hi", "hello", &[]).unwrap();
        let recall = m.recall("a b", 5, NOW).unwrap();
        assert!(recall.matches.is_empty());
        assert_eq!(recall.recent.len(), 1);
    }

    #[test]
    fn persists_on_disk_and_clears() {
        let path = std::env::temp_dir().join(format!("anytask-memory-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let m = Memory::open(&path).unwrap();
            m.add_turn(NOW, "kept", "yes", &[]).unwrap();
        }
        let m = Memory::open(&path).unwrap();
        assert_eq!(m.recent_turns(5).unwrap()[0].user, "kept");
        m.clear().unwrap();
        assert!(m.recent_turns(5).unwrap().is_empty());
    }
}
