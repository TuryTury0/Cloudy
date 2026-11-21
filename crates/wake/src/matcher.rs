//! Matching: features with deltas, streaming DTW against each trained take, and the detector that runs a match for
//! every speech onset and fires once the best score has bottomed out under the threshold.

use crate::dsp::{Frame, HOP, NCEP};
use crate::model::WakeModel;
use crate::vad::Vad;

pub const DIM: usize = NCEP * 2;
pub type Feat = [f32; DIM];
pub type Static = [f32; NCEP];

const DELTA_GAIN: f32 = 2.2;
pub const PREROLL: usize = 5;
const SLOPE: f64 = 1.8;
const BAND: usize = 5;
const STAY_PENALTY: f64 = 0.04;
const SKIP_PENALTY: f64 = 0.1;
const MIN_RATIO: f64 = 0.65;
const MAX_RATIO: f64 = 1.6;
const PEAK_SETTLE_FRAMES: usize = 6;
const PEAK_SETTLE_RISE: f64 = 0.06;
const COOLDOWN: usize = 250;
const MAX_GROUPS: usize = 6;
const MAX_FEATS: usize = 600;
const KEEP_FEATS: usize = 100;
const VOICED_NCCF: f32 = 0.4;
const MIN_VOICED_FRAMES: usize = 6;
const MIN_VOICED_RATIO: f64 = 0.25;
const LOUD_MARGIN_DB: f32 = 6.0;

pub fn normalize(mfcc: &Static, mean: &Static, std: &Static) -> Static {
    std::array::from_fn(|k| (mfcc[k] - mean[k]) / std[k])
}

/// The frame at `i` with its deltas, looking only inside `lo..=hi`.
pub fn feat_at(get: impl Fn(usize) -> Static, i: usize, lo: usize, hi: usize) -> Feat {
    let c = get(i);
    let p1 = get(hi.min(i + 1));
    let m1 = get(lo.max(i.saturating_sub(1)));
    let p2 = get(hi.min(i + 2));
    let m2 = get(lo.max(i.saturating_sub(2)));
    let mut out = [0.0; DIM];
    for k in 0..NCEP {
        out[k] = c[k];
        out[NCEP + k] = ((p1[k] - m1[k] + 2.0 * (p2[k] - m2[k])) / 10.0) * DELTA_GAIN;
    }
    out
}

fn distance(a: &Feat, b: &Feat) -> f64 {
    let s: f32 = a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum();
    (f64::from(s) / DIM as f64).sqrt()
}

/// Streaming DTW against one take, anchored at its start with a free end. One input frame moves one column, so
/// the cost divided by the frame count is a length-normalized score.
struct Rows {
    prev: Vec<f64>,
    cur: Vec<f64>,
    n: usize,
}

impl Rows {
    fn new(m: usize) -> Self {
        Self { prev: vec![f64::INFINITY; m], cur: vec![f64::INFINITY; m], n: 0 }
    }

    // j also reads prev[j - 1] and prev[j - 2], so an index loop is the clear form here.
    #[allow(clippy::needless_range_loop)]
    fn step(&mut self, t: &[Feat], f: &Feat) -> f64 {
        let m = t.len();
        let i = self.n;
        let lo = ((i as f64 / SLOPE).floor() as usize).saturating_sub(BAND);
        let hi = (m - 1).min((i as f64 * SLOPE).ceil() as usize + BAND);
        self.cur.fill(f64::INFINITY);
        for j in lo..=hi {
            let best = if i == 0 {
                match j {
                    0 => 0.0,
                    1 => SKIP_PENALTY,
                    _ => f64::INFINITY,
                }
            } else {
                let mut best = self.prev[j] + STAY_PENALTY;
                if j >= 1 {
                    best = best.min(self.prev[j - 1]);
                }
                if j >= 2 {
                    best = best.min(self.prev[j - 2] + SKIP_PENALTY);
                }
                best
            };
            if best.is_finite() {
                self.cur[j] = best + distance(f, &t[j]);
            }
        }
        std::mem::swap(&mut self.prev, &mut self.cur);
        self.n = i + 1;
        let (min_n, max_n) = ((MIN_RATIO * m as f64).ceil() as usize, (MAX_RATIO * m as f64).floor() as usize);
        if self.n >= min_n && self.n <= max_n { self.prev[m - 1] / self.n as f64 } else { f64::INFINITY }
    }
}

