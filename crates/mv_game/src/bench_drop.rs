//! `--bench-drop` / `--bench-heat` / `--bench-fire`: simulation benchmarks in the
//! real game.
//!
//! Waits until the world around the spawn point is loaded, spawns a scenario on
//! the terrain below the camera (sand + water, lava on ice, or a forest fire), then
//! records every simulation tick for a while, logs the timings and asks the app to
//! exit.

use std::time::Duration;

use mv_sim::TickStats;

use crate::drop_test::Scenario;

#[derive(Debug)]
pub struct BenchDrop {
    pub scenario: Scenario,
    duration: Duration,
    running: Option<Duration>,
    tick_ms: Vec<f64>,
    max_active: usize,
    max_moves: u32,
    max_thermal: usize,
    max_transitions: u32,
    max_reactions: u32,
    max_flames: u32,
}

impl BenchDrop {
    pub fn new(scenario: Scenario, duration: Duration) -> Self {
        Self {
            scenario,
            duration,
            running: None,
            tick_ms: Vec::new(),
            max_active: 0,
            max_moves: 0,
            max_thermal: 0,
            max_transitions: 0,
            max_reactions: 0,
            max_flames: 0,
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
            self.max_thermal = self.max_thermal.max(stats.thermal_chunks);
            self.max_transitions = self.max_transitions.max(stats.transitions);
            self.max_reactions = self.max_reactions.max(stats.reactions);
            self.max_flames = self.max_flames.max(stats.flames);
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
            return format!("bench {:?}: no ticks recorded", self.scenario);
        }
        ms.sort_by(f64::total_cmp);
        let pct = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
        format!(
            "bench {:?}: {} ticks: avg {:.2} ms, p50 {:.2}, p95 {:.2}, p99 {:.2}, max {:.2} ms/tick ({} over 8 ms); max active chunks {}, max moves/tick {}, max thermal chunks {}, max phase changes/tick {}, max reactions/tick {}, max flames/tick {}",
            self.scenario,
            ms.len(),
            ms.iter().sum::<f64>() / ms.len() as f64,
            pct(0.5),
            pct(0.95),
            pct(0.99),
            ms[ms.len() - 1],
            ms.iter().filter(|&&t| t > 8.0).count(),
            self.max_active,
            self.max_moves,
            self.max_thermal,
            self.max_transitions,
            self.max_reactions,
            self.max_flames
        )
    }
}
