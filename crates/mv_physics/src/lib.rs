//! Rigid bodies made of voxels, backed by `rapier3d` (PROMPT.md §6.6, ADR 009).
//!
//! The simulation cuts floating pieces out of the grid ([`mv_sim::VoxelBody`]);
//! here each becomes a dynamic rigid body with a voxel collider (parry `Voxels`, the
//! piece's own grid). The world around each body gets a static voxel collider built
//! from the grid (solids and powders), rebuilt when the body leaves that region;
//! collision groups make it touch only its own body, so overlapping regions do not
//! duplicate contacts. When a body has been still for a while it is written back
//! into the grid ("revoxelized") by inverse mapping, which leaves no holes after a
//! rotation.
//!
//! Units: rapier works in metres; the grid in microvoxels (16 per metre). Positions
//! are relative to a physics origin (re-centred whenever there are no bodies) so
//! `f32` stays precise anywhere in the infinite world.

use glam::{DVec3, I64Vec3, IVec3, Quat, Vec3};
use mv_core::WorldPos;
use mv_core::config::PhysicsConfig;
use mv_core::consts::VOXELS_PER_BLOCK;
use mv_materials::{MaterialId, MaterialRegistry, MatterState};
use mv_sim::{Simulation, VoxelBody};
use mv_world::{BrickRef, World};
use rapier3d::prelude as rp;

/// Edge of a voxel in metres.
const VOXEL: f32 = 1.0 / VOXELS_PER_BLOCK as f32;
/// Collision group shared by every body (bodies collide with each other).
const BODIES: u32 = 1;
/// Ticks after which a body's world collider is rebuilt even if the body stayed in
/// its region (the world may have changed around it).
const REBUILD_PERIOD: u32 = 60;
/// Max distance a body moves per substep, in metres (half a voxel).
const MAX_STEP: f32 = 0.5 * VOXEL;

/// A body in flight.
#[derive(Debug)]
pub struct Body {
    /// Stable id (for the renderer's caches).
    pub id: u64,
    /// The piece; its `origin` is where it was cut out (not updated in flight).
    pub voxels: VoxelBody,
    handle: rp::RigidBodyHandle,
    /// Static collider of the world around the body and its region (world voxels).
    world_collider: Option<(rp::ColliderHandle, I64Vec3, I64Vec3)>,
    /// Bit of the collision group pairing the body with its world collider.
    group: u32,
    still_ticks: u32,
    age_ticks: u32,
    ticks_since_rebuild: u32,
}

/// Where a body is: grid voxel `g` (centre at `g + 0.5`) is at world position
/// `position + rotation * (g + 0.5)`, in microvoxels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyPose {
    pub position: DVec3,
    pub rotation: Quat,
}

/// Counters, for the overlay and tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct PhysicsStats {
    pub bodies: usize,
    /// Since start.
    pub created: u64,
    pub settled: u64,
    /// Voxels written back into the grid / that found no free cell, since start.
    pub voxels_placed: u64,
    pub voxels_lost: u64,
    /// World colliders rebuilt in the last step.
    pub rebuilds: u32,
    /// Physics substeps in the last tick.
    pub substeps: u32,
    /// Wall time of the last step, in milliseconds.
    pub step_ms: f32,
}

pub struct Physics {
    world: rp::PhysicsWorld,
    config: PhysicsConfig,
    /// World voxel at the physics origin.
    origin: I64Vec3,
    bodies: Vec<Body>,
    next_id: u64,
    /// Per material: kg/m³, and whether world voxels of it collide (solid, powder).
    density: Vec<f32>,
    collidable: Vec<bool>,
    /// Per material: may a body voxel be written over it when put back (air,
    /// liquids, gases, energy).
    replaceable: Vec<bool>,
    stats: PhysicsStats,
}

impl std::fmt::Debug for Physics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Physics")
            .field("bodies", &self.bodies.len())
            .field("origin", &self.origin)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

#[inline]
fn v3(v: Vec3) -> rp::Vector {
    rp::Vector::new(v.x, v.y, v.z)
}

