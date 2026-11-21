//! Background web tools: no browser window opens.

use super::system::{SystemOps, off_runtime};
use super::{Tool, ToolCtx, arg_str, short};
use crate::lang::tr;
use anytask_llm::ToolSpec;
use anytask_web::{Net, SearchProvider};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

/// Returns the search API to use, if the user saved a key.
pub type SearchApi = Arc<dyn Fn() -> Option<(SearchProvider, String)> + Send + Sync>;
/// The user's Gemini model that can search Google (read on every search, so a new key works at once).
pub type GoogleSource = Arc<dyn Fn() -> Option<anytask_web::GoogleSearch> + Send + Sync>;

pub struct WebSearch {
    pub net: Net,
    pub api: SearchApi,
    /// Google through the user's Gemini key: the best results for Persian questions (the free engines reachable from
    /// Iran drop the Persian words: 11-11, 19 of 53 searches found nothing, nearly all Persian).
    pub google: Option<GoogleSource>,
}

#[async_trait]
impl Tool for WebSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search".into(),
            description: "Searches the web in real time and returns short results with source URLs. The query must be self-contained \
                          (full names, no pronouns). Use it for anything current or uncertain; never guess. Then use read_web_page on \
                          the best result only when an exact value is missing (a result with an \"answer\" from Google already has the \
                          facts and their sources: use it). Search at most twice for one question: if the results do not answer it, \
                          read the best page or say you could not find it. depth \"quick\" (default) gives one short answer; \"deep\" searches \
                          several sources for a thorough answer (research, comparisons, \"search a lot\")."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Fully resolved search query, in the user's language." },
                    "depth": { "type": "string", "enum": ["quick", "deep"], "description": "quick (default) or deep." }
                },
                "required": ["query"]
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        // A deep search says so: it takes a few seconds more, and the log shows which one ran.
        let what = if arg_str(args, "depth") == Some("deep") { tr("Researching:", "جستجوی کامل:") } else { tr("Searching:", "جستجو:") };
        format!("{what} {}", short(arg_str(args, "query").unwrap_or(""), 50))
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let query = arg_str(&args, "query").ok_or("query is required.")?;
        let depth = if arg_str(&args, "depth") == Some("deep") { anytask_web::Depth::Deep } else { anytask_web::Depth::Quick };
        let api = (self.api)();
        // A paid search key the user set comes first; then Google through their Gemini key; the free engines last.
        if api.is_none()
            && let Some(google) = self.google.as_ref().and_then(|g| g())
        {
            let today = chrono::DateTime::from_timestamp_millis(if ctx.now_ms > 0 { ctx.now_ms } else { chrono::Utc::now().timestamp_millis() })
                .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let found = self
                .net
                .try_both_if(
                    |client| {
                        let (google, today) = (google.clone(), today.clone());
                        async move { anytask_web::google_search(&client, &google, query, depth, &today).await }
                    },
                    anytask_web::google_route_retry,
                    anytask_web::google_route_retry,
                )
                .await;
            match found {
                Ok(answer) => return serde_json::to_value(answer).map_err(|e| e.to_string()),
                Err(e) => {
                    anytask_web::pause_google_after(&e, &google.key);
                    (ctx.log)(format!("Google search failed, using the free search: {}", e.chars().take(200).collect::<String>()));
                }
            }
        }
        let answer = self.net.try_both(|client| { let api = api.clone(); async move { anytask_web::web_search(&client, query, api).await } }).await?;
        serde_json::to_value(answer).map_err(|e| e.to_string())
    }
}

/// Real weather data (Open-Meteo, no key), so a weather question is one call instead of many searches.
pub struct WeatherNow {
    pub net: Net,
    /// Where the user's city is kept: the first city asked about is saved, and a question without a city uses it
    /// (11-10: "hava chetore?" asked for the city, and the voice model never saved the answer, so it would ask again).
    pub memory: Option<Arc<anytask_storage::Memory>>,
}

/// The fact the user's city is saved under.
const CITY_FACT: &str = "city";

