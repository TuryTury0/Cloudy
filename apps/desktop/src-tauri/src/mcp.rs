//! MCP servers in the desktop app: mcp-servers.json in the app data folder, secrets in the keychain ("mcp:NAME"),
//! and the brain's tool set rebuilt whenever a server connects, stops or changes.

use anytask_core::{Agent, CoreEvent, Registry};
use anytask_mcp::{EnvStatus, LogSink, MAX_TOOLS, McpConfig, McpManager, SecretLookup, ServerConfig, ServerStatus, ServerTestReport, ToolTestReport};
use anytask_storage::SecretStore;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};

pub fn secret_key_name(name: &str) -> String {
    format!("mcp:{name}")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretState {
    pub name: String,
    pub set: bool,
}

/// Everything the Tools page shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpReport {
    pub servers: Vec<ServerStatus>,
    /// The file could not be read (the reason); servers keep running as they were.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub path: String,
    /// Secrets the file asks for ("secret:NAME"), and whether each is saved.
    pub secrets: Vec<SecretState>,
    /// The built-in tools, for the page's overview.
    pub builtin: Vec<String>,
    /// The file as it is (for editing a server).
    pub config: McpConfig,
    /// Maximum MCP tools exposed to the brain simultaneously.
    pub max_tools: usize,
    /// Total active MCP tools before the cap.
    pub active_tool_count: usize,
    /// Local runtimes (node/npx/uv/uvx...) detected on this PC.
    pub env: EnvStatus,
    /// Secrets used exclusively by each server (offered for cleanup on deletion).
    pub exclusive_secrets: BTreeMap<String, Vec<String>>,
}

pub struct Mcp {
    pub manager: Arc<McpManager>,
    pub path: PathBuf,
    base: Registry,
    /// Tools that come and go with something else (the connected phone).
    extra: std::sync::RwLock<Vec<Arc<dyn anytask_core::Tool>>>,
    agent: Arc<Agent>,
    pub secrets: Arc<dyn SecretStore>,
    /// One change at a time (file edits and reconnects).
    pub changes: tokio::sync::Mutex<()>,
}

impl Mcp {
    pub fn new(path: PathBuf, base: Registry, agent: Arc<Agent>, secrets: Arc<dyn SecretStore>) -> Arc<Self> {
        let store = secrets.clone();
        let lookup: SecretLookup = Arc::new(move |name| store.get(&secret_key_name(name)));
        let events = agent.events().clone();
        let log: LogSink = Arc::new(move |text| events.emit(CoreEvent::Log { turn_id: None, source: "mcp".into(), text }));
        Arc::new(Self {
            manager: McpManager::new(lookup, log),
            path,
            base,
            extra: Default::default(),
            agent,
            secrets,
            changes: tokio::sync::Mutex::new(()),
        })
    }

    /// Connects the servers in the file (in the background, so the app starts at once).
    pub fn start<R: Runtime>(self: &Arc<Self>, app: AppHandle<R>) {
        let mcp = self.clone();
        tauri::async_runtime::spawn(async move {
            let _guard = mcp.changes.lock().await;
            let config = mcp.config().unwrap_or_default();
            if !config.mcp_servers.is_empty() {
                mcp.manager.start_all(&config).await;
                mcp.refresh(&app);
            }
        });
    }

    pub fn config(&self) -> Result<McpConfig, String> {
        McpConfig::load(&self.path)
    }

    /// Replaces the extra tools (the phone's) and gives the brain the new set.
    pub fn set_extra<R: Runtime>(&self, app: &AppHandle<R>, tools: Vec<Arc<dyn anytask_core::Tool>>) {
        *self.extra.write().unwrap_or_else(|e| e.into_inner()) = tools;
        self.refresh(app);
    }

    /// Gives the brain the built-in tools plus the connected servers' tools and tells the windows.
    pub fn refresh<R: Runtime>(&self, app: &AppHandle<R>) {
        let mut tools = self.base.clone();
        for tool in self.manager.tools() {
            tools.add(tool);
        }
        for tool in self.extra.read().unwrap_or_else(|e| e.into_inner()).iter() {
            tools.add(tool.clone());
        }
        self.agent.set_tools(tools);
        let _ = app.emit("mcp-changed", self.report());
    }

    pub fn report(&self) -> McpReport {
        let (config, error) = match self.config() {
            Ok(config) => (config, None),
            Err(e) => (McpConfig::default(), Some(e)),
        };
        let wanted = config.referenced_secrets();
        let mut exclusive_secrets = BTreeMap::new();
        for (name, server) in &config.mcp_servers {
            let mine = server.referenced_secrets();
            if mine.is_empty() {
                continue;
            }
            let others: BTreeSet<String> = config
                .mcp_servers
                .iter()
                .filter(|(other, _)| *other != name)
                .flat_map(|(_, cfg)| cfg.referenced_secrets())
                .collect();
            let only_mine: Vec<String> = mine.difference(&others).cloned().collect();
            if !only_mine.is_empty() {
                exclusive_secrets.insert(name.clone(), only_mine);
            }
        }
        McpReport {
            servers: self.manager.status(&config),
            error,
            path: self.path.display().to_string(),
            secrets: wanted.into_iter().map(|name| SecretState { set: self.secrets.get(&secret_key_name(&name)).is_some(), name }).collect(),
            builtin: self.base.names(),
            config,
            max_tools: MAX_TOOLS,
            active_tool_count: self.manager.active_tool_count(),
            env: McpManager::check_environment(),
            exclusive_secrets,
        }
    }

