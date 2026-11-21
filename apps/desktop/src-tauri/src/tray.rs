//! Tray icon and its menu.

use crate::actions;
use tauri::menu::{MenuBuilder, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Runtime};

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = MenuBuilder::new(app)
        .item(&MenuItem::with_id(app, "toggle-island", "Show / hide island", true, None::<&str>)?)
        .item(&MenuItem::with_id(app, "type", "Type a request", true, None::<&str>)?)
        .item(&MenuItem::with_id(app, "chat", "Chat", true, None::<&str>)?)
        .item(&MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&MenuItem::with_id(app, "demo", "Animation demo", true, None::<&str>)?)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&MenuItem::with_id(app, "quit", "Quit Cloudy", true, None::<&str>)?)
        .build()?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Cloudy")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if id == "quit" {
                app.exit(0);
            } else {
                actions::dispatch(app, id);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                actions::dispatch(tray.app_handle(), "toggle-island");
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}