//! mcp-servers.json: the list of servers, in the format Claude Desktop uses (so configs can be copied over).

use crate::SecretLookup;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, ServerConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// A program that speaks MCP on stdin/stdout ("npx", "uvx", a path...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Or a remote server (streamable HTTP).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Only these tools (empty = all).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

impl ServerConfig {
    /// All secret names referenced as `secret:NAME` in `env` or `headers`.
    pub fn referenced_secrets(&self) -> BTreeSet<String> {
        self.env
            .values()
            .chain(self.headers.values())
            .filter_map(|v| v.strip_prefix("secret:").map(|n| n.trim().to_string()))
            .filter(|n| !n.is_empty())
            .collect()
    }
}

impl McpConfig {
    /// Reads the file. A missing file is created empty (so the user can open and edit it); a broken one is an error
    /// with the reason, and the file is left as it is.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) if text.trim().is_empty() => Ok(Self::default()),
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{} is not valid: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let empty = Self::default();
                let _ = empty.save(path);
                Ok(empty)
            }
            Err(e) => Err(format!("Could not read {}: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("Could not save {}: {e}", path.display()))
    }

    /// All secret names referenced across every configured server.
    pub fn referenced_secrets(&self) -> BTreeSet<String> {
        self.mcp_servers.values().flat_map(ServerConfig::referenced_secrets).collect()
    }
}

/// Resolves "secret:NAME" values with the stored secret; other values stay as they are.
/// Returns both the resolved map and the secret strings that were substituted (for log redaction).
pub(crate) fn resolve_with_secrets(
    values: &BTreeMap<String, String>,
    secrets: &SecretLookup,
) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let mut resolved = BTreeMap::new();
    let mut secret_values = Vec::new();
    for (key, value) in values {
        match value.strip_prefix("secret:") {
            Some(name) => {
                let secret = secrets(name.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("The secret {} is missing; save it in Settings > Tools.", name.trim()))?;
                if secret.len() >= 6 {
                    secret_values.push(secret.clone());
                }
                resolved.insert(key.clone(), secret);
            }
            None => {
                resolved.insert(key.clone(), value.clone());
            }
        }
    }
    Ok((resolved, secret_values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn reads_claude_desktop_files_and_writes_them_back_unchanged() {
        let text = r#"{ "mcpServers": {
            "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": { "TOKEN": "secret:GITHUB" } },
            "remote": { "url": "https://example.com/mcp", "disabled": true }
        } }"#;
        let config: McpConfig = serde_json::from_str(text).unwrap();
        assert_eq!(config.mcp_servers["github"].args.len(), 2);
        assert!(config.mcp_servers["remote"].disabled);
        assert_eq!(config.referenced_secrets(), BTreeSet::from(["GITHUB".to_string()]));
        let again: serde_json::Value = serde_json::to_value(&config).unwrap();
        assert_eq!(again["mcpServers"]["remote"], serde_json::json!({ "url": "https://example.com/mcp", "disabled": true }));
        assert!(again["mcpServers"]["github"].get("disabled").is_none());
    }

    #[test]
    fn missing_file_is_created_and_broken_file_is_reported() {
        let dir = std::env::temp_dir().join(format!("anytask-mcp-{}", std::process::id()));
        let path = dir.join("mcp-servers.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(McpConfig::load(&path).unwrap(), McpConfig::default());
        assert!(path.exists());
        std::fs::write(&path, "{ broken").unwrap();
        assert!(McpConfig::load(&path).unwrap_err().contains("not valid"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn secrets_are_filled_in_and_a_missing_one_is_named() {
        let secrets: SecretLookup = Arc::new(|name| (name == "GITHUB").then(|| "ghp_x_secret".to_string()));
        let values = BTreeMap::from([("TOKEN".to_string(), "secret:GITHUB".to_string()), ("MODE".to_string(), "fast".to_string())]);
        let (out, secret_vals) = resolve_with_secrets(&values, &secrets).unwrap();
        assert_eq!(out["TOKEN"], "ghp_x_secret");
        assert_eq!(out["MODE"], "fast");
        assert_eq!(secret_vals, vec!["ghp_x_secret".to_string()]);
        let missing = BTreeMap::from([("X".to_string(), "secret:OTHER".to_string())]);
        assert!(resolve_with_secrets(&missing, &secrets).unwrap_err().contains("OTHER"));
    }
}