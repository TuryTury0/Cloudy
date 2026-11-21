//! MCP client: connects to the servers listed in mcp-servers.json (same format as Claude Desktop) and offers their
//! tools to the brain and the voice model as "mcp__<server>__<tool>".
//!
//! "secret:NAME" values are read from the OS keychain, so tokens never sit in the file or logs.

mod config;
mod tools;

pub use config::{McpConfig, ServerConfig};
pub use tools::{exposed_name, restore_json_args, result_to_text};

use anytask_core::Tool;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// More tools than this only confuse the model (and make every request bigger).
pub const MAX_TOOLS: usize = 40;
const ERROR_CHARS: usize = 300;
const MAX_SERVER_LOGS: usize = 50;

type Client = RunningService<RoleClient, ()>;
/// Finds a secret by name ("secret:NAME" in the file).
pub type SecretLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
/// Receives a line for the log (server output, connections).
pub type LogSink = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Connecting,
    Connected,
    Error,
    Disabled,
    /// In the file but not started yet.
    Stopped,
}

/// Full tool description (for testing and toggling tools in the Settings UI).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub schema: Value,
    pub enabled: bool,
}

/// One server as the settings page shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub name: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub tools: Vec<String>,
    pub all_tools: Vec<ToolInfo>,
    /// The command line or the URL.
    pub runs: String,
    pub logs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

/// Result of testing a server connection and tool listing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTestReport {
    pub name: String,
    pub ok: bool,
    pub status: Status,
    pub ms: u64,
    pub tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub logs: Vec<String>,
}

/// Result of running a single MCP tool test call.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTestReport {
    pub server: String,
    pub tool: String,
    pub ok: bool,
    pub ms: u64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Local runtimes detected on this machine for stdio MCP servers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvStatus {
    pub node: bool,
    pub npx: bool,
    pub uv: bool,
    pub uvx: bool,
    pub python: bool,
    pub git: bool,
    pub docker: bool,
}

#[derive(Debug, Clone)]
struct RemoteTool {
    name: String,
    description: String,
    schema: Value,
}

struct ServerState {
    status: Status,
    error: Option<String>,
    /// All tools reported by the server.
    tools: Vec<RemoteTool>,
    /// Allowlist from `ServerConfig.tools` (empty = all enabled).
    allowed_tools: Vec<String>,
    client: Option<Arc<Client>>,
    logs: VecDeque<String>,
    latency_ms: Option<u64>,
    /// Which connect attempt this is, so a slow old attempt never overwrites a newer one.
    attempt: u64,
}

impl ServerState {
    fn is_tool_enabled(&self, tool_name: &str) -> bool {
        self.allowed_tools.is_empty() || self.allowed_tools.iter().any(|t| t == tool_name)
    }

    fn push_log(&mut self, line: String) {
        if self.logs.len() >= MAX_SERVER_LOGS {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }
}

pub struct McpManager {
    servers: Arc<Mutex<BTreeMap<String, ServerState>>>,
    secrets: SecretLookup,
    log: LogSink,
    attempts: AtomicU64,
}

impl McpManager {
    pub fn new(secrets: SecretLookup, log: LogSink) -> Arc<Self> {
        Arc::new(Self {
            servers: Arc::new(Mutex::new(BTreeMap::new())),
            secrets,
            log,
            attempts: AtomicU64::new(0),
        })
    }

    /// Checks whether common MCP runtimes (`npx`, `uvx`, `node`, `uv`, `python`, `git`, `docker`) are usable.
    pub fn check_environment() -> EnvStatus {
        EnvStatus {
            node: find_program("node").is_some(),
            npx: find_program("npx").is_some(),
            uv: find_program("uv").is_some(),
            uvx: find_program("uvx").is_some(),
            python: find_program("python").is_some(),
            git: find_program("git").is_some(),
            docker: find_program("docker").is_some(),
        }
    }

    /// Connects every server in the file at the same time (a slow one does not hold up the others).
    pub async fn start_all(self: &Arc<Self>, config: &McpConfig) {
        let jobs = config.mcp_servers.iter().map(|(name, server)| self.connect(name, server));
        futures_util::future::join_all(jobs).await;
    }

