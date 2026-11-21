//! Global keyboard shortcuts from the settings. Each shortcut maps to an action name that the app dispatches.

use crate::settings::Hotkeys;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

/// Shortcut id -> action name, filled by `register`.
#[derive(Default)]
pub struct HotkeyMap(pub Mutex<HashMap<u32, &'static str>>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyProblem {
    pub action: String,
    pub shortcut: String,
    pub reason: String,
}

/// Parses a shortcut such as "Ctrl+Alt+Space"; the error text is shown in the settings window.
pub fn parse(text: &str) -> Result<Shortcut, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("Empty shortcut.".into());
    }
    trimmed.parse::<Shortcut>().map_err(|e| format!("Not a valid shortcut: {e}"))
}

/// Checks all shortcuts without registering: invalid ones and duplicates.
pub fn validate(hotkeys: &Hotkeys) -> Vec<HotkeyProblem> {
    let mut problems = Vec::new();
    let mut seen: HashMap<u32, &str> = HashMap::new();
    for (action, text) in hotkeys.entries() {
        match parse(text) {
            Ok(shortcut) => {
                if let Some(other) = seen.insert(shortcut.id(), action) {
                    problems.push(HotkeyProblem {
                        action: action.into(),
                        shortcut: text.into(),
                        reason: format!("Same shortcut as \"{other}\"."),
                    });
                }
            }
            Err(reason) => problems.push(HotkeyProblem { action: action.into(), shortcut: text.into(), reason }),
        }
    }
    problems
}

/// Replaces all registered shortcuts. Returns the ones that could not be used (invalid, duplicate, or already
/// taken by another program) so the settings window can show them.
pub fn register<R: Runtime>(app: &AppHandle<R>, hotkeys: &Hotkeys) -> Vec<HotkeyProblem> {
    let manager = app.global_shortcut();
    let _ = manager.unregister_all();
    let mut problems = validate(hotkeys);
    let mut map = HashMap::new();
    for (action, text) in hotkeys.entries() {
        if problems.iter().any(|p| p.action == action) {
            continue;
        }
        let Ok(shortcut) = parse(text) else { continue };
        match manager.register(shortcut) {
            Ok(()) => {
                map.insert(shortcut.id(), action);
            }
            Err(e) => problems.push(HotkeyProblem {
                action: action.into(),
                shortcut: text.into(),
                reason: format!("Already used by another program ({e})."),
            }),
        }
    }
    if let Ok(mut current) = app.state::<HotkeyMap>().0.lock() {
        *current = map;
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_shortcuts() {
        assert!(parse("Ctrl+Alt+Space").is_ok());
        assert!(parse("ctrl+shift+T").is_ok());
        assert!(parse("").is_err());
        assert!(parse("Ctrl+Alt+NotAKey").is_err());
    }

    #[test]
    fn defaults_are_valid_and_unique() {
        assert!(validate(&Hotkeys::default()).is_empty());
    }

    #[test]
    fn duplicates_and_invalid_are_reported() {
        let hotkeys = Hotkeys { talk: "Ctrl+Alt+Q".into(), stop: "ctrl+alt+q".into(), open_settings: "???".into(), ..Hotkeys::default() };
        let problems = validate(&hotkeys);
        assert!(problems.iter().any(|p| p.action == "stop" && p.reason.contains("talk")));
        assert!(problems.iter().any(|p| p.action == "settings"));
    }
}
