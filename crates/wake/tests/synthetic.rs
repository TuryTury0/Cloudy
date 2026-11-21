//! The whole wake word flow with synthetic speech (harmonic vowels with formants): a scripted "user" answers the
//! training prompts, then the trained word must fire on the phrase and stay quiet on other speech.

use anytask_wake::{EnrollEvent, Enrollment, Listener, SR, Stage, WakeModel};
use std::collections::VecDeque;
use std::f64::consts::PI;

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

/// Vowels as (F1, F2) formants.
const HEY_NOVA: &[(f64, f64)] = &[(700.0, 1200.0), (400.0, 2000.0), (300.0, 800.0), (650.0, 1100.0)];
const OTHERS: &[&[(f64, f64)]] = &[
    &[(300.0, 2300.0), (500.0, 1500.0), (300.0, 900.0), (300.0, 2300.0)],
    &[(650.0, 1100.0), (650.0, 1100.0), (300.0, 2300.0)],
    &[(500.0, 1500.0), (300.0, 800.0), (700.0, 1200.0), (300.0, 2300.0), (400.0, 2000.0)],
    &[(300.0, 800.0), (300.0, 2300.0), (650.0, 1100.0)],
];

fn speak(vowels: &[(f64, f64)], rng: &mut Rng, tempo: f64, amp: f64) -> Vec<f32> {
    let seg = (0.17 * tempo * SR as f64) as usize;
    let total = seg * vowels.len();
    let f0_start = rng.around(130.0, 8.0);
    let mut phases = vec![0.0f64; 40];
    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        let progress = n as f64 / total as f64;
        let f0 = f0_start - 20.0 * progress;
        // Formants glide between neighbouring vowels.
        let pos = n as f64 / seg as f64;
        let (i, frac) = (pos.floor() as usize, pos.fract());
        let (a, b) = (vowels[i.min(vowels.len() - 1)], vowels[(i + 1).min(vowels.len() - 1)]);
        let glide = if frac > 0.75 { (frac - 0.75) / 0.25 } else { 0.0 };
        let (f1, f2) = (a.0 + (b.0 - a.0) * glide, a.1 + (b.1 - a.1) * glide);
        let mut s = 0.0;
        for (h, phase) in phases.iter_mut().enumerate() {
            let freq = f0 * (h + 1) as f64;
            if freq > 4000.0 {
                break;
            }
            let w = (-((freq - f1) / 120.0).powi(2)).exp() + 0.6 * (-((freq - f2) / 180.0).powi(2)).exp() + 0.03;
            *phase += 2.0 * PI * freq / SR as f64;
            s += w * phase.sin();
        }
        let envelope = (n as f64 / 400.0).min(1.0).min((total - n) as f64 / 400.0);
        out.push((amp * envelope * s / 3.0 + rng.around(0.0, 0.001)) as f32);
    }
    out
}

fn quiet(seconds: f64, rng: &mut Rng) -> Vec<f32> {
    (0..(seconds * SR as f64) as usize).map(|_| rng.around(0.0, 0.002) as f32).collect()
}

fn take(rng: &mut Rng) -> Vec<f32> {
    let (tempo, amp) = (rng.around(1.0, 0.08), rng.around(0.25, 0.06));
    speak(HEY_NOVA, rng, tempo, amp)
}

fn chatter(seconds: f64, rng: &mut Rng) -> Vec<f32> {
    let mut out = Vec::new();
    while (out.len() as f64) < seconds * SR as f64 {
        let words = OTHERS[(rng.next() * OTHERS.len() as f64) as usize % OTHERS.len()];
        let (tempo, amp) = (rng.around(1.0, 0.15), rng.around(0.25, 0.08));
        out.extend(speak(words, rng, tempo, amp));
        out.extend(quiet(rng.around(0.15, 0.1), rng));
    }
    out.truncate((seconds * SR as f64) as usize);
    out
}

/// Plays the "user" until training ends.
fn train(rng: &mut Rng) -> (Option<(WakeModel, bool)>, Vec<EnrollEvent>) {
    let mut enrollment = Enrollment::new("hey nova", 1);
    let mut queue: VecDeque<f32> = VecDeque::new();
    let mut log = Vec::new();
    for _ in 0..(240 * SR / 640) {
        if queue.len() < 640 {
            queue.extend(quiet(0.04, rng));
        }
        let chunk: Vec<f32> = queue.drain(..640).collect();
        for event in enrollment.push(&chunk) {
            match &event {
                EnrollEvent::Prompt(Stage::Enroll { .. } | Stage::Test | Stage::TestRetry) => {
                    queue.extend(quiet(0.5, rng));
                    queue.extend(take(rng));
                }
                EnrollEvent::Prompt(Stage::Negatives { seconds }) => {
                    queue.extend(quiet(0.4, rng));
                    queue.extend(chatter(*seconds as f64, rng));
                }
                EnrollEvent::Finished { model, tested } => return (Some(((**model).clone(), *tested)), log),
                EnrollEvent::Failed(_) => {
                    log.push(event);
                    return (None, log);
                }
                _ => {}
            }
            log.push(event);
        }
    }
    (None, log)
}

#[test]
fn training_then_detection_with_synthetic_speech() {
    let mut rng = Rng(42);
    let (trained, log) = train(&mut rng);
    let (model, tested) = trained.unwrap_or_else(|| panic!("training did not finish: {log:?}"));
    let accepted = log.iter().filter(|e| matches!(e, EnrollEvent::Accepted { .. })).count();
    assert_eq!(accepted, 6, "{log:?}");
    assert!(tested, "the self-test fires");
    assert_ne!(model.quality, "weak", "phrase and chatter are told apart (threshold {})", model.threshold);
    assert_eq!(model.label, "hey nova");
    let saved = WakeModel::from_json(&model.to_json()).expect("saved model loads");

    // Chatter, the phrase, more chatter, the phrase again after the cooldown.
    let mut stream = quiet(1.0, &mut rng);
    stream.extend(chatter(6.0, &mut rng));
    stream.extend(quiet(0.8, &mut rng));
    let first = stream.len();
    stream.extend(take(&mut rng));
    stream.extend(quiet(1.0, &mut rng));
    stream.extend(chatter(5.0, &mut rng));
    stream.extend(quiet(0.8, &mut rng));
    let second = stream.len();
    stream.extend(take(&mut rng));
    stream.extend(quiet(1.5, &mut rng));

    let mut listener = Listener::new(saved);
    let mut fired = Vec::new();
    for (i, chunk) in stream.chunks(640).enumerate() {
        if let Some(trigger) = listener.push(chunk) {
            fired.push((i * 640, trigger));
        }
    }
    assert_eq!(fired.len(), 2, "fires once per phrase and never on chatter: {fired:?}");
    for ((at, trigger), said) in fired.iter().zip([first, second]) {
        let start = trigger.start_sample as i64;
        assert!((start - said as i64).abs() < SR as i64 / 4, "phrase start {start} vs {said}");
        assert!(*at > said && *at < said + 2 * SR, "fires right after the phrase ({at} vs {said})");
    }
}

#[test]
fn silence_only_training_fails_with_no_speech() {
    let mut rng = Rng(7);
    let mut enrollment = Enrollment::new("", 0);
    let mut last = None;
    for _ in 0..(60 * SR / 640) {
        for event in enrollment.push(&quiet(0.04, &mut rng)) {
            last = Some(event);
        }
        if enrollment.is_done() {
            break;
        }
    }
    assert_eq!(last, Some(EnrollEvent::Failed("no_speech")));
}