    /// Closes every server and starts again from the file.
    pub async fn reload(self: &Arc<Self>, config: &McpConfig) {
        let old = std::mem::take(&mut *self.lock());
        drop(old);
        self.start_all(config).await;
    }

    /// (Re)connects one server. Errors end up in its status.
    pub async fn connect(self: &Arc<Self>, name: &str, server: &ServerConfig) {
        let attempt = self.attempts.fetch_add(1, Ordering::Relaxed) + 1;
        let status = if server.disabled { Status::Disabled } else { Status::Connecting };
        let started = Instant::now();
        {
            let mut logs = VecDeque::new();
            if !server.disabled {
                logs.push_back("Connecting…".to_string());
            }
            self.lock().insert(
                name.to_string(),
                ServerState {
                    status,
                    error: None,
                    tools: Vec::new(),
                    allowed_tools: server.tools.clone(),
                    client: None,
                    logs,
                    latency_ms: None,
                    attempt,
                },
            );
        }
        if server.disabled {
            return;
        }
        let outcome = match tokio::time::timeout(CONNECT_TIMEOUT, self.open(name, server, attempt)).await {
            Ok(Ok((client, secret_values))) => match tokio::time::timeout(LIST_TIMEOUT, client.list_all_tools()).await {
                Ok(Ok(listed)) => Ok((client, listed, secret_values)),
                Ok(Err(e)) => Err((format!("Listing the tools failed: {e}"), secret_values)),
                Err(_) => Err((format!("Listing the tools took longer than {} s.", LIST_TIMEOUT.as_secs()), secret_values)),
            },
            Ok(Err(e)) => Err((e, Vec::new())),
            Err(_) => Err((format!("Starting the server took longer than {} s.", CONNECT_TIMEOUT.as_secs()), Vec::new())),
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let mut servers = self.lock();
        let Some(state) = servers.get_mut(name).filter(|s| s.attempt == attempt) else {
            return;
        };
        match outcome {
            Ok((client, listed, _secrets)) => {
                let tools: Vec<RemoteTool> = listed
                    .iter()
                    .filter_map(|t| serde_json::to_value(t).ok())
                    .map(|t| RemoteTool {
                        name: t["name"].as_str().unwrap_or_default().to_string(),
                        description: t["description"].as_str().unwrap_or_default().to_string(),
                        schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({ "type": "object" })),
                    })
                    .filter(|t| !t.name.is_empty())
                    .collect();
                let enabled_count = tools.iter().filter(|t| state.is_tool_enabled(&t.name)).count();
                let summary = format!("connected with {} tool(s) ({} ms)", enabled_count, elapsed_ms);
                state.push_log(summary.clone());
                state.status = Status::Connected;
                state.error = None;
                state.tools = tools;
                state.client = Some(Arc::new(client));
                state.latency_ms = Some(elapsed_ms);
                drop(servers);
                (self.log)(format!("{name}: {summary}"));
            }
            Err((error, secret_values)) => {
                let safe_error = clip(&redact(&error, &secret_values), ERROR_CHARS);
                state.push_log(format!("FAILED: {safe_error}"));
                state.status = Status::Error;
                state.error = Some(safe_error.clone());
                state.tools.clear();
                state.client = None;
                state.latency_ms = Some(elapsed_ms);
                drop(servers);
                (self.log)(format!("{name}: FAILED {safe_error}"));
            }
        }
    }

    /// Updates which tools of an already-connected server are exposed without restarting its process.
    pub fn set_tool_filter(&self, name: &str, allowed: Vec<String>) {
        if let Some(state) = self.lock().get_mut(name) {
            state.allowed_tools = allowed;
        }
    }

    /// Closes a server (its process ends).
    pub fn disconnect(&self, name: &str) {
        let removed = self.lock().remove(name);
        drop(removed);
    }

