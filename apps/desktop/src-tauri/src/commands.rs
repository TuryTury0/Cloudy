//! Commands the UI can call.

use crate::actions;
use crate::brain::{Brain, search_key_name};
use crate::hotkeys::{self, HotkeyProblem};
use crate::island::{self, ISLAND, IslandState, Rect};
use crate::models;
use crate::settings::Settings;
use crate::voice::VoiceSlot;
use anytask_control::NativeSystem;
use anytask_core::tools::system::SystemOps;
use anytask_core::{Identity, TurnOutcome};
use anytask_llm::{ConfiguredModel, GenerateRequest, GenerateResponse, Generator, LlmError, Message, ModelConfig};
use anytask_storage::{Turn, model_key_name};
use anytask_web::SearchProvider;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

/// Current settings and where they are saved.
pub struct SettingsStore {
    pub path: PathBuf,
    pub settings: Mutex<Settings>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReport {
    pub saved: bool,
    pub problems: Vec<HotkeyProblem>,
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub settings_path: String,
}

/// The UI reports where its clickable glass surfaces are (CSS px, window relative).
#[tauri::command]
pub fn set_hit_regions(regions: Vec<Rect>, state: State<'_, IslandState>) {
    if let Ok(mut current) = state.regions.lock() {
        *current = regions;
    }
}

/// A window's page has loaded. Windows are shown only now, so a page that fails to load (for example when the
/// dev server is not running) never covers the screen with an error page.
#[tauri::command]
pub fn window_ready(window: WebviewWindow, island_state: State<'_, IslandState>, store: State<'_, SettingsStore>) {
    if window.label() == ISLAND {
        island_state.ready.store(true, Ordering::Relaxed);
        if current(&store).show_island {
            let _ = island::fit_to_primary(&window);
            let _ = window.show();
        }
    } else {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
pub fn island_capture(capture: bool, app: AppHandle) {
    actions::set_capture(&app, capture);
}

/// Async on purpose: actions may open a window, and building a window inside a synchronous command blocks
/// the main thread on Windows (the new window stays white).
#[tauri::command]
pub async fn run_action(action: String, app: AppHandle) {
    actions::dispatch(&app, &action);
}

#[tauri::command]
pub fn get_settings(store: State<'_, SettingsStore>) -> Settings {
    current(&store)
}

pub(crate) fn current(store: &SettingsStore) -> Settings {
    store.settings.lock().map(|s| s.clone()).unwrap_or_default()
}

/// Saves settings changed by a command (not the hotkeys) and tells every window.
pub(crate) fn apply_settings(app: &AppHandle, store: &SettingsStore, settings: Settings) -> Result<Settings, String> {
    apply(app, store, settings)
}

/// Points the models and the web tools at the user's proxy as the settings say.
pub(crate) fn apply_proxy(app: &AppHandle, settings: &Settings) {
    let brain = app.state::<Brain>();
    let url = settings.proxy.url();
    let problem = [
        brain.llm.set_proxy(url.as_deref(), &settings.proxy.models).err(),
        brain.net.set_proxy(url.as_deref().filter(|_| settings.proxy.web)).err(),
    ];
    for problem in problem.into_iter().flatten() {
        brain.agent.events().emit(anytask_core::CoreEvent::Log { turn_id: None, source: "proxy".into(), text: problem });
    }
}

fn apply(app: &AppHandle, store: &SettingsStore, settings: Settings) -> Result<Settings, String> {
    settings.save(&store.path).map_err(|e| format!("Could not save the settings: {e}"))?;
    anytask_core::lang::set(&settings.language);
    apply_proxy(app, &settings);
    if let Ok(mut current) = store.settings.lock() {
        *current = settings.clone();
    }
    let _ = app.emit("settings-changed", &settings);
    Ok(settings)
}

/// Saves the settings and applies the shortcuts at once. Shortcuts that could not be registered are reported
/// (the rest still work).
#[tauri::command]
pub fn save_settings(settings: Settings, app: AppHandle, store: State<'_, SettingsStore>) -> SaveReport {
    if let Err(e) = settings.save(&store.path) {
        return SaveReport { saved: false, problems: Vec::new(), error: Some(e.to_string()) };
    }
    let problems = hotkeys::register(&app, &settings.hotkeys);
    anytask_core::lang::set(&settings.language);
    apply_proxy(&app, &settings);
    // Start with Windows: written only when the switch changed (the entry names this program's path).
    let was = store.settings.lock().map(|s| s.start_with_windows).unwrap_or(false);
    let mut error = None;
    if was != settings.start_with_windows
        && let Err(e) = crate::autostart::set(settings.start_with_windows)
    {
        error = Some(e);
    }
    if let Ok(mut current) = store.settings.lock() {
        *current = settings.clone();
    }
    let _ = app.emit("settings-changed", &settings);
    SaveReport { saved: true, problems, error }
}

/// Checks one shortcut while the user records it.
#[tauri::command]
pub fn validate_hotkey(shortcut: String) -> Result<(), String> {
    hotkeys::parse(&shortcut).map(|_| ())
}

#[tauri::command]
pub fn app_info(app: AppHandle, store: State<'_, SettingsStore>) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        settings_path: store.path.display().to_string(),
    }
}

/// Runs one typed or spoken request with the active model. Progress arrives as "core-event"s.
#[tauri::command]
pub async fn send_prompt(text: String, brain: State<'_, Brain>, store: State<'_, SettingsStore>) -> Result<TurnOutcome, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("The request is empty.".into());
    }
    let settings = current(&store);
    let agent = brain.agent.clone();
    let model = active_generator(&brain, &settings);
    let identity = Identity { assistant_name: settings.assistant_name, user_name: settings.user_name };
    Ok(agent.run(model, &identity, &text).await)
}

