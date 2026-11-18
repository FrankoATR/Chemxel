//! Rigid bodies end to end: integrity cuts a piece out, rapier drops it, and it is
//! written back into the grid.

use mv_core::config::{PhysicsConfig, SimConfig};
use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry, ReactionSet};
use mv_world::edit::fill_box;
use mv_world::{Chunk, World};

use super::*;

const MATERIALS: &str = r#"[
    Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
    Material(name: "stone", state: Solid,  density: 2600, color: (1, 1, 1, 255)),
    Material(name: "wood",  state: Solid,  density: 700,  color: (2, 2, 2, 255)),
    Material(name: "water", state: Liquid, density: 1000, color: (3, 3, 3, 255), dispersion: 3),
]"#;

const DT: f32 = 1.0 / 30.0;

struct Fixture {
    world: World,
    sim: Simulation,
    physics: Physics,
    registry: MaterialRegistry,
}

fn w(x: i64, y: i64, z: i64) -> WorldPos {
    WorldPos::new(x, y, z)
}

/// Air chunks `x, z in -1..=1`, `y in -1..=2`, stone floor at `y = 0`; the ground
/// (y <= 0) is the anchor.
fn fixture(physics: PhysicsConfig) -> Fixture {
    let registry = MaterialRegistry::from_sources([("m.ron", MATERIALS)]).unwrap();
    let mut world = World::new();
    for z in -1..=1 {
        for y in -1..=2 {
            for x in -1..=1 {
                world.insert(ChunkPos::new(x, y, z), Chunk::uniform(MaterialId::AIR));
            }
        }
    }
    let stone = registry.id("stone").unwrap();
    fill_box(&mut world, w(-64, -8, -64), w(127, 0, 127), stone, |_| true);
    let config = SimConfig {
        integrity_min_voxels: 1,
        ..SimConfig::default()
    };
    let mut sim = Simulation::new(&registry, &ReactionSet::empty(&registry), &config);
    sim.set_anchor(Some(Box::new(|p: WorldPos| p.0.y <= 0)));
    Fixture {
        physics: Physics::new(&registry, &physics, DT),
        sim,
        world,
        registry,
    }
}

impl Fixture {
    fn put(&mut self, min: WorldPos, max: WorldPos, name: &str) {
        let m = self.registry.id(name).unwrap();
        fill_box(&mut self.world, min, max, m, |_| true);
        self.sim.edited(&self.world, min, max, Some(m));
    }

    fn dig(&mut self, min: WorldPos, max: WorldPos) {
        fill_box(&mut self.world, min, max, MaterialId::AIR, |_| true);
        self.sim.edited(&self.world, min, max, None);
    }

    fn tick(&mut self) {
        self.sim.step(&mut self.world);
        self.physics.step(&mut self.world, &mut self.sim);
    }

    /// Ticks until no body is left (or `max` ticks); returns the ticks run.
    fn settle(&mut self, max: u32) -> u32 {
        for t in 0..max {
            self.tick();
            if t > 2 && self.physics.bodies().is_empty() && self.sim.integrity_pending() == 0 {
                return t;
            }
        }
        max
    }

    fn count(&self, name: &str, min: WorldPos, max: WorldPos) -> usize {
        let m = self.registry.id(name).unwrap();
        let mut n = 0;
        for z in min.0.z..=max.0.z {
            for y in min.0.y..=max.0.y {
                for x in min.0.x..=max.0.x {
                    n += usize::from(self.world.voxel(w(x, y, z)) == Some(m));
                }
            }
        }
        n
    }
}

/// PROMPT.md milestone 6 acceptance: digging out the base of a tower makes it fall
/// as one block, and once it hits the ground it is part of the world again.
#[test]
fn a_tower_with_its_base_dug_out_falls_as_a_block_and_rejoins_the_world() {
    let mut f = fixture(PhysicsConfig::default());
    f.put(w(10, 1, 10), w(17, 40, 17), "stone");
    f.dig(w(10, 1, 10), w(17, 4, 17));
    let falling = 8 * 36 * 8;

    f.tick();
    assert_eq!(f.physics.bodies().len(), 1, "the tower becomes one body");
    assert_eq!(f.physics.bodies()[0].voxels.count, falling);
    let start = f.physics.pose(&f.physics.bodies()[0]).position;
    // Let it fall for a moment: it moves down as a whole.
    for _ in 0..5 {
        f.tick();
    }
    if let Some(b) = f.physics.bodies().first() {
        let now = f.physics.pose(b).position;
        assert!(now.y < start.y - 0.5, "it falls: {start} -> {now}");
        let tilt = f.physics.pose(b).rotation.angle_between(Quat::IDENTITY);
        assert!(tilt < 0.1, "it falls straight (tilt {tilt})");
    }

    let ticks = f.settle(600);
    eprintln!("tower settled after {ticks} ticks: {:?}", f.physics.stats());
    assert!(ticks < 600, "the body comes to rest and is put back");
    let s = f.physics.stats();
    assert_eq!((s.created, s.settled), (1, 1));
    // Back in the grid, standing on the ground where the base was.
    let stone = f.count("stone", w(4, 1, 4), w(23, 45, 23));
    let lost = falling as i64 - stone as i64;
    assert!(
        lost.abs() <= (falling / 50) as i64,
        "voxels conserved within 2%: {stone} of {falling}"
    );
    assert!(
        f.count("stone", w(10, 1, 10), w(17, 2, 17)) > 100,
        "it reached the ground"
    );
    assert_eq!(
        f.count("stone", w(10, 38, 10), w(17, 40, 17)),
        0,
        "and is lower than before"
    );
    // Standing on the ground again: not detached a second time.
    for _ in 0..5 {
        f.tick();
    }
    assert!(f.physics.bodies().is_empty());
}

