//! `--bench-drop`: milestone 3 acceptance in the real game.
//!
//! Waits until the world around the spawn point is loaded, drops ~100k voxels of
//! sand and water onto the terrain below the camera, then records every simulation
//! tick for a while, logs the timings and asks the app to exit.

use std::time::Duration;

use mv_sim::TickStats;

#[derive(Debug)]
pub struct BenchDrop {
    duration: Duration,
    running: Option<Duration>,
    tick_ms: Vec<f64>,
    max_active: usize,
    max_moves: u32,
}

impl BenchDrop {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            running: None,
            tick_ms: Vec::new(),
            max_active: 0,
            max_moves: 0,
        }
    }

    /// Whether the drop should happen now (world ready and not dropped yet).
    pub fn should_drop(&mut self, world_ready: bool) -> bool {
        if self.running.is_none() && world_ready {
            self.running = Some(Duration::ZERO);
            return true;
        }
        false
    }

    pub fn record_tick(&mut self, elapsed: Duration, stats: &TickStats) {
        if self.running.is_some() {
            self.tick_ms.push(elapsed.as_secs_f64() * 1000.0);
            self.max_active = self.max_active.max(stats.active_chunks);
            self.max_moves = self.max_moves.max(stats.moves);
        }
    }

    /// Advances the clock; returns `true` when the benchmark is over.
    pub fn advance(&mut self, dt: Duration) -> bool {
        match self.running.as_mut() {
            Some(t) => {
                *t += dt;
                *t >= self.duration
            }
            None => false,
        }
    }

    pub fn report(&self) -> String {
        let mut ms = self.tick_ms.clone();
        if ms.is_empty() {
            return "bench-drop: no ticks recorded".to_owned();
        }
        ms.sort_by(f64::total_cmp);
        let pct = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
        format!(
            "bench-drop: {} ticks: avg {:.2} ms, p50 {:.2}, p95 {:.2}, max {:.2} ms/tick; max active chunks {}, max moves/tick {}",
            ms.len(),
            ms.iter().sum::<f64>() / ms.len() as f64,
            pct(0.5),
            pct(0.95),
            ms[ms.len() - 1],
            self.max_active,
            self.max_moves
        )
    }
}
