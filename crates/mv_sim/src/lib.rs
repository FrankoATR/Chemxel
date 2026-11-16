//! Headless voxel simulation.
//!
//! This crate must never depend on rendering or windowing: it has to run in tests
//! and, in the future, on a dedicated server. See `PROMPT.md` §6 for the tick order.

/// Owns the simulation state and advances it one fixed tick at a time.
#[derive(Debug, Default)]
pub struct Simulation {
    tick: u64,
}

impl Simulation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances the simulation by exactly one fixed tick.
    pub fn step(&mut self) {
        let _span = tracing::trace_span!("sim_step", tick = self.tick).entered();
        self.tick += 1;
    }

    /// Number of ticks simulated so far.
    pub fn tick(&self) -> u64 {
        self.tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_advances_tick() {
        let mut sim = Simulation::new();
        sim.step();
        sim.step();
        assert_eq!(sim.tick(), 2);
    }
}