#[test]
fn an_overhanging_piece_tips_over_its_support() {
    let mut f = fixture(PhysicsConfig::default());
    // A beam on a short pillar, with most of its length hanging to the right, held
    // by a thin column at the far end. Cutting the column lets it fall and tip.
    f.put(w(0, 1, 0), w(3, 6, 3), "stone"); // pillar (left)
    f.put(w(0, 7, 0), w(40, 10, 3), "wood"); // beam
    f.put(w(39, 1, 0), w(40, 6, 3), "stone"); // column (right)
    // The column now hangs from the beam, which the pillar holds: nothing falls...
    f.dig(w(39, 1, 0), w(40, 1, 3));
    f.tick();
    assert!(f.physics.bodies().is_empty(), "the beam holds the column");
    // ...but cutting the beam next to the pillar does, and the beam tips: its right
    // end ends up near the ground.
    f.dig(w(4, 7, 0), w(4, 10, 3));
    f.tick();
    assert_eq!(f.physics.bodies().len(), 1);
    let mut max_tilt = 0.0f32;
    for _ in 0..900 {
        f.tick();
        if let Some(b) = f.physics.bodies().first() {
            max_tilt = max_tilt.max(f.physics.pose(b).rotation.angle_between(Quat::IDENTITY));
        } else {
            break;
        }
    }
    eprintln!("beam: max tilt {max_tilt:.2} rad, {:?}", f.physics.stats());
    assert!(max_tilt > 0.1, "the beam tips over its support");
    assert!(f.physics.bodies().is_empty());
    assert!(
        f.count("wood", w(5, 1, -10), w(50, 12, 14)) > 400,
        "the beam is back in the grid"
    );
    assert!(
        f.count("wood", w(5, 1, -10), w(50, 6, 14)) > 0,
        "it fell to the ground"
    );
}

#[test]
fn pieces_beyond_the_body_limit_stay_in_place() {
    let config = PhysicsConfig {
        max_bodies: 1,
        ..PhysicsConfig::default()
    };
    let mut f = fixture(config);
    f.put(w(10, 1, 10), w(11, 10, 11), "stone");
    f.put(w(30, 1, 10), w(31, 10, 11), "wood");
    f.dig(w(10, 1, 10), w(31, 2, 11));
    f.tick();
    assert_eq!(f.physics.bodies().len(), 1);
    // The second piece was put back where it was, and is not cut out again.
    let restored = f.count("stone", w(10, 3, 10), w(11, 10, 11))
        + f.count("wood", w(30, 3, 10), w(31, 10, 11));
    assert_eq!(restored, 2 * 2 * 8, "one piece stays in place");
    f.tick();
    assert_eq!(f.physics.stats().created, 1);
}

#[test]
fn a_body_falls_through_water_and_lands_on_the_bottom() {
    let mut f = fixture(PhysicsConfig::default());
    // A pool, and a block hanging above it from a thin rod.
    f.put(w(0, 1, -10), w(20, 1, 30), "stone");
    f.put(w(8, 2, 8), w(14, 8, 14), "water");
    f.put(w(9, 20, 9), w(13, 24, 13), "stone");
    f.put(w(30, 1, 11), w(30, 26, 11), "stone");
    f.put(w(13, 25, 11), w(30, 26, 11), "stone");
    f.dig(w(14, 25, 11), w(14, 26, 11));
    f.tick();
    assert!(!f.physics.bodies().is_empty());
    f.settle(900);
    assert!(f.physics.bodies().is_empty());
    assert!(
        f.count("stone", w(8, 2, 8), w(14, 8, 14)) > 50,
        "the block sank to the bottom of the pool"
    );
}
