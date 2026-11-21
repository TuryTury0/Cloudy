//! Curated catalog of verified MCP servers so common integrations (Gmail, Google Workspace, GitHub, Notion,
//! Playwright, Postgres, Filesystem...) connect reliably without the AI guessing package names or env vars.

use crate::mcp::secret_key_name;
use anytask_mcp::EnvStatus;
use anytask_storage::SecretStore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Secret,
    Text,
    FileJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldTarget {
    Env,
    Header,
    ArgReplace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupField {
    pub key: String,
    pub label_en: String,
    pub label_fa: String,
    pub help_en: String,
    pub help_fa: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help_url: Option<String>,
    pub kind: FieldKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_name: Option<String>,
    pub target: FieldTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub already_set: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    pub server_name: String,
    pub title_en: String,
    pub title_fa: String,
    pub summary_en: String,
    pub summary_fa: String,
    /// "command" or "url"
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub static_env: BTreeMap<String, String>,
    #[serde(default)]
    pub static_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub missing_requires: Vec<String>,
    #[serde(default)]
    pub fields: Vec<SetupField>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSummary {
    pub id: String,
    pub server_name: String,
    pub title_en: String,
    pub title_fa: String,
    pub summary_en: String,
    pub summary_fa: String,
    pub requires: Vec<String>,
}

pub struct PresetDef {
    pub id: &'static str,
    pub server_name: &'static str,
    pub title_en: &'static str,
    pub title_fa: &'static str,
    pub summary_en: &'static str,
    pub summary_fa: &'static str,
    pub kind: &'static str,
    pub command: Option<&'static str>,
    pub args: &'static [&'static str],
    pub url: Option<&'static str>,
    pub static_env: &'static [(&'static str, &'static str)],
    pub requires: &'static [&'static str],
    pub tags: &'static [&'static str],
    pub fields: &'static [PresetFieldDef],
}

pub struct PresetFieldDef {
    pub key: &'static str,
    pub label_en: &'static str,
    pub label_fa: &'static str,
    pub help_en: &'static str,
    pub help_fa: &'static str,
    pub help_url: Option<&'static str>,
    pub kind: FieldKind,
    pub secret_name: Option<&'static str>,
    pub target: FieldTarget,
    pub placeholder: &'static str,
    pub required: bool,
}

