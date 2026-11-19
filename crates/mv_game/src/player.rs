//! The walking player (milestone 8, ADR 011).
//!
//! A box of `player_width × player_height` (feet at the bottom centre) moved by
//! [`mv_world::collide::move_box`]: gravity, jumping, automatic steps up to
//! `step_height`, slow swimming in liquids. Solids and powders block; liquids and
//! gases do not. Flying mode (toggled with `F`) is the old free camera, without
//! collision.

use glam::{DVec3, Vec3};
use mv_core::config::ControlsConfig;
use mv_core::consts::VOXELS_PER_BLOCK;
use mv_materials::{MaterialId, MaterialRegistry, MatterState};
use mv_render::Camera;
use mv_world::World;
use mv_world::collide::{Probe, move_box, unstick};
use winit::keyboard::KeyCode;

use crate::input::InputState;

const M: f64 = VOXELS_PER_BLOCK as f64;

#[derive(Debug, Clone)]
pub struct Player {
    /// Bottom centre of the box, in microvoxels.
    pub feet: DVec3,
    velocity: DVec3,
    pub walking: bool,
    pub on_ground: bool,
    size: DVec3,
    eye: f64,
    walk_speed: f64,
    run_multiplier: f64,
    jump_speed: f64,
    gravity: f64,
    step: f64,
    solid: Vec<bool>,
    liquid: Vec<bool>,
}

impl Player {
    pub fn new(controls: &ControlsConfig, registry: &MaterialRegistry, feet: DVec3) -> Self {
        let gravity = f64::from(controls.gravity) * M;
        Self {
            feet,
            velocity: DVec3::ZERO,
            walking: controls.start_walking,
            on_ground: false,
            size: DVec3::new(
                f64::from(controls.player_width),
                f64::from(controls.player_height),
                f64::from(controls.player_width),
            ) * M,
            eye: f64::from(controls.eye_height) * M,
            walk_speed: f64::from(controls.walk_speed) * M,
            run_multiplier: f64::from(controls.fast_multiplier).min(2.0),
            // v = sqrt(2 g h)
            jump_speed: (2.0 * gravity * f64::from(controls.jump_height) * M).sqrt(),
            gravity,
            step: (f64::from(controls.step_height) * M).floor(),
            solid: registry
                .iter()
                .map(|(_, m)| matches!(m.state, MatterState::Solid | MatterState::Powder))
                .collect(),
            liquid: registry
                .iter()
                .map(|(_, m)| m.state == MatterState::Liquid)
                .collect(),
        }
    }

    /// Updates the material tables after a hot reload.
    pub fn set_materials(&mut self, registry: &MaterialRegistry) {
        let other = Self::new(&ControlsConfig::default(), registry, self.feet);
        self.solid = other.solid;
        self.liquid = other.liquid;
    }

    pub fn eye_position(&self) -> DVec3 {
        self.feet + DVec3::Y * self.eye
    }

    /// Puts the feet under a camera at `eye` (when switching from flying).
    pub fn set_eye_position(&mut self, eye: DVec3) {
        self.feet = eye - DVec3::Y * self.eye;
        self.velocity = DVec3::ZERO;
    }

    fn min_corner(&self) -> DVec3 {
        self.feet - DVec3::new(self.size.x * 0.5, 0.0, self.size.z * 0.5)
    }

    /// Switches between walking and flying; when landing inside something, steps
    /// out of it upwards.
    pub fn toggle_walking(&mut self, world: &World) {
        self.walking = !self.walking;
        self.velocity = DVec3::ZERO;
        if self.walking {
            self.unstick(world);
        }
    }

    /// Lifts the player out of solid voxels (e.g. after placing material on it).
    pub fn unstick(&mut self, world: &World) {
        let solid = |m: MaterialId| self.solid.get(m.index()).copied().unwrap_or(true);
        if let Some(min) = unstick(world, self.min_corner(), self.size, 64.0, &solid) {
            self.feet.y = min.y;
        }
    }

