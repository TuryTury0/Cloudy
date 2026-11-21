//! The trained wake word: normalization stats, the user's takes and the threshold. Saved as JSON in the same
//! format as v1 (version 3), so a v1 wake word can be brought over.

use crate::dsp::NCEP;
use crate::matcher::{DIM, Feat, Static, build_template, normalize, score_sequence};
use base64::Engine;
use serde_json::{Value, json};

const MODEL_VERSION: u64 = 3;
const QUANT: f32 = 20.0;
pub const MIN_TEMPLATE: usize = 20;
pub const MAX_TEMPLATE: usize = crate::enroll::MAX_UTTERANCE + 40;
const LENGTH_LOW: f64 = 0.7;
const LENGTH_HIGH: f64 = 1.4;

#[derive(Debug, Clone, PartialEq)]
pub struct WakeModel {
    pub label: String,
    pub created_at: i64,
    pub mean: Static,
    pub std: Static,
    pub templates: Vec<Vec<Feat>>,
    pub threshold: f64,
    /// "good", "fair", "weak" or "unverified".
    pub quality: String,
    /// The self-test after training fired.
    pub tested: bool,
    /// How many times it was made stricter after false triggers (at most 4).
    pub tightened: u32,
}

impl WakeModel {
    /// One step stricter after a false trigger; None when it is already as strict as allowed.
    pub fn tighten(&mut self) -> bool {
        if self.tightened >= 4 {
            return false;
        }
        self.threshold *= 0.96;
        self.tightened += 1;
        true
    }

    pub fn to_json(&self) -> Value {
        json!({
            "v": MODEL_VERSION,
            "label": self.label,
            "createdAt": self.created_at,
            "mean": self.mean.to_vec(),
            "std": self.std.to_vec(),
            "threshold": self.threshold,
            "quality": self.quality,
            "tested": self.tested,
            "tightened": self.tightened,
            "templates": self.templates.iter().map(|t| pack(t)).collect::<Vec<_>>(),
        })
    }

    /// None for anything damaged or from another version.
    pub fn from_json(data: &Value) -> Option<Self> {
        if data["v"].as_u64() != Some(MODEL_VERSION) {
            return None;
        }
        let threshold = data["threshold"].as_f64().filter(|t| t.is_finite() && *t > 0.0)?;
        let floats = |key: &str| -> Option<Static> {
            let list = data[key].as_array().filter(|a| a.len() == NCEP)?;
            let values: Vec<f32> = list.iter().map(|v| v.as_f64().filter(|f| f.is_finite()).map(|f| f as f32)).collect::<Option<_>>()?;
            values.try_into().ok()
        };
        let mean = floats("mean")?;
        let std = floats("std").filter(|s| s.iter().all(|v| *v > 0.0))?;
        let templates: Vec<Vec<Feat>> = data["templates"].as_array().filter(|a| a.len() >= 2)?.iter().map(|t| t.as_str().and_then(unpack)).collect::<Option<_>>()?;
        if templates.iter().any(|t| t.len() < MIN_TEMPLATE || t.len() > MAX_TEMPLATE) {
            return None;
        }
        Some(Self {
            label: data["label"].as_str().unwrap_or_default().to_string(),
            created_at: data["createdAt"].as_i64().unwrap_or(0),
            mean,
            std,
            templates,
            threshold,
            quality: data["quality"].as_str().unwrap_or("unverified").to_string(),
            tested: data["tested"].as_bool().unwrap_or(false),
            tightened: data["tightened"].as_u64().unwrap_or(0) as u32,
        })
    }
}

fn pack(template: &[Feat]) -> String {
    let bytes: Vec<u8> = template.iter().flat_map(|f| f.iter().map(|v| (v * QUANT).round().clamp(-127.0, 127.0) as i8 as u8)).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unpack(b64: &str) -> Option<Vec<Feat>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    Some(bytes.chunks_exact(DIM).map(|c| std::array::from_fn(|k| f32::from(c[k] as i8) / QUANT)).collect())
}

pub fn compute_stats(frames: &[Static]) -> (Static, Static) {
    let n = frames.len().max(1) as f64;
    let mut mean = [0.0f64; NCEP];
    for f in frames {
        for k in 0..NCEP {
            mean[k] += f64::from(f[k]);
        }
    }
    mean.iter_mut().for_each(|m| *m /= n);
    let mut var = [0.0f64; NCEP];
    for f in frames {
        for k in 0..NCEP {
            var[k] += (f64::from(f[k]) - mean[k]).powi(2);
        }
    }
    (std::array::from_fn(|k| mean[k] as f32), std::array::from_fn(|k| (var[k] / n).sqrt().max(1e-3) as f32))
}

