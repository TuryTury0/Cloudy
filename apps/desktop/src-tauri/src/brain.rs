//! The brain inside the desktop app: builds the agent with its tools, picks the active model and its key,
//! forwards every core event to the windows, and asks the user on the island before risky steps.

use crate::actions;
use crate::island::ISLAND;
use crate::logger::Logger;
use crate::mcp::Mcp;
use crate::mcp_setup::McpSetup;
use crate::usage::UsageBook;
use anytask_control::{NativeDesktop, NativeSystem};
use anytask_core::reminders::{CancelReminder, ListReminders, SetReminder};
use anytask_core::tools::apps::{CloseApp, OpenApp};
use anytask_core::tools::desktop::DesktopTask;
use anytask_core::tools::files::{FindFiles, ListDirectory, ReadTextFile, WriteTextFile};
use anytask_core::tools::memory::{RecallMemory, RememberFact};
use anytask_core::tools::system::{ClipboardRead, ClipboardWrite, MediaControl, OpenTarget, SystemOps};
use anytask_core::tools::web::{GoogleSource, ReadBrowserTab, ReadWebPage, SearchApi, WeatherNow, WebSearch};
use anytask_core::{Activity, Agent, Confirmer, EventBus, GetActivity, Learner, Registry, Reminders, UserFolders};
use anytask_llm::LlmClient;
use anytask_storage::{Keychain, Memory, SecretStore};
use anytask_web::SearchProvider;
use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::oneshot;

/// Keychain service name; separate from v1 so both versions can run side by side.
pub const KEYCHAIN_SERVICE: &str = "AnyTask Next";
/// How long a confirmation waits before it counts as "no".
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);

pub fn search_key_name(provider: SearchProvider) -> &'static str {
    match provider {
        SearchProvider::Tavily => "search:tavily",
        SearchProvider::Brave => "search:brave",
    }
}

pub struct Brain {
    pub logger: Arc<Logger>,
    pub usage: Arc<UsageBook>,
    pub agent: Arc<Agent>,
    pub llm: Arc<LlmClient>,
    /// The tools' web access (straight out, then through the proxy).
    pub net: anytask_web::Net,
    pub secrets: Arc<dyn SecretStore>,
    pub confirmer: Arc<IslandConfirmer>,
    pub mcp: Arc<Mcp>,
    pub mcp_setup: Arc<McpSetup>,
    /// Train mode: what worked and what went wrong, reused in later requests.
    pub learner: Arc<Learner>,
    /// Reminders and timers the user set; reminders::start gives them out when due.
    pub reminders: Arc<Reminders>,
    /// App tips for the screen agent: built-in ones and the user's skills folder.
    pub skills: Arc<anytask_core::skills::Skills>,
}

impl Brain {
    pub fn start(app: &AppHandle, data_dir: &Path, log_dir: &Path) -> Result<Self, String> {
        // A damaged database is kept aside (not deleted) so the app still starts.
        let path = data_dir.join("memory.sqlite3");
        let memory = Memory::open(&path).or_else(|_| {
            let _ = std::fs::rename(&path, path.with_extension("broken.sqlite3"));
            Memory::open(&path)
        });
        let memory = Arc::new(memory.map_err(|e| format!("Could not open the memory database: {e}"))?);
        let secrets: Arc<dyn SecretStore> = Arc::new(Keychain::new(KEYCHAIN_SERVICE));
        let events = EventBus::default();
        let confirmer = Arc::new(IslandConfirmer::new(app.clone(), events.clone()));
        let activity = Activity::new();
        tauri::async_runtime::spawn(activity.follow(&events));
        let learner = Arc::new(Learner::load(data_dir.join("learning.json")));
        let net = anytask_web::Net::new();
        let reminders = Arc::new(Reminders::open(data_dir.join("reminders.json")));
        // The second check reads its settings and key when it is asked, so turning it on needs no restart.
        let judge: Arc<dyn anytask_core::Judge> = Arc::new(crate::decision::DecisionJudge { app: app.clone(), decider: Arc::new(anytask_web::Decider::new(net.clone())) });
        let skills = Arc::new(anytask_core::skills::Skills::new(Some(data_dir.join("skills"))));
        let google = google_source(app.clone(), secrets.clone());
        let search_api = search_source(secrets.clone());
        let mut builtin = tools(
            net.clone(),
            memory.clone(),
            search_api.clone(),
            activity,
            learner.clone(),
            reminders.clone(),
            judge,
            skills.clone(),
            google.clone(),
        );
        builtin.add(Arc::new(crate::screenshot::Screenshot { app: app.clone(), pictures: UserFolders::detect().pictures }));
        let agent = Arc::new(Agent::new(builtin.clone(), memory, events.clone(), confirmer.clone()).with_learner(learner.clone()));
        forward_events(app.clone(), &events);
        let logger = Arc::new(Logger::new(log_dir.to_path_buf()));
        logger.write("app", format!("AnyTask {} started", app.package_info().version));
        let usage = Arc::new(UsageBook::load(data_dir.join("usage.json")));
        logger.follow(app.clone(), &events, usage.clone());
        let mcp = Mcp::new(data_dir.join("mcp-servers.json"), builtin, agent.clone(), secrets.clone());
        mcp.start(app.clone());
        let mcp_setup = McpSetup::new(data_dir.join("mcp-data"), net.clone(), search_api, google, secrets.clone());
        let llm = Arc::new(LlmClient::new());
        // Retries of a busy model and slow answers go to the log, so a slow request shows why.
        let model_events = events.clone();
        llm.set_log(move |text| model_events.emit(anytask_core::CoreEvent::Log { turn_id: None, source: "model".into(), text }));
        Ok(Self { logger, usage, agent, llm, net, secrets, confirmer, mcp, mcp_setup, learner, reminders, skills })
    }
}