    /// One frame of walking for `dt` seconds. Movement keys are relative to the
    /// camera's heading. Does nothing until the ground under the player is loaded.
    pub fn walk(&mut self, world: &World, input: &InputState, camera: &Camera, dt: f32) {
        let dt = f64::from(dt.min(0.1));
        let feet_chunk = mv_core::WorldPos(self.feet.floor().as_i64vec3()).chunk();
        if !world.contains(feet_chunk) {
            return;
        }
        let solid = |m: MaterialId| self.solid.get(m.index()).copied().unwrap_or(true);
        let liquid = |m: MaterialId| self.liquid.get(m.index()).copied().unwrap_or(false);
        let min = self.min_corner();
        let swimming = Probe::new(world).box_any(min, min + self.size, &liquid);

        let axis = |pos: KeyCode, neg: KeyCode| {
            f32::from(u8::from(input.is_held(pos))) - f32::from(u8::from(input.is_held(neg)))
        };
        let wish = camera.flat_forward() * axis(KeyCode::KeyW, KeyCode::KeyS)
            + camera.right() * axis(KeyCode::KeyD, KeyCode::KeyA);
        let run = input.is_held(KeyCode::ShiftLeft) || input.is_held(KeyCode::ShiftRight);
        let mut speed = self.walk_speed * if run { self.run_multiplier } else { 1.0 };
        if swimming {
            speed *= 0.5;
        }
        let horizontal = if wish == Vec3::ZERO {
            DVec3::ZERO
        } else {
            wish.normalize().as_dvec3() * speed
        };
        self.velocity.x = horizontal.x;
        self.velocity.z = horizontal.z;

        let jump = input.is_held(KeyCode::Space);
        if swimming {
            // Sink slowly, swim up with Space.
            self.velocity.y -= self.gravity * 0.25 * dt;
            if jump {
                self.velocity.y = 0.25 * self.jump_speed;
            }
            self.velocity.y = self.velocity.y.max(-0.15 * self.jump_speed);
        } else {
            if jump && self.on_ground {
                self.velocity.y = self.jump_speed;
            }
            self.velocity.y -= self.gravity * dt;
        }
        let step = if self.on_ground || swimming {
            self.step
        } else {
            0.0
        };
        let moved = move_box(world, min, self.size, self.velocity * dt, step, &solid);
        self.feet += moved.min - min;
        if moved.blocked.y {
            self.velocity.y = 0.0;
        }
        self.on_ground = moved.on_ground;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mv_core::{ChunkPos, WorldPos};
    use mv_world::Chunk;
    use mv_world::edit::fill_box;

    fn setup() -> (World, Player, InputState, Camera) {
        let registry = MaterialRegistry::from_sources([(
            "m.ron",
            r#"[
                Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
                Material(name: "stone", state: Solid,  density: 2600, color: (1, 1, 1, 255)),
                Material(name: "water", state: Liquid, density: 1000, color: (2, 2, 2, 255)),
            ]"#,
        )])
        .unwrap();
        let mut world = World::new();
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    world.insert(ChunkPos::new(x, y, z), Chunk::uniform(MaterialId::AIR));
                }
            }
        }
        fill_box(
            &mut world,
            WorldPos::new(-64, 0, -64),
            WorldPos::new(127, 0, 127),
            MaterialId(1),
            |_| true,
        );
        let controls = ControlsConfig::default();
        let player = Player::new(&controls, &registry, DVec3::new(0.5, 30.0, 0.5));
        let camera = Camera::new(DVec3::ZERO, 0.0, 0.0, 1.2);
        (world, player, InputState::default(), camera)
    }

    #[test]
    fn falls_lands_and_jumps() {
        let (world, mut p, mut input, camera) = setup();
        assert!(p.walking);
        for _ in 0..120 {
            p.walk(&world, &input, &camera, 1.0 / 60.0);
        }
        assert_eq!(p.feet.y, 1.0, "standing on the floor");
        assert!(p.on_ground);
        input.set_key(KeyCode::Space, true);
        p.walk(&world, &input, &camera, 1.0 / 60.0);
        input.set_key(KeyCode::Space, false);
        let mut top: f64 = 0.0;
        for _ in 0..60 {
            p.walk(&world, &input, &camera, 1.0 / 60.0);
            top = top.max(p.feet.y);
        }
        let jump_m = (top - 1.0) / M;
        assert!((0.9..1.4).contains(&jump_m), "jumped {jump_m:.2} m");
        assert_eq!(p.feet.y, 1.0, "and landed again");
    }

    #[test]
    fn walks_forward_and_climbs_terrain_steps() {
        let (mut world, mut p, mut input, camera) = setup();
        // A staircase of 1-voxel terraces ahead (camera looks towards -z).
        for i in 0..6 {
            fill_box(
                &mut world,
                WorldPos::new(-20, 1, -20 - 4 * i),
                WorldPos::new(20, 1 + i, -60),
                MaterialId(1),
                |_| true,
            );
        }
        for _ in 0..120 {
            p.walk(&world, &input, &camera, 1.0 / 60.0);
        }
        input.set_key(KeyCode::KeyW, true);
        for _ in 0..90 {
            p.walk(&world, &input, &camera, 1.0 / 60.0);
        }
        assert!(p.feet.z < -40.0, "walked forward: z = {}", p.feet.z);
        assert!(p.feet.y >= 6.0, "climbed the terraces: y = {}", p.feet.y);
    }

    #[test]
    fn placed_material_does_not_trap_the_player() {
        let (mut world, mut p, input, camera) = setup();
        for _ in 0..60 {
            p.walk(&world, &input, &camera, 1.0 / 60.0);
        }
        fill_box(
            &mut world,
            WorldPos::new(-10, 1, -10),
            WorldPos::new(10, 5, 10),
            MaterialId(1),
            |_| true,
        );
        p.unstick(&world);
        assert_eq!(p.feet.y, 6.0);
    }
}