/// Takes with very different lengths cannot match (outside the DTW window), so such takes are refused early.
pub fn length_fits(takes: &[Vec<Static>], length: usize) -> bool {
    let mut sorted: Vec<usize> = takes.iter().map(Vec::len).collect();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2] as f64;
    length as f64 >= median * LENGTH_LOW && length as f64 <= median * LENGTH_HIGH
}

/// The new take is much further from the others than they are from each other.
pub fn is_inconsistent(existing: &[Vec<Static>], candidate: &[Static]) -> bool {
    let all: Vec<Static> = existing.iter().flatten().chain(candidate).copied().collect();
    let (mean, std) = compute_stats(&all);
    let norm = |s: &[Static]| -> Vec<Static> { s.iter().map(|f| normalize(f, &mean, &std)).collect() };
    let seqs: Vec<Vec<Static>> = existing.iter().map(|s| norm(s)).collect();
    let templates: Vec<Vec<Feat>> = seqs.iter().map(|s| build_template(s)).collect();
    let mut reference: Vec<f64> = seqs
        .iter()
        .enumerate()
        .map(|(i, s)| score_sequence(&templates.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, t)| t.clone()).collect::<Vec<_>>(), s))
        .filter(|v| v.is_finite())
        .collect();
    reference.sort_by(f64::total_cmp);
    let d = score_sequence(&templates, &norm(candidate));
    if !d.is_finite() {
        return true;
    }
    let Some(median) = reference.get(reference.len() / 2) else { return false };
    d > (median * 1.9).max(median + 0.3)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    pub threshold: f64,
    pub quality: String,
    /// The best impostor score (free speech), if any.
    pub impostor_min: f64,
}

/// The threshold sits between the worst genuine score and the best impostor score from the free-speech phase.
pub fn pick_threshold(genuine: &[f64], impostors: &[f64]) -> Calibration {
    let g_max = genuine.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let g_mean = genuine.iter().sum::<f64>() / genuine.len().max(1) as f64;
    if impostors.is_empty() {
        return Calibration { threshold: g_max * 1.12 + 0.02, quality: "unverified".into(), impostor_min: f64::INFINITY };
    }
    let n_min = impostors.iter().copied().fold(f64::INFINITY, f64::min);
    let ratio = n_min / g_max;
    let threshold = if ratio > 1.15 { (g_max * 1.2 + 0.02).min(g_max + (n_min - g_max) * 0.55) } else { (g_mean * 1.05).max(n_min * 0.9) };
    let quality = if ratio >= 1.5 {
        "good"
    } else if ratio >= 1.2 {
        "fair"
    } else {
        "weak"
    };
    Calibration { threshold, quality: quality.into(), impostor_min: n_min }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> WakeModel {
        let take: Vec<Feat> = (0..30).map(|i| std::array::from_fn(|k| ((i * k) % 7) as f32 / 4.0 - 0.8)).collect();
        WakeModel {
            label: "hey nova".into(),
            created_at: 5,
            mean: [0.5; NCEP],
            std: [2.0; NCEP],
            templates: vec![take.clone(), take],
            threshold: 0.6,
            quality: "good".into(),
            tested: true,
            tightened: 0,
        }
    }

    #[test]
    fn saved_models_load_back_and_damaged_ones_are_refused() {
        let m = model();
        let back = WakeModel::from_json(&m.to_json()).unwrap();
        assert_eq!(back.templates.len(), 2);
        let close = back.templates[0][3].iter().zip(&m.templates[0][3]).all(|(a, b)| (a - b).abs() < 1e-5);
        assert!(close, "values on the 1/20 grid survive packing");
        assert_eq!((back.label.as_str(), back.threshold, back.tested), ("hey nova", 0.6, true));
        let mut broken = m.to_json();
        broken["v"] = json!(2);
        assert!(WakeModel::from_json(&broken).is_none());
        let mut broken = m.to_json();
        broken["std"][0] = json!(0.0);
        assert!(WakeModel::from_json(&broken).is_none());
        let mut broken = m.to_json();
        broken["templates"] = json!([broken["templates"][0]]);
        assert!(WakeModel::from_json(&broken).is_none());
    }

    #[test]
    fn tightening_is_capped() {
        let mut m = model();
        for _ in 0..4 {
            assert!(m.tighten());
        }
        assert!(!m.tighten());
        assert!((m.threshold - 0.6 * 0.96f64.powi(4)).abs() < 1e-12);
    }

    #[test]
    fn thresholds_sit_between_genuine_and_impostor_scores() {
        let c = pick_threshold(&[0.3, 0.35, 0.4], &[0.9, 1.2]);
        assert_eq!(c.quality, "good");
        assert!(c.threshold > 0.4 && c.threshold < 0.9, "{}", c.threshold);
        let close = pick_threshold(&[0.3, 0.4], &[0.42]);
        assert_eq!(close.quality, "weak");
        let none = pick_threshold(&[0.4], &[]);
        assert_eq!(none.quality, "unverified");
    }
}