    /// Adds or changes a server (a rename removes the old entry) and connects it.
    pub async fn save_server<R: Runtime>(&self, app: &AppHandle<R>, name: &str, server: ServerConfig, previous: Option<&str>) -> Result<McpReport, String> {
        let name = valid_name(name)?;
        let has_command = server.command.as_deref().is_some_and(|c| !c.trim().is_empty());
        let has_url = server.url.as_deref().is_some_and(|u| !u.trim().is_empty());
        if has_command == has_url {
            return Err("Give either a command or a URL.".into());
        }
        if has_url && !server.url.as_deref().unwrap_or_default().trim().starts_with("http") {
            return Err("The URL must start with http:// or https://.".into());
        }
        let _guard = self.changes.lock().await;
        let mut config = self.config()?;
        if let Some(old) = previous.filter(|old| *old != name) {
            config.mcp_servers.remove(old);
            self.manager.disconnect(old);
        }
        config.mcp_servers.insert(name.clone(), server.clone());
        config.save(&self.path)?;
        let _ = app.emit("mcp-changed", self.report());
        self.manager.connect(&name, &server).await;
        self.refresh(app);
        Ok(self.report())
    }

    /// Updates the enabled tools list (`ServerConfig.tools`) without restarting a running server process.
    pub async fn set_server_tools<R: Runtime>(&self, app: &AppHandle<R>, name: &str, tools: Vec<String>) -> Result<McpReport, String> {
        let _guard = self.changes.lock().await;
        let mut config = self.config()?;
        let server = config.mcp_servers.get_mut(name).ok_or_else(|| format!("No MCP server named {name}."))?;
        server.tools = tools.clone();
        config.save(&self.path)?;
        self.manager.set_tool_filter(name, tools);
        self.refresh(app);
        Ok(self.report())
    }

    pub async fn remove_server<R: Runtime>(&self, app: &AppHandle<R>, name: &str) -> Result<McpReport, String> {
        self.remove_server_with_secrets(app, name, false).await
    }

    /// Removes a server and optionally deletes secrets that no other configured server uses.
    pub async fn remove_server_with_secrets<R: Runtime>(&self, app: &AppHandle<R>, name: &str, clean_secrets: bool) -> Result<McpReport, String> {
        let _guard = self.changes.lock().await;
        let mut config = self.config()?;
        let removed = config.mcp_servers.remove(name);
        config.save(&self.path)?;
        self.manager.disconnect(name);
        if clean_secrets && let Some(old) = removed {
            let still_used = config.referenced_secrets();
            for secret_name in old.referenced_secrets() {
                if !still_used.contains(&secret_name) {
                    let _ = self.secrets.delete(&secret_key_name(&secret_name));
                }
            }
        }
        self.refresh(app);
        Ok(self.report())
    }

    pub async fn set_disabled<R: Runtime>(&self, app: &AppHandle<R>, name: &str, disabled: bool) -> Result<McpReport, String> {
        let _guard = self.changes.lock().await;
        let mut config = self.config()?;
        let server = config.mcp_servers.get_mut(name).ok_or_else(|| format!("No MCP server named {name}."))?;
        server.disabled = disabled;
        let server = server.clone();
        config.save(&self.path)?;
        self.manager.connect(name, &server).await;
        self.refresh(app);
        Ok(self.report())
    }

    /// Tests a single server connection & tool list (reconnecting if currently stopped/errored) and refreshes UI state.
    pub async fn test_server<R: Runtime>(&self, app: &AppHandle<R>, name: &str) -> Result<ServerTestReport, String> {
        let _guard = self.changes.lock().await;
        let config = self.config()?;
        let server = config.mcp_servers.get(name);
        let report = self.manager.test_server(name, server).await;
        self.refresh(app);
        Ok(report)
    }

    /// Calls a single tool on a running server and returns the raw test result + latency.
    pub async fn test_tool(&self, server: &str, tool: &str, args: Value) -> ToolTestReport {
        self.manager.test_tool(server, tool, args).await
    }

    /// Reads the file again (after editing it by hand) and restarts every server.
    pub async fn reload<R: Runtime>(&self, app: &AppHandle<R>) -> McpReport {
        let _guard = self.changes.lock().await;
        if let Ok(config) = self.config() {
            self.manager.reload(&config).await;
        }
        self.refresh(app);
        self.report()
    }
}

/// Server names: letters, digits, - and _, up to 40 characters (they become part of tool names).
pub(crate) fn valid_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("The name may use English letters, digits, - and _ (up to 40).".into());
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_names_are_checked() {
        assert_eq!(valid_name(" github ").unwrap(), "github");
        assert!(valid_name("my server").is_err());
        assert!(valid_name("").is_err());
        assert!(valid_name("سرور").is_err());
    }
}