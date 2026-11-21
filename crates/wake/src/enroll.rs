//! Training a wake word, fully on the device: the user says the phrase six times, then talks freely for a few
//! seconds (to learn what is *not* the phrase), then says it once more as a self-test. Ported from v1 train().
//!
//! The steps are a state machine fed with microphone audio; time is counted in frames (10 ms each), so the whole
//! flow runs the same with recorded audio in tests. The app plays a short tone on every `Prompt` and drops the
//! machine to abort.

use crate::dsp::{Frame, FrontEnd};
use crate::matcher::{Detector, PREROLL, Static, build_template, normalize, score_sequence};
use crate::model::{WakeModel, compute_stats, is_inconsistent, length_fits, pick_threshold};
use crate::vad::{HANG, Signal, Vad};

pub const ENROLL_COUNT: usize = 6;
const MAX_ENROLL_ATTEMPTS: usize = ENROLL_COUNT + 8;
pub const NEGATIVE_SECONDS: usize = 12;
const NEGATIVE_ROUNDS: u32 = 2;
const MIN_NEGATIVE_SPEECH: usize = 200;
const MIN_UTTERANCE: usize = 35;
pub const MAX_UTTERANCE: usize = 400;
const TRAIL: u32 = 7;
/// Frames are 10 ms apart.
const PER_SECOND: usize = 100;
const WARMUP: usize = 45;
const PROMPT_TONE: usize = 26;
const QUIET_FRAMES: usize = 30;
const SETTLE_MIN: usize = 12;
const SETTLE_MAX: usize = 300;
const CAPTURE_TIMEOUT: usize = 900;
const TEST_FRAMES: usize = 900;
const RETEST_FRAMES: usize = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Say the phrase (take `index` of `total`).
    Enroll { index: usize, total: usize },
    /// Talk freely for `seconds`.
    Negatives { seconds: usize },
    /// Say the phrase once to test it.
    Test,
    TestRetry,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnrollEvent {
    /// Play the prompt tone and show what to do.
    Prompt(Stage),
    Accepted { index: usize, total: usize },
    /// "timeout", "too_short", "too_long", "too_quiet", "inconsistent" or "not_enough_speech".
    Rejected(&'static str),
    /// Seconds left of free speech.
    Tick(usize),
    Finished { model: Box<WakeModel>, tested: bool },
    /// "no_speech" or "too_many_failures".
    Failed(&'static str),
}

#[derive(Debug, Clone, Copy)]
struct Rec {
    frame: Frame,
    v: Signal,
    floor: f32,
}

enum Armed {
    Capture,
    Negatives(u32),
    Test(bool),
}

enum Phase {
    Warmup,
    Tone { until: usize, then: Armed },
    Capture { since: usize, begin: Option<usize> },
    Settle { since: usize, then: Armed },
    Negatives { round: u32, from: usize, until: usize, floor: f32, last_tick: usize },
    Test { detector: Box<Detector>, until: usize, retry: bool },
    Done,
}

pub struct Enrollment {
    label: String,
    now_ms: i64,
    front: FrontEnd,
    vad: Vad,
    all: Vec<Rec>,
    quiet_run: usize,
    phase: Phase,
    takes: Vec<Vec<Static>>,
    attempts: usize,
    idle_strikes: usize,
    mismatch_streak: usize,
    candidate: Option<(WakeModel, f64)>,
}

impl Enrollment {
    /// `now_ms` is stored as the model's creation time.
    pub fn new(label: &str, now_ms: i64) -> Self {
        Self {
            label: label.trim().to_string(),
            now_ms,
            front: FrontEnd::default(),
            vad: Vad::default(),
            all: Vec::new(),
            quiet_run: 0,
            phase: Phase::Warmup,
            takes: Vec::new(),
            attempts: 0,
            idle_strikes: 0,
            mismatch_streak: 0,
            candidate: None,
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// 16 kHz samples in -1..1.
    pub fn push(&mut self, samples: &[f32]) -> Vec<EnrollEvent> {
        let mut events = Vec::new();
        for frame in self.front.push(samples) {
            if self.is_done() {
                break;
            }
            self.step(frame, &mut events);
        }
        events
    }

    fn step(&mut self, frame: Frame, events: &mut Vec<EnrollEvent>) {
        let idx = self.all.len();
        let v = self.vad.step(frame.db);
        self.quiet_run = if v.speech { 0 } else { self.quiet_run + 1 };
        let rec = Rec { frame, v, floor: self.vad.floor };
        self.all.push(rec);

        match std::mem::replace(&mut self.phase, Phase::Done) {
            Phase::Warmup => {
                self.phase = Phase::Warmup;
                if idx >= WARMUP {
                    self.next_take(idx, events);
                }
            }
            Phase::Tone { until, then } => {
                if idx < until {
                    self.phase = Phase::Tone { until, then };
                    return;
                }
                // The tone itself looks like speech, so listening starts fresh after it.
                self.vad.reset();
                self.quiet_run = 0;
                self.phase = match then {
                    Armed::Capture => Phase::Capture { since: idx, begin: None },
                    Armed::Negatives(round) => Phase::Negatives { round, from: idx + 1, until: idx + NEGATIVE_SECONDS * PER_SECOND, floor: self.vad.floor, last_tick: idx },
                    Armed::Test(retry) => {
                        let (model, _) = self.candidate.clone().expect("a candidate exists before the test");
                        let frames = if retry { RETEST_FRAMES } else { TEST_FRAMES };
                        Phase::Test { detector: Box::new(Detector::new(model, false, Some(self.vad.floor))), until: idx + frames, retry }
                    }
                };
            }
            Phase::Capture { since, begin } => match begin {
                None if v.start => self.phase = Phase::Capture { since, begin: Some(idx.saturating_sub(2 + PREROLL)) },
                None if idx - since > CAPTURE_TIMEOUT => self.take_done(None, idx, events),
                None => self.phase = Phase::Capture { since, begin },
                Some(begin) if v.offset => {
                    let end = idx.saturating_sub((HANG - TRAIL) as usize).max(begin);
                    let frames = self.all[begin..end].to_vec();
                    self.take_done(Some(frames), idx, events);
                }
                Some(begin) if idx - begin > MAX_UTTERANCE => {
                    let frames = self.all[begin..idx].to_vec();
                    self.take_done(Some(frames), idx, events);
                }
                Some(begin) => self.phase = Phase::Capture { since, begin: Some(begin) },
            },
            Phase::Settle { since, then } => {
                let waited = idx - since;
                if waited < SETTLE_MIN || (self.quiet_run < QUIET_FRAMES && waited < SETTLE_MAX) {
                    self.phase = Phase::Settle { since, then };
                    return;
                }
                match then {
                    Armed::Capture => self.next_take(idx, events),
                    Armed::Negatives(round) => self.prompt(idx, Stage::Negatives { seconds: NEGATIVE_SECONDS }, Armed::Negatives(round), events),
                    Armed::Test(retry) => self.prompt(idx, if retry { Stage::TestRetry } else { Stage::Test }, Armed::Test(retry), events),
                }
            }
            Phase::Negatives { round, from, until, floor, last_tick } => {
                if idx < until {
                    let last_tick = if idx - last_tick >= 25 {
                        events.push(EnrollEvent::Tick((until - idx).div_ceil(PER_SECOND)));
                        idx
                    } else {
                        last_tick
                    };
                    self.phase = Phase::Negatives { round, from, until, floor, last_tick };
                    return;
                }
                let negatives = self.all[from..].to_vec();
                let speech: Vec<Static> = negatives.iter().filter(|r| r.v.speech && r.frame.db > r.floor + 8.0).map(|r| r.frame.mfcc).collect();
                if speech.len() >= MIN_NEGATIVE_SPEECH {
                    self.calibrate(&negatives, &speech, floor, idx, events);
                } else if round >= NEGATIVE_ROUNDS {
                    self.fail("no_speech", events);
                } else {
                    events.push(EnrollEvent::Rejected("not_enough_speech"));
                    self.phase = Phase::Settle { since: idx, then: Armed::Negatives(round + 1) };
                }
            }
            Phase::Test { mut detector, until, retry } => {
                if detector.on_frame(&frame).is_some() {
                    self.finish(true, events);
                } else if idx < until {
                    self.phase = Phase::Test { detector, until, retry };
                } else if !retry {
                    // Missed once: loosen a little (never past the closest impostor) and try again.
                    if let Some((model, impostor_min)) = self.candidate.as_mut() {
                        model.threshold = model.threshold.max((model.threshold * 1.15).min(*impostor_min * 0.95));
                    }
                    self.phase = Phase::Settle { since: idx, then: Armed::Test(true) };
                } else {
                    self.finish(false, events);
                }
            }
            Phase::Done => {}
        }
    }

    fn prompt(&mut self, idx: usize, stage: Stage, then: Armed, events: &mut Vec<EnrollEvent>) {
        events.push(EnrollEvent::Prompt(stage));
        self.phase = Phase::Tone { until: idx + PROMPT_TONE, then };
    }

    fn next_take(&mut self, idx: usize, events: &mut Vec<EnrollEvent>) {
        self.attempts += 1;
        if self.attempts > MAX_ENROLL_ATTEMPTS {
            return self.fail("too_many_failures", events);
        }
        self.prompt(idx, Stage::Enroll { index: self.takes.len() + 1, total: ENROLL_COUNT }, Armed::Capture, events);
    }

    fn take_done(&mut self, frames: Option<Vec<Rec>>, idx: usize, events: &mut Vec<EnrollEvent>) {
        let mut reason = validate(frames.as_deref());
        let mfccs: Vec<Static> = frames.unwrap_or_default().iter().map(|r| r.frame.mfcc).collect();
        if reason.is_none()
            && !self.takes.is_empty()
            && (!length_fits(&self.takes, mfccs.len()) || (self.takes.len() >= 2 && is_inconsistent(&self.takes, &mfccs)))
        {
            reason = Some("inconsistent");
        }
        if reason == Some("inconsistent") && self.takes.len() == 1 {
            self.mismatch_streak += 1;
            if self.mismatch_streak >= 2 {
                // Two takes in a row disagree with the first one, so the first one was most likely the odd one.
                self.takes.clear();
                reason = None;
            }
        }
        if let Some(reason) = reason {
            self.idle_strikes = if reason == "timeout" { self.idle_strikes + 1 } else { 0 };
            if self.idle_strikes >= 3 {
                return self.fail("no_speech", events);
            }
            events.push(EnrollEvent::Rejected(reason));
            self.phase = Phase::Settle { since: idx, then: Armed::Capture };
            return;
        }
        self.idle_strikes = 0;
        self.mismatch_streak = 0;
        self.takes.push(mfccs);
        events.push(EnrollEvent::Accepted { index: self.takes.len(), total: ENROLL_COUNT });
        let then = if self.takes.len() < ENROLL_COUNT { Armed::Capture } else { Armed::Negatives(1) };
        self.phase = Phase::Settle { since: idx, then };
    }

    fn calibrate(&mut self, negatives: &[Rec], speech: &[Static], floor: f32, idx: usize, events: &mut Vec<EnrollEvent>) {
        let all: Vec<Static> = self.takes.iter().flatten().chain(speech).copied().collect();
        let (mean, std) = compute_stats(&all);
        let seqs: Vec<Vec<Static>> = self.takes.iter().map(|s| s.iter().map(|f| normalize(f, &mean, &std)).collect()).collect();
        let templates: Vec<_> = seqs.iter().map(|s| build_template(s)).collect();
        let genuine: Vec<f64> = seqs
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let others: Vec<_> = templates.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, t)| t.clone()).collect();
                score_sequence(&others, s)
            })
            .filter(|v| v.is_finite())
            .collect();
        if genuine.len() < (ENROLL_COUNT as f64 * 0.66).ceil() as usize {
            return self.fail("too_many_failures", events);
        }
        let mut model = WakeModel {
            label: self.label.clone(),
            created_at: self.now_ms,
            mean,
            std,
            templates,
            threshold: f64::NEG_INFINITY,
            quality: String::new(),
            tested: false,
            tightened: 0,
        };
        let mut sim = Detector::new(model.clone(), true, Some(floor));
        for rec in negatives {
            sim.on_frame(&rec.frame);
        }
        let impostors: Vec<f64> = sim.drain().into_iter().filter(|v| v.is_finite()).collect();
        let calibration = pick_threshold(&genuine, &impostors);
        model.threshold = calibration.threshold;
        model.quality = calibration.quality;
        self.candidate = Some((model, calibration.impostor_min));
        self.phase = Phase::Settle { since: idx, then: Armed::Test(false) };
    }

    fn finish(&mut self, tested: bool, events: &mut Vec<EnrollEvent>) {
        if let Some((mut model, _)) = self.candidate.take() {
            model.tested = tested;
            events.push(EnrollEvent::Finished { model: Box::new(model), tested });
        }
        self.phase = Phase::Done;
    }

    fn fail(&mut self, code: &'static str, events: &mut Vec<EnrollEvent>) {
        events.push(EnrollEvent::Failed(code));
        self.phase = Phase::Done;
    }
}

fn validate(frames: Option<&[Rec]>) -> Option<&'static str> {
    let frames = match frames {
        None => return Some("timeout"),
        Some(f) => f,
    };
    if frames.len() < MIN_UTTERANCE {
        return Some("too_short");
    }
    if frames.len() > MAX_UTTERANCE {
        return Some("too_long");
    }
    let peak = frames.iter().map(|r| r.frame.db).fold(f32::NEG_INFINITY, f32::max);
    let floor = frames.iter().map(|r| r.floor).fold(f32::INFINITY, f32::min);
    if peak - floor < 12.0 { Some("too_quiet") } else { None }
}
