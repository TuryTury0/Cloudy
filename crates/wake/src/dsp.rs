//! Audio front end: 25 ms frames every 10 ms at 16 kHz, each turned into 12 MFCCs, a loudness in dB and a voicing
//! score (how periodic the sound is; speech is, breath and keyboard noise are not).

use std::sync::OnceLock;

pub const SR: usize = 16_000;
pub const FRAME: usize = 400;
pub const HOP: usize = 160;
const NFFT: usize = 512;
const NMEL: usize = 26;
pub const NCEP: usize = 12;
const PITCH_MIN_LAG: usize = 40;
const PITCH_MAX_LAG: usize = 260;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub mfcc: [f32; NCEP],
    pub db: f32,
    /// Peak normalized autocorrelation over the human pitch range, 0..1.
    pub pitch: f32,
}

struct Tables {
    hamming: Vec<f64>,
    cos: Vec<f64>,
    sin: Vec<f64>,
    rev: Vec<usize>,
    mel: Vec<(usize, Vec<f64>)>,
    dct: Vec<Vec<f64>>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let hamming = (0..FRAME).map(|n| 0.54 - 0.46 * (2.0 * std::f64::consts::PI * n as f64 / (FRAME - 1) as f64).cos()).collect();
        let cos = (0..NFFT / 2).map(|i| (2.0 * std::f64::consts::PI * i as f64 / NFFT as f64).cos()).collect();
        let sin = (0..NFFT / 2).map(|i| (2.0 * std::f64::consts::PI * i as f64 / NFFT as f64).sin()).collect();
        let levels = NFFT.trailing_zeros();
        let rev = (0..NFFT).map(|i| (0..levels).fold(0, |r, b| r | (((i >> b) & 1) << (levels - 1 - b)))).collect();
        let to_mel = |f: f64| 2595.0 * (1.0 + f / 700.0).log10();
        let to_hz = |m: f64| 700.0 * (10f64.powf(m / 2595.0) - 1.0);
        let (lo, hi) = (to_mel(100.0), to_mel(7000.0));
        let edges: Vec<usize> = (0..NMEL + 2).map(|i| ((NFFT + 1) as f64 * to_hz(lo + (hi - lo) * i as f64 / (NMEL + 1) as f64) / SR as f64).floor() as usize).collect();
        let mel = (0..NMEL)
            .map(|m| {
                let (a, b, c) = (edges[m], edges[m + 1], edges[m + 2]);
                let weights = (a..c).map(|k| if k < b { (k - a) as f64 / (b - a).max(1) as f64 } else { (c - k) as f64 / (c - b).max(1) as f64 }).collect();
                (a, weights)
            })
            .collect();
        let dct = (0..NCEP).map(|c| (0..NMEL).map(|m| (std::f64::consts::PI * (c + 1) as f64 * (m as f64 + 0.5) / NMEL as f64).cos()).collect()).collect();
        Tables { hamming, cos, sin, rev, mel, dct }
    })
}