#[async_trait]
impl Tool for WeatherNow {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "weather".into(),
            description: "Current weather and the forecast for the next days in a city, from a real weather service. Use it for every \
                          weather question instead of web_search. Leave city out for the user's own city (it is remembered); if it is \
                          not known yet the result says so: then ask the user once."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string", "description": "The city's name only, e.g. Gorgan or گرگان. Leave out for the user's own city." },
                    "days": { "type": "integer", "description": "Days of forecast, 1 to 7 (default 3)." }
                }
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("Weather:", "هوا:"), short(arg_str(args, "city").unwrap_or(""), 40))
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let saved = self.memory.as_ref().and_then(|m| m.fact(CITY_FACT).ok().flatten()).filter(|c| !c.trim().is_empty());
        let asked = arg_str(&args, "city").map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
        let Some(city) = asked.clone().or(saved.clone()) else {
            return Err("The user's city is not known yet: ask them once which city they live in, then call weather with it.".into());
        };
        let days = args.get("days").and_then(Value::as_u64).unwrap_or(3).clamp(1, 7) as u8;
        let place = city.clone();
        let weather = self.net.try_both(|client| {
            let place = place.clone();
            async move { anytask_web::weather(&client, &place, days).await }
        });
        let weather = weather.await?;
        // The first city the user asks about is taken as theirs; it can be changed with remember_fact.
        if saved.is_none()
            && let (Some(memory), Some(city)) = (&self.memory, asked)
        {
            let _ = memory.set_fact(ctx.now_ms, CITY_FACT, &city);
        }
        serde_json::to_value(weather).map_err(|e| e.to_string())
    }
}

/// Prices of goods in Iranian shops (Torob, which lists the shops' prices, Digikala included), several products at
/// once and side by side. Each product came from a whole page read before (a few thousand tokens each, one after the
/// other); here each is a few lines, all asked at the same time.
pub struct ShopPrices {
    pub net: Net,
}

const MAX_PRODUCTS: usize = 15;

#[async_trait]
impl Tool for ShopPrices {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shop_prices".into(),
            description: "Current prices of goods in Iranian shops (phones, laptops, appliances, anything sold online) from Torob, for one or                           many products at once (all asked together, a few seconds). Each product gets its best matches: name, cheapest                           price in toman, how many shops, condition (used) and a link. Not for currency, gold or coin prices (tgju)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "products": { "type": "array", "items": { "type": "string" }, "description": "Product names as people search them, e.g. ['iPhone 13 128', 'گوشی سامسونگ A55']." },
                    "matches": { "type": "integer", "description": "Matches per product, 1-5 (default 3)." }
                },
                "required": ["products"]
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        let n = args.get("products").and_then(Value::as_array).map_or(0, Vec::len);
        if n == 1 {
            let one = args["products"][0].as_str().unwrap_or_default();
            format!("{} {}", tr("Prices of", "قیمت"), short(one, 40))
        } else {
            format!("{} {}", tr("Prices of products:", "قیمت کالاها:"), crate::lang::digits(n as u64))
        }
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let products: Vec<String> = args
            .get("products")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect())
            .unwrap_or_default();
        if products.is_empty() {
            return Err("products is required: a list of product names.".into());
        }
        let matches = args.get("matches").and_then(Value::as_u64).unwrap_or(3).clamp(1, 5) as usize;
        let started = std::time::Instant::now();
        let mut jobs = tokio::task::JoinSet::new();
        for (i, product) in products.iter().take(MAX_PRODUCTS).cloned().enumerate() {
            let net = self.net.clone();
            jobs.spawn(async move {
                let found = net
                    .try_for("https://api.torob.com/", |client| {
                        let product = product.clone();
                        async move { anytask_web::shop_prices(&client, &product, matches).await }
                    })
                    .await;
                (i, product, found)
            });
        }
        let mut rows: Vec<(usize, Value)> = Vec::new();
        let mut failed = 0;
        while let Some(done) = jobs.join_next().await {
            let Ok((i, product, found)) = done else { continue };
            rows.push((
                i,
                match found {
                    Ok(list) if list.is_empty() => json!({ "product": product, "found": [], "note": "Torob found nothing; try other words." }),
                    Ok(list) => json!({ "product": product, "found": list }),
                    Err(error) => {
                        failed += 1;
                        json!({ "product": product, "error": error })
                    }
                },
            ));
        }
        rows.sort_by_key(|(i, _)| *i);
        (ctx.log)(format!("shop_prices: {} products in {:.1} s, {failed} failed", rows.len(), started.elapsed().as_secs_f32()));
        if failed == rows.len() {
            let first = rows.first().and_then(|(_, r)| r["error"].as_str()).unwrap_or("no answer").to_string();
            return Err(format!("Torob could not be reached ({first}). Try web_search instead."));
        }
        let mut out = json!({
            "source": "torob.com",
            "currency": "toman",
            "note": "price is the cheapest shop's price now; null means no shop has it. Give prices with the source.",
            "products": rows.into_iter().map(|(_, r)| r).collect::<Vec<_>>(),
        });
        if products.len() > MAX_PRODUCTS {
            out["skipped"] = json!(format!("Only the first {MAX_PRODUCTS} products were looked up."));
        }
        Ok(out)
    }
}

