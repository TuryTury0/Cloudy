//! Wake word accuracy on the test audio from eval/make_audio.ps1 (Windows voices). For every phrase and voice it
//! trains the way the app does (six takes, free speech, self-test), then counts:
//! - hits: the same voice saying the phrase in other ways (speed, pitch, loudness), each alone between pauses;
//! - false wakes: the same voice talking normally (every sentence, back to back), and another voice talking;
//! - other voice: another voice saying the phrase (shown only; a wake word may or may not answer other people).
//!
//!   cargo run --release -p anytask-wake --example eval [-- <dir> [noise]]
//!
//! `noise` is the background hiss level (default 0.003; 0.01 is a noisy room).

use anytask_wake::{EnrollEvent, Enrollment, Listener, SR, Stage, WakeModel};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn around(&mut self, center: f64, spread: f64) -> f64 {
        center + (self.next() * 2.0 - 1.0) * spread
    }
}

fn read_wav(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            let end = (at + 8 + size).min(bytes.len());
            return bytes[at + 8..end].chunks_exact(2).map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0).collect();
        }
        at += 8 + size + (size & 1);
    }
    panic!("{}: no audio", path.display());
}

fn list(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix))).collect())
        .unwrap_or_default();
    files.sort();
    files
}

struct Audio {
    noise: f64,
    rng: Rng,
}

impl Audio {
    fn quiet(&mut self, seconds: f64) -> Vec<f32> {
        (0..(seconds * SR as f64) as usize).map(|_| self.rng.around(0.0, self.noise) as f32).collect()
    }
    /// Speech with the room's hiss on top.
    fn sound(&mut self, clip: &[f32]) -> Vec<f32> {
        clip.iter().map(|s| s + self.rng.around(0.0, self.noise) as f32).collect()
    }
}

/// Trains like the app: every take prompt gets the next take, the free-speech prompt gets the free speech.
fn train(label: &str, takes: &[Vec<f32>], free: &[f32], audio: &mut Audio) -> Result<WakeModel, String> {
    let mut enrollment = Enrollment::new(label, 1);
    let mut queue: VecDeque<f32> = VecDeque::new();
    let mut next = 0usize;
    for _ in 0..(400 * SR / 640) {
        if queue.len() < 640 {
            queue.extend(audio.quiet(0.04));
        }
        let chunk: Vec<f32> = queue.drain(..640).collect();
        for event in enrollment.push(&chunk) {
            match event {
                EnrollEvent::Prompt(Stage::Enroll { .. } | Stage::Test | Stage::TestRetry) => {
                    queue.extend(audio.quiet(0.5));
                    let take = takes[next % takes.len()].clone();
                    next += 1;
                    queue.extend(audio.sound(&take));
                }
                EnrollEvent::Prompt(Stage::Negatives { seconds }) => {
                    queue.extend(audio.quiet(0.4));
                    let n = (seconds * SR).min(free.len());
                    queue.extend(audio.sound(&free[..n]));
                }
                EnrollEvent::Finished { model, .. } => return Ok(*model),
                EnrollEvent::Failed(why) => return Err(why.to_string()),
                _ => {}
            }
        }
    }
    Err("did not finish".into())
}

/// Which clips fire (each alone between pauses).
fn fired(model: &WakeModel, clips: &[Vec<f32>], audio: &mut Audio) -> Vec<bool> {
    let mut out = Vec::new();
    for clip in clips {
        let mut listener = Listener::new(model.clone());
        let mut stream = audio.quiet(1.2);
        stream.extend(audio.sound(clip));
        stream.extend(audio.quiet(1.2));
        out.push(stream.chunks(640).any(|c| listener.push(c).is_some()));
    }
    out
}

fn hits(model: &WakeModel, clips: &[Vec<f32>], audio: &mut Audio) -> usize {
    fired(model, clips, audio).into_iter().filter(|f| *f).count()
}

