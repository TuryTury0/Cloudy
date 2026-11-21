//! Microphone and speaker for voice conversations. The microphone delivers 16 kHz mono PCM in 40 ms chunks (what
//! Gemini Live expects); the speaker plays PCM at any rate and can be cut off at once (when the user interrupts).
//! Both report a 0..1 level for the island's animations. cpal streams are not Send on every platform, so each one
//! lives on its own thread and is stopped through its handle.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

pub const MIC_RATE: u32 = 16_000;
/// 40 ms at 16 kHz.
pub const MIC_CHUNK: usize = 640;

/// Low-pass taps for going down in rate: windowed sinc (Blackman), cutoff a little under the new Nyquist.
const LOWPASS_TAPS: usize = 47;

fn lowpass(cutoff: f64) -> Vec<f32> {
    let mid = (LOWPASS_TAPS - 1) as f64 / 2.0;
    let raw: Vec<f64> = (0..LOWPASS_TAPS)
        .map(|n| {
            let x = n as f64 - mid;
            let sinc = if x == 0.0 { 2.0 * cutoff } else { (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x) };
            let w = 2.0 * std::f64::consts::PI * n as f64 / (LOWPASS_TAPS - 1) as f64;
            sinc * (0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos())
        })
        .collect();
    let sum: f64 = raw.iter().sum();
    raw.iter().map(|t| (t / sum) as f32).collect()
}

/// Resampler that keeps its state between chunks. Going down in rate (48 kHz mic to 16 kHz) it first removes
/// what the new rate cannot hold; without that, hiss and sibilants fold back over the voice and speech
/// recognition mishears it. Then linear interpolation (enough for speech).
#[derive(Debug, Clone)]
pub struct Resampler {
    step: f64,
    pos: f64,
    last: f32,
    taps: Vec<f32>,
    /// The last input samples, for the filter.
    history: Vec<f32>,
    filtered: Vec<f32>,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        let step = f64::from(from) / f64::from(to.max(1));
        let taps = if step > 1.0 + f64::EPSILON { lowpass(0.45 / step) } else { Vec::new() };
        Self { step, pos: 0.0, last: 0.0, history: vec![0.0; taps.len().saturating_sub(1)], taps, filtered: Vec::new() }
    }

    /// Converts the next chunk; output length follows the rate ratio across calls.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if (self.step - 1.0).abs() < f64::EPSILON {
            out.extend_from_slice(input);
            return;
        }
        if !self.taps.is_empty() {
            let mut filtered = std::mem::take(&mut self.filtered);
            filtered.clear();
            self.history.extend_from_slice(input);
            for window in self.history.windows(self.taps.len()) {
                filtered.push(window.iter().zip(self.taps.iter().rev()).map(|(x, h)| x * h).sum());
            }
            let keep = self.taps.len() - 1;
            self.history.drain(..self.history.len() - keep);
            self.interpolate(&filtered, out);
            self.filtered = filtered;
        } else {
            self.interpolate(input, out);
        }
    }

    fn interpolate(&mut self, input: &[f32], out: &mut Vec<f32>) {
        // Position 0 is the last sample of the previous chunk, 1 the first of this one.
        let sample = |i: usize| if i == 0 { self.last } else { input[i - 1] };
        while (self.pos as usize) < input.len() {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = sample(i);
            let b = sample(i + 1);
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= input.len() as f64;
        if let Some(l) = input.last() {
            self.last = *l;
        }
    }
}

fn level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    (rms * 4.0).min(1.0)
}

fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0) as i16
}

#[derive(Clone)]
struct Level(Arc<AtomicU32>);

impl Level {
    fn new() -> Self {
        Self(Arc::new(AtomicU32::new(0)))
    }
    fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// An open microphone. Dropping it (or `stop`) closes the device.
pub struct Mic {
    stop: Option<mpsc::Sender<()>>,
    level: Level,
}

impl Mic {
    /// Opens the default microphone; `on_chunk` gets 40 ms of 16 kHz mono PCM at a time (on the audio thread).
    pub fn start(on_chunk: impl FnMut(Vec<i16>) + Send + 'static) -> Result<Mic, String> {
        let level = Level::new();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let meter = level.clone();
        std::thread::spawn(move || {
            let stream = match open_input(on_chunk, meter) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            let _ = stop_rx.recv();
            drop(stream);
        });
        ready_rx.recv().map_err(|_| "The microphone thread stopped.".to_string())??;
        Ok(Mic { stop: Some(stop_tx), level })
    }