pub struct ReadWebPage {
    pub net: Net,
}

#[async_trait]
impl Tool for ReadWebPage {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_web_page".into(),
            description: "Reads a web page in the background (no browser window) and returns its text; tables come back as 'cell | cell' rows. \
                          Pass query (in the page's language, e.g. 'دلار طلا ۱۸') to get only the matching rows, which is shorter and more precise."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Full http(s) address." },
                    "query": { "type": "string", "description": "Optional words to keep only the relevant rows." }
                },
                "required": ["url"]
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        let url = arg_str(args, "url").unwrap_or("");
        let host = url.split("//").nth(1).and_then(|r| r.split('/').next()).unwrap_or(url);
        format!("{} {host}", tr("Reading", "خواندن"))
    }

    async fn run(&self, args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        let url = arg_str(&args, "url").ok_or("url is required.")?;
        let query = arg_str(&args, "query").unwrap_or("");
        let page = self.net.try_for(url, |client| async move { anytask_web::read_web_page(&client, url, query).await }).await?;
        serde_json::to_value(page).map_err(|e| e.to_string())
    }
}

/// "This tab", "the page I'm on": the address comes straight from the user's browser (no screen job) and the page is
/// read in the background. The user's test of 11-06 asked "in tabi ke alan hastam ro bekhon": a screen job started in
/// AnyTask's own window, never reached the browser, and the answer came from an old link on the clipboard.
pub struct ReadBrowserTab {
    pub ops: Arc<dyn SystemOps>,
    pub net: Net,
}