/// One onset matched against every take. The mean of the two best takes counts, so one lucky take cannot fire alone.
struct Group {
    rows: Vec<Rows>,
    start: usize,
    next: usize,
    n: usize,
    best: f64,
    best_n: usize,
    done: bool,
}

impl Group {
    fn new(templates: &[Vec<Feat>], start: usize) -> Self {
        Self { rows: templates.iter().map(|t| Rows::new(t.len())).collect(), start, next: start, n: 0, best: f64::INFINITY, best_n: 0, done: false }
    }

    fn push(&mut self, templates: &[Vec<Feat>], feat: &Feat) -> f64 {
        self.n += 1;
        let (mut a, mut b) = (f64::INFINITY, f64::INFINITY);
        for (rows, t) in self.rows.iter_mut().zip(templates) {
            let s = rows.step(t, feat);
            if s < a {
                b = a;
                a = s;
            } else if s < b {
                b = s;
            }
        }
        let combined = if templates.len() > 1 { if b.is_finite() { (a + b) / 2.0 } else { f64::INFINITY } } else { a };
        if combined < self.best {
            self.best = combined;
            self.best_n = self.n;
        }
        combined
    }
}

pub fn build_template(statics: &[Static]) -> Vec<Feat> {
    let last = statics.len().saturating_sub(1);
    (0..statics.len()).map(|i| feat_at(|idx| statics[idx], i, 0, last)).collect()
}

