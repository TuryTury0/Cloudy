//! web_search. With a Tavily or Brave key the real search API is used; otherwise (or when it fails) free sources
//! are combined: Yahoo, DuckDuckGo (HTML and lite), the DuckDuckGo instant answer and Wikipedia. Ported from v1, with
//! the HTML parsed per result block so a title is never paired with the next result's snippet.
//!
//! DuckDuckGo answers a second quick search with HTTP 202 and a bot check instead of results (straight out and
//! through a VPN alike), so on its own the free search found nothing most of the time. Yahoo keeps answering.
//!
//! With the user's Gemini key, Google is searched through Gemini ("grounding with Google Search", google_search):
//! the best results for Persian questions, which the free engines reachable from Iran answer poorly (they drop the
//! Persian words). The assistant tries it before the free sources.

use crate::html::strip_tags;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::LazyLock;

const MAX_RESULTS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchProvider {
    Tavily,
    Brave,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchAnswer {
    pub query: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    pub results: Vec<SearchResult>,
}

static A_TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<a\b([^>]*)>(.*?)</a>"#).unwrap());
// Attribute values may use double or single quotes.
static HREF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)href\s*=\s*(?:"([^"]+)"|'([^']+)')"#).unwrap());
static CLASS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)class\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());

/// The value of the first matching attribute, whichever quotes it used.
fn attr(re: &Regex, attrs: &str) -> Option<String> {
    re.captures(attrs).and_then(|c| c.get(1).or_else(|| c.get(2))).map(|m| m.as_str().to_string())
}
static LITE_SNIPPET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<td[^>]*class="result-snippet"[^>]*>(.*?)</td>"#).unwrap());
static H3_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<h3\b[^>]*>(.*?)</h3>"#).unwrap());
static YAHOO_TEXT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)class="compText[^"]*"[^>]*>\s*<p\b[^>]*>(.*?)</p>"#).unwrap());

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// DuckDuckGo wraps result links in a redirect (…?uddg=<real url>&…).
fn unwrap_redirect(url: &str) -> String {
    let url = crate::html::decode_entities(url);
    match url.split_once("uddg=") {
        Some((_, rest)) => percent_decode(rest.split('&').next().unwrap_or(rest)),
        None => url,
    }
}

/// Parses DuckDuckGo's HTML page block by block.
pub fn parse_ddg_html(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    for block in html.split("result__body").skip(1) {
        let mut title = String::new();
        let mut url = String::new();
        let mut snippet = String::new();
        for caps in A_TAG_RE.captures_iter(block) {
            let attrs = &caps[1];
            let class = attr(&CLASS_RE, attrs).unwrap_or_default();
            if class.contains("result__a") && title.is_empty() {
                title = strip_tags(&caps[2]);
                url = attr(&HREF_RE, attrs).map(|h| unwrap_redirect(&h)).unwrap_or_default();
            } else if class.contains("result__snippet") && snippet.is_empty() {
                snippet = strip_tags(&caps[2]);
            }
        }
        if !title.is_empty() && snippet.chars().count() > 15 {
            results.push(SearchResult { title, snippet: snippet.chars().take(300).collect(), url });
        }
    }
    results
}

/// Parses DuckDuckGo lite: links and snippets come in matching order.
pub fn parse_ddg_lite(html: &str) -> Vec<SearchResult> {
    let links: Vec<(String, String)> = A_TAG_RE
        .captures_iter(html)
        .filter(|c| attr(&CLASS_RE, &c[1]).is_some_and(|k| k.contains("result-link")))
        .map(|c| (strip_tags(&c[2]), attr(&HREF_RE, &c[1]).map(|h| unwrap_redirect(&h)).unwrap_or_default()))
        .collect();
    let snippets: Vec<String> = LITE_SNIPPET_RE.captures_iter(html).map(|c| strip_tags(&c[1])).collect();
    links
        .into_iter()
        .zip(snippets)
        .filter(|(_, s)| s.chars().count() > 15)
        .map(|((title, url), snippet)| SearchResult { title, snippet: snippet.chars().take(300).collect(), url })
        .collect()
}