/// The chosen model with its key. Without a model a request still runs as a normal turn, so every window shows it
/// and its reason.
pub(crate) fn active_generator(brain: &Brain, settings: &Settings) -> Arc<dyn Generator> {
    let configured = |config: ModelConfig| -> Arc<dyn Generator> {
        let api_key = brain.secrets.get(&model_key_name(&config.id)).unwrap_or_default();
        Arc::new(ConfiguredModel { client: brain.llm.clone(), config, api_key })
    };
    let Some(active) = settings.active_model().cloned() else { return Arc::new(NoModel) };
    let backup = settings.models.iter().find(|m| m.id == settings.backup_model && m.id != active.id).cloned();
    match backup {
        Some(backup) => Arc::new(anytask_llm::WithBackup::new(configured(active), configured(backup))),
        None => configured(active),
    }
}

struct NoModel;

#[async_trait::async_trait]
impl Generator for NoModel {
    async fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
        Err(LlmError::Answer("Add one in Settings > Models.".into()))
    }

    fn label(&self) -> String {
        "No model".into()
    }
}

/// A typed message: Live answers first (by voice, the text shows in the chat); without Live the brain answers.
#[tauri::command]
pub fn ask(text: String, app: AppHandle) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("The request is empty.".into());
    }
    crate::voice::ask(&app, text);
    Ok(())
}

/// Stops the running request and closes a voice conversation.
#[tauri::command]
pub fn stop_turn(brain: State<'_, Brain>, voice: State<'_, VoiceSlot>, speech: State<'_, crate::speak::Speech>) -> bool {
    let voice = voice.stop();
    speech.stop();
    brain.agent.stop() || voice
}

/// Stops the running job (the island's Stop button); a voice conversation stays open and says it stopped.
#[tauri::command]
pub fn stop_task(brain: State<'_, Brain>, speech: State<'_, crate::speak::Speech>) -> bool {
    speech.stop();
    let work = brain.agent.stop_work();
    brain.agent.stop() || work
}

/// Opens or closes a voice conversation (the island's microphone and the talk shortcut).
#[tauri::command]
pub fn toggle_voice(app: AppHandle) {
    crate::voice::toggle(&app);
}

