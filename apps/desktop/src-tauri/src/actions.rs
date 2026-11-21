//! App actions triggered by hotkeys, the tray menu or the UI.
//! Window-level actions run here; the rest are forwarded to the island UI as an "action" event.

use crate::brain::Brain;
use crate::island::{self, ISLAND, IslandState};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

pub const SETTINGS: &str = "settings";
pub const CHAT: &str = "chat";

pub fn dispatch<R: Runtime>(app: &AppHandle<R>, action: &str) {
    match action {
        "toggle-island" => toggle_island(app),
        "settings" | "settings:models" => {
            let page = action.strip_prefix("settings:");
            if let Err(e) = open_settings(app, page) {
                eprintln!("Could not open settings: {e}");
            }
        }
        "chat" => {
            if let Err(e) = open_chat(app) {
                eprintln!("Could not open the chat: {e}");
            }
        }
        "talk" => {
            show_island(app);
            crate::voice::toggle(app);
        }
        "stop" => {
            if let Some(brain) = app.try_state::<Brain>() {
                brain.agent.stop();
            }
            if let Some(voice) = app.try_state::<crate::voice::VoiceSlot>() {
                voice.stop();
            }
            if let Some(speech) = app.try_state::<crate::speak::Speech>() {
                speech.stop();
            }
            let _ = app.emit_to(ISLAND, "action", action);
        }
        "type" => {
            // The text box needs the keyboard: show the island, let it take the mouse and focus it.
            show_island(app);
            set_capture(app, true);
            let _ = app.emit_to(ISLAND, "action", action);
        }
        _ => {
            show_island(app);
            let _ = app.emit_to(ISLAND, "action", action);
        }
    }
}

pub fn show_island<R: Runtime>(app: &AppHandle<R>) {
    if !island_ready(app) {
        return;
    }
    if let Some(window) = app.get_webview_window(ISLAND) {
        let _ = window.show();
    }
}

fn island_ready<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<IslandState>().ready.load(Ordering::Relaxed)
}

fn toggle_island<R: Runtime>(app: &AppHandle<R>) {
    if !island_ready(app) {
        return;
    }
    if let Some(window) = app.get_webview_window(ISLAND) {
        if window.is_visible().unwrap_or(false) {
            set_capture(app, false);
            let _ = window.hide();
        } else {
            let _ = island::fit_to_primary(&window);
            let _ = window.show();
        }
    }
}

/// While capturing, the whole island takes the mouse and has keyboard focus (text box open).
pub fn set_capture<R: Runtime>(app: &AppHandle<R>, capture: bool) {
    app.state::<IslandState>().capture_all.store(capture, Ordering::Relaxed);
    if capture && let Some(window) = app.get_webview_window(ISLAND) {
        let _ = window.set_ignore_cursor_events(false);
        let _ = window.set_focus();
    }
}

/// "Liquid glass": Windows blurs the desktop behind the window (acrylic) and the page lays a dark tint and its
/// own glass surfaces over it (.app-window in tokens.css). Where the effect is not available the window is
/// simply darker.
fn glass() -> tauri::utils::config::WindowEffectsConfig {
    tauri::window::EffectsBuilder::new()
        .effect(tauri::window::Effect::Acrylic)
        .color(tauri::window::Color(14, 16, 26, 110))
        .build()
}

/// Opens (or brings forward) the settings window, optionally on one page such as "models".
pub fn open_settings<R: Runtime>(app: &AppHandle<R>, page: Option<&str>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS) {
        if let Some(page) = page {
            let _ = app.emit_to(SETTINGS, "settings-page", page);
        }
        window.unminimize()?;
        window.show()?;
        return window.set_focus();
    }
    let url = match page {
        Some(page) => format!("index.html?view=settings&page={page}"),
        None => "index.html?view=settings".into(),
    };
    WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App(url.into()))
        .title("Cloudy Settings")
        .visible(false)
        .inner_size(960.0, 660.0)
        .min_inner_size(780.0, 540.0)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .effects(glass())
        .center()
        .build()?;
    Ok(())
}

pub fn open_chat<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(CHAT) {
        window.unminimize()?;
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, CHAT, WebviewUrl::App("index.html?view=chat".into()))
        .title("Cloudy Chat")
        .visible(false)
        .inner_size(480.0, 720.0)
        .min_inner_size(380.0, 480.0)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .effects(glass())
        .center()
        .build()?;
    Ok(())
}