/// Yahoo wraps result links in a redirect (…/RU=<real url, percent-encoded>/RK=…).
fn unwrap_yahoo(url: &str) -> String {
    let url = crate::html::decode_entities(url);
    match url.split_once("/RU=") {
        Some((_, rest)) => percent_decode(rest.split("/RK=").next().unwrap_or(rest).split("/RS=").next().unwrap_or(rest)),
        None => url,
    }
}

/// Parses Yahoo's result page: one "dd … algo" block per web result (ads, "also try" and pivots are skipped).
pub fn parse_yahoo(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    for block in html.split(r#"class="dd "#).skip(1) {
        let class = block.split('"').next().unwrap_or_default();
        if !class.split_whitespace().any(|c| c == "algo") {
            continue;
        }
        let Some(title_block) = block.split_once("compTitle").map(|(_, rest)| rest) else { continue };
        let url = A_TAG_RE.captures(title_block).and_then(|c| attr(&HREF_RE, &c[1])).map(|h| unwrap_yahoo(&h)).unwrap_or_default();
        let title = H3_RE.captures(title_block).map(|c| strip_tags(&c[1])).unwrap_or_default();
        let snippet = YAHOO_TEXT_RE.captures(block).map(|c| strip_tags(&c[1])).unwrap_or_default();
        if !title.is_empty() && url.starts_with("http") && snippet.chars().count() > 15 {
            results.push(SearchResult { title, snippet: snippet.chars().take(300).collect(), url });
        }
    }
    results
}

async fn yahoo(client: &reqwest::Client, query: &str) -> Vec<SearchResult> {
    let Ok(resp) = client.get("https://search.yahoo.com/search").query(&[("p", query)]).send().await else { return vec![] };
    if !resp.status().is_success() {
        return vec![];
    }
    resp.text().await.map(|t| parse_yahoo(&t)).unwrap_or_default()
}

fn is_persian(text: &str) -> bool {
    text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c))
}

async fn ddg_html(client: &reqwest::Client, query: &str) -> Vec<SearchResult> {
    let Ok(resp) = client.get("https://html.duckduckgo.com/html/").query(&[("q", query)]).send().await else { return vec![] };
    if !resp.status().is_success() {
        return vec![];
    }
    resp.text().await.map(|t| parse_ddg_html(&t)).unwrap_or_default()
}

async fn ddg_lite(client: &reqwest::Client, query: &str) -> Vec<SearchResult> {
    let Ok(resp) = client.post("https://lite.duckduckgo.com/lite/").form(&[("q", query)]).send().await else { return vec![] };
    if !resp.status().is_success() {
        return vec![];
    }
    resp.text().await.map(|t| parse_ddg_lite(&t)).unwrap_or_default()
}

async fn instant_answer(client: &reqwest::Client, query: &str) -> Option<SearchResult> {
    let resp = client
        .get("https://api.duckduckgo.com/")
        .query(&[("q", query), ("format", "json"), ("no_html", "1")])
        .send()
        .await
        .ok()?;
    let data: Value = resp.json().await.ok()?;
    let text = data.get("AbstractText")?.as_str()?;
    (text.chars().count() > 20).then(|| SearchResult {
        title: data.get("Heading").and_then(Value::as_str).unwrap_or(query).to_string(),
        snippet: text.chars().take(320).collect(),
        url: data.get("AbstractURL").and_then(Value::as_str).unwrap_or("https://duckduckgo.com").to_string(),
    })
}

