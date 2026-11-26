//! Spray, foam and sparks (ADR 015): short-lived particles spawned where the
//! simulation reports falling liquid landing or falling freely. Visual only: they
//! never touch the world, they just fly under gravity and die on contact.

use glam::{DVec3, Vec3};
use mv_render::{MaterialGpu, ParticleInstance};
use mv_sim::{Splash, SplashKind};
use mv_world::{MaterialId, World};

/// Gravity in voxels per second squared (16 voxels per metre).
const GRAVITY: f32 = 9.81 * 16.0;
/// Fraction of the speed lost per second to air drag.
const DRAG: f32 = 1.5;

#[derive(Debug, Clone, Copy)]
struct Particle {
    pos: DVec3,
    vel: Vec3,
    age: f32,
    life: f32,
    radius: f32,
    color: [f32; 4],
}

#[derive(Debug)]
pub struct Particles {
    list: Vec<Particle>,
    capacity: usize,
    /// Per material: spray colour (linear RGB, glowing for emissive liquids).
    colors: Vec<[f32; 4]>,
    instances: Vec<ParticleInstance>,
    rng: u64,
}

impl Particles {
    pub fn new(capacity: u32, materials: &[MaterialGpu]) -> Self {
        let mut p = Self {
            list: Vec::with_capacity(capacity as usize),
            capacity: capacity as usize,
            colors: Vec::new(),
            instances: Vec::with_capacity(capacity as usize),
            rng: 0x9E37_79B9_7F4A_7C15,
        };
        p.set_materials(materials);
        p
    }

    /// Spray colours from the material table: liquids whiten into foam, emissive
    /// ones (lava) throw glowing sparks.
    pub fn set_materials(&mut self, materials: &[MaterialGpu]) {
        self.colors = materials
            .iter()
            .map(|m| {
                let [r, g, b, _] = m.color;
                let emissive = m.params[0];
                if emissive > 0.0 {
                    let k = 1.0 + emissive;
                    [r * k, g * k, b * k, 0.9]
                } else {
                    let foam = |c: f32| c + (1.0 - c) * 0.75;
                    [foam(r), foam(g), foam(b), 0.7]
                }
            })
            .collect();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    fn random(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / 16_777_216.0
    }

    fn signed(&mut self) -> f32 {
        self.random() * 2.0 - 1.0
    }

    /// Spawns the particles of this frame's splash events.
    pub fn spawn(&mut self, splashes: &[Splash]) {
        for s in splashes {
            let color = self
                .colors
                .get(s.material.index())
                .copied()
                .unwrap_or([1.0, 1.0, 1.0, 0.7]);
            let base = s.pos.as_dvec3() + DVec3::new(0.5, 0.5, 0.5);
            let (count, speed, up, life, radius, alpha) = match s.kind {
                SplashKind::Impact => (3, 45.0, 40.0, 0.6, 0.45, 1.0),
                SplashKind::Falling => (1, 8.0, -20.0, 0.4, 0.55, 0.5),
            };
            for _ in 0..count {
                if self.list.len() >= self.capacity {
                    return;
                }
                let dir = Vec3::new(self.signed(), 0.0, self.signed()).normalize_or_zero();
                let vel =
                    dir * speed * (0.4 + self.random()) + Vec3::Y * up * (0.5 + self.random());
                let offset =
                    DVec3::new(f64::from(self.signed()), 0.0, f64::from(self.signed())) * 0.4;
                let life = life * (0.6 + 0.8 * self.random());
                let radius = radius * (0.7 + 0.6 * self.random());
                self.list.push(Particle {
                    pos: base + offset,
                    vel,
                    age: 0.0,
                    life,
                    radius,
                    color: [color[0], color[1], color[2], color[3] * alpha],
                });
            }
        }
    }

    /// Moves the particles `dt` seconds; those that age out or fly into anything
    /// but air die.
    pub fn update(&mut self, dt: f32, world: &World) {
        let _span = tracing::trace_span!("particles", n = self.list.len()).entered();
        let drag = (1.0 - DRAG * dt).max(0.0);
        self.list.retain_mut(|p| {
            p.age += dt;
            p.vel.y -= GRAVITY * dt;
            p.vel *= drag;
            p.pos += p.vel.as_dvec3() * f64::from(dt);
            p.age < p.life
                && world
                    .voxel(mv_core::WorldPos(p.pos.floor().as_i64vec3()))
                    .is_some_and(MaterialId::is_air)
        });
    }

    /// The particles to draw, fading out with age.
    pub fn instances(&mut self) -> &[ParticleInstance] {
        self.instances.clear();
        self.instances.extend(self.list.iter().map(|p| {
            let fade = 1.0 - p.age / p.life;
            ParticleInstance {
                position: p.pos,
                radius: p.radius,
                color: [p.color[0], p.color[1], p.color[2], p.color[3] * fade],
            }
        }));
        &self.instances
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water() -> MaterialGpu {
        MaterialGpu::from_srgba([40, 90, 200, 180], 0.0)
    }

    #[test]
    fn splashes_spawn_particles_that_fall_and_die() {
        let mut world = World::new();
        // Only air is loaded around: an all-air chunk.
        world.insert(
            mv_core::ChunkPos::new(0, 0, 0),
            mv_world::Chunk::uniform(MaterialId::AIR),
        );
        let mut p = Particles::new(100, &[MaterialGpu::from_srgba([0, 0, 0, 0], 0.0), water()]);
        let splash = |kind| Splash {
            pos: glam::I64Vec3::new(32, 32, 32),
            kind,
            material: MaterialId(1),
        };
        p.spawn(&[splash(SplashKind::Impact), splash(SplashKind::Falling)]);
        assert_eq!(p.len(), 4);
        // Foam is lighter than the water it comes from.
        let first = p.instances()[0];
        assert!(first.color[2] > water().color[2]);
        let y0: f64 = p.list.iter().map(|q| q.pos.y).sum();
        for _ in 0..10 {
            p.update(1.0 / 30.0, &world);
        }
        assert!(p.list.iter().map(|q| q.pos.y).sum::<f64>() < y0 + 40.0);
        for _ in 0..60 {
            p.update(1.0 / 30.0, &world);
        }
        assert_eq!(p.len(), 0, "they all age out");
        // The capacity is respected.
        let many: Vec<Splash> = (0..100).map(|_| splash(SplashKind::Impact)).collect();
        p.spawn(&many);
        assert_eq!(p.len(), 100);
    }
}