#[inline]
fn from_v3(v: rp::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

impl Physics {
    /// `dt` is the fixed step (one simulation tick).
    pub fn new(registry: &MaterialRegistry, config: &PhysicsConfig, dt: f32) -> Self {
        let mut world = rp::PhysicsWorld::new();
        world.gravity = rp::Vector::new(0.0, -config.gravity, 0.0);
        world.integration_parameters.dt = dt;
        let state = |m: MaterialId| registry.get(m).state;
        Self {
            world,
            config: config.clone(),
            origin: I64Vec3::ZERO,
            bodies: Vec::new(),
            next_id: 1,
            density: registry.iter().map(|(_, m)| m.density.max(1.0)).collect(),
            collidable: registry
                .iter()
                .map(|(id, _)| matches!(state(id), MatterState::Solid | MatterState::Powder))
                .collect(),
            replaceable: registry
                .iter()
                .map(|(id, _)| {
                    id.is_air()
                        || matches!(
                            state(id),
                            MatterState::Liquid | MatterState::Gas | MatterState::Energy
                        )
                })
                .collect(),
            stats: PhysicsStats::default(),
        }
    }

    pub fn stats(&self) -> PhysicsStats {
        PhysicsStats {
            bodies: self.bodies.len(),
            ..self.stats
        }
    }

    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    /// Pose of `body` in world microvoxels.
    pub fn pose(&self, body: &Body) -> BodyPose {
        let rb = &self.world.bodies[body.handle];
        let p = rb.position();
        let t = from_v3(p.translation);
        BodyPose {
            position: self.origin.as_dvec3() + t.as_dvec3() / f64::from(VOXEL),
            rotation: Quat::from_array(p.rotation.to_array()).normalize(),
        }
    }

    /// Puts every body back into the grid where it is now (before saving).
    pub fn settle_all(&mut self, world: &mut World, sim: &mut Simulation) {
        for piece in sim.take_detached() {
            self.add_body(piece);
        }
        while let Some(body) = self.bodies.pop() {
            self.remove(world, sim, body, true);
        }
    }

    /// Takes the pieces the simulation cut out, steps the bodies, and puts settled
    /// ones back into the grid.
    pub fn step(&mut self, world: &mut World, sim: &mut Simulation) {
        let start = std::time::Instant::now();
        self.step_inner(world, sim);
        self.stats.step_ms = start.elapsed().as_secs_f32() * 1000.0;
    }

    fn step_inner(&mut self, world: &mut World, sim: &mut Simulation) {
        let _span = tracing::trace_span!("physics_step", bodies = self.bodies.len()).entered();
        self.stats.rebuilds = 0;
        for piece in sim.take_detached() {
            if self.bodies.len() >= self.config.max_bodies as usize {
                // No room: put it back where it was.
                let pose = BodyPose {
                    position: piece.origin.0.as_dvec3(),
                    rotation: Quat::IDENTITY,
                };
                self.revoxelize(world, sim, &piece, pose, false);
                continue;
            }
            self.add_body(piece);
        }
        if self.bodies.is_empty() {
            return;
        }
        for i in 0..self.bodies.len() {
            self.update_world_collider(world, i);
        }
        self.substep();

        let dt = self.world.integration_parameters.dt;
        let max_age = (self.config.max_body_seconds / dt).ceil() as u32;
        let mut i = 0;
        while i < self.bodies.len() {
            let b = &mut self.bodies[i];
            let rb = &self.world.bodies[b.handle];
            let speed = from_v3(rb.linvel())
                .length()
                .max(from_v3(rb.angvel()).length());
            b.age_ticks += 1;
            b.still_ticks = if speed < self.config.settle_speed {
                b.still_ticks + 1
            } else {
                0
            };
            let fell_out = rb.translation().y < -4096.0 * VOXEL;
            if b.still_ticks >= self.config.settle_ticks || b.age_ticks >= max_age || fell_out {
                let body = self.bodies.swap_remove(i);
                self.remove(world, sim, body, !fell_out);
            } else {
                i += 1;
            }
        }
    }

    /// One tick, split into substeps so nothing moves more than [`MAX_STEP`] per
    /// substep: voxel colliders only produce contacts at their surface, so a body
    /// that jumps past it would fall through the ground. Speeds are capped to what
    /// `max_substeps` allows.
    fn substep(&mut self) {
        let dt = self.world.integration_parameters.dt;
        let max_n = self.config.max_substeps.max(1);
        let max_speed = MAX_STEP * max_n as f32 / dt;
        let mut fastest = 0.0f32;
        for b in &self.bodies {
            let rb = &mut self.world.bodies[b.handle];
            let v = from_v3(rb.linvel());
            let speed = v.length();
            if speed > max_speed {
                rb.set_linvel(v3(v * (max_speed / speed)), true);
            }
            // Rotation moves the far end of the body too.
            let reach = b.voxels.size.as_vec3().length() * VOXEL;
            let spin = from_v3(rb.angvel()).length() * reach;
            fastest = fastest.max(speed.min(max_speed) + spin);
        }
        let n = ((fastest * dt / MAX_STEP).ceil() as u32).clamp(1, max_n);
        self.world.integration_parameters.dt = dt / n as f32;
        for _ in 0..n {
            self.world.step();
        }
        self.world.integration_parameters.dt = dt;
        self.stats.substeps = n;
    }

    fn add_body(&mut self, piece: VoxelBody) {
        if self.bodies.is_empty() {
            self.origin = piece.origin.0;
        }
        let id = self.next_id;
        self.next_id += 1;
        let group = 1 + (id % 31) as u32;
        let coords: Vec<rp::IVector> = piece
            .filled()
            .map(|(g, _)| rp::IVector::new(g.x, g.y, g.z))
            .collect();
        let mass: f32 = piece
            .filled()
            .map(|(_, m)| self.density.get(m.index()).copied().unwrap_or(1000.0))
            .sum();
        let density = mass / piece.count.max(1) as f32;
        let translation = (piece.origin.0 - self.origin).as_vec3() * VOXEL;
        let rb = rp::RigidBodyBuilder::dynamic()
            .translation(v3(translation))
            .soft_ccd_prediction(4.0 * MAX_STEP)
            .build();
        let handle = self.world.insert_body(rb);
        let collider = rp::ColliderBuilder::voxels(rp::Vector::splat(VOXEL), &coords)
            .density(density)
            .friction(self.config.friction)
            .restitution(self.config.restitution)
            .collision_groups(groups(BODIES | (1 << group), BODIES | (1 << group)))
            .build();
        self.world.insert_collider(collider, Some(handle));
        tracing::debug!("body {id}: {} voxels", piece.count);
        self.bodies.push(Body {
            id,
            voxels: piece,
            handle,
            world_collider: None,
            group,
            still_ticks: 0,
            age_ticks: 0,
            ticks_since_rebuild: 0,
        });
        self.stats.created += 1;
    }

    /// World-voxel bounding box of a body at its current pose.
    fn world_aabb(&self, body: &Body) -> (I64Vec3, I64Vec3) {
        let pose = self.pose(body);
        let s = body.voxels.size.as_dvec3();
        let mut lo = DVec3::splat(f64::MAX);
        let mut hi = DVec3::splat(f64::MIN);
        for k in 0..8 {
            let corner = DVec3::new(
                if k & 1 == 0 { 0.0 } else { s.x },
                if k & 2 == 0 { 0.0 } else { s.y },
                if k & 4 == 0 { 0.0 } else { s.z },
            );
            let p = pose.position + pose.rotation.as_dquat() * corner;
            lo = lo.min(p);
            hi = hi.max(p);
        }
        (lo.floor().as_i64vec3(), hi.ceil().as_i64vec3())
    }

    /// (Re)builds the static collider of the world around body `i` if the body left
    /// its region or the world may have changed.
    fn update_world_collider(&mut self, world: &World, i: usize) {
        let (lo, hi) = self.world_aabb(&self.bodies[i]);
        let slack = 2;
        let b = &mut self.bodies[i];
        b.ticks_since_rebuild += 1;
        if let Some((_, rlo, rhi)) = b.world_collider
            && (lo - slack).cmpge(rlo).all()
            && (hi + slack).cmple(rhi).all()
            && b.ticks_since_rebuild < REBUILD_PERIOD
        {
            return;
        }
        let margin = i64::from(self.config.world_margin);
        let (rlo, rhi) = (lo - margin, hi + margin);
        let group = b.group;
        let old = b.world_collider.take().map(|(h, _, _)| h);
        b.ticks_since_rebuild = 0;
        if let Some(h) = old {
            self.world.remove_collider(h);
        }
        let coords = self.world_voxels(world, rlo, rhi);
        self.stats.rebuilds += 1;
        if coords.is_empty() {
            self.bodies[i].world_collider = None;
            return;
        }
        let collider = rp::ColliderBuilder::voxels(rp::Vector::splat(VOXEL), &coords)
            .friction(self.config.friction)
            .restitution(self.config.restitution)
            .collision_groups(groups(1 << group, BODIES))
            .build();
        let h = self.world.insert_collider(collider, None);
        self.bodies[i].world_collider = Some((h, rlo, rhi));
    }

    /// Collidable world voxels in `[lo, hi]`, relative to the physics origin.
    fn world_voxels(&self, world: &World, lo: I64Vec3, hi: I64Vec3) -> Vec<rp::IVector> {
        let _span = tracing::trace_span!("world_collider").entered();
        let mut out = Vec::new();
        let collidable = |m: MaterialId| self.collidable.get(m.index()).copied().unwrap_or(true);
        let (clo, chi) = (WorldPos(lo).chunk().0, WorldPos(hi).chunk().0);
        for cz in clo.z..=chi.z {
            for cy in clo.y..=chi.y {
                for cx in clo.x..=chi.x {
                    let cpos = mv_core::ChunkPos::new(cx, cy, cz);
                    let Some(chunk) = world.chunk(cpos) else {
                        continue;
                    };
                    let corigin = cpos.origin().0;
                    for b in 0..mv_core::consts::BRICKS_PER_CHUNK {
                        let bo =
                            corigin + mv_core::LocalPos::from_brick(b, 0).as_ivec3().as_i64vec3();
                        let (blo, bhi) = (bo.max(lo), (bo + 7).min(hi));
                        if blo.cmpgt(bhi).any() {
                            continue;
                        }
                        let brick = chunk.brick(b);
                        if let BrickRef::Uniform(m) = brick
                            && !collidable(m)
                        {
                            continue;
                        }
                        for z in blo.z..=bhi.z {
                            for y in blo.y..=bhi.y {
                                for x in blo.x..=bhi.x {
                                    let m = match brick {
                                        BrickRef::Uniform(m) => m,
                                        BrickRef::Dense(v) => {
                                            let l = (I64Vec3::new(x, y, z) - bo).as_ivec3();
                                            v[(l.x + l.y * 8 + l.z * 64) as usize]
                                        }
                                    };
                                    if collidable(m) {
                                        let r = I64Vec3::new(x, y, z) - self.origin;
                                        out.push(rp::IVector::new(
                                            r.x as i32, r.y as i32, r.z as i32,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    fn remove(&mut self, world: &mut World, sim: &mut Simulation, body: Body, put_back: bool) {
        let pose = self.pose(&body);
        if let Some((h, _, _)) = body.world_collider {
            self.world.remove_collider(h);
        }
        self.world.remove_body_with_colliders(body.handle, true);
        if put_back {
            self.revoxelize(world, sim, &body.voxels, pose, true);
        } else {
            self.stats.voxels_lost += body.voxels.count as u64;
        }
        self.stats.settled += 1;
    }

    /// Writes `piece` into the grid at `pose`: every world voxel whose centre falls
    /// inside a filled body voxel gets that material, if the cell is free (air,
    /// liquid, gas). `check` re-runs structural integrity there (the piece may be
    /// hanging).
    fn revoxelize(
        &mut self,
        world: &mut World,
        sim: &mut Simulation,
        piece: &VoxelBody,
        pose: BodyPose,
        check: bool,
    ) {
        let _span = tracing::trace_span!("revoxelize", voxels = piece.count).entered();
        let s = piece.size.as_dvec3();
        let mut lo = DVec3::splat(f64::MAX);
        let mut hi = DVec3::splat(f64::MIN);
        for k in 0..8 {
            let corner = DVec3::new(
                if k & 1 == 0 { 0.0 } else { s.x },
                if k & 2 == 0 { 0.0 } else { s.y },
                if k & 4 == 0 { 0.0 } else { s.z },
            );
            let p = pose.position + pose.rotation.as_dquat() * corner;
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let (lo, hi) = (lo.floor().as_i64vec3(), hi.ceil().as_i64vec3() - 1);
        let inv = pose.rotation.as_dquat().inverse();
        let mut placed = 0u64;
        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                for x in lo.x..=hi.x {
                    let c = I64Vec3::new(x, y, z);
                    let local = inv * (c.as_dvec3() + 0.5 - pose.position);
                    let g: IVec3 = local.floor().as_ivec3();
                    let m = piece.get(g);
                    if m.is_air() {
                        continue;
                    }
                    let free = world
                        .voxel(WorldPos(c))
                        .is_some_and(|t| self.replaceable.get(t.index()).copied().unwrap_or(false));
                    if free {
                        world.set_voxel(WorldPos(c), m);
                        placed += 1;
                    }
                }
            }
        }
        self.stats.voxels_placed += placed;
        self.stats.voxels_lost += (piece.count as u64).saturating_sub(placed);
        if placed > 0 && check {
            sim.voxels_placed(world, WorldPos(lo), WorldPos(hi));
        } else if placed > 0 {
            sim.voxels_restored(world, WorldPos(lo), WorldPos(hi));
        }
        tracing::debug!("revoxelized {placed} of {} voxels", piece.count);
    }
}

fn groups(memberships: u32, filter: u32) -> rp::InteractionGroups {
    rp::InteractionGroups::new(
        rp::Group::from_bits_retain(memberships),
        rp::Group::from_bits_retain(filter),
        rp::InteractionTestMode::And,
    )
}

#[cfg(test)]
mod tests;
