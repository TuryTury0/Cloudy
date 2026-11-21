//! AI-assisted MCP setup session inside Settings > Tools & MCP.
//! Keeps chat history in the backend so switching Settings tabs never loses progress,
//! searches the curated catalog + real web docs when needed, and collects secrets exclusively
//! via structured SetupPlan cards straight into the OS keychain (never echoed to the LLM).

use crate::brain::Brain;
use crate::commands::{SettingsStore, active_generator, current};
use crate::mcp::{McpReport, secret_key_name, valid_name};
use crate::mcp_catalog::{self, FieldKind, FieldTarget, SetupField, SetupPlan};
use anytask_core::lang::tr;
use anytask_core::tools::web::{GoogleSource, SearchApi};
use anytask_llm::{GenerateRequest, Message, Part, Role, Thinking, ToolSpec};
use anytask_mcp::{McpManager, ServerConfig, ServerStatus};
use anytask_storage::SecretStore;
use anytask_web::Net;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

const MAX_MESSAGES: usize = 30;
const MAX_UPLOAD_BYTES: usize = 512 * 1024;
const MAX_LLM_ROUNDS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SetupRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupMessage {
    pub id: u64,
    pub role: SetupRole,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<SetupPlan>,
    pub steps: Vec<String>,
    pub at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStateView {
    pub messages: Vec<SetupMessage>,
    pub busy: bool,
    pub progress: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadedFile {
    pub field_key: String,
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    pub report: McpReport,
    pub setup: SetupStateView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerStatus>,
}

struct SessionState {
    messages: Vec<SetupMessage>,
    busy: bool,
    progress: String,
}

pub struct McpSetup {
    state: Mutex<SessionState>,
    next_id: AtomicU64,
    turn_lock: tokio::sync::Mutex<()>,
    data_dir: PathBuf,
    net: Net,
    search_api: SearchApi,
    google: GoogleSource,
    secrets: Arc<dyn SecretStore>,
}

impl McpSetup {
    pub fn new(data_dir: PathBuf, net: Net, search_api: SearchApi, google: GoogleSource, secrets: Arc<dyn SecretStore>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(SessionState { messages: Vec::new(), busy: false, progress: String::new() }),
            next_id: AtomicU64::new(1),
            turn_lock: tokio::sync::Mutex::new(()),
            data_dir,
            net,
            search_api,
            google,
            secrets,
        })
    }

    pub fn view(&self) -> SetupStateView {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        SetupStateView { messages: s.messages.clone(), busy: s.busy, progress: s.progress.clone() }
    }

    fn emit(&self, app: &AppHandle) {
        let _ = app.emit("mcp-setup-changed", self.view());
    }

    fn set_busy(&self, app: &AppHandle, busy: bool, progress: &str) {
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.busy = busy;
            s.progress = progress.to_string();
        }
        self.emit(app);
    }

    fn push_message(&self, app: &AppHandle, role: SetupRole, text: String, plan: Option<SetupPlan>, steps: Vec<String>, error: Option<String>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let at = now_ms();
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.messages.push(SetupMessage { id, role, text, plan, steps, at, error });
            if s.messages.len() > MAX_MESSAGES {
                let drop_n = s.messages.len() - MAX_MESSAGES;
                s.messages.drain(..drop_n);
            }
        }
        self.emit(app);
    }

    pub fn clear(&self, app: &AppHandle) -> SetupStateView {
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.messages.clear();
            s.progress.clear();
        }
        let view = self.view();
        self.emit(app);
        view
    }

    /// Instantly opens a curated Setup Card (zero tokens, works even without an LLM model configured).
    pub fn start_preset(&self, app: &AppHandle, preset_id: &str) -> Result<SetupStateView, String> {
        let preset = mcp_catalog::find_by_id(preset_id).ok_or_else(|| format!("Unknown MCP preset: {preset_id}"))?;
        let env = McpManager::check_environment();
        let plan = mcp_catalog::to_plan(preset, &env, self.secrets.as_ref());
        let user_text = tr(&format!("Set up {}", preset.title_en), &format!("نصب و اتصال {}", preset.title_fa)).to_string();
        self.push_message(app, SetupRole::User, user_text, None, Vec::new(), None);
        let reply = if plan.fields.is_empty() {
            tr(
                &format!("Ready to install **{}**. Click **Install & connect** below.", preset.title_en),
                &format!("آماده نصب **{}** هستم. پایین دکمه **نصب و اتصال** را بزن.", preset.title_fa),
            )
            .to_string()
        } else {
            tr(
                &format!("Here is the setup card for **{}**. Fill in the required fields below (secrets stay only in Windows Credential Manager) and click **Install & connect**.", preset.title_en),
                &format!("کارت نصب **{}** آماده شد. موارد خواسته‌شده را پایین وارد کن (رمزها فقط در Windows Credential Manager ذخیره می‌شوند) و دکمه **نصب و اتصال** را بزن.", preset.title_fa),
            )
            .to_string()
        };
        self.push_message(app, SetupRole::Assistant, reply, Some(plan), Vec::new(), None);
        Ok(self.view())
    }

    /// Sends a user message to the AI MCP Installer assistant.
    pub async fn chat(&self, app: &AppHandle, raw_text: &str) -> Result<SetupStateView, String> {
        let text = raw_text.trim();
        if text.is_empty() {
            return Err(tr("Write what you want to connect.", "بنویس چه سرویسی را می‌خواهی وصل کنی.").into());
        }
        let _guard = self.turn_lock.lock().await;
        self.push_message(app, SetupRole::User, text.to_string(), None, Vec::new(), None);
        self.run_ai_turn(app, text).await
    }

    /// Feeds a server's config, error and stderr logs to the AI so it diagnoses and proposes a fix.
    pub async fn diagnose(&self, app: &AppHandle, server_name: &str) -> Result<SetupStateView, String> {
        let _guard = self.turn_lock.lock().await;
        let brain = app.state::<Brain>();
        let report = brain.mcp.report();
        let status = report.servers.iter().find(|s| s.name == server_name).cloned();
        let config = report.config.mcp_servers.get(server_name).cloned();
        let user_visible = tr(
            &format!("Diagnose and fix MCP server `{server_name}`"),
            &format!("مشکل سرور `{server_name}` را بررسی و برطرف کن"),
        )
        .to_string();
        self.push_message(app, SetupRole::User, user_visible, None, Vec::new(), None);

        let context = json!({
            "server": server_name,
            "config": config,
            "status": status.as_ref().map(|s| json!({ "status": s.status, "error": s.error, "logs": s.logs, "runs": s.runs })),
            "env": report.env,
        });
        let prompt = format!(
            "The user clicked 'Fix with AI' on MCP server '{server_name}'. Diagnose the root cause from this diagnostic snapshot (note: all secrets are safe as secret:NAME) and propose a corrected SetupPlan if config/args/env need changing, or tell them clearly what to fix:\n{context}"
        );
        self.run_ai_turn(app, &prompt).await
    }

    async fn run_ai_turn(&self, app: &AppHandle, latest_prompt: &str) -> Result<SetupStateView, String> {
        let settings = current(&app.state::<SettingsStore>());
        if settings.active_model().is_none() {
            // Even without a configured LLM, match obvious catalog items so the user gets immediate value!
            let matches = mcp_catalog::search(latest_prompt);
            if let Some(preset) = matches.first() {
                let env = McpManager::check_environment();
                let plan = mcp_catalog::to_plan(preset, &env, self.secrets.as_ref());
                let reply = tr(
                    &format!("No AI model is configured in Settings > Models yet, so I matched **{}** from the built-in catalog:", preset.title_en),
                    &format!("هنوز مدلی در تنظیمات مدل‌ها اضافه نکردی، اما از کاتالوگ آماده **{}** را برات آوردم:", preset.title_fa),
                )
                .to_string();
                self.push_message(app, SetupRole::Assistant, reply, Some(plan), Vec::new(), None);
                return Ok(self.view());
            }
            let msg = tr(
                "Add an AI model in Settings > Models to chat freely for any MCP server, or click one of the quick-connect chips above (Gmail, GitHub, Playwright…).",
                "برای نصب هر سرور دلخواه با چت آزاد، اول در بخش «مدل‌ها» یک مدل اضافه کن — یا همین الان از دکمه‌های آماده بالا (Gmail، GitHub، Playwright و…) استفاده کن.",
            )
            .to_string();
            self.push_message(app, SetupRole::Assistant, msg, None, Vec::new(), None);
            return Ok(self.view());
        }

        self.set_busy(app, true, tr("Thinking…", "در حال بررسی…"));
        let brain = app.state::<Brain>();
        let model = active_generator(&brain, &settings);

        let mut conversation = self.build_llm_messages(latest_prompt);
        let tools = setup_tool_specs();
        let mut collected_steps: Vec<String> = Vec::new();
        let mut proposed_plan: Option<SetupPlan> = None;
        let mut final_text = String::new();
        let mut run_error: Option<String> = None;

        for round in 0..MAX_LLM_ROUNDS {
            let allow_tools = round + 1 < MAX_LLM_ROUNDS;
            let req = GenerateRequest {
                system: system_prompt(&settings.language),
                messages: conversation.clone(),
                tools: if allow_tools { tools.clone() } else { Vec::new() },
                allow_tools,
                max_output_tokens: 2400,
                thinking: Some(Thinking::Low),
            };
            let resp = match model.generate(&req).await {
                Ok(r) => r,
                Err(e) => {
                    run_error = Some(e.to_string());
                    break;
                }
            };
            let calls: Vec<(String, String, Value)> = resp
                .tool_calls()
                .into_iter()
                .map(|(id, name, args)| (id.to_string(), name.to_string(), args.clone()))
                .collect();
            if calls.is_empty() {
                final_text = resp.text();
                break;
            }
            conversation.push(Message { role: Role::Model, parts: resp.parts.clone() });
            let mut result_parts = Vec::with_capacity(calls.len());
            for (id, name, args) in calls {
                let step_label = setup_step_label(&name, &args);
                collected_steps.push(step_label.clone());
                self.set_busy(app, true, &step_label);
                let result = self.run_setup_tool(app, &name, &args, &mut proposed_plan).await;
                result_parts.push(Part::ToolResult { id, name, result });
            }
            conversation.push(Message { role: Role::User, parts: result_parts });
        }

        if final_text.is_empty() && run_error.is_none() {
            final_text = if proposed_plan.is_some() {
                tr(
                    "Fill in the card below and click **Install & connect**.",
                    "فیلدهای کارت پایین را کامل کن و دکمه **نصب و اتصال** را بزن.",
                )
                .to_string()
            } else {
                tr("Done.", "انجام شد.").to_string()
            };
        }

        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.busy = false;
            s.progress.clear();
        }
        self.push_message(app, SetupRole::Assistant, final_text, proposed_plan, collected_steps, run_error);
        Ok(self.view())
    }

    fn build_llm_messages(&self, latest_override: &str) -> Vec<Message> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let tail: Vec<&SetupMessage> = s.messages.iter().rev().take(14).collect::<Vec<_>>().into_iter().rev().collect();
        let mut out = Vec::with_capacity(tail.len());
        let last_idx = tail.len().saturating_sub(1);
        for (idx, m) in tail.iter().enumerate() {
            let role = match m.role {
                SetupRole::User => Role::User,
                SetupRole::Assistant => Role::Model,
            };
            let mut content = if idx == last_idx && m.role == SetupRole::User {
                latest_override.to_string()
            } else {
                m.text.clone()
            };
            if let Some(plan) = &m.plan {
                let fields: Vec<String> = plan.fields.iter().map(|f| format!("{} ({:?})", f.key, f.kind)).collect();
                content.push_str(&format!("\n[Proposed setup card: server={}, fields={:?}]", plan.server_name, fields));
            }
            out.push(Message { role, parts: vec![Part::text(content)] });
        }
        out
    }

    async fn run_setup_tool(&self, app: &AppHandle, name: &str, args: &Value, proposed_plan: &mut Option<SetupPlan>) -> Value {
        let brain = app.state::<Brain>();
        match name {
            "check_environment" => serde_json::to_value(McpManager::check_environment()).unwrap_or_default(),
            "list_installed_servers" => {
                let report = brain.mcp.report();
                let list: Vec<Value> = report
                    .servers
                    .iter()
                    .map(|s| {
                        json!({
                            "name": s.name,
                            "status": s.status,
                            "runs": s.runs,
                            "toolsCount": s.tools.len(),
                            "tools": s.tools,
                            "error": s.error,
                            "logs": s.logs.iter().rev().take(10).cloned().collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                json!({ "servers": list, "maxTools": report.max_tools, "activeToolCount": report.active_tool_count, "env": report.env })
            }
            "search_mcp_catalog" => {
                let query = args["query"].as_str().unwrap_or("");
                let env = McpManager::check_environment();
                let items: Vec<SetupPlan> = mcp_catalog::search(query)
                    .into_iter()
                    .take(5)
                    .map(|p| mcp_catalog::to_plan(p, &env, self.secrets.as_ref()))
                    .collect();
                json!({ "matches": items })
            }
            "web_search" => {
                let query = args["query"].as_str().unwrap_or("").trim();
                if query.is_empty() {
                    return json!({ "error": "query is required" });
                }
                let api = (self.search_api)();
                if api.is_none()
                    && let Some(google) = (self.google)()
                {
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    let found = self
                        .net
                        .try_both_if(
                            |client| {
                                let (google, today) = (google.clone(), today.clone());
                                async move { anytask_web::google_search(&client, &google, query, anytask_web::Depth::Quick, &today).await }
                            },
                            anytask_web::google_route_retry,
                            anytask_web::google_route_retry,
                        )
                        .await;
                    match found {
                        Ok(ans) => return serde_json::to_value(ans).unwrap_or_default(),
                        Err(e) => anytask_web::pause_google_after(&e, &google.key),
                    }
                }
                match self
                    .net
                    .try_both(|client| {
                        let api = api.clone();
                        async move { anytask_web::web_search(&client, query, api).await }
                    })
                    .await
                {
                    Ok(ans) => serde_json::to_value(ans).unwrap_or_default(),
                    Err(e) => json!({ "error": e }),
                }
            }
            "read_web_page" => {
                let url = args["url"].as_str().unwrap_or("").trim();
                if url.is_empty() {
                    return json!({ "error": "url is required" });
                }
                let query = args["query"].as_str().unwrap_or("");
                match self.net.try_for(url, |client| async move { anytask_web::read_web_page(&client, url, query).await }).await {
                    Ok(page) => serde_json::to_value(page).unwrap_or_default(),
                    Err(e) => json!({ "error": e }),
                }
            }
            "propose_mcp_install" => match self.build_plan_from_args(args) {
                Ok(plan) => {
                    *proposed_plan = Some(plan.clone());
                    json!({
                        "ok": true,
                        "serverName": plan.server_name,
                        "missingRequires": plan.missing_requires,
                        "note": "Setup card is attached to your reply. Explain briefly to the user what to fill in and click Install & connect."
                    })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            "test_mcp_server" => {
                let server = args["server"].as_str().unwrap_or("").trim();
                if server.is_empty() {
                    return json!({ "error": "server name is required" });
                }
                match brain.mcp.test_server(app, server).await {
                    Ok(r) => serde_json::to_value(r).unwrap_or_default(),
                    Err(e) => json!({ "error": e }),
                }
            }
            "test_mcp_tool" => {
                let server = args["server"].as_str().unwrap_or("").trim();
                let tool = args["tool"].as_str().unwrap_or("").trim();
                if server.is_empty() || tool.is_empty() {
                    return json!({ "error": "server and tool are required" });
                }
                let tool_args = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let res = brain.mcp.test_tool(server, tool, tool_args).await;
                serde_json::to_value(res).unwrap_or_default()
            }
            "remove_mcp_server" => {
                let server = args["server"].as_str().unwrap_or("").trim();
                let clean_secrets = args["cleanSecrets"].as_bool().unwrap_or(false);
                if server.is_empty() {
                    return json!({ "error": "server name is required" });
                }
                match brain.mcp.remove_server_with_secrets(app, server, clean_secrets).await {
                    Ok(_) => {
                        self.cleanup_server_files(server);
                        json!({ "ok": true, "removed": server })
                    }
                    Err(e) => json!({ "error": e }),
                }
            }
            other => json!({ "error": format!("Unknown setup tool: {other}") }),
        }
    }

    fn build_plan_from_args(&self, args: &Value) -> Result<SetupPlan, String> {
        // Shortcut: if the model passed a known presetId and no overrides, return the curated preset directly.
        if let Some(pid) = args.get("presetId").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
            && args.get("command").is_none()
            && args.get("url").is_none()
            && let Some(preset) = mcp_catalog::find_by_id(pid)
        {
            return Ok(mcp_catalog::to_plan(preset, &McpManager::check_environment(), self.secrets.as_ref()));
        }

        let raw_name = args["serverName"].as_str().unwrap_or("mcp-server");
        let server_name = sanitize_server_name(raw_name);
        let kind = args["kind"]
            .as_str()
            .unwrap_or_else(|| if args.get("url").and_then(Value::as_str).is_some() { "url" } else { "command" })
            .to_string();

        let mut command = args.get("command").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let mut parsed_args: Vec<String> = args
            .get("args")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        if kind == "command"
            && let Some(cmd) = command.clone()
            && parsed_args.is_empty()
            && cmd.contains(char::is_whitespace)
        {
            let parts: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
            if let Some((head, tail)) = parts.split_first() {
                command = Some(head.clone());
                parsed_args = tail.to_vec();
            }
        }
        let url = args.get("url").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if kind == "command" && command.is_none() {
            return Err("command is required when kind is 'command'".into());
        }
        if kind == "url" && !url.as_deref().is_some_and(|u| u.starts_with("http://") || u.starts_with("https://")) {
            return Err("url starting with http:// or https:// is required when kind is 'url'".into());
        }

        let static_env = string_map(args.get("staticEnv"));
        let static_headers = string_map(args.get("staticHeaders"));
        let tools: Vec<String> = args
            .get("tools")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();

        let mut requires: Vec<String> = args
            .get("requires")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        if requires.is_empty()
            && let Some(cmd) = command.as_deref()
        {
            let lower = cmd.to_ascii_lowercase();
            if ["npx", "node", "uvx", "uv", "python", "docker"].contains(&lower.as_str()) {
                requires.push(lower);
            }
        }
        let env_status = McpManager::check_environment();
        let missing_requires = mcp_catalog::missing_requirements(&requires, &env_status);

        let mut fields = Vec::new();
        for f in args.get("fields").and_then(Value::as_array).into_iter().flatten() {
            let key = f["key"].as_str().unwrap_or("").trim().to_string();
            if key.is_empty() {
                continue;
            }
            let fkind = match f["kind"].as_str().unwrap_or("text") {
                "secret" => FieldKind::Secret,
                "file_json" | "fileJson" | "file" => FieldKind::FileJson,
                _ => FieldKind::Text,
            };
            let target = match f["target"].as_str().unwrap_or(if kind == "url" { "header" } else { "env" }) {
                "header" => FieldTarget::Header,
                "arg_replace" | "argReplace" => FieldTarget::ArgReplace,
                _ => FieldTarget::Env,
            };
            let secret_name = if fkind == FieldKind::Secret {
                let raw_sn = f["secretName"].as_str().unwrap_or(&key);
                Some(sanitize_secret_name(raw_sn, &server_name, &key))
            } else {
                None
            };
            let already_set = secret_name.as_deref().is_some_and(|n| self.secrets.get(&secret_key_name(n)).is_some());
            fields.push(SetupField {
                key,
                label_en: f["labelEn"].as_str().or_else(|| f["label"].as_str()).unwrap_or("Value").to_string(),
                label_fa: f["labelFa"].as_str().or_else(|| f["label"].as_str()).unwrap_or("مقدار").to_string(),
                help_en: f["helpEn"].as_str().or_else(|| f["help"].as_str()).unwrap_or("").to_string(),
                help_fa: f["helpFa"].as_str().or_else(|| f["help"].as_str()).unwrap_or("").to_string(),
                help_url: f["helpUrl"].as_str().map(str::trim).filter(|u| u.starts_with("https://")).map(str::to_string),
                kind: fkind,
                secret_name,
                target,
                placeholder: f["placeholder"].as_str().map(str::to_string),
                default_value: f["defaultValue"].as_str().map(str::to_string),
                required: f["required"].as_bool().unwrap_or(true),
                already_set,
            });
        }

        let title = args["title"].as_str().unwrap_or(&server_name).to_string();
        let title_en = args["titleEn"].as_str().unwrap_or(&title).to_string();
        let title_fa = args["titleFa"].as_str().unwrap_or(&title).to_string();

        Ok(SetupPlan {
            preset_id: args["presetId"].as_str().map(str::to_string),
            server_name,
            title_en,
            title_fa,
            summary_en: args["summaryEn"].as_str().unwrap_or("").to_string(),
            summary_fa: args["summaryFa"].as_str().unwrap_or("").to_string(),
            kind,
            command,
            args: parsed_args,
            url,
            static_env,
            static_headers,
            tools,
            requires,
            missing_requires,
            fields,
        })
    }

    /// Saves secrets to Credential Manager, writes uploaded JSON credential files safely under app_data_dir/mcp-data,
    /// writes mcp-servers.json, connects the server, and records a sanitized outcome in the setup chat.
    pub async fn apply_plan(
        &self,
        app: &AppHandle,
        plan: SetupPlan,
        values: HashMap<String, String>,
        files: Vec<UploadedFile>,
    ) -> Result<ApplyOutcome, String> {
        let name = valid_name(&plan.server_name)?;
        let mut env = plan.static_env.clone();
        let mut headers = plan.static_headers.clone();
        let mut cmd_args = plan.args.clone();

        for field in &plan.fields {
            let uploaded = files.iter().find(|f| f.field_key == field.key);
            let typed = values.get(&field.key).map(String::as_str).map(str::trim).unwrap_or("");

            let resolved_value = match field.kind {
                FieldKind::Secret => {
                    let sname = field
                        .secret_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| sanitize_secret_name(&field.key, &name, &field.key));
                    if !typed.is_empty() {
                        self.secrets
                            .set(&secret_key_name(&sname), typed)
                            .map_err(|e| format!("Could not save secret {sname}: {e}"))?;
                    } else if self.secrets.get(&secret_key_name(&sname)).is_none() {
                        if field.required {
                            return Err(tr(
                                &format!("Please fill in \"{}\".", field.label_en),
                                &format!("لطفاً فیلد «{}» را پر کن.", field.label_fa),
                            )
                            .into());
                        }
                        continue;
                    }
                    format!("secret:{sname}")
                }
                FieldKind::FileJson => {
                    if let Some(file) = uploaded {
                        if file.content.len() > MAX_UPLOAD_BYTES {
                            return Err("Credential file is too large (max 512 KB).".into());
                        }
                        let safe_file = sanitize_filename(&file.filename);
                        if safe_file.ends_with(".json") && serde_json::from_str::<Value>(&file.content).is_err() {
                            return Err(tr(
                                &format!("{safe_file} is not valid JSON."),
                                &format!("فایل {safe_file} ساختار JSON معتبر ندارد."),
                            )
                            .into());
                        }
                        let dir = self.data_dir.join(&name);
                        std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create mcp-data folder: {e}"))?;
                        let dest = dir.join(safe_file);
                        std::fs::write(&dest, &file.content).map_err(|e| format!("Could not save credential file: {e}"))?;
                        dest.to_string_lossy().to_string()
                    } else if !typed.is_empty() {
                        typed.to_string()
                    } else if field.required {
                        return Err(tr(
                            &format!("Please select a file or path for \"{}\".", field.label_en),
                            &format!("لطفاً فایل یا مسیر «{}» را انتخاب کن.", field.label_fa),
                        )
                        .into());
                    } else {
                        continue;
                    }
                }
                FieldKind::Text => {
                    let v = if !typed.is_empty() { typed } else { field.default_value.as_deref().unwrap_or("").trim() };
                    if v.is_empty() {
                        if field.required {
                            return Err(tr(
                                &format!("Please fill in \"{}\".", field.label_en),
                                &format!("لطفاً فیلد «{}» را پر کن.", field.label_fa),
                            )
                            .into());
                        }
                        continue;
                    }
                    v.to_string()
                }
            };

            match field.target {
                FieldTarget::Env => {
                    env.insert(field.key.clone(), resolved_value);
                }
                FieldTarget::Header => {
                    headers.insert(field.key.clone(), resolved_value);
                }
                FieldTarget::ArgReplace => {
                    let placeholder = format!("{{{}}}", field.key);
                    let mut replaced = false;
                    for a in &mut cmd_args {
                        if a.contains(&placeholder) {
                            *a = a.replace(&placeholder, &resolved_value);
                            replaced = true;
                        }
                    }
                    if !replaced {
                        cmd_args.push(resolved_value);
                    }
                }
            }
        }

        let server = if plan.kind == "url" {
            ServerConfig {
                url: plan.url.clone(),
                headers,
                tools: plan.tools.clone(),
                ..Default::default()
            }
        } else {
            ServerConfig {
                command: plan.command.clone(),
                args: cmd_args,
                env,
                tools: plan.tools.clone(),
                ..Default::default()
            }
        };

        let brain = app.state::<Brain>();
        let report = brain.mcp.save_server(app, &name, server, None).await?;
        let server_status = report.servers.iter().find(|s| s.name == name).cloned();

        let summary = match &server_status {
            Some(s) if s.status == anytask_mcp::Status::Connected => tr(
                &format!("Connected **`{name}`** ✓ ({} tools ready)", s.tools.len()),
                &format!("سرور **`{name}`** با موفقیت وصل شد ✓ ({} ابزار آماده استفاده است)", s.tools.len()),
            )
            .to_string(),
            Some(s) => tr(
                &format!("Saved **`{name}`**, but connecting failed: `{}`\n\nClick **Fix with AI** or ask me below to troubleshoot it.", s.error.as_deref().unwrap_or("unknown error")),
                &format!("سرور **`{name}`** ذخیره شد اما اتصال خطا خورد: `{}`\n\nمی‌تونی پایین ازم بخوای برطرفش کنم یا دکمه **رفع مشکل با AI** را بزنی.", s.error.as_deref().unwrap_or("خطای ناشناخته")),
            )
            .to_string(),
            None => format!("Saved `{name}`."),
        };
        self.push_message(app, SetupRole::Assistant, summary, None, Vec::new(), None);

        Ok(ApplyOutcome { report, setup: self.view(), server: server_status })
    }

    pub fn cleanup_server_files(&self, name: &str) {
        if let Ok(valid) = valid_name(name) {
            let _ = std::fs::remove_dir_all(self.data_dir.join(valid));
        }
    }

    /// Installs Node.js LTS or uv on Windows via `winget` when explicitly clicked by the user.
    pub async fn install_prereq(&self, kind: &str) -> Result<String, String> {
        #[cfg(windows)]
        {
            let (pkg_id, label) = match kind {
                "node" | "npx" => ("OpenJS.NodeJS.LTS", "Node.js (npx)"),
                "uv" | "uvx" => ("astral-sh.uv", "uv (uvx)"),
                _ => return Err(format!("Unsupported prerequisite: {kind}")),
            };
            let mut cmd = tokio::process::Command::new("winget");
            cmd.args([
                "install",
                "--id",
                pkg_id,
                "-e",
                "--accept-source-agreements",
                "--accept-package-agreements",
            ]);
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
            let output = tokio::time::timeout(std::time::Duration::from_secs(180), cmd.output())
                .await
                .map_err(|_| format!("Installing {label} timed out after 3 minutes."))?
                .map_err(|_| format!("Could not run winget. Install {label} manually from its official website."))?;
            if output.status.success() {
                Ok(format!("{label} installed."))
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let detail = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
                Err(format!("winget could not install {label}: {}", detail.chars().take(220).collect::<String>()))
            }
        }
        #[cfg(not(windows))]
        {
            let _ = kind;
            Err("Automatic prerequisite install is supported on Windows via winget; install Node.js or uv with your package manager.".into())
        }
    }
}

fn system_prompt(language: &str) -> String {
    let lang_rule = if language == "fa" {
        "Reply in natural, friendly Persian (even if the user types Finglish). Keep technical commands/names in English backticks."
    } else {
        "Reply in the same language the user writes in (Persian if Persian or Finglish; English if English)."
    };
    format!(
        "You are AnyTask's MCP Setup Assistant inside Settings > Tools & MCP.\n\
        Your mission is to make adding, testing, troubleshooting and removing MCP servers effortless.\n\
        {lang_rule}\n\
        Rules:\n\
        1. First check `search_mcp_catalog` and `check_environment` (and `list_installed_servers` if helpful) before answering.\n\
        2. If the user asks for something NOT in the built-in catalog, ALWAYS use `web_search` and `read_web_page` to verify the REAL current npm/PyPI package name, command, args and environment variable names. NEVER guess package names or env var names.\n\
        3. NEVER ask the user to paste passwords, API keys or tokens in plain chat text! Always call `propose_mcp_install` so the UI renders a secure form card where secrets go straight into Windows Credential Manager.\n\
        4. For Gmail: recommend the quick `gmail` preset (App Password, 1 minute, no Google Cloud needed) first, and mention `google-workspace` (OAuth for Gmail+Calendar+Drive) if they want full Workspace OAuth.\n\
        5. Only call `test_mcp_tool` when the user explicitly asks to test a server or tool, and only pick harmless read/list checks (never send/delete/modify user data when testing).\n\
        6. Be concise, clear and practical."
    )
}

fn setup_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "check_environment".into(),
            description: "Checks whether node, npx, uv, uvx, python, git and docker are available on this PC.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "list_installed_servers".into(),
            description: "Lists currently configured MCP servers, their statuses, active tools, errors and recent stderr logs.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "search_mcp_catalog".into(),
            description: "Searches the built-in catalog of verified MCP presets (gmail, google-workspace, github, notion, playwright, filesystem, memory, fetch, postgres, brave-search, sequential-thinking).".into(),
            parameters: json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "Service name or keyword, or empty string for all." } },
                "required": ["query"]
            }),
        },
        ToolSpec {
            name: "web_search".into(),
            description: "Searches the web for an MCP server not in the catalog (npm package, GitHub repo, or remote MCP URL).".into(),
            parameters: json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
        },
        ToolSpec {
            name: "read_web_page".into(),
            description: "Reads a web page (such as a GitHub README or npm package page) to verify exact MCP command, args and env vars.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "url": { "type": "string" }, "query": { "type": "string" } },
                "required": ["url"]
            }),
        },
        ToolSpec {
            name: "propose_mcp_install".into(),
            description: "Attaches an interactive secure setup card to your reply. For catalog matches you can pass presetId alone, or pass full command/url and fields.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "presetId": { "type": "string", "description": "Optional catalog preset id (e.g. 'gmail', 'github', 'notion')." },
                    "serverName": { "type": "string", "description": "Short ASCII name (letters, digits, -, _)." },
                    "titleEn": { "type": "string" },
                    "titleFa": { "type": "string" },
                    "summaryEn": { "type": "string" },
                    "summaryFa": { "type": "string" },
                    "kind": { "type": "string", "enum": ["command", "url"] },
                    "command": { "type": "string", "description": "Executable name like npx or uvx." },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "url": { "type": "string", "description": "Streamable HTTP URL when kind is url." },
                    "staticEnv": { "type": "object", "description": "Non-secret fixed env vars." },
                    "staticHeaders": { "type": "object", "description": "Non-secret fixed HTTP headers." },
                    "requires": { "type": "array", "items": { "type": "string" }, "description": "Required local commands like ['npx'] or ['uvx']." },
                    "fields": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "key": { "type": "string" },
                                "labelEn": { "type": "string" },
                                "labelFa": { "type": "string" },
                                "helpEn": { "type": "string" },
                                "helpFa": { "type": "string" },
                                "helpUrl": { "type": "string" },
                                "kind": { "type": "string", "enum": ["secret", "text", "file_json"] },
                                "secretName": { "type": "string" },
                                "target": { "type": "string", "enum": ["env", "header", "arg_replace"] },
                                "placeholder": { "type": "string" },
                                "defaultValue": { "type": "string" },
                                "required": { "type": "boolean" }
                            },
                            "required": ["key"]
                        }
                    }
                },
                "required": ["serverName"]
            }),
        },
        ToolSpec {
            name: "test_mcp_server".into(),
            description: "Tests connection and tool listing for an installed MCP server.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "server": { "type": "string" } },
                "required": ["server"]
            }),
        },
        ToolSpec {
            name: "test_mcp_tool".into(),
            description: "Runs a single read-only test call on an MCP tool when requested by the user.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "server": { "type": "string" },
                    "tool": { "type": "string" },
                    "arguments": { "type": "object" }
                },
                "required": ["server", "tool"]
            }),
        },
        ToolSpec {
            name: "remove_mcp_server".into(),
            description: "Removes a configured MCP server (optionally deleting its exclusive secrets).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "server": { "type": "string" },
                    "cleanSecrets": { "type": "boolean" }
                },
                "required": ["server"]
            }),
        },
    ]
}