fn search_source(secrets: Arc<dyn SecretStore>) -> SearchApi {
    Arc::new(move || {
        [SearchProvider::Tavily, SearchProvider::Brave]
            .into_iter()
            .find_map(|p| secrets.get(search_key_name(p)).map(|key| (p, key)))
    })
}

/// Google search through the user's Gemini key: the models that search on free keys first, then the user's own
/// Gemini models (the active one first). None when no Gemini model has a saved key.
fn google_source(app: AppHandle, secrets: Arc<dyn SecretStore>) -> GoogleSource {
    Arc::new(move || {
        let store = app.try_state::<crate::commands::SettingsStore>()?;
        let settings = crate::commands::current(&store);
        let mut gemini: Vec<&anytask_llm::ModelConfig> = settings.models.iter().filter(|m| m.provider == anytask_llm::Provider::Gemini).collect();
        gemini.sort_by_key(|m| m.id != settings.active_model);
        let (base_url, key) = gemini.iter().find_map(|m| secrets.get(&anytask_storage::model_key_name(&m.id)).map(|key| (m.base_url.clone(), key)))?;
        let mut models: Vec<String> = anytask_web::GOOGLE_SEARCH_MODELS.iter().map(|m| m.to_string()).collect();
        for m in gemini {
            if !models.contains(&m.model_id) {
                models.push(m.model_id.clone());
            }
        }
        Some(anytask_web::GoogleSearch { base_url, models, key })
    })
}

// Every built-in tool gets what it needs from here; the list grows with the tools.
#[allow(clippy::too_many_arguments)]
fn tools(
    net: anytask_web::Net,
    memory: Arc<Memory>,
    api: SearchApi,
    activity: Arc<Activity>,
    learner: Arc<Learner>,
    reminders: Arc<Reminders>,
    judge: Arc<dyn anytask_core::Judge>,
    skills: Arc<anytask_core::skills::Skills>,
    google: GoogleSource,
) -> Registry {
    let folders = Arc::new(UserFolders::detect());
    let system: Arc<dyn SystemOps> = Arc::new(NativeSystem);
    let mut tools = Registry::default();
    tools.add(Arc::new(WebSearch { net: net.clone(), api, google: Some(google) }));
    tools.add(Arc::new(WeatherNow { net: net.clone(), memory: Some(memory.clone()) }));
    tools.add(Arc::new(ReadWebPage { net: net.clone() }));
    tools.add(Arc::new(anytask_core::tools::web::ShopPrices { net: net.clone() }));
    tools.add(Arc::new(ReadBrowserTab { ops: system.clone(), net }));
    tools.add(Arc::new(ListDirectory { folders: folders.clone() }));
    tools.add(Arc::new(FindFiles { folders: folders.clone() }));
    tools.add(Arc::new(ReadTextFile { folders: folders.clone() }));
    tools.add(Arc::new(WriteTextFile { folders: folders.clone(), here: Some(Arc::new(|| anytask_control::apps::explorer_folder().map(std::path::PathBuf::from))) }));
    tools.add(Arc::new(OpenTarget { ops: system.clone(), folders }));
    tools.add(Arc::new(ClipboardRead { ops: system.clone() }));
    tools.add(Arc::new(ClipboardWrite { ops: system.clone() }));
    tools.add(Arc::new(anytask_core::tools::system::Volume { ops: system.clone() }));
    tools.add(Arc::new(MediaControl { ops: system.clone() }));
    tools.add(Arc::new(anytask_core::tools::music::PlayMusic { ops: system }));
    tools.add(Arc::new(OpenApp { ops: Arc::new(NativeDesktop) }));
    tools.add(Arc::new(CloseApp { ops: Arc::new(NativeDesktop) }));
    tools.add(Arc::new(DesktopTask { ops: Arc::new(NativeDesktop), learner: Some(learner), judge: Some(judge), skills: Some(skills) }));
    tools.add(Arc::new(RememberFact { memory: memory.clone() }));
    tools.add(Arc::new(RecallMemory { memory }));
    tools.add(Arc::new(GetActivity { activity }));
    tools.add(Arc::new(SetReminder { reminders: reminders.clone() }));
    tools.add(Arc::new(ListReminders { reminders: reminders.clone() }));
    tools.add(Arc::new(CancelReminder { reminders }));
    tools
}