/// The best score of a whole sequence against the takes (enrollment checks and calibration).
pub fn score_sequence(templates: &[Vec<Feat>], statics: &[Static]) -> f64 {
    let mut group = Group::new(templates, 0);
    let last = statics.len().saturating_sub(1);
    for i in 0..statics.len() {
        group.push(templates, &feat_at(|idx| statics[idx], i, 0, last));
    }
    group.best
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trigger {
    pub score: f64,
    pub threshold: f64,
    /// Where the phrase started, in samples since the detector started.
    pub start_sample: usize,
}

pub struct Detector {
    model: WakeModel,
    collect: bool,
    pub vad: Vad,
    feats: Vec<Static>,
    voicing: Vec<i8>,
    base: usize,
    index: usize,
    groups: Vec<Group>,
    cool: usize,
    bests: Vec<f64>,
    max_n: usize,
}

impl Detector {
    /// `collect` keeps every onset's best score (calibration); `vad_floor` skips the VAD warm-up.
    pub fn new(model: WakeModel, collect: bool, vad_floor: Option<f32>) -> Self {
        let longest = model.templates.iter().map(Vec::len).max().unwrap_or(0);
        Self {
            vad: vad_floor.map(Vad::warm).unwrap_or_default(),
            max_n: (MAX_RATIO * longest as f64).floor() as usize + 4,
            model,
            collect,
            feats: Vec::new(),
            voicing: Vec::new(),
            base: 0,
            index: 0,
            groups: Vec::new(),
            cool: 0,
            bests: Vec::new(),
        }
    }

    pub fn model(&self) -> &WakeModel {
        &self.model
    }

    pub fn on_frame(&mut self, rec: &Frame) -> Option<Trigger> {
        let idx = self.index;
        self.index += 1;
        self.feats.push(normalize(&rec.mfcc, &self.model.mean, &self.model.std));
        let signal = self.vad.step(rec.db);
        // -1 quiet, 0 loud but not voiced (breath, noise), 1 loud and voiced (speech).
        let loud = rec.db > self.vad.floor + LOUD_MARGIN_DB;
        self.voicing.push(if !loud { -1 } else if rec.pitch >= VOICED_NCCF { 1 } else { 0 });
        if signal.start && idx >= self.cool {
            // Calibration keeps every onset so no impostor score is lost.
            if !self.collect && self.groups.len() >= MAX_GROUPS {
                self.groups.remove(0);
            }
            self.groups.push(Group::new(&self.model.templates, self.base.max(idx.saturating_sub(2 + PREROLL))));
        }
        let trigger = self.advance();
        if self.feats.len() > MAX_FEATS {
            let drop = self.feats.len() - KEEP_FEATS;
            self.feats.drain(..drop);
            self.voicing.drain(..drop);
            self.base += drop;
        }
        trigger
    }

    fn advance(&mut self) -> Option<Trigger> {
        let latest = self.base + self.feats.len() - 1;
        for gi in 0..self.groups.len() {
            loop {
                let g = &self.groups[gi];
                if g.done || g.next + 2 > latest {
                    break;
                }
                if g.start.max(g.next.saturating_sub(2)) < self.base {
                    self.groups[gi].done = true;
                    break;
                }
                let (base, feats) = (self.base, &self.feats);
                let feat = feat_at(|i| feats[i - base], g.next, g.start, latest);
                let g = &mut self.groups[gi];
                let combined = g.push(&self.model.templates, &feat);
                g.next += 1;
                if let Some(trigger) = self.check(gi, combined) {
                    return Some(trigger);
                }
            }
        }
        self.groups.retain(|g| !g.done);
        None
    }

    /// Fires once the score has bottomed out, so the trigger lands just after the phrase ends.
    fn check(&mut self, gi: usize, combined: f64) -> Option<Trigger> {
        let threshold = self.model.threshold;
        let g = &self.groups[gi];
        let settled = g.n - g.best_n >= PEAK_SETTLE_FRAMES || combined > g.best + PEAK_SETTLE_RISE;
        if g.best < threshold && settled {
            if !self.has_voice(gi) {
                self.groups[gi].done = true;
                return None;
            }
            let trigger = Trigger { score: g.best, threshold, start_sample: g.start * HOP };
            self.cool = self.index + COOLDOWN;
            self.groups.clear();
            return Some(trigger);
        }
        if g.n >= self.max_n {
            let best = g.best;
            self.groups[gi].done = true;
            if self.collect {
                self.bests.push(best);
            }
        }
        None
    }

    /// The matched window must hold enough voiced speech; a breath can look like the phrase but is not harmonic.
    fn has_voice(&self, gi: usize) -> bool {
        let g = &self.groups[gi];
        let (mut loud, mut voiced) = (0usize, 0usize);
        for i in g.start.max(self.base)..g.next {
            match self.voicing.get(i - self.base) {
                Some(1) => {
                    loud += 1;
                    voiced += 1;
                }
                Some(0) => loud += 1,
                _ => {}
            }
        }
        voiced >= MIN_VOICED_FRAMES && voiced as f64 >= MIN_VOICED_RATIO * loud as f64
    }

    /// Every onset's best score so far (calibration).
    pub fn drain(&mut self) -> Vec<f64> {
        let open: Vec<f64> = self.groups.drain(..).map(|g| g.best).collect();
        self.bests.extend(open);
        std::mem::take(&mut self.bests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(values: &[f32]) -> Vec<Static> {
        values.iter().map(|v| std::array::from_fn(|k| v * (k as f32 + 1.0) / 6.0)).collect()
    }

    #[test]
    fn a_sequence_matches_itself_better_than_a_different_one() {
        let a = seq(&[0.0, 1.0, 2.0, 3.0, 2.0, 1.0, 0.0, -1.0, -2.0, -1.0, 0.0, 1.0, 2.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.5, 1.0, 1.5]);
        let b = seq(&[2.0, 2.0, 2.0, -2.0, -2.0, -2.0, 2.0, 2.0, 2.0, -2.0, -2.0, -2.0, 2.0, 2.0, 2.0, -2.0, -2.0, -2.0, 2.0, 2.0, 2.0, 2.0]);
        let templates = vec![build_template(&a), build_template(&a)];
        let same = score_sequence(&templates, &a);
        let other = score_sequence(&templates, &b);
        assert!(same < 1e-9, "{same}");
        assert!(other > 0.5, "{other}");
        // A slower take (each frame twice) still matches better than a different phrase.
        let slow: Vec<Static> = a.iter().flat_map(|f| [*f, *f]).take(34).collect();
        let stretched = score_sequence(&templates, &slow);
        assert!(stretched.is_finite() && stretched < other, "{stretched}");
        // Far too short: outside the allowed length window.
        assert!(score_sequence(&templates, &a[..8]).is_infinite());
    }
}