/// The latest turns, oldest first.
#[tauri::command]
pub fn conversation(limit: Option<usize>, brain: State<'_, Brain>) -> Result<Vec<Turn>, String> {
    brain.agent.memory().recent_turns(limit.unwrap_or(60).min(500)).map_err(|e| e.to_string())
}

/// Copies text for the UI (the island has no keyboard focus, so the web clipboard API cannot be used there).
#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    NativeSystem.clipboard_write(&text)
}

/// Opens an external https/http link safely in the default browser.
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !trimmed.starts_with("https://") && !trimmed.starts_with("http://") {
        return Err("Only http:// or https:// URLs can be opened.".into());
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || ":/?#[]@!$()*+,;=-._~%".contains(c)) {
        return Err("URL contains unsupported characters.".into());
    }
    NativeSystem.open(trimmed)
}

#[tauri::command]
pub fn answer_confirm(id: u64, allow: bool, brain: State<'_, Brain>) {
    brain.confirmer.answer(id, allow);
}

/// Adds or changes a model. A key, when given, goes to the OS keychain only.
#[tauri::command]
pub fn save_model(
    model: ModelConfig,
    api_key: Option<String>,
    app: AppHandle,
    store: State<'_, SettingsStore>,
    brain: State<'_, Brain>,
) -> Result<Settings, String> {
    models::validate(&model)?;
    let mut settings = current(&store);
    let saved = models::upsert(&mut settings, model, now_ms());
    if let Some(key) = api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        brain.secrets.set(&model_key_name(&saved.id), key).map_err(|e| format!("Could not save the key in the keychain: {e}"))?;
    }
    apply(&app, &store, settings)
}

#[tauri::command]
pub fn delete_model(id: String, app: AppHandle, store: State<'_, SettingsStore>, brain: State<'_, Brain>) -> Result<Settings, String> {
    let mut settings = current(&store);
    models::remove(&mut settings, &id);
    let _ = brain.secrets.delete(&model_key_name(&id));
    apply(&app, &store, settings)
}

/// Saves the proxy settings (address, which models, voice, web second tries) and applies them at once.
#[tauri::command]
pub fn set_proxy(proxy: crate::settings::ProxySettings, app: AppHandle, store: State<'_, SettingsStore>) -> Result<Settings, String> {
    if let Some(url) = proxy.url() {
        anytask_web::Net::new().set_proxy(Some(&url)).map_err(|e| format!("This proxy address is not valid: {e}"))?;
    }
    let mut settings = current(&store);
    settings.proxy = proxy;
    apply(&app, &store, settings)
}

/// Picks the model that takes over when the chosen one is busy ("" = none).
#[tauri::command]
pub fn set_backup_model(id: String, app: AppHandle, store: State<'_, SettingsStore>) -> Result<Settings, String> {
    let mut settings = current(&store);
    if !id.is_empty() && !settings.models.iter().any(|m| m.id == id) {
        return Err("That model no longer exists.".into());
    }
    settings.backup_model = id;
    apply(&app, &store, settings)
}

#[tauri::command]
pub fn set_active_model(id: String, app: AppHandle, store: State<'_, SettingsStore>) -> Result<Settings, String> {
    let mut settings = current(&store);
    if !settings.models.iter().any(|m| m.id == id) {
        return Err("That model no longer exists.".into());
    }
    settings.active_model = id;
    apply(&app, &store, settings)
}