/// Every window gets every core event as "core-event".
fn forward_events(app: AppHandle, events: &EventBus) {
    let mut rx = events.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let _ = app.emit("core-event", &event);
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmRequest<'a> {
    id: u64,
    title: &'a str,
    detail: &'a str,
}

/// Shows an Allow / Don't allow card on the island and waits for the answer (no answer = no).
pub struct IslandConfirmer {
    app: AppHandle,
    /// Each card and its answer go to the log: a denied write could not be told from an unanswered one (round 2).
    events: EventBus,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
}

impl IslandConfirmer {
    fn new(app: AppHandle, events: EventBus) -> Self {
        Self { app, events, next: AtomicU64::new(0), pending: Mutex::new(HashMap::new()) }
    }

    fn log(&self, text: String) {
        self.events.emit(anytask_core::CoreEvent::Log { turn_id: None, source: "confirm".into(), text });
    }

    pub fn answer(&self, id: u64, allow: bool) {
        if let Some(sender) = self.pending.lock().ok().and_then(|mut p| p.remove(&id)) {
            let _ = sender.send(allow);
        }
    }
}

/// Removes the card when the question ends in any way, including when the request is cancelled meanwhile.
struct Pending<'a> {
    confirmer: &'a IslandConfirmer,
    id: u64,
    /// The wait ended (answer or timeout); otherwise the request was stopped while the card was open.
    answered: bool,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if !self.answered {
            self.confirmer.log("the request was stopped while the card was open: not allowed".into());
        }
        if let Ok(mut pending) = self.confirmer.pending.lock() {
            pending.remove(&self.id);
        }
        self.confirmer.app.state::<crate::island::IslandState>().asking.store(false, Ordering::Relaxed);
        let _ = self.confirmer.app.emit_to(ISLAND, "confirm-closed", self.id);
    }
}

#[async_trait]
impl Confirmer for IslandConfirmer {
    async fn confirm(&self, title: &str, detail: &str) -> bool {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id, sender);
        }
        let mut guard = Pending { confirmer: self, id, answered: false };
        self.app.state::<crate::island::IslandState>().asking.store(true, Ordering::Relaxed);
        actions::show_island(&self.app);
        if self.app.emit_to(ISLAND, "confirm-request", ConfirmRequest { id, title, detail }).is_err() {
            guard.answered = true;
            self.log(format!("could not show \"{title}\" ({detail}): not allowed"));
            return false;
        }
        self.log(format!("asked \"{title}\" ({detail})"));
        let answer = tokio::time::timeout(CONFIRM_TIMEOUT, receiver).await;
        guard.answered = true;
        self.log(match answer {
            Ok(Ok(true)) => "the user allowed it".to_string(),
            Ok(Ok(false)) => "the user did not allow it".to_string(),
            Ok(Err(_)) => "the card was closed without an answer: not allowed".to_string(),
            Err(_) => format!("no answer in {} s: not allowed", CONFIRM_TIMEOUT.as_secs()),
        });
        matches!(answer, Ok(Ok(true)))
    }
}