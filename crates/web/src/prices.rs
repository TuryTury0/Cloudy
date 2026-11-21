//! Prices of goods in Iranian shops from Torob's search API: one small JSON answer per product with the cheapest price
//! and how many shops sell it. Reading Torob's search page as text cost a whole page per product (thousands of
//! tokens) and its prices sit in scripts; the API gives the same numbers in a few hundred bytes.

use serde::{Deserialize, Serialize};

const API: &str = "https://api.torob.com/v4/base-product/search/";

/// One product Torob found.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShopPrice {
    pub name: String,
    /// Toman; None when no shop has it now.
    pub price: Option<u64>,
    /// The price is the cheapest of several shops ("from ...").
    pub cheapest_of_many: bool,
    /// "in 13 shops" / the one shop, as Torob says it (Persian).
    pub shops: String,
    /// Torob's note on the item, such as "کارکرده" (used); empty for new goods.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub condition: String,
    pub link: String,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    results: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    name1: String,
    #[serde(default)]
    name2: String,
    #[serde(default)]
    price: u64,
    #[serde(default)]
    price_prefix: String,
    #[serde(default)]
    shop_text: String,
    #[serde(default)]
    stock_status: String,
    #[serde(default)]
    is_adv: bool,
    #[serde(default)]
    random_key: String,
}

/// The products of a Torob answer, best match first, at most `limit`. Paid listings are left out: they are often
/// another product (a search for the A55 had the A36 as its first, paid, result). New goods come before used ones
/// ("iphone 13 128" had a used phone first) and ones no shop has come last, each kind in Torob's order.
pub fn parse(json: &str, limit: usize) -> Result<Vec<ShopPrice>, String> {
    let answer: Answer = serde_json::from_str(json).map_err(|e| format!("Torob gave an answer that could not be read: {e}"))?;
    let mut found: Vec<ShopPrice> = answer
        .results
        .into_iter()
        .filter(|item| !item.is_adv)
        .map(|item| ShopPrice {
            name: if item.name1.trim().is_empty() { item.name2.trim().to_string() } else { item.name1.split_whitespace().collect::<Vec<_>>().join(" ") },
            price: (item.price > 0).then_some(item.price),
            cheapest_of_many: item.price_prefix == "cheapest" && !item.shop_text.is_empty() && item.shop_text.chars().any(|c| c.is_numeric()),
            shops: item.shop_text.trim().to_string(),
            condition: item.stock_status.trim().to_string(),
            link: if item.random_key.is_empty() { String::new() } else { format!("https://torob.com/p/{}/", item.random_key) },
        })
        .filter(|p| !p.name.is_empty())
        .collect();
    found.sort_by_key(|p| (p.price.is_none(), !p.condition.is_empty()));
    found.truncate(limit);
    Ok(found)
}

/// Searches Torob for one product.
pub async fn shop_prices(client: &reqwest::Client, query: &str, limit: usize) -> Result<Vec<ShopPrice>, String> {
    let response = client
        .get(API)
        .query(&[("q", query.trim()), ("page", "0"), ("size", "10")])
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| if e.is_timeout() { "No answer from Torob within 12 s".to_string() } else { e.to_string() })?;
    if !response.status().is_success() {
        return Err(format!("Torob answered {}", response.status()));
    }
    let text = response.text().await.map_err(|e| e.to_string())?;
    parse(&text, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_listings_are_dropped_and_prices_kept() {
        let json = r#"{"results":[
            {"name1":"گوشی سامسونگ A36","name2":"Samsung A36","price":104500000,"price_prefix":"cheapest","shop_text":"در ۱۴۴ فروشگاه","is_adv":true,"random_key":"a"},
            {"name1":"گوشی سامسونگ  A55 5G","name2":"Samsung A55","price":128490000,"price_prefix":"cheapest","shop_text":"در ۱۳ فروشگاه","stock_status":"","is_adv":false,"random_key":"b"},
            {"name1":"","name2":"Samsung A55 (Stock)","price":94989000,"price_prefix":"cheapest","shop_text":"در ایسام","stock_status":"کارکرده","is_adv":false,"random_key":"c"},
            {"name1":"گوشی ناموجود","price":0,"is_adv":false}
        ],"count":4}"#;
        let found = parse(json, 5).unwrap();
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].name, "گوشی سامسونگ A55 5G");
        assert_eq!(found[0].price, Some(128_490_000));
        assert!(found[0].cheapest_of_many);
        assert_eq!(found[0].link, "https://torob.com/p/b/");
        assert_eq!((found[1].name.as_str(), found[1].condition.as_str(), found[1].cheapest_of_many), ("Samsung A55 (Stock)", "کارکرده", false));
        assert_eq!(found[2].price, None, "out of stock: last");
        assert_eq!(parse(json, 1).unwrap().len(), 1);
        assert!(parse("<html>", 3).is_err());
    }

    /// Asks the real Torob: cargo test -p anytask-web --lib prices -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live_torob() {
        let client = crate::http_client();
        for q in ["iphone 13 128", "گوشی سامسونگ A55", "لپ تاپ لنوو ideapad"] {
            let t = std::time::Instant::now();
            let found = shop_prices(&client, q, 3).await.unwrap();
            println!("{q}: {} ms {}", t.elapsed().as_millis(), serde_json::to_string(&found).unwrap());
            assert!(!found.is_empty());
        }
    }
}