const PRESETS: &[PresetDef] = &[
    PresetDef {
        id: "gmail",
        server_name: "gmail",
        title_en: "Gmail (Quick App Password)",
        title_fa: "Gmail (سریع با App Password)",
        summary_en: "Read, search, send and reply to Gmail in 1 minute with an App Password (no Google Cloud needed).",
        summary_fa: "خواندن، جستجو، ارسال و پاسخ ایمیل‌های جیمیل در ۱ دقیقه با رمز برنامه گوگل (بدون نیاز به Google Cloud).",
        kind: "command",
        command: Some("npx"),
        args: &["-y", "@n24q02m/better-email-mcp"],
        url: None,
        static_env: &[("EMAIL_PROVIDER", "gmail")],
        requires: &["npx"],
        tags: &["gmail", "email", "mail", "google mail", "جیمیل", "ایمیل"],
        fields: &[
            PresetFieldDef {
                key: "EMAIL_USER",
                label_en: "Gmail address",
                label_fa: "آدرس جیمیل",
                help_en: "Your full Gmail address.",
                help_fa: "آدرس کامل ایمیل جیمیل شما.",
                help_url: None,
                kind: FieldKind::Text,
                secret_name: None,
                target: FieldTarget::Env,
                placeholder: "you@gmail.com",
                required: true,
            },
            PresetFieldDef {
                key: "EMAIL_APP_PASSWORD",
                label_en: "Google App Password",
                label_fa: "رمز برنامه گوگل (App Password)",
                help_en: "Turn on 2-Step Verification in your Google Account, then create an App Password (16 letters) and paste it here. Stored only in Credential Manager.",
                help_fa: "در حساب گوگل «تأیید دومرحله‌ای» را روشن کن، یک App Password بساز (کد ۱۶ حرفی) و اینجا بزن. فقط در Windows Credential Manager ذخیره می‌شود.",
                help_url: Some("https://myaccount.google.com/apppasswords"),
                kind: FieldKind::Secret,
                secret_name: Some("GMAIL_APP_PASSWORD"),
                target: FieldTarget::Env,
                placeholder: "xxxx xxxx xxxx xxxx",
                required: true,
            },
        ],
    },
    PresetDef {
        id: "google-workspace",
        server_name: "google-workspace",
        title_en: "Google Workspace (Gmail + Calendar + Drive OAuth)",
        title_fa: "Google Workspace (جیمیل + تقویم + درایو با OAuth)",
        summary_en: "Official OAuth access to Gmail, Google Calendar and Google Drive via Google Cloud Desktop credentials.",
        summary_fa: "اتصال کامل OAuth به جیمیل، تقویم گوگل و گوگل درایو با کلید Google Cloud (بار اول مرورگر باز می‌شود).",
        kind: "command",
        command: Some("uvx"),
        args: &["workspace-mcp", "--tools", "gmail", "calendar", "drive"],
        url: None,
        static_env: &[],
        requires: &["uvx"],
        tags: &["google", "workspace", "calendar", "drive", "oauth", "gmail oauth", "تقویم", "درایو"],
        fields: &[
            PresetFieldDef {
                key: "GOOGLE_OAUTH_CLIENT_ID",
                label_en: "OAuth Client ID",
                label_fa: "شناسه کلاینت OAuth (Client ID)",
                help_en: "Create an OAuth 2.0 Client ID (Desktop app) in Google Cloud Console and enable Gmail/Calendar/Drive APIs.",
                help_fa: "در Google Cloud Console یک OAuth Client ID (نوع Desktop app) بساز و APIهای جیمیل/تقویم/درایو را فعال کن.",
                help_url: Some("https://console.cloud.google.com/apis/credentials"),
                kind: FieldKind::Secret,
                secret_name: Some("GOOGLE_OAUTH_CLIENT_ID"),
                target: FieldTarget::Env,
                placeholder: "xxxx.apps.googleusercontent.com",
                required: true,
            },
            PresetFieldDef {
                key: "GOOGLE_OAUTH_CLIENT_SECRET",
                label_en: "OAuth Client Secret",
                label_fa: "کلید مخفی OAuth (Client Secret)",
                help_en: "The Client Secret shown right below your Client ID.",
                help_fa: "مقدار Client Secret که کنار Client ID در Google Cloud نمایش داده می‌شود.",
                help_url: None,
                kind: FieldKind::Secret,
                secret_name: Some("GOOGLE_OAUTH_CLIENT_SECRET"),
                target: FieldTarget::Env,
                placeholder: "GOCSPX-...",
                required: true,
            },
        ],
    },
    PresetDef {
        id: "github",
        server_name: "github",
        title_en: "GitHub (Local MCP)",
        title_fa: "GitHub (اجرای محلی)",
        summary_en: "Read repos, issues, pull requests and code on GitHub using your Personal Access Token.",
        summary_fa: "مدیریت ریپوها، Issueها، Pull Requestها و کدهای گیت‌هاب با توکن شخصی.",
        kind: "command",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-github"],
        url: None,
        static_env: &[],
        requires: &["npx"],
        tags: &["github", "git", "repo", "pr", "issue", "گیت‌هاب", "گیت"],
        fields: &[PresetFieldDef {
            key: "GITHUB_PERSONAL_ACCESS_TOKEN",
            label_en: "GitHub Personal Access Token",
            label_fa: "توکن دسترسی گیت‌هاب (PAT)",
            help_en: "Create a Classic or Fine-grained Personal Access Token on GitHub.",
            help_fa: "یک توکن دسترسی شخصی (PAT) در تنظیمات گیت‌هاب بساز و اینجا بگذار.",
            help_url: Some("https://github.com/settings/tokens"),
            kind: FieldKind::Secret,
            secret_name: Some("GITHUB_TOKEN"),
            target: FieldTarget::Env,
            placeholder: "ghp_...",
            required: true,
        }],
    },
    PresetDef {
        id: "github-remote",
        server_name: "github",
        title_en: "GitHub (Cloud HTTP — no Node needed)",
        title_fa: "GitHub (ابری بدون نیاز به Node)",
        summary_en: "Connects straight to GitHub's hosted MCP endpoint over HTTPS using your token.",
        summary_fa: "اتصال مستقیم به سرور رسمی ابری گیت‌هاب بدون نیاز به نصب Node.js یا پکیج محلی.",
        kind: "url",
        command: None,
        args: &[],
        url: Some("https://api.githubcopilot.com/mcp/"),
        static_env: &[],
        requires: &[],
        tags: &["github remote", "github cloud", "copilot mcp"],
        fields: &[PresetFieldDef {
            key: "Authorization",
            label_en: "GitHub Personal Access Token",
            label_fa: "توکن گیت‌هاب (PAT)",
            help_en: "Paste your GitHub token; AnyTask sends it securely as a Bearer header.",
            help_fa: "توکن گیت‌هاب را وارد کن؛ برنامه خودش امن در هدر Authorization می‌فرستد.",
            help_url: Some("https://github.com/settings/tokens"),
            kind: FieldKind::Secret,
            secret_name: Some("GITHUB_TOKEN"),
            target: FieldTarget::Header,
            placeholder: "ghp_...",
            required: true,
        }],
    },
    PresetDef {
        id: "notion",
        server_name: "notion",
        title_en: "Notion",
        title_fa: "Notion (نوشن)",
        summary_en: "Search, read and update your Notion pages and databases.",
        summary_fa: "جستجو، خواندن و ویرایش صفحات و دیتابیس‌های Notion.",
        kind: "command",
        command: Some("npx"),
        args: &["-y", "@notionhq/notion-mcp-server"],
        url: None,
        static_env: &[],
        requires: &["npx"],
        tags: &["notion", "notes", "wiki", "نوشن", "یادداشت"],
        fields: &[PresetFieldDef {
            key: "NOTION_TOKEN",
            label_en: "Notion Integration Token",
            label_fa: "توکن Integration نوشن",
            help_en: "Create an Internal Integration in Notion, copy its secret (ntn_...), and share your target pages with that integration.",
            help_fa: "یک Integration در نوشن بساز، رمز آن (ntn_...) را کپی کن و صفحات مورد نظرت را با آن Share کن.",
            help_url: Some("https://www.notion.so/profile/integrations"),
            kind: FieldKind::Secret,
            secret_name: Some("NOTION_TOKEN"),
            target: FieldTarget::Env,
            placeholder: "ntn_...",
            required: true,
        }],
    },
    PresetDef {
        id: "playwright",
        server_name: "playwright",
        title_en: "Playwright (Browser)",
        title_fa: "Playwright (کنترل مرورگر)",
        summary_en: "Gives the assistant a real headless/visible browser for web apps and forms.",
        summary_fa: "یک مرورگر کامل و دقیق در اختیار دستیار می‌گذارد (بدون نیاز به کلید یا رمز).",
        kind: "command",
        command: Some("npx"),
        args: &["-y", "@playwright/mcp@latest"],
        url: None,
        static_env: &[],
        requires: &["npx"],
        tags: &["playwright", "browser", "web", "مرورگر", "وب"],
        fields: &[],
    },
    PresetDef {
        id: "filesystem",
        server_name: "filesystem",
        title_en: "Filesystem (Folder access)",
        title_fa: "دسترسی به پوشه (Filesystem)",
        summary_en: "Lets MCP tools read and write inside a specific folder you pick.",
        summary_fa: "اجازه خواندن و نوشتن فایل‌ها در یک پوشه مشخص روی سیستم.",
        kind: "command",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-filesystem", "{FOLDER}"],
        url: None,
        static_env: &[],
        requires: &["npx"],
        tags: &["filesystem", "folder", "files", "پوشه", "فایل"],
        fields: &[PresetFieldDef {
            key: "FOLDER",
            label_en: "Folder path",
            label_fa: "مسیر پوشه مجاز",
            help_en: "Full path to the folder the server may access.",
            help_fa: "مسیر کامل پوشه‌ای که سرور اجازه دسترسی به آن را دارد.",
            help_url: None,
            kind: FieldKind::Text,
            secret_name: None,
            target: FieldTarget::ArgReplace,
            placeholder: "C:/Users/YourName/Documents",
            required: true,
        }],
    },
    PresetDef {
        id: "memory",
        server_name: "memory",
        title_en: "Memory (Knowledge Graph)",
        title_fa: "حافظه (گراف یادداشت‌ها)",
        summary_en: "Persistent knowledge graph memory of people, projects and notes.",
        summary_fa: "دفترچه حافظه ساختاریافته برای ثبت افراد، پروژه‌ها و نکات.",
        kind: "command",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-memory"],
        url: None,
        static_env: &[],
        requires: &["npx"],
        tags: &["memory", "notes", "graph", "حافظه", "یادداشت"],
        fields: &[],
    },
    PresetDef {
        id: "fetch",
        server_name: "fetch",
        title_en: "Fetch (Web page reader)",
        title_fa: "Fetch (خواندن صفحات وب)",
        summary_en: "Fetches web pages and converts HTML to clean Markdown.",
        summary_fa: "دریافت صفحات وب و تبدیل به متن تمیز (بدون نیاز به کلید).",
        kind: "command",
        command: Some("uvx"),
        args: &["mcp-server-fetch"],
        url: None,
        static_env: &[],
        requires: &["uvx"],
        tags: &["fetch", "http", "webpage", "سایت"],
        fields: &[],
    },
    PresetDef {
        id: "postgres",
        server_name: "postgres",
        title_en: "PostgreSQL (Read-safe)",
        title_fa: "PostgreSQL (دیتابیس پستگرس)",
        summary_en: "Inspect schemas and run safe read-only queries on a PostgreSQL database.",
        summary_fa: "بررسی ساختار و اجرای کوئری‌های امن روی دیتابیس PostgreSQL.",
        kind: "command",
        command: Some("uvx"),
        args: &["postgres-mcp", "--access-mode=restricted"],
        url: None,
        static_env: &[],
        requires: &["uvx"],
        tags: &["postgres", "postgresql", "sql", "database", "دیتابیس"],
        fields: &[PresetFieldDef {
            key: "DATABASE_URI",
            label_en: "Database URI",
            label_fa: "آدرس اتصال دیتابیس (URI)",
            help_en: "PostgreSQL connection string (stored securely in Windows Credential Manager).",
            help_fa: "رشته اتصال دیتابیس (امن در Credential Manager ذخیره می‌شود).",
            help_url: None,
            kind: FieldKind::Secret,
            secret_name: Some("POSTGRES_URI"),
            target: FieldTarget::Env,
            placeholder: "postgresql://user:pass@localhost:5432/mydb",
            required: true,
        }],
    },
    PresetDef {
        id: "brave-search",
        server_name: "brave-search",
        title_en: "Brave Search",
        title_fa: "جستجوی Brave",
        summary_en: "Web and local search powered by the Brave Search API.",
        summary_fa: "جستجوی سریع وب با کلید رسمی Brave Search.",
        kind: "command",
        command: Some("npx"),
        args: &["-y", "@brave/brave-search-mcp-server", "--transport", "stdio"],
        url: None,
        static_env: &[],
        requires: &["npx"],
        tags: &["brave", "search", "جستجو"],
        fields: &[PresetFieldDef {
            key: "BRAVE_API_KEY",
            label_en: "Brave Search API Key",
            label_fa: "کلید API جستجوی Brave",
            help_en: "Get a free/paid API key from the Brave Search API dashboard.",
            help_fa: "یک کلید از پنل Brave Search API بگیر و اینجا وارد کن.",
            help_url: Some("https://api-dashboard.search.brave.com/"),
            kind: FieldKind::Secret,
            secret_name: Some("BRAVE_API_KEY"),
            target: FieldTarget::Env,
            placeholder: "BSA...",
            required: true,
        }],
    },
    PresetDef {
        id: "sequential-thinking",
        server_name: "sequential-thinking",
        title_en: "Sequential Thinking",
        title_fa: "فکر قدم‌به‌قدم",
        summary_en: "Structured step-by-step reasoning helper for complex multi-step tasks.",
        summary_fa: "کمک به برنامه‌ریزی و حل گام‌به‌گام کارهای پیچیده.",
        kind: "command",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-sequential-thinking"],
        url: None,
        static_env: &[],
        requires: &["npx"],
        tags: &["thinking", "planning", "فکر", "برنامه‌ریزی"],
        fields: &[],
    },
];