/// False wakes in continuous talk (the clips back to back with short pauses); also returns the talk's length.
fn false_wakes(model: &WakeModel, clips: &[Vec<f32>], audio: &mut Audio) -> (usize, f64) {
    let mut listener = Listener::new(model.clone());
    let mut stream = audio.quiet(1.0);
    for clip in clips {
        stream.extend(audio.sound(clip));
        let gap = audio.rng.around(0.4, 0.3);
        stream.extend(audio.quiet(gap));
    }
    let fired = stream.chunks(640).filter(|c| listener.push(c).is_some()).count();
    (fired, stream.len() as f64 / SR as f64)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "target/wake-eval".into()));
    let noise: f64 = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(0.003);
    let voices = ["david", "zira"];
    let phrases = [("heynova", "Hey Nova"), ("jarvis", "Jarvis"), ("anytask", "Hey AnyTask")];
    // The takes of training vary a little, as a person's do (slow, medium, fast; low, medium and high pitch); the
    // self-test take is the next one; all the rest test. Clip n: speed n % 5 (a quarter slower .. a fifth faster), pitch n / 5 % 3.
    let take_ids = [2usize, 1, 3, 12, 11, 13, 22, 21, 23];
    let self_test = 7usize;
    let (mut by_speed, mut by_speed_n) = ([0usize; 5], [0usize; 5]);

    println!("noise {noise}");
    println!("{:<12} {:<6} {:<10} {:>9} {:>8} {:>16} {:>16} {:>10}", "phrase", "voice", "quality", "threshold", "hits", "false (same)", "false (other)", "other says");
    let (mut all_hits, mut all_tests, mut all_false, mut all_seconds) = (0, 0, 0, 0.0);
    for (id, label) in phrases {
        for (vi, voice) in voices.iter().enumerate() {
            let other = voices[1 - vi];
            let mut audio = Audio { noise, rng: Rng(17 + vi as u64 * 31 + id.len() as u64) };
            let pos: Vec<Vec<f32>> = list(&root.join(id).join("pos"), voice).iter().map(|p| read_wav(p)).collect();
            let other_pos: Vec<Vec<f32>> = list(&root.join(id).join("pos"), other).iter().map(|p| read_wav(p)).collect();
            let neg: Vec<Vec<f32>> = list(&root.join("neg"), voice).iter().map(|p| read_wav(p)).collect();
            let other_neg: Vec<Vec<f32>> = list(&root.join("neg"), other).iter().map(|p| read_wav(p)).collect();
            let free = read_wav(&root.join("free").join(format!("{voice}.wav")));
            assert!(pos.len() >= 30 && !neg.is_empty(), "run eval/make_audio.ps1 first");
            let mut takes: Vec<Vec<f32>> = take_ids.iter().map(|&i| pos[i].clone()).collect();
            takes.insert(6, pos[self_test].clone());
            let test_ids: Vec<usize> = (0..pos.len()).filter(|i| !take_ids.contains(i) && *i != self_test).collect();
            let tests: Vec<Vec<f32>> = test_ids.iter().map(|&i| pos[i].clone()).collect();
            let model = match train(label, &takes, &free, &mut audio) {
                Ok(m) => m,
                Err(e) => {
                    println!("{id:<12} {voice:<6} training failed: {e}");
                    continue;
                }
            };
            let shots = fired(&model, &tests, &mut audio);
            for (i, shot) in test_ids.iter().zip(&shots) {
                by_speed_n[i % 5] += 1;
                by_speed[i % 5] += usize::from(*shot);
            }
            let h = shots.iter().filter(|s| **s).count();
            let (fs, s_same) = false_wakes(&model, &neg, &mut audio);
            let (fo, s_other) = false_wakes(&model, &other_neg, &mut audio);
            let oh = hits(&model, &other_pos, &mut audio);
            println!(
                "{id:<12} {voice:<6} {:<10} {:>9.3} {:>8} {:>16} {:>16} {:>10}",
                model.quality,
                model.threshold,
                format!("{h}/{}", tests.len()),
                format!("{fs} in {:.0} min", s_same / 60.0),
                format!("{fo} in {:.0} min", s_other / 60.0),
                format!("{oh}/{}", other_pos.len())
            );
            all_hits += h;
            all_tests += tests.len();
            all_false += fs + fo;
            all_seconds += s_same + s_other;
        }
    }
    let speeds = ["-25%", "-12%", "normal", "+12%", "+25%"];
    let parts: Vec<String> = speeds.iter().enumerate().map(|(i, s)| format!("{s} {}/{}", by_speed[i], by_speed_n[i])).collect();
    println!("\nhits by speed: {}", parts.join(", "));
    println!(
        "TOTAL hits {all_hits}/{all_tests} ({:.0}%), false wakes {all_false} in {:.0} min of talk ({:.1} per hour)",
        100.0 * all_hits as f64 / all_tests.max(1) as f64,
        all_seconds / 60.0,
        all_false as f64 / (all_seconds / 3600.0)
    );
}