fn fft(re: &mut [f64], im: &mut [f64], t: &Tables) {
    for i in 0..NFFT {
        let j = t.rev[i];
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut size = 2;
    while size <= NFFT {
        let half = size / 2;
        let step = NFFT / size;
        for i in (0..NFFT).step_by(size) {
            for (j, k) in (i..i + half).zip((0..).step_by(step)) {
                let l = j + half;
                let tre = re[l] * t.cos[k] + im[l] * t.sin[k];
                let tim = -re[l] * t.sin[k] + im[l] * t.cos[k];
                re[l] = re[j] - tre;
                im[l] = im[j] - tim;
                re[j] += tre;
                im[j] += tim;
            }
        }
        size <<= 1;
    }
}

/// Streams samples in, frames out.
pub struct FrontEnd {
    buf: Vec<f32>,
    prev: f32,
    re: Vec<f64>,
    im: Vec<f64>,
    emph: Vec<f64>,
}

impl Default for FrontEnd {
    fn default() -> Self {
        Self { buf: Vec::with_capacity(16_384), prev: 0.0, re: vec![0.0; NFFT], im: vec![0.0; NFFT], emph: vec![0.0; FRAME] }
    }
}

impl FrontEnd {
    /// Samples in -1..1 at 16 kHz.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Frame> {
        self.buf.extend_from_slice(samples);
        let mut frames = Vec::new();
        let mut pos = 0;
        while self.buf.len() - pos >= FRAME {
            let before = if pos > 0 { self.buf[pos - 1] } else { self.prev };
            frames.push(self.analyze(pos, before));
            pos += HOP;
        }
        if pos > 0 {
            self.prev = self.buf[pos - 1];
            self.buf.drain(..pos);
        }
        frames
    }

    fn analyze(&mut self, pos: usize, prev_sample: f32) -> Frame {
        let t = tables();
        let mut energy = 0.0f64;
        for n in 0..FRAME {
            let x = f64::from(self.buf[pos + n]);
            energy += x * x;
            let before = f64::from(if n == 0 { prev_sample } else { self.buf[pos + n - 1] });
            let emphasized = x - 0.97 * before;
            self.emph[n] = emphasized;
            self.re[n] = emphasized * t.hamming[n];
            self.im[n] = 0.0;
        }
        self.re[FRAME..].fill(0.0);
        self.im[FRAME..].fill(0.0);
        fft(&mut self.re, &mut self.im, t);

        let mut log_mel = [0.0f64; NMEL];
        for (m, (start, weights)) in t.mel.iter().enumerate() {
            let e: f64 = weights.iter().enumerate().map(|(k, w)| w * (self.re[start + k].powi(2) + self.im[start + k].powi(2))).sum();
            log_mel[m] = (e + 1e-9).ln();
        }
        let mut mfcc = [0.0f32; NCEP];
        for (c, row) in t.dct.iter().enumerate() {
            mfcc[c] = row.iter().zip(log_mel.iter()).map(|(d, l)| d * l).sum::<f64>() as f32;
        }
        let db = (10.0 * (energy / FRAME as f64 + 1e-10).log10()) as f32;
        Frame { mfcc, db, pitch: periodicity(&self.emph) as f32 }
    }
}

fn periodicity(emph: &[f64]) -> f64 {
    let mut best = 0.0;
    for lag in PITCH_MIN_LAG..=PITCH_MAX_LAG {
        let (mut cross, mut e0, mut e1) = (0.0, 0.0, 0.0);
        for n in 0..FRAME - lag {
            let (a, b) = (emph[n], emph[n + lag]);
            cross += a * b;
            e0 += a * a;
            e1 += b * b;
        }
        let norm = (e0 * e1).sqrt();
        if norm > 1e-9 {
            best = f64::max(best, cross / norm);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, seconds: f64, amp: f64) -> Vec<f32> {
        (0..(seconds * SR as f64) as usize).map(|n| (amp * (2.0 * std::f64::consts::PI * freq * n as f64 / SR as f64).sin()) as f32).collect()
    }

    #[test]
    fn fft_finds_a_pure_tone() {
        let t = tables();
        let k = 32; // bin 32 = 1000 Hz at 16 kHz / 512
        let mut re: Vec<f64> = (0..NFFT).map(|n| (2.0 * std::f64::consts::PI * k as f64 * n as f64 / NFFT as f64).cos()).collect();
        let mut im = vec![0.0; NFFT];
        fft(&mut re, &mut im, t);
        let peak = (0..NFFT / 2).max_by(|a, b| (re[*a].hypot(im[*a])).total_cmp(&re[*b].hypot(im[*b]))).unwrap();
        assert_eq!(peak, k);
        assert!((re[k].hypot(im[k]) - NFFT as f64 / 2.0).abs() < 1e-6);
    }

    #[test]
    fn frames_arrive_every_10_ms_whatever_the_chunk_size() {
        let audio = tone(200.0, 1.0, 0.3);
        let mut whole = FrontEnd::default();
        let all = whole.push(&audio);
        assert_eq!(all.len(), (SR - FRAME) / HOP + 1);
        let mut chunked = FrontEnd::default();
        let pieces: Vec<Frame> = audio.chunks(640).flat_map(|c| chunked.push(c)).collect();
        assert_eq!(pieces.len(), all.len());
        assert!((pieces[50].mfcc[3] - all[50].mfcc[3]).abs() < 1e-3);
    }

    #[test]
    fn voiced_sound_is_periodic_and_noise_is_not() {
        let mut fe = FrontEnd::default();
        let voiced = fe.push(&tone(150.0, 0.2, 0.3));
        assert!(voiced[5].pitch > 0.8, "{}", voiced[5].pitch);
        assert!(voiced[5].db > -20.0);
        let mut seed = 7u32;
        let noise: Vec<f32> = (0..3200)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect();
        let noisy = FrontEnd::default().push(&noise);
        assert!(noisy[5].pitch < 0.4, "{}", noisy[5].pitch);
        let quiet = FrontEnd::default().push(&[0.0; 800]);
        assert!(quiet[0].db < -90.0);
    }
}