    /// Loudness of the last chunk, 0..1.
    pub fn level(&self) -> f32 {
        self.level.get()
    }

    pub fn stop(&mut self) {
        self.stop.take();
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.stop();
    }
}

fn open_input(mut on_chunk: impl FnMut(Vec<i16>) + Send + 'static, level: Level) -> Result<cpal::Stream, String> {
    let device = cpal::default_host().default_input_device().ok_or("No microphone was found.")?;
    let config = device.default_input_config().map_err(|e| format!("Microphone: {e}"))?;
    let channels = usize::from(config.channels().max(1));
    let mut resampler = Resampler::new(config.sample_rate().0, MIC_RATE);
    let mut mono = Vec::new();
    let mut resampled = Vec::new();
    let mut pending: Vec<f32> = Vec::new();
    let mut feed = move |frames: &mut dyn Iterator<Item = f32>| {
        mono.clear();
        let all: Vec<f32> = frames.collect();
        for frame in all.chunks(channels) {
            mono.push(frame.iter().sum::<f32>() / frame.len() as f32);
        }
        resampled.clear();
        resampler.process(&mono, &mut resampled);
        pending.extend_from_slice(&resampled);
        while pending.len() >= MIC_CHUNK {
            let chunk: Vec<f32> = pending.drain(..MIC_CHUNK).collect();
            level.set(self::level(&chunk));
            on_chunk(chunk.into_iter().map(to_i16).collect());
        }
    };
    let error = |e| eprintln!("microphone error: {e}");
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(&config.into(), move |data: &[f32], _| feed(&mut data.iter().copied()), error, None),
        cpal::SampleFormat::I16 => device.build_input_stream(&config.into(), move |data: &[i16], _| feed(&mut data.iter().map(|s| f32::from(*s) / 32768.0)), error, None),
        cpal::SampleFormat::U16 => {
            device.build_input_stream(&config.into(), move |data: &[u16], _| feed(&mut data.iter().map(|s| (f32::from(*s) - 32768.0) / 32768.0)), error, None)
        }
        other => return Err(format!("Unsupported microphone format {other:?}.")),
    }
    .map_err(|e| format!("Microphone: {e}"))?;
    stream.play().map_err(|e| format!("Microphone: {e}"))?;
    Ok(stream)
}

/// The speaker: queue PCM to play; `clear` stops at once.
#[derive(Clone)]
pub struct Speaker {
    queue: Arc<Mutex<VecDeque<f32>>>,
    rate: u32,
    level: Level,
    stop: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    resamplers: Arc<Mutex<Option<(u32, Resampler)>>>,
    playing: Arc<AtomicBool>,
}

impl Speaker {
    pub fn start() -> Result<Speaker, String> {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let level = Level::new();
        let playing = Arc::new(AtomicBool::new(false));
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        let (q, l, p) = (queue.clone(), level.clone(), playing.clone());
        std::thread::spawn(move || {
            let (stream, rate) = match open_output(q, l, p) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(rate));
            let _ = stop_rx.recv();
            drop(stream);
        });
        let rate = ready_rx.recv().map_err(|_| "The speaker thread stopped.".to_string())??;
        Ok(Speaker { queue, rate, level, stop: Arc::new(Mutex::new(Some(stop_tx))), resamplers: Arc::default(), playing })
    }

    /// Queues mono PCM recorded at `rate` (e.g. Gemini Live's 24 kHz).
    pub fn play(&self, pcm: &[i16], rate: u32) {
        let input: Vec<f32> = pcm.iter().map(|s| f32::from(*s) / 32768.0).collect();
        let mut out = Vec::with_capacity(input.len() * 2);
        if let Ok(mut r) = self.resamplers.lock() {
            if r.as_ref().map(|(from, _)| *from) != Some(rate) {
                *r = Some((rate, Resampler::new(rate, self.rate)));
            }
            if let Some((_, resampler)) = r.as_mut() {
                resampler.process(&input, &mut out);
            }
        }
        if let Ok(mut q) = self.queue.lock() {
            q.extend(out);
        }
    }

    /// Stops what is playing now (the user started talking over the assistant).
    pub fn clear(&self) {
        if let Ok(mut q) = self.queue.lock() {
            q.clear();
        }
    }

    /// Sound is still queued or playing.
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn level(&self) -> f32 {
        self.level.get()
    }

    pub fn stop(&self) {
        if let Ok(mut s) = self.stop.lock() {
            s.take();
        }
    }
}