#[async_trait]
impl Tool for ReadBrowserTab {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_browser_tab".into(),
            description: "Reads the page the user has open in their web browser right now (the front-most browser window; works while the \
                          assistant's own window is in front) and returns the browser, the page title, its address and its text. Use it for \
                          'this tab', 'this page', 'the page I am on', 'in safe/tab ro bekhon'. Never use desktop_task or the clipboard to \
                          find the page. The page is fetched again without the user's login, so a page behind a login may come back as \
                          a login page: then say so. Pass query to keep only the matching rows."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "Optional words to keep only the relevant rows." } }
            }),
        }
    }

    fn label(&self, _: &Value) -> String {
        tr("Reading the browser tab", "خواندن تب مرورگر").into()
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let query = arg_str(&args, "query").unwrap_or("");
        let tab = off_runtime(self.ops.clone(), |ops| ops.browser_tab())
            .await?
            .ok_or("No browser window is open, so there is no tab to read. Ask the user which page they mean, or for its link.")?;
        // Only the site goes to the log: a full address can carry tokens.
        let site = tab.url.as_deref().map(|u| u.split("://").nth(1).unwrap_or(u).split(['/', '?', '#']).next().unwrap_or_default()).unwrap_or("(address not read)");
        (ctx.log)(format!("browser tab: {} | {} | {site}", tab.browser, short(&tab.title, 80)));
        let Some(url) = tab.url.clone().filter(|u| u.starts_with("http://") || u.starts_with("https://")) else {
            return Ok(json!({
                "browser": tab.browser, "title": tab.title, "url": tab.url, "text": "",
                "note": "The address could not be read, or it is not a web page (a new tab or a browser page). Answer from the title, or web_search the title."
            }));
        };
        let read = self.net.try_for(&url, |client| {
            let url = url.clone();
            async move { anytask_web::read_web_page(&client, &url, query).await }
        });
        match read.await {
            Ok(page) => {
                let mut value = serde_json::to_value(page).map_err(|e| e.to_string())?;
                value["browser"] = json!(tab.browser);
                value["tabTitle"] = json!(tab.title);
                Ok(value)
            }
            Err(error) => Ok(json!({
                "browser": tab.browser, "title": tab.title, "url": url, "text": "",
                "note": format!("The page itself could not be read in the background ({error}). Answer from the title and address and say that the text could not be read.")
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::DenyAll;
    use crate::tools::system::BrowserTab;
    use crate::tools::test_support::ctx;

    struct Browser(Option<BrowserTab>);

    impl SystemOps for Browser {
        fn clipboard_read(&self) -> Result<String, String> {
            Ok("https://meet.google.com/old-link".into())
        }
        fn clipboard_write(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn media_key(&self, _: crate::tools::system::MediaKey) -> Result<(), String> {
            Ok(())
        }
        fn open(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn browser_tab(&self) -> Result<Option<BrowserTab>, String> {
            Ok(self.0.clone())
        }
    }

    fn tool(tab: Option<BrowserTab>) -> ReadBrowserTab {
        ReadBrowserTab { ops: Arc::new(Browser(tab)), net: Net::new() }
    }

    #[tokio::test]
    async fn weather_without_a_city_uses_the_saved_one_or_asks() {
        let memory = Arc::new(anytask_storage::Memory::open_in_memory().unwrap());
        let tool = WeatherNow { net: Net::new(), memory: Some(memory.clone()) };
        let error = tool.run(json!({}), &ctx(Arc::new(DenyAll))).await.unwrap_err();
        assert!(error.contains("not known yet"), "{error}");
        memory.set_fact(0, CITY_FACT, "Mashhad").unwrap();
        assert_eq!(tool.label(&json!({})), "Weather: ");
        assert_eq!(memory.fact(CITY_FACT).unwrap().as_deref(), Some("Mashhad"));
    }

    #[tokio::test]
    async fn no_browser_is_said_plainly() {
        let error = tool(None).run(json!({}), &ctx(Arc::new(DenyAll))).await.unwrap_err();
        assert!(error.contains("No browser window is open"), "{error}");
    }

    #[tokio::test]
    async fn a_page_that_cannot_be_fetched_still_gives_title_and_address() {
        let tab = BrowserTab { browser: "Vivaldi".into(), title: "Local page".into(), url: Some("http://127.0.0.1:1/".into()) };
        let value = tool(Some(tab)).run(json!({}), &ctx(Arc::new(DenyAll))).await.unwrap();
        assert_eq!((value["title"].as_str(), value["url"].as_str()), (Some("Local page"), Some("http://127.0.0.1:1/")));
        assert!(value["note"].as_str().unwrap().contains("could not be read in the background"), "{value}");
    }

    #[tokio::test]
    async fn a_browser_page_that_is_not_on_the_web_gives_its_title() {
        let tab = BrowserTab { browser: "Vivaldi".into(), title: "Settings".into(), url: Some("vivaldi://settings".into()) };
        let value = tool(Some(tab)).run(json!({}), &ctx(Arc::new(DenyAll))).await.unwrap();
        assert_eq!(value["title"], "Settings");
        assert_eq!(value["text"], "");
        assert!(value["note"].as_str().unwrap().contains("not a web page"));
    }
}
