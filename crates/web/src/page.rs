//! read_web_page: fetch a page in the background (no browser window) and return it as text.

use crate::html;
use serde::Serialize;
use std::time::Duration;

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";
const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BYTES: usize = 2 * 1024 * 1024;
const OUTPUT_CHARS: usize = 15_000;
const FILTERED_CHARS: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub url: String,
    pub title: String,
    pub text: String,
    pub only_matching_lines: bool,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub fn http_client() -> reqwest::Client {
    http_client_builder().build().unwrap_or_default()
}

pub(crate) fn http_client_builder() -> reqwest::ClientBuilder {
    // A connection that hangs (filtering) fails as "could not connect" after 8 s, so another route can be tried.
    reqwest::Client::builder().timeout(TIMEOUT).connect_timeout(std::time::Duration::from_secs(8)).user_agent(USER_AGENT)
}

/// Decodes bytes with the charset from the header or the page's meta tag (utf-8 by default).
pub fn decode_body(bytes: &[u8], content_type: &str) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_lowercase();
    let label = charset(content_type).or_else(|| meta_charset(&head)).unwrap_or_else(|| "utf-8".into());
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

fn charset(content_type: &str) -> Option<String> {
    let lower = content_type.to_lowercase();
    let start = lower.find("charset=")? + 8;
    Some(lower[start..].trim_matches(|c| c == '"' || c == '\'').split(';').next()?.trim().to_string())
}

fn meta_charset(head: &str) -> Option<String> {
    let start = head.find("charset=")? + 8;
    let rest = head[start..].trim_start_matches(['"', '\'']);
    Some(rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect())
}

/// Turns a fetched body into the result (shared by the network path and tests).
pub fn page_from_body(url: &str, content_type: &str, body: &str, query: &str, body_truncated: bool) -> Page {
    let mut title = String::new();
    let lines: Vec<String> = if content_type.contains("json") {
        match serde_json::from_str::<serde_json::Value>(body) {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default().lines().map(str::to_string).collect(),
            Err(_) => body.lines().map(str::to_string).collect(),
        }
    } else if content_type.contains("html") || content_type.contains("xml") || body[..body.len().min(2000)].to_lowercase().contains("<html") {
        title = html::title(body);
        html::html_to_lines(body)
    } else {
        body.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
    };

    let filtered = if query.trim().is_empty() { None } else { html::filter_lines(&lines, query) };
    let note = (!query.trim().is_empty() && filtered.is_none()).then(|| format!("No line mentions \"{query}\"; showing the start of the page."));
    let limit = if filtered.is_some() { FILTERED_CHARS } else { OUTPUT_CHARS };
    let only_matching_lines = filtered.is_some();
    let mut text = filtered.unwrap_or(lines).join("\n");
    let cut = text.chars().count() > limit;
    if cut {
        text = text.chars().take(limit).collect::<String>() + "\n...[page continues]";
    }
    Page { url: url.to_string(), title, text, only_matching_lines, truncated: cut || body_truncated, note }
}

/// The page came from a cache that knows it is out of date (nginx's X-Cache-Status, or Cloudflare's).
fn stale(headers: &reqwest::header::HeaderMap) -> bool {
    ["x-cache-status", "cf-cache-status"].iter().any(|name| {
        headers.get(*name).and_then(|v| v.to_str().ok()).is_some_and(|v| matches!(v.trim().to_ascii_uppercase().as_str(), "STALE" | "EXPIRED" | "UPDATING"))
    })
}

pub async fn read_web_page(client: &reqwest::Client, raw_url: &str, query: &str) -> Result<Page, String> {
    let url = reqwest::Url::parse(raw_url.trim()).map_err(|_| "Invalid URL.".to_string())?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("Only http and https pages can be read.".into());
    }
    let host = url.host_str().unwrap_or_default().to_string();
    let fetch = |url: reqwest::Url| {
        client
            .get(url)
            .header("Accept", "text/html,application/xhtml+xml,application/json,text/plain;q=0.9,*/*;q=0.5")
            .header("Accept-Language", "fa,en-US;q=0.9,en;q=0.8")
            // Prices and news must be current: ask caches on the way for a fresh copy.
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .send()
    };
    let timeout = |e: reqwest::Error| if e.is_timeout() { format!("No answer from {host} within 15s") } else { e.to_string() };
    let mut response = fetch(url.clone()).await.map_err(timeout)?;
    // A site's own cache can hand out an old copy that it marks as stale (11-11: tgju.org gave a dollar price three
    // days old through the VPN, "X-Cache-Status: STALE"). Asked once more under an address the cache has not seen.
    if stale(response.headers()) {
        let mut fresh = url.clone();
        fresh.query_pairs_mut().append_pair("_", &std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).to_string());
        if let Ok(again) = fetch(fresh).await
            && again.status().is_success()
        {
            response = again;
        }
    }
    if !response.status().is_success() {
        return Err(format!("HTTP {} from {host}", response.status().as_u16()));
    }
    let content_type = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    if !content_type.is_empty() && !["html", "text", "json", "xml"].iter().any(|k| content_type.contains(k)) {
        return Err(format!("Not a readable page ({content_type})."));
    }
    let final_url = response.url().to_string();
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() >= MAX_BYTES {
            bytes.truncate(MAX_BYTES);
            truncated = true;
            break;
        }
    }
    let body = decode_body(&bytes, &content_type);
    Ok(page_from_body(&final_url, &content_type, &body, query, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_the_cache_marks_stale_is_asked_again() {
        let headers = |name: &'static str, value: &'static str| {
            let mut map = reqwest::header::HeaderMap::new();
            map.insert(name, reqwest::header::HeaderValue::from_static(value));
            map
        };
        assert!(stale(&headers("x-cache-status", "STALE")), "tgju.org through the VPN, 11-11");
        assert!(stale(&headers("cf-cache-status", "expired")));
        assert!(!stale(&headers("x-cache-status", "HIT")));
        assert!(!stale(&reqwest::header::HeaderMap::new()));
    }

    #[test]
    fn decodes_windows_1256_from_meta() {
        // "سلام" in windows-1256
        let mut bytes = br#"<html><head><meta charset="windows-1256"></head><body>"#.to_vec();
        bytes.extend_from_slice(&[0xD3, 0xE1, 0xC7, 0xE3]);
        assert!(decode_body(&bytes, "text/html").contains("سلام"));
        assert_eq!(charset("text/html; charset=UTF-8"), Some("utf-8".into()));
    }

    #[test]
    fn json_and_query_note() {
        let page = page_from_body("https://x", "application/json", r#"{"price":1}"#, "", false);
        assert!(page.text.contains("\"price\": 1"));
        let html = "<html><body><p>hello</p></body></html>";
        let page = page_from_body("https://x", "text/html", html, "missing", false);
        assert!(page.note.is_some() && !page.only_matching_lines && page.text == "hello");
    }

    #[test]
    fn long_text_is_cut() {
        let body = "line\n".repeat(10_000);
        let page = page_from_body("https://x", "text/plain", &body, "", false);
        assert!(page.truncated && page.text.ends_with("[page continues]"));
    }

    /// Network: run with `cargo test -p anytask-web -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn reads_a_real_page() {
        let page = read_web_page(&http_client(), "https://example.com/", "").await.unwrap();
        assert_eq!(page.title, "Example Domain");
    }
}
