//! Weather from Open-Meteo: free, no key, and reachable from Iran. Web search snippets almost never hold today's
//! weather, so the model searched again and again (24 searches, 130,000 tokens for one question). A place name goes
//! through Open-Meteo's geocoder; the most populous match wins ("گرگان" alone also matches a village in Kermanshah).

use chrono::{DateTime, NaiveDate};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Weather {
    pub place: String,
    pub now: String,
    pub days: Vec<String>,
}

/// The biggest place among the geocoder's matches: name with province and country, latitude, longitude.
pub fn pick_place(geo: &Value) -> Option<(String, f64, f64)> {
    let results = geo.get("results")?.as_array()?;
    let population = |r: &Value| r.get("population").and_then(Value::as_u64).unwrap_or(0);
    let mut best = results.first()?;
    for r in results {
        if population(r) > population(best) {
            best = r;
        }
    }
    let mut place = best.get("name")?.as_str()?.to_string();
    for key in ["admin1", "country"] {
        if let Some(part) = best.get(key).and_then(Value::as_str) {
            place.push_str(", ");
            place.push_str(part.trim_matches(|c: char| c == '\u{200e}' || c == '\u{200f}' || c.is_whitespace()));
        }
    }
    Some((place, best.get("latitude")?.as_f64()?, best.get("longitude")?.as_f64()?))
}

/// WMO weather code in words.
pub fn describe(code: u64) -> &'static str {
    match code {
        0 => "clear",
        1 => "mostly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51 | 53 | 55 => "drizzle",
        56 | 57 => "freezing drizzle",
        61 => "light rain",
        63 => "rain",
        65 => "heavy rain",
        66 | 67 => "freezing rain",
        71 => "light snow",
        73 => "snow",
        75 => "heavy snow",
        77 => "snow grains",
        80 | 81 => "rain showers",
        82 => "heavy rain showers",
        85 | 86 => "snow showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm with hail",
        _ => "unknown",
    }
}

/// Short lines from Open-Meteo's forecast answer, labelled today and tomorrow by the place's own date at `now_ms`.
/// Open-Meteo starts the days at the UTC date: at 00:24 in Tehran (20:54 UTC) the list began with yesterday, and
/// "tomorrow" was answered from today's row (test of 11-05).
pub fn parse_forecast(place: String, data: &Value, now_ms: i64, days: usize) -> Option<Weather> {
    let c = data.get("current")?;
    let temp = c.get("temperature_2m")?.as_f64()?;
    let num = |v: &Value, key: &str| v.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let now = format!(
        "{temp:.0}°C (feels like {:.0}°C), {}, wind {:.0} km/h, humidity {:.0}%",
        c.get("apparent_temperature").and_then(Value::as_f64).unwrap_or(temp),
        describe(c.get("weather_code").and_then(Value::as_u64).unwrap_or(999)),
        num(c, "wind_speed_10m"),
        num(c, "relative_humidity_2m"),
    );
    let offset_ms = data.get("utc_offset_seconds").and_then(Value::as_i64).unwrap_or(0) * 1000;
    let today = DateTime::from_timestamp_millis(now_ms + offset_ms)?.date_naive();
    let d = data.get("daily")?;
    let lines = d
        .get("time")?
        .as_array()?
        .iter()
        .enumerate()
        .filter_map(|(i, date)| {
            let date = NaiveDate::parse_from_str(date.as_str()?, "%Y-%m-%d").ok()?;
            let label = match (date - today).num_days() {
                ..0 => return None,
                0 => "today, ",
                1 => "tomorrow, ",
                _ => "",
            };
            Some(format!(
                "{label}{} ({}): {:.0} to {:.0}°C, {}, chance of rain {}%",
                date.format("%Y-%m-%d"),
                date.format("%a"),
                d["temperature_2m_min"][i].as_f64()?,
                d["temperature_2m_max"][i].as_f64()?,
                describe(d["weather_code"][i].as_u64().unwrap_or(999)),
                d["precipitation_probability_max"][i].as_u64().unwrap_or(0),
            ))
        })
        .take(days)
        .collect();
    Some(Weather { place, now, days: lines })
}

/// The geocoder finds a name only in the language it is asked in: "تهران" with "en" finds nothing (11-08: the
/// assistant had to try again with "Tehran"), with "fa" it is Tehran.
fn geo_language(name: &str) -> &'static str {
    if name.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c)) { "fa" } else { "en" }
}

