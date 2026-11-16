//! Fixed timestep clock that decouples simulation ticks from the render framerate.

use std::time::Duration;

/// Accumulates real elapsed time and tells the caller how many fixed simulation
/// ticks to run this frame.
///
/// To avoid a "spiral of death" when the simulation cannot keep up, at most
/// `max_ticks_per_update` ticks are reported per call and any excess backlog is
/// dropped (the simulation slows down instead of freezing the game).
#[derive(Debug, Clone)]
pub struct FixedTimestep {
    step: Duration,
    accumulator: Duration,
    max_ticks_per_update: u32,
    tick: u64,
    dropped_ticks: u64,
}

impl FixedTimestep {
    /// Creates a clock ticking at `tick_rate_hz`.
    ///
    /// # Panics
    /// If `tick_rate_hz` is not finite and positive, or `max_ticks_per_update` is 0.
    pub fn new(tick_rate_hz: f64, max_ticks_per_update: u32) -> Self {
        assert!(
            tick_rate_hz.is_finite() && tick_rate_hz > 0.0,
            "tick rate must be positive, got {tick_rate_hz}"
        );
        assert!(max_ticks_per_update > 0, "max_ticks_per_update must be > 0");
        Self {
            step: Duration::from_secs_f64(1.0 / tick_rate_hz),
            accumulator: Duration::ZERO,
            max_ticks_per_update,
            tick: 0,
            dropped_ticks: 0,
        }
    }

    /// Adds `elapsed` real time and returns how many ticks should run now.
    /// The internal tick counter is advanced by the returned amount.
    pub fn advance(&mut self, elapsed: Duration) -> u32 {
        self.accumulator += elapsed;
        let mut ticks = 0;
        while self.accumulator >= self.step && ticks < self.max_ticks_per_update {
            self.accumulator -= self.step;
            ticks += 1;
        }
        if self.accumulator >= self.step {
            let backlog = self.accumulator.as_nanos() / self.step.as_nanos();
            self.dropped_ticks += backlog as u64;
            self.accumulator =
                Duration::from_nanos((self.accumulator.as_nanos() % self.step.as_nanos()) as u64);
        }
        self.tick += u64::from(ticks);
        ticks
    }

    /// Fraction of a tick accumulated but not yet simulated, in `[0, 1)`.
    /// Use it to interpolate rendered state between ticks.
    pub fn alpha(&self) -> f32 {
        (self.accumulator.as_secs_f64() / self.step.as_secs_f64()) as f32
    }

    /// Duration of one tick.
    pub fn step(&self) -> Duration {
        self.step
    }

    /// Total ticks reported so far.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Ticks discarded because the simulation fell too far behind.
    pub fn dropped_ticks(&self) -> u64 {
        self.dropped_ticks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn accumulates_partial_frames() {
        let mut ts = FixedTimestep::new(100.0, 10); // 10 ms per tick
        assert_eq!(ts.advance(4 * MS), 0);
        assert_eq!(ts.advance(4 * MS), 0);
        assert_eq!(ts.advance(4 * MS), 1);
        assert_eq!(ts.tick(), 1);
        assert!((ts.alpha() - 0.2).abs() < 1e-4);
    }

    #[test]
    fn runs_multiple_ticks_for_long_frames() {
        let mut ts = FixedTimestep::new(100.0, 10);
        assert_eq!(ts.advance(35 * MS), 3);
        assert!((ts.alpha() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn clamps_and_drops_backlog() {
        let mut ts = FixedTimestep::new(100.0, 4);
        assert_eq!(ts.advance(1000 * MS + 5 * MS), 4);
        assert_eq!(ts.dropped_ticks(), 96);
        assert!(ts.alpha() < 1.0);
        // After dropping, the clock behaves normally again.
        assert_eq!(ts.advance(10 * MS), 1);
    }

    #[test]
    fn tick_count_is_exact_over_many_frames() {
        let mut ts = FixedTimestep::new(30.0, 8);
        let mut total = 0;
        // 60 frames of 1/60 s = 1 s of real time => 30 ticks.
        for _ in 0..60 {
            total += ts.advance(Duration::from_secs_f64(1.0 / 60.0));
        }
        assert!((29..=30).contains(&total), "got {total} ticks");
    }

    #[test]
    #[should_panic]
    fn rejects_zero_rate() {
        let _ = FixedTimestep::new(0.0, 1);
    }
}