    async fn open(&self, name: &str, server: &ServerConfig, attempt: u64) -> Result<(Client, Vec<String>), String> {
        if let Some(url) = server.url.as_deref().filter(|u| !u.trim().is_empty()) {
            let (headers, secret_values) = config::resolve_with_secrets(&server.headers, &self.secrets)?;
            let mut transport = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url.trim().to_string());
            let mut custom = HashMap::new();
            for (key, value) in headers {
                // "Authorization: Bearer x" goes through the transport's own field (custom headers may not set it).
                if key.eq_ignore_ascii_case("authorization") {
                    transport.auth_header = Some(value.strip_prefix("Bearer ").unwrap_or(&value).to_string());
                    continue;
                }
                let hname = http::HeaderName::from_bytes(key.as_bytes()).map_err(|_| format!("Header name \"{key}\" is not valid."))?;
                let hval = http::HeaderValue::from_str(&value).map_err(|_| format!("Header \"{key}\" has characters a header cannot carry."))?;
                custom.insert(hname, hval);
            }
            transport.custom_headers = custom;
            let transport = rmcp::transport::StreamableHttpClientTransport::from_config(transport);
            let client = ().serve(transport).await.map_err(|e| format!("Could not connect: {}", redact(&e.to_string(), &secret_values)))?;
            return Ok((client, secret_values));
        }
        let Some(command) = server.command.as_deref().filter(|c| !c.trim().is_empty()) else {
            return Err("The server needs a \"command\" or a \"url\".".into());
        };
        let (env, secret_values) = config::resolve_with_secrets(&server.env, &self.secrets)?;
        let mut cmd = find_program(command.trim()).ok_or_else(|| format!("\"{command}\" was not found. Is it installed and on PATH?"))?;
        cmd.args(&server.args).envs(env).kill_on_drop(true);
        #[cfg(windows)]
        {
            // No console window flashing up for the server process.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let (transport, stderr) = rmcp::transport::TokioChildProcess::builder(cmd)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start \"{command}\": {}", redact(&e.to_string(), &secret_values)))?;
        if let Some(stderr) = stderr {
            let log = self.log.clone();
            let servers_weak = Arc::downgrade(&self.servers);
            let name = name.to_string();
            let secrets = secret_values.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let clipped = clip(&redact(&line, &secrets), ERROR_CHARS);
                    if let Some(servers) = servers_weak.upgrade() {
                        let mut guard = servers.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(state) = guard.get_mut(&name).filter(|s| s.attempt == attempt) {
                            state.push_log(clipped.clone());
                        }
                    }
                    log(format!("{name}: {clipped}"));
                }
            });
        }
        let client = ().serve(transport).await.map_err(|e| format!("The server did not start the MCP handshake: {}", redact(&e.to_string(), &secret_values)))?;
        Ok((client, secret_values))
    }

    /// Tests a server: if already connected, pings it by listing tools; otherwise attempts to connect using `config`.
    pub async fn test_server(self: &Arc<Self>, name: &str, config: Option<&ServerConfig>) -> ServerTestReport {
        let existing_client = {
            let servers = self.lock();
            servers.get(name).and_then(|s| (s.status == Status::Connected).then(|| s.client.clone()).flatten())
        };
        if let Some(client) = existing_client {
            let started = Instant::now();
            let outcome = tokio::time::timeout(LIST_TIMEOUT, client.list_all_tools()).await;
            let ms = started.elapsed().as_millis() as u64;
            match outcome {
                Ok(Ok(listed)) => {
                    let count = listed.len();
                    let mut servers = self.lock();
                    if let Some(state) = servers.get_mut(name) {
                        state.latency_ms = Some(ms);
                        state.push_log(format!("health check ok: {count} tool(s) in {ms} ms"));
                        let tools: Vec<String> = state.tools.iter().filter(|t| state.is_tool_enabled(&t.name)).map(|t| t.name.clone()).collect();
                        return ServerTestReport {
                            name: name.to_string(),
                            ok: true,
                            status: state.status,
                            ms,
                            tools,
                            error: None,
                            logs: state.logs.iter().cloned().collect(),
                        };
                    }
                }
                Ok(Err(e)) => {
                    if client.is_transport_closed() {
                        self.mark_stopped(name);
                    }
                    let msg = clip(&e.to_string(), ERROR_CHARS);
                    let logs = self.server_logs(name);
                    return ServerTestReport {
                        name: name.to_string(),
                        ok: false,
                        status: Status::Error,
                        ms,
                        tools: Vec::new(),
                        error: Some(msg),
                        logs,
                    };
                }
                Err(_) => {
                    let msg = format!("Health check timed out after {} s.", LIST_TIMEOUT.as_secs());
                    let logs = self.server_logs(name);
                    return ServerTestReport {
                        name: name.to_string(),
                        ok: false,
                        status: Status::Error,
                        ms,
                        tools: Vec::new(),
                        error: Some(msg),
                        logs,
                    };
                }
            }
        }
        if let Some(server) = config {
            if server.disabled {
                return ServerTestReport {
                    name: name.to_string(),
                    ok: false,
                    status: Status::Disabled,
                    ms: 0,
                    tools: Vec::new(),
                    error: Some("Server is disabled. Turn it on first to test it.".into()),
                    logs: self.server_logs(name),
                };
            }
            self.connect(name, server).await;
            let servers = self.lock();
            if let Some(state) = servers.get(name) {
                let tools: Vec<String> = state.tools.iter().filter(|t| state.is_tool_enabled(&t.name)).map(|t| t.name.clone()).collect();
                return ServerTestReport {
                    name: name.to_string(),
                    ok: state.status == Status::Connected,
                    status: state.status,
                    ms: state.latency_ms.unwrap_or(0),
                    tools,
                    error: state.error.clone(),
                    logs: state.logs.iter().cloned().collect(),
                };
            }
        }
        ServerTestReport {
            name: name.to_string(),
            ok: false,
            status: Status::Stopped,
            ms: 0,
            tools: Vec::new(),
            error: Some(format!("MCP server {name} is not configured.")),
            logs: Vec::new(),
        }
    }

    /// Runs a single tool test and returns structured output + timing.
    pub async fn test_tool(&self, server: &str, tool: &str, args: Value) -> ToolTestReport {
        let schema = {
            let servers = self.lock();
            servers
                .get(server)
                .and_then(|s| s.tools.iter().find(|t| t.name == tool))
                .map(|t| t.schema.clone())
                .unwrap_or_else(|| json!({ "type": "object" }))
        };
        let started = Instant::now();
        let outcome = self.call(server, tool, args, &schema).await;
        let ms = started.elapsed().as_millis() as u64;
        match outcome {
            Ok(val) => {
                let text = val["text"].as_str().map(str::to_string).unwrap_or_else(|| val.to_string());
                ToolTestReport { server: server.to_string(), tool: tool.to_string(), ok: true, ms, text, error: None }
            }
            Err(err) => ToolTestReport { server: server.to_string(), tool: tool.to_string(), ok: false, ms, text: String::new(), error: Some(err) },
        }
    }

    fn server_logs(&self, name: &str) -> Vec<String> {
        self.lock().get(name).map(|s| s.logs.iter().cloned().collect()).unwrap_or_default()
    }

    /// Every server in the file (and any still running), with what it offers.
    pub fn status(&self, config: &McpConfig) -> Vec<ServerStatus> {
        let servers = self.lock();
        let mut names: Vec<&String> = config.mcp_servers.keys().chain(servers.keys()).collect();
        names.sort();
        names.dedup();
        names
            .into_iter()
            .map(|name| {
                let server = config.mcp_servers.get(name).cloned().unwrap_or_default();
                let runs = server
                    .url
                    .clone()
                    .unwrap_or_else(|| std::iter::once(server.command.clone().unwrap_or_default()).chain(server.args.clone()).collect::<Vec<_>>().join(" "));
                match servers.get(name) {
                    Some(state) => {
                        let tools = state.tools.iter().filter(|t| state.is_tool_enabled(&t.name)).map(|t| t.name.clone()).collect();
                        let all_tools = state
                            .tools
                            .iter()
                            .map(|t| ToolInfo {
                                name: t.name.clone(),
                                description: t.description.clone(),
                                schema: t.schema.clone(),
                                enabled: state.is_tool_enabled(&t.name),
                            })
                            .collect();
                        ServerStatus {
                            name: name.clone(),
                            status: state.status,
                            error: state.error.clone(),
                            tools,
                            all_tools,
                            runs,
                            logs: state.logs.iter().cloned().collect(),
                            latency_ms: state.latency_ms,
                        }
                    }
                    None => ServerStatus {
                        name: name.clone(),
                        status: if server.disabled { Status::Disabled } else { Status::Stopped },
                        error: None,
                        tools: Vec::new(),
                        all_tools: Vec::new(),
                        runs,
                        logs: Vec::new(),
                        latency_ms: None,
                    },
                }
            })
            .collect()
    }

    /// Count of all currently active MCP tools (before the MAX_TOOLS cap).
    pub fn active_tool_count(&self) -> usize {
        self.lock()
            .values()
            .filter(|s| s.status == Status::Connected)
            .map(|s| s.tools.iter().filter(|t| s.is_tool_enabled(&t.name)).count())
            .sum()
    }

    /// The connected servers' tools for the brain (at most MAX_TOOLS).
    pub fn tools(self: &Arc<Self>) -> Vec<Arc<dyn Tool>> {
        let servers = self.lock();
        let mut taken: Vec<String> = Vec::new();
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        for (server, state) in servers.iter().filter(|(_, s)| s.status == Status::Connected) {
            for tool in state.tools.iter().filter(|t| state.is_tool_enabled(&t.name)) {
                if out.len() >= MAX_TOOLS {
                    return out;
                }
                let exposed = exposed_name(server, &tool.name, &taken);
                taken.push(exposed.clone());
                out.push(Arc::new(tools::McpTool {
                    manager: self.clone(),
                    exposed,
                    server: server.clone(),
                    tool: tool.name.clone(),
                    description: tool.description.clone(),
                    schema: tool.schema.clone(),
                }));
            }
        }
        out
    }

    /// Calls a tool on its server.
    pub async fn call(&self, server: &str, tool: &str, args: Value, schema: &Value) -> Result<Value, String> {
        let client = {
            let servers = self.lock();
            let state = servers.get(server).ok_or_else(|| format!("MCP server {server} is not running."))?;
            match (&state.client, state.status) {
                (Some(client), Status::Connected) => client.clone(),
                _ => {
                    return Err(format!(
                        "MCP server {server} is not connected ({}).",
                        state.error.clone().unwrap_or_else(|| format!("{:?}", state.status).to_lowercase())
                    ));
                }
            }
        };
        let arguments: Map<String, Value> = match restore_json_args(args, schema) {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let request = CallToolRequestParams::new(tool.to_string()).with_arguments(arguments);
        let answer = match tokio::time::timeout(CALL_TIMEOUT, client.call_tool(request)).await {
            Ok(answer) => answer,
            Err(_) => return Err(format!("{tool} took longer than {} s.", CALL_TIMEOUT.as_secs())),
        };
        match answer {
            Ok(result) => {
                let value = serde_json::to_value(&result).unwrap_or_default();
                let text = result_to_text(&value);
                if value["isError"].as_bool() == Some(true) {
                    Err(if text.is_empty() { "The MCP tool reported an error.".into() } else { text })
                } else {
                    Ok(json!({ "text": text }))
                }
            }
            Err(error) => {
                if client.is_transport_closed() {
                    self.mark_stopped(server);
                }
                Err(clip(&error.to_string(), ERROR_CHARS))
            }
        }
    }

    fn mark_stopped(&self, server: &str) {
        if let Some(state) = self.lock().get_mut(server) {
            state.push_log("the server stopped".into());
            state.status = Status::Error;
            state.error = Some("The server stopped.".into());
            state.client = None;
            state.tools.clear();
        }
        (self.log)(format!("{server}: the server stopped"));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, ServerState>> {
        self.servers.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Resolves a command on PATH, with Windows fallbacks for tools installed during the current session (like Node.js / uv).
pub(crate) fn find_program(cmd: &str) -> Option<tokio::process::Command> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(c) = rmcp::transport::which_command(trimmed) {
        return Some(c);
    }
    #[cfg(windows)]
    {
        let lower = trimmed.to_ascii_lowercase();
        let candidates = match lower.as_str() {
            "node" | "node.exe" => windows_candidates(&[
                ("ProgramFiles", "nodejs\\node.exe"),
                ("ProgramFiles(x86)", "nodejs\\node.exe"),
                ("LOCALAPPDATA", "Programs\\nodejs\\node.exe"),
            ]),
            "npx" | "npx.cmd" => windows_candidates(&[
                ("ProgramFiles", "nodejs\\npx.cmd"),
                ("ProgramFiles(x86)", "nodejs\\npx.cmd"),
                ("LOCALAPPDATA", "Programs\\nodejs\\npx.cmd"),
                ("APPDATA", "npm\\npx.cmd"),
            ]),
            "uv" | "uv.exe" => windows_candidates(&[
                ("USERPROFILE", ".local\\bin\\uv.exe"),
                ("USERPROFILE", ".cargo\\bin\\uv.exe"),
                ("LOCALAPPDATA", "Programs\\uv\\uv.exe"),
            ]),
            "uvx" | "uvx.exe" => windows_candidates(&[
                ("USERPROFILE", ".local\\bin\\uvx.exe"),
                ("USERPROFILE", ".cargo\\bin\\uvx.exe"),
                ("LOCALAPPDATA", "Programs\\uv\\uvx.exe"),
            ]),
            "python" | "python.exe" => windows_candidates(&[
                ("LOCALAPPDATA", "Programs\\Python\\Python313\\python.exe"),
                ("LOCALAPPDATA", "Programs\\Python\\Python312\\python.exe"),
                ("LOCALAPPDATA", "Programs\\Python\\Python311\\python.exe"),
            ]),
            "git" | "git.exe" => windows_candidates(&[
                ("ProgramFiles", "Git\\cmd\\git.exe"),
                ("ProgramFiles(x86)", "Git\\cmd\\git.exe"),
            ]),
            _ => Vec::new(),
        };
        for path in candidates {
            if path.is_file() {
                let mut command = tokio::process::Command::new(&path);
                // Ensure sibling binaries (e.g., node.exe next to npx.cmd) are reachable even if PATH is stale.
                if let Some(parent) = path.parent() {
                    let current = std::env::var_os("PATH").unwrap_or_default();
                    let mut paths = vec![parent.to_path_buf()];
                    if let Some(node_dir) = windows_candidates(&[("ProgramFiles", "nodejs"), ("LOCALAPPDATA", "Programs\\nodejs")]).into_iter().find(|d| d.is_dir())
                        && node_dir != parent
                    {
                        paths.push(node_dir);
                    }
                    paths.extend(std::env::split_paths(&current));
                    if let Ok(joined) = std::env::join_paths(paths) {
                        command.env("PATH", joined);
                    }
                }
                return Some(command);
            }
        }
    }
    None
}

#[cfg(windows)]
fn windows_candidates(entries: &[(&str, &str)]) -> Vec<std::path::PathBuf> {
    entries
        .iter()
        .filter_map(|(var, rel)| std::env::var_os(var).map(|base| std::path::PathBuf::from(base).join(rel)))
        .collect()
}

fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if secret.len() >= 6 {
            out = out.replace(secret, "***");
        }
    }
    out
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secret_values_from_logs_and_errors() {
        let secrets = vec!["super-secret-token".to_string(), "ab".to_string()];
        assert_eq!(redact("failed with token super-secret-token here", &secrets), "failed with token *** here");
        assert_eq!(redact("ab stays short", &secrets), "ab stays short");
    }

    #[test]
    fn tool_filter_updates_all_tools_vs_enabled_tools() {
        let manager = McpManager::new(Arc::new(|_| None), Arc::new(|_| {}));
        manager.lock().insert(
            "srv".into(),
            ServerState {
                status: Status::Connected,
                error: None,
                tools: vec![
                    RemoteTool { name: "a".into(), description: "tool a".into(), schema: json!({ "type": "object" }) },
                    RemoteTool { name: "b".into(), description: "tool b".into(), schema: json!({ "type": "object" }) },
                ],
                allowed_tools: vec!["a".into()],
                client: None,
                logs: VecDeque::new(),
                latency_ms: Some(42),
                attempt: 1,
            },
        );
        let cfg = McpConfig::default();
        let st = manager.status(&cfg);
        assert_eq!(st[0].tools, vec!["a"]);
        assert_eq!(st[0].all_tools.len(), 2);
        assert!(st[0].all_tools[0].enabled);
        assert!(!st[0].all_tools[1].enabled);
        assert_eq!(manager.active_tool_count(), 1);

        manager.set_tool_filter("srv", vec![]);
        let st2 = manager.status(&cfg);
        assert_eq!(st2[0].tools, vec!["a", "b"]);
        assert_eq!(manager.active_tool_count(), 2);
    }
}