//! What this PC has (default browser, apps by kind), scanned at first start and again after a week or on request;
//! the assistant gets it in its prompt so it uses the installed apps. Also the preferences it learned
//! ("preferred_music_app": Spotify), shown in Settings so they can be removed.

use crate::brain::Brain;
use anytask_control::inventory::{self, Inventory};
use anytask_core::CoreEvent;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

const RESCAN_AFTER_MS: i64 = 7 * 24 * 3600 * 1000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preference {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PcView {
    pub scanned_at: i64,
    /// The lines the assistant sees.
    pub profile: String,
    pub app_count: usize,
    pub preferences: Vec<Preference>,
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("pc-profile.json")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn load(data_dir: &Path) -> Option<Inventory> {
    serde_json::from_str(&std::fs::read_to_string(path(data_dir)).ok()?).ok()
}

/// Scans (a hidden PowerShell call), saves and hands the profile to the assistant.
fn scan_and_apply(app: &AppHandle, data_dir: &Path) -> Result<Inventory, String> {
    let inv = inventory::scan(now_ms())?;
    if let Ok(text) = serde_json::to_string(&inv) {
        let _ = std::fs::write(path(data_dir), text);
    }
    apply(app, &inv);
    let brain = app.state::<Brain>();
    brain.agent.events().emit(CoreEvent::Log { turn_id: None, source: "pc".into(), text: format!("scanned: {} apps, default browser {}", inv.apps.len(), inv.default_browser.clone().unwrap_or_else(|| "unknown".into())) });
    Ok(inv)
}

fn apply(app: &AppHandle, inv: &Inventory) {
    app.state::<Brain>().agent.set_pc_profile(inv.profile());
    let _ = app.emit("pc-changed", view(app, inv));
}

/// At start: use the saved scan, and scan again (in the background) when there is none or it is a week old.
pub fn start(app: &AppHandle, data_dir: PathBuf) {
    let saved = load(&data_dir);
    if let Some(inv) = &saved {
        apply(app, inv);
    }
    if saved.is_none_or(|inv| now_ms() - inv.scanned_at > RESCAN_AFTER_MS) {
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = scan_and_apply(&app, &data_dir);
        });
    }
}

pub fn view(app: &AppHandle, inv: &Inventory) -> PcView {
    let preferences = app
        .state::<Brain>()
        .agent
        .memory()
        .facts()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.key.starts_with("preferred_"))
        .map(|f| Preference { key: f.key, value: f.value })
        .collect();
    PcView { scanned_at: inv.scanned_at, profile: inv.profile(), app_count: inv.apps.len(), preferences }
}

pub fn current(app: &AppHandle, data_dir: &Path) -> PcView {
    view(app, &load(data_dir).unwrap_or_default())
}

pub fn rescan(app: &AppHandle, data_dir: &Path) -> Result<PcView, String> {
    let inv = scan_and_apply(app, data_dir)?;
    Ok(view(app, &inv))
}
