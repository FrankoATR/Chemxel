//! `--bench-fly`: reproducible streaming benchmark in the real game.
//!
//! Waits until the region around the spawn point is fully loaded, then flies the
//! camera in a straight line at constant speed and altitude (10 m above the spawn, or
//! above the highest ground along the route) while recording every frame time. At the end it logs percentiles and stutter counts and asks the app
//! to exit. Use with `vsync: false` to measure the real frame cost.

use std::time::Duration;

use glam::DVec3;
use mv_render::Camera;

use crate::game_world::SyncStats;

#[derive(Debug)]
pub struct BenchFly {
    /// Microvoxels per second.
    speed: f64,
    duration: Duration,
    flying: Option<Duration>,
    frame_ms: Vec<f32>,
    max_pending_uploads: usize,
    max_missing: usize,
    start: DVec3,
}

impl BenchFly {
    pub fn new(speed: f64, duration: Duration) -> Self {
        Self {
            speed,
            duration,
            flying: None,
            frame_ms: Vec::with_capacity(1 << 16),
            max_pending_uploads: 0,
            max_missing: 0,
            start: DVec3::ZERO,
        }
    }

    /// Drives the camera for one frame. `world_ready` tells whether the initial
    /// region finished loading; `sync` is the previous frame's streaming state.
    /// Returns `true` once the benchmark is over.
    pub fn step(
        &mut self,
        camera: &mut Camera,
        dt: Duration,
        world_ready: bool,
        sync: &SyncStats,
        ground: &dyn Fn(i64, i64) -> i32,
    ) -> bool {
        let Some(elapsed) = self.flying.as_mut() else {
            if world_ready {
                tracing::info!("bench-fly: world loaded, flying for {:?}", self.duration);
                self.flying = Some(Duration::ZERO);
                // 10 m above the spawn point (or above the highest ground of the
                // route, so it never flies through a hill), looking ahead and
                // slightly down.
                let length = self.speed * self.duration.as_secs_f64();
                let (x0, z) = (camera.position.x as i64, camera.position.z as i64);
                let top = (0..=(length as i64 / 32))
                    .map(|i| ground(x0 + i * 32, z))
                    .max()
                    .unwrap_or(i32::MIN);
                self.start = camera.position;
                self.start.y = (camera.position.y + 160.0).max(f64::from(top) + 160.0);
                camera.yaw = -std::f32::consts::FRAC_PI_2; // look along +X
                camera.pitch = -0.25;
            }
            return false;
        };
        *elapsed += dt;
        self.frame_ms.push(dt.as_secs_f32() * 1000.0);
        self.max_pending_uploads = self.max_pending_uploads.max(sync.pending_uploads);
        self.max_missing = self
            .max_missing
            .max(sync.stream.missing + sync.stream.inflight);
        camera.position = self.start + DVec3::X * self.speed * elapsed.as_secs_f64();
        *elapsed >= self.duration
    }

    /// Human-readable summary of the run.
    pub fn report(&self) -> String {
        let mut ms = self.frame_ms.clone();
        if ms.is_empty() {
            return "bench-fly: no frames recorded".to_owned();
        }
        ms.sort_by(f32::total_cmp);
        let pct = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
        let median = pct(0.5);
        let stutters = self
            .frame_ms
            .iter()
            .filter(|&&t| t > (2.0 * median).max(1000.0 / 60.0))
            .count();
        let distance = self.speed * self.duration.as_secs_f64();
        format!(
            "bench-fly: {} frames over {:.0} m ({:.1} chunks/s): avg {:.2} ms, p50 {:.2}, p95 {:.2}, p99 {:.2}, max {:.2} ms; {} frames > max(2x p50, 16.7 ms); max not-yet-loaded chunks {}, max pending uploads {}",
            ms.len(),
            distance / mv_core::consts::VOXELS_PER_BLOCK as f64,
            self.speed / mv_core::consts::CHUNK_SIZE as f64,
            ms.iter().sum::<f32>() / ms.len() as f32,
            median,
            pct(0.95),
            pct(0.99),
            ms[ms.len() - 1],
            stutters,
            self.max_missing,
            self.max_pending_uploads,
        )
    }
}
