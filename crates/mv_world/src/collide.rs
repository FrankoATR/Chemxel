//! Moving a box through the voxel grid (the player, milestone 8, ADR 011).
//!
//! Positions are in microvoxels (`f64`). The box moves one axis at a time in steps
//! of at most half a voxel; a step that would overlap a solid voxel is undone and
//! the box is pushed flush against it. Moving horizontally into an obstacle no
//! taller than `step_up` lifts the box onto it (stairs and the 1-voxel terraces of
//! the terrain). Voxels in unloaded chunks count as solid, so nothing falls out of
//! the world while it streams in.

use glam::{BVec3, DVec3, I64Vec3};
use mv_core::{ChunkPos, WorldPos};

use crate::{Chunk, MaterialId, World};

/// Reads voxels with the last chunk cached.
#[derive(Debug)]
pub struct Probe<'w> {
    world: &'w World,
    last: Option<(ChunkPos, Option<&'w Chunk>)>,
}

impl<'w> Probe<'w> {
    pub fn new(world: &'w World) -> Self {
        Self { world, last: None }
    }

    /// Material at `p`, `None` if its chunk is not loaded.
    pub fn get(&mut self, p: I64Vec3) -> Option<MaterialId> {
        let (c, l) = WorldPos(p).to_chunk_local();
        let chunk = match self.last {
            Some((lc, ch)) if lc == c => ch,
            _ => {
                let ch = self.world.chunk(c);
                self.last = Some((c, ch));
                ch
            }
        };
        chunk.map(|ch| ch.get(l))
    }

