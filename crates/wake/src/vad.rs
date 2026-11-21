//! Voice activity detector with an adaptive noise floor. Every `start` follows at least HANG frames of quiet (or
//! the warm-up), so it also marks where a phrase may begin.

pub const INIT_FRAMES: u32 = 25;
pub const HANG: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Signal {
    pub speech: bool,
    pub start: bool,
    pub offset: bool,
}

#[derive(Debug, Clone)]
pub struct Vad {
    pub floor: f32,
    seen: u32,
    active: bool,
    run: u32,
    hang: u32,
}

impl Default for Vad {
    fn default() -> Self {
        Self { floor: -60.0, seen: 0, active: false, run: 0, hang: 0 }
    }
}

impl Vad {
    /// Starts with a known floor (skips the warm-up).
    pub fn warm(floor: f32) -> Self {
        Self { floor, seen: INIT_FRAMES, ..Self::default() }
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.run = 0;
        self.hang = 0;
    }

    pub fn step(&mut self, db: f32) -> Signal {
        if self.seen < INIT_FRAMES {
            self.floor = if self.seen == 0 { db } else { self.floor + (db - self.floor) / (self.seen + 1) as f32 };
            self.seen += 1;
            return Signal::default();
        }
        let threshold = if self.active { (self.floor + 6.0).max(-58.0) } else { (self.floor + 10.0).max(-55.0) };
        let loud = db > threshold;
        let rate = if db < self.floor {
            0.3
        } else if self.active {
            0.0003
        } else {
            0.01
        };
        self.floor += (db - self.floor) * rate;

        if !self.active {
            self.run = if loud { self.run + 1 } else { 0 };
            if self.run < 3 {
                return Signal::default();
            }
            self.active = true;
            self.hang = 0;
            return Signal { speech: true, start: true, offset: false };
        }
        self.hang = if loud { 0 } else { self.hang + 1 };
        if self.hang < HANG {
            return Signal { speech: true, ..Signal::default() };
        }
        self.active = false;
        self.run = 0;
        Signal { offset: true, ..Signal::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_starts_after_three_loud_frames_and_ends_after_the_hang() {
        let mut vad = Vad::default();
        for _ in 0..INIT_FRAMES {
            assert_eq!(vad.step(-70.0), Signal::default());
        }
        assert!(!vad.step(-30.0).start);
        assert!(!vad.step(-30.0).start);
        assert!(vad.step(-30.0).start);
        assert!(vad.step(-30.0).speech);
        let mut ended_after = 0;
        for i in 1..=HANG + 2 {
            if vad.step(-70.0).offset {
                ended_after = i;
                break;
            }
        }
        assert_eq!(ended_after, HANG);
        assert!((vad.floor - -70.0).abs() < 1.0);
    }
}