fn open_output(queue: Arc<Mutex<VecDeque<f32>>>, level: Level, playing: Arc<AtomicBool>) -> Result<(cpal::Stream, u32), String> {
    let device = cpal::default_host().default_output_device().ok_or("No speaker was found.")?;
    let config = device.default_output_config().map_err(|e| format!("Speaker: {e}"))?;
    let channels = usize::from(config.channels().max(1));
    let rate = config.sample_rate().0;
    let next = move |count: usize| -> Vec<f32> {
        let mut q = queue.lock().ok();
        let samples: Vec<f32> = (0..count).map(|_| q.as_mut().and_then(|q| q.pop_front()).unwrap_or(0.0)).collect();
        let still = q.map(|q| !q.is_empty()).unwrap_or(false);
        playing.store(still, Ordering::Relaxed);
        level.set(self::level(&samples));
        samples
    };
    let error = |e| eprintln!("speaker error: {e}");
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _| {
                let mono = next(data.len() / channels);
                for (frame, s) in data.chunks_mut(channels).zip(mono) {
                    frame.fill(s);
                }
            },
            error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config.into(),
            move |data: &mut [i16], _| {
                let mono = next(data.len() / channels);
                for (frame, s) in data.chunks_mut(channels).zip(mono) {
                    frame.fill(to_i16(s));
                }
            },
            error,
            None,
        ),
        other => return Err(format!("Unsupported speaker format {other:?}.")),
    }
    .map_err(|e| format!("Speaker: {e}"))?;
    stream.play().map_err(|e| format!("Speaker: {e}"))?;
    Ok((stream, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_the_length_ratio_across_chunks() {
        let mut r = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        for _ in 0..10 {
            r.process(&[0.5; 480], &mut out);
        }
        assert!((out.len() as i64 - 1600).abs() <= 2, "{}", out.len());
        // After the filter's short start (47 input samples) a steady signal comes out unchanged.
        assert!(out[20..].iter().all(|s| (*s - 0.5).abs() < 1e-4));
        let mut up = Resampler::new(24_000, 48_000);
        let mut out = Vec::new();
        up.process(&[0.0, 1.0, 0.0, 1.0], &mut out);
        assert_eq!(out.len(), 8);
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    fn tone(freq: f32, rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize).map(|n| 0.5 * (2.0 * std::f32::consts::PI * freq * n as f32 / rate as f32).sin()).collect()
    }

    #[test]
    fn going_down_keeps_speech_and_removes_what_would_fold_back() {
        let run = |freq: f32| {
            let mut r = Resampler::new(48_000, 16_000);
            let mut out = Vec::new();
            for chunk in tone(freq, 48_000, 0.5).chunks(960) {
                r.process(chunk, &mut out);
            }
            assert!((out.len() as i64 - 8000).abs() <= 2, "{}", out.len());
            rms(&out[200..])
        };
        let input = 0.5 / 2f32.sqrt();
        assert!((run(1000.0) - input).abs() < 0.03, "speech band passes: {}", run(1000.0));
        assert!(run(12_000.0) < 0.02 * input, "12 kHz must not fold back to 4 kHz: {}", run(12_000.0));
    }

    #[test]
    fn level_and_conversion() {
        assert_eq!(level(&[]), 0.0);
        assert!(level(&[0.5, -0.5]) > 0.9);
        assert_eq!(to_i16(2.0), 32767);
        assert_eq!(to_i16(-1.0), -32767);
    }
}