pub fn summaries() -> Vec<CatalogSummary> {
    PRESETS
        .iter()
        .map(|p| CatalogSummary {
            id: p.id.to_string(),
            server_name: p.server_name.to_string(),
            title_en: p.title_en.to_string(),
            title_fa: p.title_fa.to_string(),
            summary_en: p.summary_en.to_string(),
            summary_fa: p.summary_fa.to_string(),
            requires: p.requires.iter().map(|s| s.to_string()).collect(),
        })
        .collect()
}

pub fn find_by_id(id: &str) -> Option<&'static PresetDef> {
    let needle = id.trim().to_ascii_lowercase();
    PRESETS.iter().find(|p| p.id == needle || p.server_name == needle)
}

pub fn search(query: &str) -> Vec<&'static PresetDef> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return PRESETS.iter().collect();
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut scored: Vec<(usize, &'static PresetDef)> = PRESETS
        .iter()
        .filter_map(|p| {
            let hay = format!("{} {} {} {} {}", p.id, p.title_en, p.title_fa, p.summary_en, p.tags.join(" ")).to_lowercase();
            let hits = words.iter().filter(|w| hay.contains(**w)).count();
            (hits > 0).then_some((hits, p))
        })
        .collect();
    scored.sort_by_key(|b| std::cmp::Reverse(b.0));
    scored.into_iter().map(|(_, p)| p).collect()
}

