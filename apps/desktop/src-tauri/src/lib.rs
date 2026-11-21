//! AnyTask desktop app: island overlay, tray, global shortcuts, settings and chat windows, and the brain.

mod actions;
mod autostart;
mod brain;
mod commands;
mod decision;
mod hotkeys;
mod island;
mod logger;
mod mcp;
mod mcp_catalog;
mod mcp_setup;
mod migrate;
mod models;
mod pc;
mod phone;
mod phone_remote;
mod phone_tools;
mod reminders;
mod screenshot;
mod settings;
mod speak;
mod tray;
mod updater;
mod usage;
mod voice;
mod wake;

use brain::Brain;
use commands::SettingsStore;
use hotkeys::HotkeyMap;
use island::{ISLAND, IslandState};
use settings::Settings;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::ShortcutState;

pub fn run() {
    tauri::Builder::default()
        // Must be the first plugin: a second launch only brings the running AnyTask forward (one island, one tray icon).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| actions::show_island(app)))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let action = app.state::<HotkeyMap>().0.lock().ok().and_then(|map| map.get(&shortcut.id()).copied());
                    if let Some(action) = action {
                        actions::dispatch(app, action);
                    }
                })
                .build(),
        )
        .manage(IslandState::default())
        .manage(voice::VoiceSlot::default())
        .manage(speak::Speech::default())
        .manage(HotkeyMap::default())
        .invoke_handler(tauri::generate_handler![
            commands::set_hit_regions,
            commands::island_capture,
            commands::run_action,
            commands::get_settings,
            decision::save_decision_model,
            decision::delete_decision_model,
            decision::set_decision,
            decision::decision_keys,
            decision::test_decision_model,
            commands::save_settings,
            commands::validate_hotkey,
            commands::app_info,
            commands::window_ready,
            commands::logs_recent,
            commands::usage_summary,
            commands::open_logs_folder,
            commands::send_prompt,
            commands::stop_turn,
            commands::stop_task,
            commands::set_proxy,
            commands::conversation,
            commands::answer_confirm,
            commands::copy_text,
            commands::open_external_url,
            commands::save_model,
            commands::delete_model,
            commands::set_active_model,
            commands::set_backup_model,
            commands::model_keys,
            commands::test_model,
            commands::search_keys,
            commands::set_search_key,
            commands::toggle_voice,
            commands::ask,
            commands::mcp_report,
            commands::mcp_save_server,
            commands::mcp_remove_server,
            commands::mcp_remove_server_full,
            commands::mcp_set_disabled,
            commands::mcp_set_server_tools,
            commands::mcp_test_server,
            commands::mcp_test_tool,
            commands::mcp_reload,
            commands::mcp_open_file,
            commands::mcp_set_secret,
            commands::mcp_catalog,
            commands::mcp_setup_state,
            commands::mcp_setup_clear,
            commands::mcp_setup_preset,
            commands::mcp_setup_chat,
            commands::mcp_setup_diagnose,
            commands::mcp_setup_apply,
            commands::mcp_install_prereq,
            commands::wake_status,
            commands::wake_set_enabled,
            commands::wake_remove,
            commands::wake_train,
            commands::wake_cancel_training,
            commands::phone_status,
            commands::phone_set_enabled,
            commands::phone_pairing,
            commands::phone_close_pairing,
            commands::phone_forget,
            commands::phone_send_piece,
            commands::phone_ring,
            commands::v1_report,
            commands::v1_import,
            commands::voice_key_saved,
            commands::set_voice_key,
            commands::pc_view,
            commands::pc_rescan,
            commands::forget_preference,
            commands::learn_summary,
            commands::learn_set_enabled,
            commands::learn_forget,
            commands::skills_list,
            commands::open_skills_folder,
            updater::update_status,
            updater::update_check,
            updater::update_install,
        ])
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let settings_path = config_dir.join("settings.json");
            let settings = Settings::load(&settings_path);
            anytask_core::lang::set(&settings.language);
            let data_dir = app.path().app_data_dir()?;
            let brain = Brain::start(app.handle(), &data_dir, &app.path().app_log_dir()?)?;
            app.manage(brain);
            app.manage(wake::WakeService::load(data_dir.join("wake-word.json")));
            pc::start(app.handle(), data_dir.clone());
            let secrets = app.state::<Brain>().secrets.clone();
            let phone = phone::Phone::new(app.handle().clone(), &data_dir, &secrets);
            app.manage(phone.clone());
            app.state::<wake::WakeService>().sync(app.handle());

            let island = app.get_webview_window(ISLAND).expect("island window is declared in tauri.conf.json");
            island::fit_to_primary(&island)?;
            island::start_click_through(&island, &app.state::<IslandState>())?;
            // Hidden until commands::window_ready reports that its page loaded (explicitly: window setup calls
            // above can leave it shown).
            island.hide()?;
            island::start_pointer_watch(app.handle().clone());
            island::follow_desktop_tasks(app.handle().clone());

            let problems = hotkeys::register(app.handle(), &settings.hotkeys);
            commands::apply_proxy(app.handle(), &settings);
            app.manage(SettingsStore { path: settings_path, settings: Mutex::new(settings) });
            tray::create(app.handle())?;
            // After the settings store exists: phones get the assistant's name from it.
            phone.start();
            speak::start(app.handle());
            reminders::start(app.handle());
            updater::start(app.handle());

            if !problems.is_empty() {
                // Shown by the island once its page is ready.
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    let _ = handle.emit_to(ISLAND, "hotkey-problems", &problems);
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running AnyTask");
}