fn setup_step_label(name: &str, args: &Value) -> String {
    match name {
        "check_environment" => tr("Checking Node / uv on PC…", "بررسی پیش‌نیازها روی سیستم…").into(),
        "list_installed_servers" => tr("Checking installed MCP servers…", "بررسی سرورهای MCP فعلی…").into(),
        "search_mcp_catalog" => tr("Checking verified MCP catalog…", "جستجو در کاتالوگ آماده…").into(),
        "web_search" => format!("{} {}", tr("Searching web:", "جستجوی وب:"), args["query"].as_str().unwrap_or("")),
        "read_web_page" => tr("Reading MCP documentation…", "خواندن مستندات سرور…").into(),
        "propose_mcp_install" => tr("Preparing setup card…", "آماده‌سازی کارت نصب…").into(),
        "test_mcp_server" => format!("{} {}", tr("Testing server:", "تست سرور:"), args["server"].as_str().unwrap_or("")),
        "test_mcp_tool" => format!("{} {}", tr("Testing tool:", "تست ابزار:"), args["tool"].as_str().unwrap_or("")),
        "remove_mcp_server" => format!("{} {}", tr("Removing server:", "حذف سرور:"), args["server"].as_str().unwrap_or("")),
        _ => name.to_string(),
    }
}

fn string_map(val: Option<&Value>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(obj) = val.and_then(Value::as_object) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                out.insert(k.clone(), s.to_string());
            }
        }
    } else if let Some(s) = val.and_then(Value::as_str)
        && let Ok(parsed) = serde_json::from_str::<BTreeMap<String, String>>(s)
    {
        return parsed;
    }
    out
}

fn sanitize_server_name(raw: &str) -> String {
    let clean: String = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .take(40)
        .collect();
    let trimmed = clean.trim_matches('-').to_string();
    if trimmed.is_empty() { "mcp-server".into() } else { trimmed }
}

fn sanitize_secret_name(raw: &str, server: &str, key: &str) -> String {
    let candidate = if raw.trim().is_empty() { format!("{server}_{key}") } else { raw.to_string() };
    let clean: String = candidate
        .trim()
        .to_ascii_uppercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .take(50)
        .collect();
    if clean.is_empty() { "MCP_SECRET".into() } else { clean }
}

fn sanitize_filename(raw: &str) -> String {
    let base = std::path::Path::new(raw).file_name().and_then(|n| n.to_str()).unwrap_or("credentials.json");
    let clean: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .take(64)
        .collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        "credentials.json".into()
    } else {
        clean
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names_and_filenames_against_traversal() {
        assert_eq!(sanitize_server_name("My Gmail!"), "my-gmail");
        assert_eq!(sanitize_server_name("   "), "mcp-server");
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("gcp-oauth.keys.json"), "gcp-oauth.keys.json");
        assert_eq!(sanitize_secret_name("github token", "github", "TOKEN"), "GITHUB_TOKEN");
    }
}