/// Which models have a saved key (the keys themselves never leave the keychain).
#[tauri::command]
pub fn model_keys(store: State<'_, SettingsStore>, brain: State<'_, Brain>) -> HashMap<String, bool> {
    current(&store).models.iter().map(|m| (m.id.clone(), brain.secrets.get(&model_key_name(&m.id)).is_some())).collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTest {
    pub ms: u64,
    pub reply: String,
}

/// Sends one tiny request to check the address, model id and key. Uses the typed key, else the saved one.
#[tauri::command]
pub async fn test_model(model: ModelConfig, api_key: Option<String>, brain: State<'_, Brain>) -> Result<ModelTest, String> {
    models::validate(&model)?;
    let key = api_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| brain.secrets.get(&model_key_name(&model.id)))
        .unwrap_or_default();
    let request = GenerateRequest {
        system: "Reply with exactly: OK".into(),
        messages: vec![Message::user("ping")],
        tools: Vec::new(),
        allow_tools: false,
        max_output_tokens: 1500,
        thinking: None,
    };
    let started = Instant::now();
    // The result goes to the log too, so a failing model can be diagnosed from Settings > Logs.
    let log = |text: String| brain.agent.events().emit(anytask_core::CoreEvent::Log { turn_id: None, source: "models".into(), text });
    let response = match brain.llm.generate(&model, &key, &request).await {
        Ok(response) => response,
        Err(e) => {
            log(format!("test {} ({}) FAILED: {e}", model.name, model.model_id));
            return Err(e.to_string());
        }
    };
    log(format!("test {} ({}) ok in {} ms", model.name, model.model_id, started.elapsed().as_millis()));
    let reply = response.text();
    Ok(ModelTest {
        ms: started.elapsed().as_millis() as u64,
        reply: if reply.is_empty() { "(empty reply, but the connection works)".into() } else { reply.chars().take(120).collect() },
    })
}

#[derive(Serialize)]
pub struct SearchKeys {
    pub tavily: bool,
    pub brave: bool,
}

#[tauri::command]
pub fn search_keys(brain: State<'_, Brain>) -> SearchKeys {
    let has = |p| brain.secrets.get(search_key_name(p)).is_some();
    SearchKeys { tavily: has(SearchProvider::Tavily), brave: has(SearchProvider::Brave) }
}

