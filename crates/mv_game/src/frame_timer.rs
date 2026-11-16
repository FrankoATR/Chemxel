//! Frame delta measurement and FPS statistics.

use std::time::{Duration, Instant};

/// Measures time between frames and aggregates FPS over one-second windows.
#[derive(Debug)]
pub struct FrameTimer {
    last_frame: Instant,
    window_start: Instant,
    frames_in_window: u32,
    last_fps: f64,
    last_frame_ms: f64,
    worst_in_window: Duration,
    last_worst_ms: f64,
}

impl FrameTimer {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            last_frame: now,
            window_start: now,
            frames_in_window: 0,
            last_fps: 0.0,
            last_frame_ms: 0.0,
            worst_in_window: Duration::ZERO,
            last_worst_ms: 0.0,
        }
    }

    /// Marks the start of a new frame and returns the time since the previous one.
    /// Returns `Some(fps)` as the second value whenever a one-second window closes.
    pub fn tick(&mut self) -> (Duration, Option<f64>) {
        let now = Instant::now();
        let dt = now - self.last_frame;
        self.last_frame = now;
        self.frames_in_window += 1;
        self.worst_in_window = self.worst_in_window.max(dt);

        let window = now - self.window_start;
        if window >= Duration::from_secs(1) {
            self.last_fps = f64::from(self.frames_in_window) / window.as_secs_f64();
            self.last_frame_ms = window.as_secs_f64() * 1000.0 / f64::from(self.frames_in_window);
            self.frames_in_window = 0;
            self.window_start = now;
            self.last_worst_ms = self.worst_in_window.as_secs_f64() * 1000.0;
            self.worst_in_window = Duration::ZERO;
            return (dt, Some(self.last_fps));
        }
        (dt, None)
    }

    /// Restarts measurement (e.g. after the window was hidden), so the next delta
    /// does not include the pause.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn fps(&self) -> f64 {
        self.last_fps
    }

    pub fn frame_ms(&self) -> f64 {
        self.last_frame_ms
    }

    /// Longest frame of the last one-second window, in milliseconds (stutter metric).
    pub fn worst_frame_ms(&self) -> f64 {
        self.last_worst_ms
    }
}