async fn wikipedia(client: &reqwest::Client, query: &str) -> Vec<SearchResult> {
    let lang = if is_persian(query) { "fa" } else { "en" };
    let clean: String = query.chars().filter(|c| *c != '?' && *c != '؟').collect();
    let Ok(resp) = client
        .get(format!("https://{lang}.wikipedia.org/w/api.php"))
        .query(&[("action", "query"), ("list", "search"), ("srsearch", clean.trim()), ("utf8", "1"), ("format", "json")])
        .send()
        .await
    else {
        return vec![];
    };
    let Ok(data) = resp.json::<Value>().await else { return vec![] };
    data.pointer("/query/search")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(3)
                .filter_map(|i| {
                    let title = i.get("title")?.as_str()?.to_string();
                    Some(SearchResult {
                        snippet: strip_tags(i.get("snippet").and_then(Value::as_str).unwrap_or_default()).chars().take(300).collect(),
                        url: format!("https://{lang}.wikipedia.org/wiki/{}", title.replace(' ', "_")),
                        title,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn with_api(client: &reqwest::Client, query: &str, provider: SearchProvider, key: &str) -> Result<(Option<String>, Vec<SearchResult>), String> {
    match provider {
        SearchProvider::Tavily => {
            let resp = client
                .post("https://api.tavily.com/search")
                .bearer_auth(key)
                .json(&json!({ "query": query, "max_results": MAX_RESULTS, "include_answer": true, "search_depth": "basic" }))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("Tavily HTTP {}", resp.status().as_u16()));
            }
            let data: Value = resp.json().await.map_err(|e| e.to_string())?;
            let results = data
                .get("results")
                .and_then(Value::as_array)
                .map(|r| {
                    r.iter()
                        .map(|i| SearchResult {
                            title: i.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
                            snippet: i.get("content").and_then(Value::as_str).unwrap_or_default().chars().take(400).collect(),
                            url: i.get("url").and_then(Value::as_str).unwrap_or_default().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok((data.get("answer").and_then(Value::as_str).map(str::to_string), results))
        }
        SearchProvider::Brave => {
            let resp = client
                .get("https://api.search.brave.com/res/v1/web/search")
                .query(&[("q", query), ("count", "5")])
                .header("X-Subscription-Token", key)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("Brave HTTP {}", resp.status().as_u16()));
            }
            let data: Value = resp.json().await.map_err(|e| e.to_string())?;
            let results = data
                .pointer("/web/results")
                .and_then(Value::as_array)
                .map(|r| {
                    r.iter()
                        .map(|i| SearchResult {
                            title: i.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
                            snippet: strip_tags(i.get("description").and_then(Value::as_str).unwrap_or_default()).chars().take(400).collect(),
                            url: i.get("url").and_then(Value::as_str).unwrap_or_default().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok((None, results))
        }
    }
}

/// Gemini models that can search Google for the answer with the user's own key ("grounding with Google Search"),
/// tried in order. Free keys have search quota on some models only (11-12: gemini-2.5-flash-lite and 2.5-flash
/// answered in 2 s with sources; every 3.x model said "exceeded your current quota").
#[derive(Clone, PartialEq, Eq)]
pub struct GoogleSearch {
    pub base_url: String,
    pub models: Vec<String>,
    pub key: String,
}

impl std::fmt::Debug for GoogleSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleSearch").field("base_url", &self.base_url).field("models", &self.models).field("key", &"***").finish()
    }
}

/// Models that search Google on free keys, tried before the user's own.
pub const GOOGLE_SEARCH_MODELS: &[&str] = &["gemini-2.5-flash-lite", "gemini-2.5-flash"];
/// How long Google is not asked after every model refused (no quota left, a bad key, a refused location): each
/// search would otherwise spend several requests before the free search.
const GOOGLE_PAUSE: std::time::Duration = std::time::Duration::from_secs(600);

/// What Google search remembers between searches: the model that searched last (tried first), and a pause after
/// every model refused.
#[derive(Default)]
pub struct GoogleState {
    last_good: std::sync::Mutex<Option<String>>,
    /// Until when, and for which key (a hash): a new key is asked at once.
    paused_until: std::sync::Mutex<Option<(std::time::Instant, u64)>>,
}

fn key_hash(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

static GOOGLE_STATE: LazyLock<GoogleState> = LazyLock::new(GoogleState::default);

/// A refusal no other model can change: the key, the location, billing or permissions.
fn hard_refusal(error: &str) -> bool {
    let lower = error.to_lowercase();
    ["api key", "location is not supported", "location refused", "billing", "permission", "http 401", "http 403"].iter().any(|w| lower.contains(w))
        && !lower.contains("exceeded your current quota")
}

/// How much to search: one short answer, or a thorough one from several sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Quick,
    Deep,
}

/// The question as asked of the searching model, with today's date so it can tell an old figure from today's.
fn grounded_prompt(query: &str, depth: Depth, today: &str) -> String {
    let when = format!("Today is {today}. Give the date of each figure you found and say plainly when it is not from today.");
    match depth {
        Depth::Quick => format!(
            "Search the web and answer briefly (2-4 sentences) with the exact facts, numbers, dates and units found. {when} Answer in \
             the language of the question. If the sources disagree or nothing was found, say so. Question: {query}"
        ),
        Depth::Deep => format!(
            "Search the web thoroughly (several searches and sources) and answer with a short summary, then the key facts as a list \
             with numbers, dates and units, noting where sources disagree. {when} Answer in the language of the question. Question: {query}"
        ),
    }
}

/// The answer and its sources from a Gemini response with grounding metadata.
pub fn parse_grounded(query: &str, data: &Value) -> Option<SearchAnswer> {
    let candidate = data.pointer("/candidates/0")?;
    let text: String = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true)).filter_map(|p| p.get("text").and_then(Value::as_str)).collect())
        .unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut seen = HashSet::new();
    let results: Vec<SearchResult> = candidate
        .pointer("/groundingMetadata/groundingChunks")
        .and_then(Value::as_array)
        .map(|chunks| {
            chunks
                .iter()
                .filter_map(|c| c.get("web"))
                .filter_map(|web| {
                    let url = web.get("uri").and_then(Value::as_str)?.to_string();
                    let title = web.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
                    let key = if title.is_empty() { url.clone() } else { title.to_lowercase() };
                    seen.insert(key).then_some(SearchResult { snippet: format!("Source: {title}"), title, url })
                })
                .take(MAX_RESULTS)
                .collect()
        })
        .unwrap_or_default();
    // The model may answer from what it knows without searching: said so, since a price or a date from memory is
    // likely out of date.
    let (source, answer) = if results.is_empty() {
        ("google (answered without searching; may be out of date)", format!("[Not from a search, may be out of date] {text}"))
    } else {
        ("google", text.to_string())
    };
    Some(SearchAnswer { query: query.into(), source: source.into(), answer: Some(answer.chars().take(3000).collect()), results })
}

/// Asks Google through the user's Gemini key (one request per model tried; the model searches and writes the answer
/// with sources). A model without quota, unknown to the key or busy passes to the next; a refusal no model can change
/// (key, location, billing) stops at once. When every model refused, Google is not asked for GOOGLE_PAUSE.
/// `today` ("2026-11-12") lets the model tell an old figure from today's.
pub async fn google_search(client: &reqwest::Client, google: &GoogleSearch, query: &str, depth: Depth, today: &str) -> Result<SearchAnswer, String> {
    google_search_in(client, google, query, depth, today, &GOOGLE_STATE).await
}

pub async fn google_search_in(client: &reqwest::Client, google: &GoogleSearch, query: &str, depth: Depth, today: &str, state: &GoogleState) -> Result<SearchAnswer, String> {
    let key = key_hash(&google.key);
    if let Some((until, paused_key)) = *state.paused_until.lock().unwrap_or_else(|e| e.into_inner())
        && paused_key == key
        && std::time::Instant::now() < until
    {
        return Err("Google search is paused for a few minutes (every model refused just now).".into());
    }
    let last = state.last_good.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut order: Vec<&str> = last.iter().map(String::as_str).filter(|m| google.models.iter().any(|g| g == m)).collect();
    for m in &google.models {
        if !order.contains(&m.as_str()) {
            order.push(m);
        }
    }
    let pause = || *state.paused_until.lock().unwrap_or_else(|e| e.into_inner()) = Some((std::time::Instant::now() + GOOGLE_PAUSE, key));
    let mut errors = Vec::new();
    let mut refused_all = true;
    for model in order {
        match google_search_with(client, &google.base_url, model, &google.key, query, depth, today).await {
            Ok(answer) => {
                *state.last_good.lock().unwrap_or_else(|e| e.into_inner()) = Some(model.to_string());
                return Ok(answer);
            }
            Err(e) if hard_refusal(&e) => {
                // A refused location may pass through the VPN on the next route: no pause for that one.
                if !google_route_retry(&e) {
                    pause();
                }
                return Err(e);
            }
            Err(e) if ["HTTP 429", "HTTP 404", "HTTP 400", "HTTP 503"].iter().any(|c| e.contains(c)) => {
                refused_all &= e.contains("HTTP 429") || e.contains("HTTP 404");
                errors.push(format!("{model}: {e}"));
            }
            // No network, a timeout: the same for every model.
            Err(e) => return Err(e),
        }
    }
    if refused_all {
        pause();
    }
    Err(format!("Google search: no model could search ({}).", errors.join("; ").chars().take(300).collect::<String>()))
}

async fn google_search_with(client: &reqwest::Client, base_url: &str, model: &str, key: &str, query: &str, depth: Depth, today: &str) -> Result<SearchAnswer, String> {
    let url = format!("{}/models/{model}:generateContent", base_url.trim_end_matches('/'));
    let body = json!({
        "contents": [{ "role": "user", "parts": [{ "text": grounded_prompt(query, depth, today) }] }],
        "tools": [{ "google_search": {} }],
    });
    // A deep search runs several searches and thinks: more time than the shared client's 15 s.
    let wait = std::time::Duration::from_secs(if depth == Depth::Deep { 45 } else { 20 });
    let resp = client.post(&url).header("x-goog-api-key", key).json(&body).timeout(wait).send().await.map_err(|e| {
        // A connection that never came up is "could not connect" (another route may work), even when it ended by
        // a timeout; a timeout after connecting would only repeat.
        if e.is_connect() {
            format!("Google search: could not connect: {e}")
        } else if e.is_timeout() {
            "Google search: timed out".to_string()
        } else {
            format!("Google search: {e}")
        }
    })?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| if e.is_timeout() { "Google search: timed out".to_string() } else { format!("Google search: {e}") })?;
    let data: Option<Value> = serde_json::from_str(&text).ok();
    if !status.is_success() {
        let Some(data) = data else {
            // Google's own block page (HTML, 403) for a refused address; any other page is not an API answer (a
            // proxy's 502, a front end's error page): neither a reason to try another model nor to pause.
            return Err(if status.as_u16() == 403 {
                "Google search: HTTP 403: location refused (block page)".to_string()
            } else {
                format!("Google search: HTTP {}: not an API answer", status.as_u16())
            });
        };
        let message = data.pointer("/error/message").and_then(Value::as_str).unwrap_or_default();
        return Err(format!("Google search: HTTP {}: {}", status.as_u16(), message.chars().take(200).collect::<String>()));
    }
    data.and_then(|d| parse_grounded(query, &d)).ok_or_else(|| "Google search gave no answer.".to_string())
}

/// After every route failed: a refusal no model or route changed (a refused location everywhere, a key the API
/// refuses with a JSON 403) pauses Google for this key, so each search does not spend three requests first.
pub fn pause_google_after(error: &str, key: &str) {
    if hard_refusal(error) {
        *GOOGLE_STATE.paused_until.lock().unwrap_or_else(|e| e.into_inner()) = Some((std::time::Instant::now() + GOOGLE_PAUSE, key_hash(key)));
    }
}

/// Whether a Google search failure may go better on another route (through the proxy, or with no proxy): only when
/// it could not connect, or the address was refused. A timeout or a quota answer would only repeat.
pub fn google_route_retry(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("could not connect") || lower.contains("location") || lower.contains("http 403")
}

pub async fn web_search(client: &reqwest::Client, query: &str, api: Option<(SearchProvider, String)>) -> Result<SearchAnswer, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("Empty search query.".into());
    }
    if let Some((provider, key)) = api {
        match with_api(client, query, provider, &key).await {
            Ok((answer, results)) if answer.is_some() || !results.is_empty() => {
                let source = format!("{provider:?}").to_lowercase();
                return Ok(SearchAnswer { query: query.into(), source, answer, results });
            }
            Ok(_) => {}
            Err(e) => eprintln!("Search API failed, using the free search instead: {e}"),
        }
    }
    let (instant, yahoo, html, lite, wiki) =
        tokio::join!(instant_answer(client, query), yahoo(client, query), ddg_html(client, query), ddg_lite(client, query), wikipedia(client, query));
    let mut seen = HashSet::new();
    let results: Vec<SearchResult> = instant
        .into_iter()
        .chain(yahoo)
        .chain(html)
        .chain(lite)
        .chain(wiki)
        .filter(|r| !r.snippet.is_empty() && seen.insert(if r.url.is_empty() { r.title.to_lowercase() } else { r.url.to_lowercase() }))
        .take(MAX_RESULTS)
        .collect();
    if results.is_empty() {
        return Err(format!("No search results found for \"{query}\". Do not guess."));
    }
    Ok(SearchAnswer { query: query.into(), source: "free".into(), answer: None, results })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DDG: &str = r#"
      <div class="result results_links"><div class="links_main links_deep result__body">
        <h2 class="result__title"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.tgju.org%2Fprofile%2Fprice_dollar_rl&amp;rut=abc">قیمت <b>دلار</b></a></h2>
        <a class="result__snippet" href="x">قیمت دلار در بازار امروز 2,695,000 ریال اعلام شده است.</a>
      </div></div>
      <div class="result"><div class="links_main result__body">
        <h2 class="result__title"><a href="https://no-snippet.example" class="result__a">No snippet here</a></h2>
      </div></div>
      <div class="result"><div class="result__body">
        <h2><a class="result__a" href="https://second.example">Second</a></h2>
        <a class="result__snippet" href="y">The second result has its own snippet text.</a>
      </div></div>"#;

    #[test]
    fn ddg_results_are_parsed_per_block() {
        let results = parse_ddg_html(DDG);
        assert_eq!(results.len(), 2, "the block without a snippet must not steal the next snippet");
        assert_eq!(results[0].url, "https://www.tgju.org/profile/price_dollar_rl");
        assert_eq!(results[0].title, "قیمت دلار");
        assert!(results[0].snippet.contains("2,695,000"));
        assert_eq!(results[1].title, "Second");
    }

    #[test]
    fn ddg_lite_pairs_links_and_snippets() {
        let html = r#"<a rel="nofollow" href="https://a.example" class='x result-link'>A</a>
            <td class="result-snippet">Snippet for the first result, long enough.</td>
            <a href="https://b.example" class="result-link">B</a><td class="result-snippet">Snippet for the second result here.</td>"#;
        let results = parse_ddg_lite(html);
        assert_eq!(results.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(), ["https://a.example", "https://b.example"]);
    }

    /// Shortened from a real Yahoo page: a pivot block, two results (the first link double-encoded), "also try".
    const YAHOO: &str = r#"
      <div class="dd assist horizontal-pivots"><a href="https://r.search.yahoo.com/x/RU=https%3a%2f%2fpivot.example%2f/RK=2">Pivot</a></div>
      <ol class="reg searchCenterMiddle"><li class="first"><div class="dd fst algo algo-sr relsrch Sr" data-yga='{"yT2Pos":1}'><div class="compTitle options-toggle"><a class="d-ib va-top" data-yga="{&quot;yT4&quot;:&quot;title&quot;}" target="_blank" href="https://r.search.yahoo.com/_ylt=A2RR;_ylu=Y29s/RV=2/RE=1795012874/RO=10/RU=https%3a%2f%2fwww.tgju.org%2f%25D9%2582%25DB%258C%25D9%2585%25D8%25AA-%25D8%25AF%25D9%2584%25D8%25A7%25D8%25B1/RK=2/RS=0MR5c5-"><div class="d-ib p-abs"><span class="fc-141414 d-b">شبکه اطلاع رسانی طلا و ارز</span>https://www.tgju.org › قیمت-دلار</span></div><h3 style="display:block" class="title fc-2015C2-imp"><span class="d-b fz-20">قیمت دلار | شبکه اطلاع&zwnj; رسانی طلا و ارز</span></h3></a></div><div class="compText aAbs"><p class="fc-dustygray fz-14"> در صفحه <b>قیمت</b> <b>دلار</b>، می توانید قیمت امروز دلار آزاد را رصد کنید </p></div></div></li>
      <li><div class="dd algo algo-sr relsrch Sr"><div class="compTitle options-toggle"><a href="https://second.example/page">x</a><h3 class="title"><span>Second &amp; last</span></h3></div><div class="compText aAbs"><p>The second result has its own snippet text.</p></div></div></li></ol>
      <div class="dd AlsoTry"><div class="compTitle"><a href="https://r.search.yahoo.com/y/RU=https%3a%2f%2falso.example/RK=0"><h3>Also try</h3></a></div></div>"#;

    #[test]
    fn yahoo_results_are_parsed_and_unwrapped() {
        let results = parse_yahoo(YAHOO);
        assert_eq!(results.len(), 2, "pivots and 'also try' are not results: {results:?}");
        assert_eq!(results[0].url, "https://www.tgju.org/%D9%82%DB%8C%D9%85%D8%AA-%D8%AF%D9%84%D8%A7%D8%B1");
        assert_eq!(results[0].title, "قیمت دلار | شبکه اطلاع\u{200c} رسانی طلا و ارز");
        assert!(results[0].snippet.starts_with("در صفحه قیمت دلار"), "{}", results[0].snippet);
        assert_eq!((results[1].title.as_str(), results[1].url.as_str()), ("Second & last", "https://second.example/page"));
    }

    #[test]
    fn a_grounded_answer_keeps_its_text_and_sources_and_drops_thoughts() {
        let data = json!({ "candidates": [{
            "content": { "parts": [{ "text": "thinking...", "thought": true }, { "text": "دمای گرگان امروز ۱۸ درجه است." }] },
            "groundingMetadata": { "groundingChunks": [
                { "web": { "uri": "https://vertexaisearch.cloud.google.com/a", "title": "irimo.ir" } },
                { "web": { "uri": "https://vertexaisearch.cloud.google.com/b", "title": "irimo.ir" } },
                { "web": { "uri": "https://vertexaisearch.cloud.google.com/c", "title": "accuweather.com" } }
            ] }
        }] });
        let answer = parse_grounded("هوای گرگان", &data).unwrap();
        assert_eq!(answer.answer.as_deref(), Some("دمای گرگان امروز ۱۸ درجه است."));
        assert_eq!(answer.source, "google");
        let titles: Vec<&str> = answer.results.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["irimo.ir", "accuweather.com"]);
        assert!(parse_grounded("x", &json!({ "candidates": [{ "content": { "parts": [] } }] })).is_none());
        assert!(grounded_prompt("q", Depth::Deep, "2026-11-12").contains("several searches"));
        assert!(grounded_prompt("q", Depth::Quick, "2026-11-12").contains("Today is 2026-11-12"));
        // Answered from memory, without searching: marked.
        let memory = parse_grounded("x", &json!({ "candidates": [{ "content": { "parts": [{ "text": "42" }] } }] })).unwrap();
        assert!(memory.source.contains("without searching") && memory.answer.unwrap().starts_with("[Not from a search"));
        assert!(!format!("{:?}", GoogleSearch { base_url: "b".into(), models: vec![], key: "SECRET".into() }).contains("SECRET"));
        assert!(google_route_retry("Google search: could not connect: dns") && google_route_retry("Google search: HTTP 403: location refused (block page)"));
        assert!(!google_route_retry("Google search: timed out") && !google_route_retry("Google search: HTTP 429: quota"));
    }

    fn grounded_body(text: &str) -> Value {
        json!({ "candidates": [{ "content": { "parts": [{ "text": text }] }, "groundingMetadata": { "groundingChunks": [{ "web": { "uri": "https://x/1", "title": "tgju.org" } }] } }] })
    }

    #[tokio::test]
    async fn a_model_without_quota_passes_to_the_next_and_the_good_one_is_remembered() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/models/a:generateContent"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "error": { "message": "You exceeded your current quota" } })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path("/models/b:generateContent")).respond_with(ResponseTemplate::new(200).set_body_json(grounded_body("۲۶۴,۹۰۰ تومان"))).mount(&server).await;
        let state = GoogleState::default();
        let google = GoogleSearch { base_url: server.uri(), models: vec!["a".into(), "b".into()], key: "K".into() };
        let client = reqwest::Client::new();
        let answer = google_search_in(&client, &google, "دلار", Depth::Quick, "2026-11-12", &state).await.unwrap();
        assert_eq!(answer.answer.as_deref(), Some("۲۶۴,۹۰۰ تومان"));
        // The second search starts with the model that worked: "a" is not asked again (expect(1) above).
        google_search_in(&client, &google, "دلار", Depth::Quick, "2026-11-12", &state).await.unwrap();
    }

    #[tokio::test]
    async fn a_refused_location_stops_at_once_without_a_pause_and_a_bad_key_pauses() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        // Refused straight out, answered on the next try (as through the VPN).
        Mock::given(method("POST")).and(path("/models/a:generateContent"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": { "message": "User location is not supported for the API use." } })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path("/models/a:generateContent")).respond_with(ResponseTemplate::new(200).set_body_json(grounded_body("ok"))).mount(&server).await;
        Mock::given(method("POST")).and(path("/models/b:generateContent")).respond_with(ResponseTemplate::new(200).set_body_json(grounded_body("x"))).expect(0).mount(&server).await;
        let state = GoogleState::default();
        let google = GoogleSearch { base_url: server.uri(), models: vec!["a".into(), "b".into()], key: "K".into() };
        let client = reqwest::Client::new();
        let error = google_search_in(&client, &google, "q", Depth::Quick, "2026-11-12", &state).await.unwrap_err();
        assert!(error.contains("location") && google_route_retry(&error), "{error}");
        let again = google_search_in(&client, &google, "q", Depth::Quick, "2026-11-12", &state).await.unwrap();
        assert_eq!(again.answer.as_deref(), Some("ok"), "not paused: the next route got through");

        let bad_key = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": { "message": "API key not valid. Please pass a valid API key." } }))).expect(2).mount(&bad_key).await;
        let state = GoogleState::default();
        let google = GoogleSearch { base_url: bad_key.uri(), models: vec!["a".into(), "b".into()], key: "K".into() };
        assert!(google_search_in(&client, &google, "q", Depth::Quick, "2026-11-12", &state).await.unwrap_err().contains("API key"));
        let paused = google_search_in(&client, &google, "q", Depth::Quick, "2026-11-12", &state).await.unwrap_err();
        assert!(paused.contains("paused"), "{paused}");
        // A new key is asked at once, not after the pause.
        let new_key = GoogleSearch { key: "K2".into(), ..google.clone() };
        let error = google_search_in(&client, &new_key, "q", Depth::Quick, "2026-11-12", &state).await.unwrap_err();
        assert!(!error.contains("paused"), "{error}");
    }

    #[tokio::test]
    async fn an_error_page_that_is_not_googles_block_page_is_not_called_a_refusal() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad gateway</html>")).mount(&server).await;
        let google = GoogleSearch { base_url: server.uri(), models: vec!["a".into()], key: "K".into() };
        let state = GoogleState::default();
        let error = google_search_in(&reqwest::Client::new(), &google, "q", Depth::Quick, "2026-11-12", &state).await.unwrap_err();
        assert!(error.contains("not an API answer") && !hard_refusal(&error), "{error}");
        assert!(state.paused_until.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn googles_html_block_page_keeps_its_status() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(403).set_body_string("<!DOCTYPE html><title>Error 403 (Forbidden)!!1</title>")).mount(&server).await;
        let google = GoogleSearch { base_url: server.uri(), models: vec!["a".into()], key: "K".into() };
        let error = google_search_in(&reqwest::Client::new(), &google, "q", Depth::Quick, "2026-11-12", &GoogleState::default()).await.unwrap_err();
        assert!(error.contains("HTTP 403") && error.contains("location refused"), "{error}");
        assert!(google_route_retry(&error), "another route may get through");
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b%2Fc+d"), "a b/c d");
        assert_eq!(percent_decode("%D8%AF"), "د");
        assert_eq!(percent_decode("bad%zz"), "bad%zz");
    }

    /// Network: `cargo test -p anytask-web -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn real_free_search() {
        let answer = web_search(&crate::page::http_client(), "قیمت دلار امروز", None).await.unwrap();
        assert!(!answer.results.is_empty());
    }

    /// Searches in quick succession, as the model does (DuckDuckGo blocks the second one).
    #[tokio::test]
    #[ignore]
    async fn real_searches_in_a_row() {
        let client = crate::page::http_client();
        for query in ["قیمت گوشی سامسونگ A55 دیجی کالا", "samsung a55 digikala price", "آب و هوای گرگان امروز", "gold price today"] {
            let answer = web_search(&client, query, None).await;
            assert!(answer.as_ref().is_ok_and(|a| !a.results.is_empty()), "{query}: {answer:?}");
        }
        assert!(!yahoo(&client, "tehran").await.is_empty(), "Yahoo itself answers");
    }
}