/// Saves (or, when empty, removes) a search API key in the keychain.
#[tauri::command]
pub fn set_search_key(provider: SearchProvider, key: String, brain: State<'_, Brain>) -> Result<SearchKeys, String> {
    let name = search_key_name(provider);
    let key = key.trim();
    if key.is_empty() { brain.secrets.delete(name)? } else { brain.secrets.set(name, key)? }
    Ok(search_keys(brain))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The latest log lines, oldest first (the Logs page; new lines arrive as "log-line" events).
#[tauri::command]
pub fn logs_recent(limit: Option<usize>, brain: State<'_, Brain>) -> Vec<crate::logger::LogLine> {
    brain.logger.recent(limit.unwrap_or(500).min(2000))
}

#[tauri::command]
pub fn open_logs_folder(brain: State<'_, Brain>) -> Result<(), String> {
    NativeSystem.open(&brain.logger.dir().display().to_string())
}

/// The app tips (skills) the screen agent knows: built-in ones and the user's.
#[tauri::command]
pub fn skills_list(brain: State<'_, Brain>) -> Vec<anytask_core::skills::Skill> {
    brain.skills.all()
}

/// Opens the user's skills folder (made on first use, with a short guide in it).
#[tauri::command]
pub fn open_skills_folder(brain: State<'_, Brain>) -> Result<(), String> {
    let dir = brain.skills.dir().ok_or("No skills folder.")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not make the skills folder: {e}"))?;
    let guide = dir.join("HOW-TO.txt");
    if !guide.exists() {
        let _ = std::fs::write(&guide, SKILLS_GUIDE);
    }
    NativeSystem.open(&dir.display().to_string())
}

const SKILLS_GUIDE: &str = "App tips (skills) for AnyTask's screen agent\r\n\
\r\n\
Each .md file in this folder is one skill. Its first line names the words that bring it in (looked for in the\r\n\
job's goal and in the title of the window in front); the rest are the tips, short and plain. Example, spotify.md:\r\n\
\r\n\
keys: spotify, اسپاتیفای\r\n\
# Spotify\r\n\
- My playlists are under Your Library on the left.\r\n\
\r\n\
A file with the name of a built-in skill (browser, youtube, explorer, settings, chat-apps, notepad) replaces it.\r\n\
Changes count from the next job; no restart is needed.\r\n";

/// MCP servers, their tools and the secrets they need (the Tools page).
#[tauri::command]
pub fn mcp_report(brain: State<'_, Brain>) -> crate::mcp::McpReport {
    brain.mcp.report()
}

#[tauri::command]
pub async fn mcp_save_server(
    name: String,
    server: anytask_mcp::ServerConfig,
    previous: Option<String>,
    app: AppHandle,
    brain: State<'_, Brain>,
) -> Result<crate::mcp::McpReport, String> {
    brain.mcp.save_server(&app, &name, server, previous.as_deref()).await
}

#[tauri::command]
pub async fn mcp_remove_server(name: String, app: AppHandle, brain: State<'_, Brain>) -> Result<crate::mcp::McpReport, String> {
    let report = brain.mcp.remove_server(&app, &name).await?;
    brain.mcp_setup.cleanup_server_files(&name);
    Ok(report)
}

#[tauri::command]
pub async fn mcp_remove_server_full(name: String, clean_secrets: bool, app: AppHandle, brain: State<'_, Brain>) -> Result<crate::mcp::McpReport, String> {
    let report = brain.mcp.remove_server_with_secrets(&app, &name, clean_secrets).await?;
    brain.mcp_setup.cleanup_server_files(&name);
    Ok(report)
}

#[tauri::command]
pub async fn mcp_set_disabled(name: String, disabled: bool, app: AppHandle, brain: State<'_, Brain>) -> Result<crate::mcp::McpReport, String> {
    brain.mcp.set_disabled(&app, &name, disabled).await
}

#[tauri::command]
pub async fn mcp_set_server_tools(name: String, tools: Vec<String>, app: AppHandle, brain: State<'_, Brain>) -> Result<crate::mcp::McpReport, String> {
    brain.mcp.set_server_tools(&app, &name, tools).await
}

#[tauri::command]
pub async fn mcp_test_server(name: String, app: AppHandle, brain: State<'_, Brain>) -> Result<anytask_mcp::ServerTestReport, String> {
    brain.mcp.test_server(&app, &name).await
}

#[tauri::command]
pub async fn mcp_test_tool(server: String, tool: String, args: serde_json::Value, brain: State<'_, Brain>) -> Result<anytask_mcp::ToolTestReport, String> {
    Ok(brain.mcp.test_tool(&server, &tool, args).await)
}

#[tauri::command]
pub async fn mcp_reload(app: AppHandle, brain: State<'_, Brain>) -> Result<crate::mcp::McpReport, String> {
    Ok(brain.mcp.reload(&app).await)
}

/// Opens mcp-servers.json in the default editor (for pasting a Claude Desktop config).
#[tauri::command]
pub fn mcp_open_file(brain: State<'_, Brain>) -> Result<(), String> {
    brain.mcp.config()?;
    NativeSystem.open(&brain.mcp.path.display().to_string())
}

/// Saves (or, when empty, removes) a secret that the file names as "secret:NAME".
#[tauri::command]
pub fn mcp_set_secret(name: String, value: String, brain: State<'_, Brain>) -> Result<crate::mcp::McpReport, String> {
    let key = crate::mcp::secret_key_name(name.trim());
    let value = value.trim();
    if value.is_empty() { brain.secrets.delete(&key)? } else { brain.secrets.set(&key, value)? }
    Ok(brain.mcp.report())
}

#[tauri::command]
pub fn mcp_catalog() -> Vec<crate::mcp_catalog::CatalogSummary> {
    crate::mcp_catalog::summaries()
}

#[tauri::command]
pub fn mcp_setup_state(brain: State<'_, Brain>) -> crate::mcp_setup::SetupStateView {
    brain.mcp_setup.view()
}

#[tauri::command]
pub fn mcp_setup_clear(app: AppHandle, brain: State<'_, Brain>) -> crate::mcp_setup::SetupStateView {
    brain.mcp_setup.clear(&app)
}

#[tauri::command]
pub fn mcp_setup_preset(preset_id: String, app: AppHandle, brain: State<'_, Brain>) -> Result<crate::mcp_setup::SetupStateView, String> {
    brain.mcp_setup.start_preset(&app, &preset_id)
}

#[tauri::command]
pub async fn mcp_setup_chat(text: String, app: AppHandle, brain: State<'_, Brain>) -> Result<crate::mcp_setup::SetupStateView, String> {
    brain.mcp_setup.chat(&app, &text).await
}

#[tauri::command]
pub async fn mcp_setup_diagnose(server: String, app: AppHandle, brain: State<'_, Brain>) -> Result<crate::mcp_setup::SetupStateView, String> {
    brain.mcp_setup.diagnose(&app, &server).await
}

#[tauri::command]
pub async fn mcp_setup_apply(
    plan: crate::mcp_catalog::SetupPlan,
    values: HashMap<String, String>,
    files: Vec<crate::mcp_setup::UploadedFile>,
    app: AppHandle,
    brain: State<'_, Brain>,
) -> Result<crate::mcp_setup::ApplyOutcome, String> {
    brain.mcp_setup.apply_plan(&app, plan, values, files).await
}

#[tauri::command]
pub async fn mcp_install_prereq(kind: String, app: AppHandle, brain: State<'_, Brain>) -> Result<crate::mcp::McpReport, String> {
    brain.mcp_setup.install_prereq(&kind).await?;
    let report = brain.mcp.report();
    let _ = app.emit("mcp-changed", &report);
    Ok(report)
}

/// The wake word: trained or not, on or off, listening now or not.
#[tauri::command]
pub fn wake_status(wake: State<'_, crate::wake::WakeService>) -> crate::wake::WakeStatus {
    wake.status()
}

#[tauri::command]
pub fn wake_set_enabled(enabled: bool, app: AppHandle, wake: State<'_, crate::wake::WakeService>) -> Result<crate::wake::WakeStatus, String> {
    wake.set_enabled(&app, enabled)
}

#[tauri::command]
pub fn wake_remove(app: AppHandle, wake: State<'_, crate::wake::WakeService>) -> Result<crate::wake::WakeStatus, String> {
    wake.remove(&app)
}

/// Starts training a new wake word; progress arrives as "wake-training" events.
#[tauri::command]
pub fn wake_train(label: String, app: AppHandle, wake: State<'_, crate::wake::WakeService>) -> Result<(), String> {
    wake.train(&app, label)
}

#[tauri::command]
pub fn wake_cancel_training(wake: State<'_, crate::wake::WakeService>) {
    wake.cancel_training();
}

/// Phones: on or off, this PC's address, paired phones and which are connected.
#[tauri::command]
pub async fn phone_status(phone: State<'_, Arc<crate::phone::Phone>>) -> Result<crate::phone::PhoneStatus, String> {
    Ok(phone.status().await)
}

#[tauri::command]
pub async fn phone_set_enabled(enabled: bool, phone: State<'_, Arc<crate::phone::Phone>>) -> Result<crate::phone::PhoneStatus, String> {
    Ok(phone.inner().clone().set_enabled(enabled).await)
}

/// A one-time QR code for pairing a phone.
#[tauri::command]
pub async fn phone_pairing(phone: State<'_, Arc<crate::phone::Phone>>) -> Result<crate::phone::PairingView, String> {
    phone.pairing().await
}

#[tauri::command]
pub async fn phone_close_pairing(phone: State<'_, Arc<crate::phone::Phone>>) -> Result<(), String> {
    phone.close_pairing().await;
    Ok(())
}

#[tauri::command]
pub async fn phone_forget(key: String, phone: State<'_, Arc<crate::phone::Phone>>) -> Result<crate::phone::PhoneStatus, String> {
    Ok(phone.forget(&key).await)
}

/// A file chosen on the Phone page, sent in pieces from the page (base64), then offered to the connected phone,
/// which downloads it into its Downloads/AnyTask. Kept in the temp folder until the app starts again.
#[tauri::command]
pub async fn phone_send_piece(id: String, name: String, offset: u64, data: String, last: bool, phone: State<'_, Arc<crate::phone::Phone>>) -> Result<serde_json::Value, String> {
    let folder = std::env::temp_dir().join("anytask-to-phone");
    let staged = crate::phone_remote::stage_piece(&folder, &id, &name, offset, &data, last);
    let result = match staged {
        Ok(Some(path)) => phone.offer_file(&path.to_string_lossy()).await,
        Ok(None) => return Ok(serde_json::json!({ "offset": offset })),
        Err(e) => Err(e),
    };
    // A send that failed leaves no copy behind.
    if result.is_err() {
        let safe: String = id.chars().filter(char::is_ascii_alphanumeric).take(32).collect();
        if !safe.is_empty() {
            let _ = std::fs::remove_dir_all(folder.join(safe));
        }
    }
    result
}

/// Rings the connected phone (to find it), or stops the ringing.
#[tauri::command]
pub async fn phone_ring(stop: bool, phone: State<'_, Arc<crate::phone::Phone>>) -> Result<serde_json::Value, String> {
    phone.call_device(if stop { "stopRing" } else { "ring" }, serde_json::json!({})).await
}

/// What AnyTask v1 left on this PC (no keys), and whether it was imported already.
#[tauri::command]
pub fn v1_report(app: AppHandle) -> crate::migrate::V1Report {
    crate::migrate::report(&app)
}

/// Copies v1's settings, models and keys, memory, MCP servers and wake word into this version.
#[tauri::command]
pub async fn v1_import(app: AppHandle) -> Result<Vec<String>, String> {
    crate::migrate::import(&app).await
}

/// Whether the voice has its own key (the key itself never leaves the keychain).
#[tauri::command]
pub fn voice_key_saved(brain: State<'_, Brain>) -> bool {
    brain.secrets.get(crate::voice::VOICE_KEY).is_some()
}

/// Saves (or, when empty, removes) the voice's own Gemini key.
#[tauri::command]
pub fn set_voice_key(key: String, brain: State<'_, Brain>) -> Result<bool, String> {
    let key = key.trim();
    if key.is_empty() { brain.secrets.delete(crate::voice::VOICE_KEY)? } else { brain.secrets.set(crate::voice::VOICE_KEY, key)? }
    Ok(!key.is_empty())
}

/// What the assistant knows about this PC and the preferences it learned.
#[tauri::command]
pub fn pc_view(app: AppHandle) -> Result<crate::pc::PcView, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::pc::current(&app, &data_dir))
}

