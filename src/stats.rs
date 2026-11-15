//! Frame timing for `--stats`: how many frames were drawn and how long they took.

use std::time::Duration;

/// Print a summary every this many frames (about 2 s of motion at 120 Hz).
const PRINT_EVERY: u64 = 240;

#[derive(Default, Debug)]
pub struct Stats {
    /// Frames that drew something.
    pub frames: u64,
    total: Duration,
    max: Duration,
}

impl Stats {
    /// Count a frame that took `took` to render. True when a summary is due.
    pub fn record(&mut self, took: Duration) -> bool {
        self.frames += 1;
        self.total += took;
        self.max = self.max.max(took);
        self.frames.is_multiple_of(PRINT_EVERY)
    }

    /// `frames=… avg=…us max=…us`.
    pub fn summary(&self) -> String {
        let avg = self.total.as_secs_f64() * 1e6 / self.frames.max(1) as f64;
        format!("frames={} avg={avg:.0}us max={:.0}us", self.frames, self.max.as_secs_f64() * 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_every_240_frames() {
        let mut s = Stats::default();
        let due: Vec<bool> = (0..480).map(|_| s.record(Duration::from_micros(500))).collect();
        assert_eq!(due.iter().filter(|&&d| d).count(), 2);
        assert!(due[239] && due[479]);
        assert_eq!(s.summary(), "frames=480 avg=500us max=500us");
    }
}
