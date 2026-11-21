//! Decision models (Liquid AI's d1 family, e.g. d1-ultra). They do not write text: they answer a yes/no question
//! about a situation (a few sentences, with or without a screenshot) with a probability. The assistant uses one as
//! a second opinion, never as its brain. Any model served with Liquid's decisions API works (its address and
//! model id come from Settings).
//!
//! The service refuses some countries with "403 Forbidden" straight out and answers through a VPN, so a request
//! tries both ways and remembers the one that worked.

use crate::Net;
use base64::Engine;
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// Liquid's decisions endpoint and its largest model: the defaults offered in Settings.
pub const DECISION_URL: &str = "https://api.liquid.ai/decisions/v1/systemone";
pub const DECISION_MODEL: &str = "d1-ultra";

/// Where a decision model is served and with which key.
pub struct DecisionTarget<'a> {
    pub url: &'a str,
    pub model: &'a str,
    pub key: &'a str,
}

pub struct Decider {
    net: Net,
    /// The last answer came through the proxy: go that way first.
    proxy_first: AtomicBool,
}

impl Decider {
    pub fn new(net: Net) -> Self {
        Self { net, proxy_first: AtomicBool::new(false) }
    }

    /// The probability (0 to 1) that the answer is yes.
    pub async fn yes_no(&self, target: &DecisionTarget<'_>, state: &str, question: &str, jpeg: Option<&[u8]>) -> Result<f64, String> {
        let body = request(target.model, state, question, jpeg);
        let mut ways = vec![(self.net.direct().clone(), false)];
        if let Some(proxied) = self.net.proxied() {
            ways.push((proxied, true));
        }
        if self.proxy_first.load(Ordering::Relaxed) {
            ways.reverse();
        }
        let mut problems = Vec::new();
        for (client, through_proxy) in ways {
            match send(&client, target.url, target.key, &body).await {
                Ok(probability) => {
                    self.proxy_first.store(through_proxy, Ordering::Relaxed);
                    return Ok(probability);
                }
                // A wrong key is wrong on every way out.
                Err(problem) if problem.starts_with("HTTP 401") => return Err(problem),
                Err(problem) => problems.push(format!("{}: {problem}", if through_proxy { "through the proxy (VPN)" } else { "straight out" })),
            }
        }
        Err(problems.join("; "))
    }
}

/// The request for one yes/no question ("noul" is the service's name for it).
fn request(model: &str, state: &str, question: &str, jpeg: Option<&[u8]>) -> Value {
    let mut body = json!({
        "model": model,
        "state": state,
        "questions": { "q": { "type": "noul", "instructions": question } }
    });
    if let Some(jpeg) = jpeg {
        body["images"] = json!([format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpeg))]);
    }
    body
}

/// The probability in the service's answer.
fn probability(answer: &Value) -> Result<f64, String> {
    answer["answers"]["q"]["noul"].as_f64().filter(|p| (0.0..=1.0).contains(p)).ok_or_else(|| "the answer has no probability".to_string())
}

async fn send(client: &Client, url: &str, key: &str, body: &Value) -> Result<f64, String> {
    let response = client.post(url).bearer_auth(key).json(body).send().await.map_err(|e| format!("Network error: {e}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| format!("Network error: {e}"))?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        // Errors come as {"error": {"message": ...}}; a refused country gets an HTML page.
        let message = value["error"]["message"].as_str().unwrap_or(if status.as_u16() == 403 { "refused from this address" } else { "" });
        return Err(format!("HTTP {}: {message}", status.as_u16()).trim_end_matches([':', ' ']).to_string());
    }
    probability(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_carries_the_question_and_the_screenshot() {
        let plain = request("d1-ultra", "The user said: stop", "Is this a stop request?", None);
        assert_eq!(plain["model"], "d1-ultra");
        assert_eq!(plain["questions"]["q"]["type"], "noul");
        assert_eq!(plain["questions"]["q"]["instructions"], "Is this a stop request?");
        assert!(plain.get("images").is_none());
        let with_image = request("d1-ultra", "A screenshot", "Is a dialog open?", Some(&[0xFF, 0xD8]));
        assert_eq!(with_image["images"][0], "data:image/jpeg;base64,/9g=");
    }

    #[test]
    fn the_probability_is_read_from_the_answer() {
        let answer = json!({ "model": "d1-ultra", "answers": { "q": { "type": "noul", "noul": 0.987 } }, "usage": { "input_tokens": 76 } });
        assert_eq!(probability(&answer), Ok(0.987));
        assert!(probability(&json!({ "answers": {} })).is_err());
        assert!(probability(&json!({ "answers": { "q": { "noul": 7 } } })).is_err(), "not a probability");
    }
}