/// Scans the apps again (after installing something new).
#[tauri::command]
pub async fn pc_rescan(app: AppHandle) -> Result<crate::pc::PcView, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || crate::pc::rescan(&app, &data_dir)).await.map_err(|e| e.to_string())?
}

/// Forgets a learned preference (the assistant will ask again).
#[tauri::command]
pub fn forget_preference(key: String, app: AppHandle, brain: State<'_, Brain>) -> Result<crate::pc::PcView, String> {
    brain.agent.memory().forget_fact(&key).map_err(|e| e.to_string())?;
    pc_view(app)
}

/// Train mode: on or off, and everything learned so far.
#[tauri::command]
pub fn learn_summary(brain: State<'_, Brain>) -> anytask_core::LearnSummary {
    brain.learner.summary()
}

#[tauri::command]
pub fn learn_set_enabled(enabled: bool, brain: State<'_, Brain>) -> anytask_core::LearnSummary {
    brain.learner.set_enabled(enabled);
    brain.learner.summary()
}

/// Forgets one lesson (by id), one playbook (by goal), or everything ("all").
#[tauri::command]
pub fn learn_forget(lesson: Option<u64>, playbook: Option<String>, all: Option<bool>, brain: State<'_, Brain>) -> anytask_core::LearnSummary {
    if all == Some(true) {
        brain.learner.clear();
    }
    if let Some(id) = lesson {
        brain.learner.forget_lesson(id);
    }
    if let Some(goal) = playbook {
        brain.learner.forget_playbook(&goal);
    }
    brain.learner.summary()
}

/// Tokens of the last request, today and all time.
#[tauri::command]
pub fn usage_summary(brain: State<'_, Brain>) -> crate::usage::Summary {
    brain.usage.summary()
}