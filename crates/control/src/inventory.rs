//! What this PC has: the default browser and the installed apps (Start menu), grouped so the assistant knows
//! which app to use for music, video, chat, documents... Scanned once at first start (and on request).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    /// Milliseconds since 1970 when it was scanned.
    pub scanned_at: i64,
    pub default_browser: Option<String>,
    /// Start menu app names.
    pub apps: Vec<String>,
}

/// Known apps by kind: (kind, names as they appear in the Start menu, matched case-insensitively as prefixes).
const KINDS: &[(&str, &[&str])] = &[
    ("music", &["Spotify", "Apple Music", "iTunes", "Deezer", "TIDAL", "Amazon Music", "YouTube Music", "AIMP", "foobar2000", "MusicBee", "Winamp", "SoundCloud"]),
    ("video", &["VLC", "PotPlayer", "Media Player", "Windows Media Player", "MPC-HC", "KMPlayer", "Netflix", "Plex", "Kodi", "Stremio"]),
    ("messaging", &["Telegram", "WhatsApp", "Discord", "Signal", "Microsoft Teams", "Slack", "Skype", "Viber", "Zoom", "Bale", "Eitaa", "Rubika"]),
    ("browsers", &["Google Chrome", "Microsoft Edge", "Firefox", "Mozilla Firefox", "Vivaldi", "Brave", "Opera", "Opera GX", "Arc", "Tor Browser", "Yandex"]),
    ("documents", &["Word", "Excel", "PowerPoint", "OneNote", "LibreOffice", "WPS Office", "Notepad++", "Visual Studio Code", "Obsidian", "Notion", "Adobe Acrobat"]),
    ("mail", &["Outlook", "Mail", "Thunderbird"]),
    ("games", &["Steam", "Epic Games Launcher", "Battle.net", "EA app", "Ubisoft Connect", "Xbox", "Riot Client"]),
    ("design", &["Photoshop", "Illustrator", "Figma", "GIMP", "Canva", "Paint", "Blender"]),
];

/// The kind of a known app, if any.
pub fn kind_of(app: &str) -> Option<&'static str> {
    let lower = app.to_lowercase();
    KINDS
        .iter()
        .find(|(_, names)| names.iter().any(|n| lower == n.to_lowercase() || lower.starts_with(&format!("{} ", n.to_lowercase()))))
        .map(|(kind, _)| *kind)
}

/// Browser names from the https handler's ProgId ("VivaldiHTM", "ChromeHTML", "MSEdgeHTM", "FirefoxURL-...").
pub fn browser_name(prog_id: &str) -> String {
    known_browser(prog_id).unwrap_or_else(|| prog_id.to_string())
}

/// The browser's name when the ProgId is a known one (None for "AppXq0fe…", "ChromiumHTM.…" and the like, which
/// are no app name anything can be launched by).
pub fn known_browser(prog_id: &str) -> Option<String> {
    let lower = prog_id.to_lowercase();
    let known = [("vivaldi", "Vivaldi"), ("chrome", "Google Chrome"), ("msedge", "Microsoft Edge"), ("firefox", "Firefox"), ("brave", "Brave"), ("opera", "Opera"), ("arc", "Arc")];
    known.iter().find(|(key, _)| lower.contains(key)).map(|(_, name)| name.to_string())
}

impl Inventory {
    /// A few lines for the system prompt: the default browser and the known apps by kind.
    pub fn profile(&self) -> String {
        let mut lines = Vec::new();
        if let Some(browser) = &self.default_browser {
            lines.push(format!("- Default browser: {browser}."));
        }
        for (kind, _) in KINDS {
            let mut found: Vec<&str> = self.apps.iter().filter(|a| kind_of(a) == Some(kind)).map(String::as_str).collect();
            found.sort_unstable();
            found.dedup();
            if !found.is_empty() {
                lines.push(format!("- {}: {}.", label(kind), found.join(", ")));
            }
        }
        if lines.is_empty() {
            return String::new();
        }
        lines.push(format!("- {} apps in the Start menu in all; open_app opens any of them by name.", self.apps.len()));
        lines.join("\n")
    }
}

fn label(kind: &str) -> &'static str {
    match kind {
        "music" => "Music apps",
        "video" => "Video players",
        "messaging" => "Chat apps",
        "browsers" => "Browsers",
        "documents" => "Documents and notes",
        "mail" => "Mail",
        "games" => "Games",
        _ => "Design",
    }
}

/// Reads the default browser and the Start menu apps (one hidden PowerShell call, about a second).
#[cfg(windows)]
pub fn scan(now_ms: i64) -> Result<Inventory, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = "$ErrorActionPreference='SilentlyContinue'; \
        [Console]::OutputEncoding=[Text.Encoding]::UTF8; \
        $p=(Get-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\https\\UserChoice').ProgId; \
        $a=@(Get-StartApps | Select-Object -ExpandProperty Name); \
        @{ browser=$p; apps=$a } | ConvertTo-Json -Compress";
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Could not scan the apps: {e}"))?;
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| "The app list could not be read.".to_string())?;
    let apps = value["apps"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let default_browser = value["browser"].as_str().filter(|p| !p.is_empty()).map(browser_name);
    Ok(Inventory { scanned_at: now_ms, default_browser, apps })
}

#[cfg(not(windows))]
pub fn scan(now_ms: i64) -> Result<Inventory, String> {
    Ok(Inventory { scanned_at: now_ms, ..Inventory::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_are_grouped_by_kind_for_the_prompt() {
        let inv = Inventory {
            scanned_at: 1,
            default_browser: Some(browser_name("VivaldiHTM")),
            apps: ["Spotify", "VLC media player", "Telegram Desktop", "Vivaldi", "Google Chrome", "Calculator", "Word", "Spotify"].map(String::from).to_vec(),
        };
        let profile = inv.profile();
        assert!(profile.contains("- Default browser: Vivaldi."), "{profile}");
        assert!(profile.contains("- Music apps: Spotify."), "{profile}");
        assert!(profile.contains("- Video players: VLC media player."), "{profile}");
        assert!(profile.contains("- Chat apps: Telegram Desktop."), "{profile}");
        assert!(profile.contains("- Browsers: Google Chrome, Vivaldi."), "{profile}");
        assert!(profile.contains("8 apps in the Start menu"), "{profile}");
        assert_eq!(kind_of("Calculator"), None);
        assert_eq!(kind_of("Wordpad"), None, "prefix match needs a word boundary");
        assert_eq!(browser_name("FirefoxURL-308046B0AF4A39CB"), "Firefox");
        assert_eq!(known_browser("AppXq0fevzme2pys62n3e0fbqa7peapykr8v"), None);
        assert_eq!(Inventory::default().profile(), "");
    }

    #[test]
    #[ignore = "reads this PC's apps"]
    fn scans_this_pc() {
        let inv = scan(0).unwrap();
        println!("{:?}\n{}", inv.default_browser, inv.profile());
        assert!(!inv.apps.is_empty());
    }
}