/// Today's weather and the next `days` days (1–7) for a place name in any language.
pub async fn weather(client: &reqwest::Client, place: &str, days: u8) -> Result<Weather, String> {
    // The geocoder takes a bare name: "Gorgan, Iran" finds nothing.
    let name = place.split([',', '،']).next().unwrap_or(place).trim();
    if name.is_empty() {
        return Err("Which city? Give the city's name.".into());
    }
    let unreachable = |e: reqwest::Error| format!("The weather service is not reachable now: {e}");
    let key = name.to_lowercase();
    if let Some((place, lat, lon)) = known_place(&key) {
        return forecast(client, place, lat, lon, days).await;
    }
    let geo: Value = client
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[("name", name), ("count", "10"), ("language", geo_language(name))])
        .send()
        .await
        .map_err(unreachable)?
        .json()
        .await
        .map_err(unreachable)?;
    let (place, lat, lon) = pick_place(&geo).ok_or_else(|| format!("No place called \"{name}\" was found. Try the city's English name."))?;
    PLACES.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (place.clone(), lat, lon));
    forecast(client, place, lat, lon, days).await
}

/// Places already looked up: a city does not move, and the lookup was half of every weather question (test 3:
/// 10 s for one answer through a slow proxy).
static PLACES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Place>>> = std::sync::LazyLock::new(Default::default);

/// A place's full name, latitude and longitude.
type Place = (String, f64, f64);

fn known_place(key: &str) -> Option<Place> {
    PLACES.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned()
}

async fn forecast(client: &reqwest::Client, place: String, lat: f64, lon: f64, days: u8) -> Result<Weather, String> {
    let unreachable = |e: reqwest::Error| format!("The weather service is not reachable now: {e}");
    let days = days.clamp(1, 7);
    // One day more: the list can start with yesterday, which is dropped.
    let (lat, lon, asked) = (lat.to_string(), lon.to_string(), (days + 1).to_string());
    let data: Value = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", lat.as_str()),
            ("longitude", lon.as_str()),
            ("current", "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m"),
            ("daily", "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max"),
            ("timezone", "auto"),
            ("forecast_days", asked.as_str()),
        ])
        .send()
        .await
        .map_err(unreachable)?
        .json()
        .await
        .map_err(unreachable)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    parse_forecast(place, &data, now_ms, days.into()).ok_or_else(|| "The weather service sent an answer that could not be read.".into())
}

#[cfg(test)]
mod tests {

    #[test]
    fn persian_names_are_looked_up_in_persian() {
        assert_eq!(geo_language("تهران"), "fa");
        assert_eq!(geo_language("Tehran"), "en");
        assert_eq!(geo_language("Gorgan"), "en");
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn the_biggest_place_wins_over_a_village_with_the_same_name() {
        let geo = json!({ "results": [
            { "name": "Gorgan", "latitude": 34.74, "longitude": 46.86, "admin1": "Kermanshah\u{200e}", "country": "Iran" },
            { "name": "Gorgan", "latitude": 36.84, "longitude": 54.44, "population": 350676, "admin1": "Golestan", "country": "Iran" }
        ]});
        assert_eq!(pick_place(&geo), Some(("Gorgan, Golestan, Iran".to_string(), 36.84, 54.44)));
        assert_eq!(pick_place(&json!({ "generationtime_ms": 0.2 })), None);
    }

    #[test]
    fn the_forecast_becomes_short_lines() {
        let data = json!({
            "utc_offset_seconds": 12600,
            "current": { "temperature_2m": 18.4, "apparent_temperature": 17.2, "relative_humidity_2m": 71, "weather_code": 61, "wind_speed_10m": 12.3 },
            "daily": {
                "time": ["2026-11-04", "2026-11-05", "2026-11-06", "2026-11-07"],
                "temperature_2m_min": [10.0, 11.0, 9.1, 8.0], "temperature_2m_max": [21.0, 20.0, 19.6, 18.0],
                "weather_code": [3, 3, 3, 0], "precipitation_probability_max": [10, 15, 40, 0]
            }
        });
        // 2026-11-04 20:54 UTC is 2026-11-05 00:24 in Tehran: the first row is yesterday there.
        let now_ms = DateTime::parse_from_rfc3339("2026-11-04T20:54:00Z").unwrap().timestamp_millis();
        let weather = parse_forecast("Gorgan, Golestan, Iran".into(), &data, now_ms, 2).unwrap();
        assert_eq!(weather.now, "18°C (feels like 17°C), light rain, wind 12 km/h, humidity 71%");
        assert_eq!(
            weather.days,
            vec![
                "today, 2026-11-05 (Thu): 11 to 20°C, overcast, chance of rain 15%".to_string(),
                "tomorrow, 2026-11-06 (Fri): 9 to 20°C, overcast, chance of rain 40%".to_string(),
            ]
        );
    }
}