    /// Whether any voxel overlapping the box `[min, max)` is blocked.
    pub fn box_blocked(
        &mut self,
        min: DVec3,
        max: DVec3,
        solid: &impl Fn(MaterialId) -> bool,
    ) -> bool {
        let lo = min.floor().as_i64vec3();
        let hi = (max - 1e-9).floor().as_i64vec3();
        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                for x in lo.x..=hi.x {
                    match self.get(I64Vec3::new(x, y, z)) {
                        None => return true,
                        Some(m) if solid(m) => return true,
                        Some(_) => {}
                    }
                }
            }
        }
        false
    }

    /// Whether any voxel overlapping the box `[min, max)` satisfies `pred`.
    pub fn box_any(&mut self, min: DVec3, max: DVec3, pred: &impl Fn(MaterialId) -> bool) -> bool {
        let lo = min.floor().as_i64vec3();
        let hi = (max - 1e-9).floor().as_i64vec3();
        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                for x in lo.x..=hi.x {
                    if self.get(I64Vec3::new(x, y, z)).is_some_and(pred) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Result of [`move_box`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moved {
    /// New minimum corner.
    pub min: DVec3,
    /// Axes on which the motion was stopped by a voxel.
    pub blocked: BVec3,
    /// Stopped while moving down: standing on something.
    pub on_ground: bool,
}

/// Moves the box with minimum corner `min` and extent `size` by `delta`.
pub fn move_box(
    world: &World,
    min: DVec3,
    size: DVec3,
    delta: DVec3,
    step_up: f64,
    solid: &impl Fn(MaterialId) -> bool,
) -> Moved {
    let mut probe = Probe::new(world);
    let mut pos = min;
    let mut blocked = [false; 3];
    let mut on_ground = false;
    let steps = (delta.abs().max_element() / 0.5).ceil().max(1.0) as u32;
    let d = delta / f64::from(steps);
    for _ in 0..steps {
        // Vertical first: falling onto a ledge before sliding over it.
        for axis in [1usize, 0, 2] {
            if blocked[axis] || d[axis] == 0.0 {
                continue;
            }
            let mut next = pos;
            next[axis] += d[axis];
            if !probe.box_blocked(next, next + size, solid) {
                pos = next;
                continue;
            }
            if axis != 1 && step_up > 0.0 {
                // Try climbing onto the obstacle, one voxel at a time.
                let mut lift = 1.0;
                let mut climbed = false;
                while lift <= step_up {
                    let mut up = next;
                    up.y = (pos.y + lift).floor().max(pos.y);
                    let above = pos + DVec3::Y * (up.y - pos.y);
                    if !probe.box_blocked(above, above + size, solid)
                        && !probe.box_blocked(up, up + size, solid)
                    {
                        pos = up;
                        climbed = true;
                        break;
                    }
                    lift += 1.0;
                }
                if climbed {
                    continue;
                }
            }
            // Flush against the voxel boundary.
            if d[axis] > 0.0 {
                pos[axis] = (pos[axis] + size[axis]).ceil() - size[axis];
                if probe.box_blocked(pos, pos + size, solid) {
                    pos[axis] -= 1.0;
                }
            } else {
                pos[axis] = pos[axis].floor();
                if probe.box_blocked(pos, pos + size, solid) {
                    pos[axis] += 1.0;
                }
            }
            blocked[axis] = true;
            if axis == 1 && d.y < 0.0 {
                on_ground = true;
            }
        }
    }
    Moved {
        min: pos,
        blocked: BVec3::from(blocked),
        on_ground,
    }
}

/// Pushes a box that overlaps solid voxels upwards until it is free (at most
/// `max_lift` voxels). Returns the new minimum corner, or `None` if still stuck.
pub fn unstick(
    world: &World,
    min: DVec3,
    size: DVec3,
    max_lift: f64,
    solid: &impl Fn(MaterialId) -> bool,
) -> Option<DVec3> {
    let mut probe = Probe::new(world);
    let mut lift = 0.0;
    while lift <= max_lift {
        let p = DVec3::new(min.x, min.y.floor() + lift, min.z);
        if !probe.box_blocked(p, p + size, solid) {
            return Some(if lift == 0.0 { min } else { p });
        }
        lift += 1.0;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::fill_box;

    const STONE: MaterialId = MaterialId(1);
    const WATER: MaterialId = MaterialId(2);

    fn solid(m: MaterialId) -> bool {
        m == STONE
    }

    /// Air chunks around the origin with a stone floor at y = 0.
    fn world() -> World {
        let mut w = World::new();
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    w.insert(ChunkPos::new(x, y, z), Chunk::uniform(MaterialId::AIR));
                }
            }
        }
        fill_box(
            &mut w,
            WorldPos::new(-64, 0, -64),
            WorldPos::new(127, 0, 127),
            STONE,
            |_| true,
        );
        w
    }

    const SIZE: DVec3 = DVec3::new(10.0, 29.0, 10.0);

    #[test]
    fn falls_onto_the_floor() {
        let w = world();
        let m = move_box(
            &w,
            DVec3::new(0.0, 10.0, 0.0),
            SIZE,
            DVec3::new(0.0, -50.0, 0.0),
            0.0,
            &solid,
        );
        assert_eq!(m.min.y, 1.0, "stands on top of the floor (y = 0 is solid)");
        assert!(m.on_ground && m.blocked.y);
    }

    #[test]
    fn walls_stop_and_steps_are_climbed() {
        let mut w = world();
        // A wall 40 voxels high at x = 20, and a 3-voxel step at z = 20.
        fill_box(
            &mut w,
            WorldPos::new(20, 1, -5),
            WorldPos::new(22, 40, 15),
            STONE,
            |_| true,
        );
        fill_box(
            &mut w,
            WorldPos::new(-5, 1, 20),
            WorldPos::new(15, 3, 40),
            STONE,
            |_| true,
        );
        let start = DVec3::new(0.0, 1.0, 0.0);
        let m = move_box(&w, start, SIZE, DVec3::new(30.0, 0.0, 0.0), 8.0, &solid);
        assert_eq!(m.min.x, 10.0, "flush against the wall");
        assert!(m.blocked.x);
        let m = move_box(&w, start, SIZE, DVec3::new(0.0, 0.0, 30.0), 8.0, &solid);
        assert_eq!(m.min.y, 4.0, "climbed onto the step");
        assert!((m.min.z - 30.0).abs() < 1e-9);
        // Without step-up the step blocks.
        let m = move_box(&w, start, SIZE, DVec3::new(0.0, 0.0, 30.0), 0.0, &solid);
        assert!(m.blocked.z && m.min.z < 11.0);
    }

    #[test]
    fn liquids_do_not_block_and_unloaded_chunks_do() {
        let mut w = world();
        fill_box(
            &mut w,
            WorldPos::new(-10, 1, -10),
            WorldPos::new(10, 10, 10),
            WATER,
            |_| true,
        );
        let m = move_box(
            &w,
            DVec3::new(0.0, 20.0, 0.0),
            SIZE,
            DVec3::new(0.0, -30.0, 0.0),
            0.0,
            &solid,
        );
        assert_eq!(m.min.y, 1.0, "sinks through water to the floor");
        assert!(Probe::new(&w).box_any(m.min, m.min + SIZE, &|x| x == WATER));
        // The loaded world ends at x = 127.
        let m = move_box(
            &w,
            DVec3::new(100.0, 1.0, 0.0),
            SIZE,
            DVec3::new(40.0, 0.0, 0.0),
            0.0,
            &solid,
        );
        assert!(m.blocked.x && m.min.x <= 118.0);
    }

    #[test]
    fn a_buried_box_is_pushed_out() {
        let mut w = world();
        fill_box(
            &mut w,
            WorldPos::new(-5, 1, -5),
            WorldPos::new(15, 4, 15),
            STONE,
            |_| true,
        );
        let p = unstick(&w, DVec3::new(0.0, 2.0, 0.0), SIZE, 16.0, &solid).unwrap();
        assert_eq!(p.y, 5.0);
        assert!(unstick(&w, DVec3::new(0.0, 2.0, 0.0), SIZE, 1.0, &solid).is_none());
    }
}
