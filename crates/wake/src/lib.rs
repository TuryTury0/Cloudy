//! On-device wake word, trained by the user: no AI, nothing leaves the PC. Ported from v1 audio.js (same features,
//! matching, calibration and saved format).
//!
//! - `dsp`: 16 kHz audio to frames (12 MFCCs, loudness, voicing).
//! - `vad`: speech onsets and ends with an adaptive noise floor.
//! - `matcher`: streaming DTW against the user's takes; the `Detector` fires after the phrase ends.
//! - `model`: the saved wake word and its calibration.
//! - `enroll`: the training steps (six takes, free speech, self-test).

pub mod dsp;
pub mod enroll;
pub mod matcher;
pub mod model;
pub mod vad;

pub use dsp::{Frame, FrontEnd, SR};
pub use enroll::{EnrollEvent, Enrollment, Stage};
pub use matcher::{Detector, Trigger};
pub use model::WakeModel;

/// Listens for the wake word in a stream of 16 kHz samples.
pub struct Listener {
    front: FrontEnd,
    detector: Detector,
}

impl Listener {
    pub fn new(model: WakeModel) -> Self {
        Self { front: FrontEnd::default(), detector: Detector::new(model, false, None) }
    }

    /// True when the wake word was just said.
    pub fn push(&mut self, samples: &[f32]) -> Option<Trigger> {
        let mut fired = None;
        for frame in self.front.push(samples) {
            if let Some(trigger) = self.detector.on_frame(&frame) {
                fired = Some(trigger);
            }
        }
        fired
    }

    pub fn model(&self) -> &WakeModel {
        self.detector.model()
    }
}