pub fn missing_requirements(requires: &[String], env: &EnvStatus) -> Vec<String> {
    requires
        .iter()
        .filter(|r| match r.as_str() {
            "node" => !env.node,
            "npx" => !env.npx,
            "uv" => !env.uv,
            "uvx" => !env.uvx,
            "python" => !env.python,
            "git" => !env.git,
            "docker" => !env.docker,
            _ => false,
        })
        .cloned()
        .collect()
}

pub fn to_plan(preset: &PresetDef, env: &EnvStatus, secrets: &dyn SecretStore) -> SetupPlan {
    let default_docs = anytask_core::UserFolders::detect().documents.to_string_lossy().replace('\\', "/");
    let requires: Vec<String> = preset.requires.iter().map(|s| s.to_string()).collect();
    let missing_requires = missing_requirements(&requires, env);
    let fields = preset
        .fields
        .iter()
        .map(|f| {
            let already_set = f.secret_name.is_some_and(|n| secrets.get(&secret_key_name(n)).is_some());
            let default_value = if f.key == "FOLDER" { Some(default_docs.clone()) } else { None };
            SetupField {
                key: f.key.to_string(),
                label_en: f.label_en.to_string(),
                label_fa: f.label_fa.to_string(),
                help_en: f.help_en.to_string(),
                help_fa: f.help_fa.to_string(),
                help_url: f.help_url.map(str::to_string),
                kind: f.kind,
                secret_name: f.secret_name.map(str::to_string),
                target: f.target,
                placeholder: (!f.placeholder.is_empty()).then(|| f.placeholder.to_string()),
                default_value,
                required: f.required,
                already_set,
            }
        })
        .collect();
    SetupPlan {
        preset_id: Some(preset.id.to_string()),
        server_name: preset.server_name.to_string(),
        title_en: preset.title_en.to_string(),
        title_fa: preset.title_fa.to_string(),
        summary_en: preset.summary_en.to_string(),
        summary_fa: preset.summary_fa.to_string(),
        kind: preset.kind.to_string(),
        command: preset.command.map(str::to_string),
        args: preset.args.iter().map(|s| s.to_string()).collect(),
        url: preset.url.map(str::to_string),
        static_env: preset.static_env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        static_headers: BTreeMap::new(),
        tools: Vec::new(),
        requires,
        missing_requires,
        fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anytask_storage::MemorySecrets;

    #[test]
    fn finds_gmail_by_persian_or_english_and_checks_existing_secret() {
        assert_eq!(search("جیمیل")[0].id, "gmail");
        assert_eq!(search("github")[0].id, "github");
        let secrets = MemorySecrets::default();
        secrets.set(&secret_key_name("GMAIL_APP_PASSWORD"), "abcd").unwrap();
        let plan = to_plan(find_by_id("gmail").unwrap(), &EnvStatus { npx: true, node: true, ..Default::default() }, &secrets);
        assert!(plan.missing_requires.is_empty());
        assert!(plan.fields.iter().find(|f| f.key == "EMAIL_APP_PASSWORD").unwrap().already_set);
    }
}