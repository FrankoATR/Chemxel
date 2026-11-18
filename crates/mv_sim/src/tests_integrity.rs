//! Structural integrity: flood fill and extraction of floating pieces (milestone 6).

use mv_core::config::SimConfig;
use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry, ReactionSet};
use mv_world::edit::fill_box;
use mv_world::{Chunk, World};

use super::*;

const MATERIALS: &str = r#"[
    Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
    Material(name: "stone", state: Solid,  density: 2600, color: (1, 1, 1, 255)),
    Material(name: "wood",  state: Solid,  density: 700,  color: (2, 2, 2, 255)),
    Material(name: "sand",  state: Powder, density: 1600, color: (3, 3, 3, 255)),
]"#;

struct Fixture {
    world: World,
    sim: Simulation,
    registry: MaterialRegistry,
}

fn w(x: i64, y: i64, z: i64) -> WorldPos {
    WorldPos::new(x, y, z)
}

/// Air chunks `x, z in -1..=1`, `y in -1..=2`, stone floor at `y = 0` (it reaches
/// the unloaded border, so it is anchored).
fn fixture(config: SimConfig) -> Fixture {
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
    fill_box(&mut world, w(-64, 0, -64), w(127, 0, 127), stone, |_| true);
    // Exact flood fill: every floating piece detaches, however small (the minimum
    // size has its own test).
    let config = SimConfig {
        integrity_min_voxels: config.integrity_min_voxels.min(1),
        ..config
    };
    Fixture {
        sim: Simulation::new(&registry, &ReactionSet::empty(&registry), &config),
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

    /// Removes like the editor does (queues an integrity check).
    fn dig(&mut self, min: WorldPos, max: WorldPos) {
        fill_box(&mut self.world, min, max, MaterialId::AIR, |_| true);
        self.sim.edited(&self.world, min, max, None);
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.sim.step(&mut self.world);
        }
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

#[test]
fn cutting_the_base_of_a_tower_detaches_it_whole() {
    let mut f = fixture(SimConfig::default());
    f.put(w(10, 1, 10), w(17, 40, 17), "stone");
    f.dig(w(10, 1, 10), w(17, 3, 17));
    f.run(1);
    let bodies = f.sim.take_detached();
    assert_eq!(bodies.len(), 1);
    let b = &bodies[0];
    assert_eq!(b.count, 8 * 37 * 8);
    assert_eq!(b.origin, w(10, 4, 10));
    assert_eq!(b.size, glam::IVec3::new(8, 37, 8));
    assert_eq!(
        f.count("stone", w(10, 1, 10), w(17, 40, 17)),
        0,
        "cut out of the grid"
    );
    assert_eq!(f.sim.stats().detached, 1);
}

#[test]
fn a_partial_cut_keeps_the_tower_standing() {
    let mut f = fixture(SimConfig::default());
    f.put(w(10, 1, 10), w(17, 40, 17), "stone");
    // Everything but one column of the base.
    f.dig(w(10, 1, 10), w(17, 3, 16));
    f.run(2);
    assert!(f.sim.take_detached().is_empty());
    assert_eq!(f.count("stone", w(10, 4, 10), w(17, 40, 17)), 8 * 37 * 8);
}

#[test]
fn an_overhang_connected_to_the_ground_stays() {
    let mut f = fixture(SimConfig::default());
    // An L: a pillar with a long arm.
    f.put(w(10, 1, 10), w(11, 20, 11), "stone");
    f.put(w(12, 19, 10), w(40, 20, 11), "stone");
    f.dig(w(30, 19, 10), w(30, 20, 11)); // cut the arm near its tip
    f.run(1);
    let bodies = f.sim.take_detached();
    assert_eq!(bodies.len(), 1, "only the tip falls");
    assert_eq!(bodies[0].count, 10 * 2 * 2);
    assert_eq!(f.count("stone", w(12, 19, 10), w(29, 20, 11)), 18 * 2 * 2);
}

#[test]
fn separate_pieces_become_separate_bodies() {
    let mut f = fixture(SimConfig::default());
    f.put(w(10, 1, 10), w(10, 10, 10), "stone");
    f.put(w(20, 1, 10), w(20, 10, 10), "wood");
    f.dig(w(10, 1, 10), w(20, 1, 10));
    f.run(1);
    let mut bodies = f.sim.take_detached();
    bodies.sort_by_key(|b| b.origin.0.x);
    assert_eq!(bodies.len(), 2);
    let wood = f.registry.id("wood").unwrap();
    assert!(bodies[1].voxels.iter().all(|&m| m == wood));
}

#[test]
fn the_anchor_predicate_holds_pieces() {
    let mut f = fixture(SimConfig::default());
    f.sim.set_anchor(Some(Box::new(|p: WorldPos| p.0.y >= 30)));
    // Hanging from the "ceiling" (anchored at y >= 30): stays.
    f.put(w(10, 20, 10), w(10, 35, 10), "stone");
    f.dig(w(10, 22, 10), w(10, 22, 10));
    f.run(1);
    let bodies = f.sim.take_detached();
    assert_eq!(bodies.len(), 1, "the part below the cut falls");
    assert_eq!(bodies[0].count, 2);
    assert_eq!(f.count("stone", w(10, 23, 10), w(10, 35, 10)), 13);
}

#[test]
fn powders_are_not_structural_and_fall_when_their_support_goes() {
    let mut f = fixture(SimConfig::default());
    f.put(w(10, 1, 10), w(10, 5, 10), "stone");
    f.put(w(10, 6, 10), w(10, 8, 10), "sand");
    f.dig(w(10, 1, 10), w(10, 1, 10));
    f.run(1);
    let bodies = f.sim.take_detached();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].count, 4, "the sand is not part of the piece");
    f.run(30);
    assert_eq!(
        f.count("sand", w(8, 1, 8), w(12, 2, 12)),
        3,
        "the sand falls"
    );
}

#[test]
fn crumbs_stay_in_place() {
    let mut f = fixture(SimConfig::default());
    f.sim = Simulation::new(
        &f.registry,
        &ReactionSet::empty(&f.registry),
        &SimConfig {
            integrity_min_voxels: 10,
            ..SimConfig::default()
        },
    );
    f.put(w(10, 1, 10), w(10, 9, 10), "stone"); // 9 voxels: a crumb
    f.put(w(20, 1, 10), w(20, 12, 10), "stone"); // 12: a piece
    f.dig(w(10, 1, 10), w(20, 1, 10));
    f.run(1);
    let bodies = f.sim.take_detached();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].count, 11);
    assert_eq!(
        f.count("stone", w(10, 2, 10), w(10, 9, 10)),
        8,
        "the crumb hangs on"
    );
}

#[test]
fn too_big_pieces_count_as_anchored() {
    let config = SimConfig {
        integrity_max_voxels: 100,
        integrity_budget_voxels: 1000,
        ..SimConfig::default()
    };
    let mut f = fixture(config);
    f.put(w(10, 1, 10), w(14, 20, 14), "stone"); // 500 voxels
    f.dig(w(10, 1, 10), w(14, 1, 14));
    f.run(1);
    assert!(f.sim.take_detached().is_empty());
}

#[test]
fn checks_beyond_the_budget_wait_for_later_ticks() {
    let config = SimConfig {
        integrity_max_voxels: 400,
        integrity_budget_voxels: 400,
        ..SimConfig::default()
    };
    let mut f = fixture(config);
    // Three separate towers far apart (separate regions), each cut at the base.
    for x in [-40i64, 0, 40] {
        f.put(w(x, 1, 10), w(x + 2, 30, 12), "stone");
    }
    for x in [-40i64, 0, 40] {
        f.dig(w(x, 1, 10), w(x + 2, 1, 12));
    }
    f.run(1);
    let first = f.sim.take_detached().len();
    assert!(first < 3, "the budget defers some checks ({first} done)");
    f.run(5);
    assert_eq!(first + f.sim.take_detached().len(), 3);
    assert_eq!(f.sim.integrity_pending(), 0);
}

#[test]
fn reactions_that_remove_solids_trigger_checks() {
    let materials = r#"[
        Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
        Material(name: "stone", state: Solid,  density: 2600, color: (1, 1, 1, 255)),
        Material(name: "wood",  state: Solid,  density: 700,  color: (2, 2, 2, 255)),
        Material(name: "acid",  state: Liquid, density: 1200, color: (3, 3, 3, 255)),
    ]"#;
    let registry = MaterialRegistry::from_sources([("m.ron", materials)]).unwrap();
    let reactions = ReactionSet::from_sources(
        [(
            "r.ron",
            r#"[Reaction(a: "acid", b: "wood", result_a: "air", result_b: "air", probability: 1)]"#,
        )],
        &registry,
    )
    .unwrap();
    let mut f = fixture(SimConfig::default());
    f.sim = Simulation::new(&registry, &reactions, &SimConfig::default());
    f.registry = registry;
    // A stone block on a 1-voxel wooden neck; acid around the neck eats it.
    f.put(w(10, 1, 10), w(10, 1, 10), "wood");
    f.put(w(8, 2, 8), w(12, 6, 12), "stone");
    for (x, z) in [(9, 10), (11, 10), (10, 9), (10, 11)] {
        f.put(w(x, 1, z), w(x, 1, z), "acid");
    }
    // Natural removals are gathered and checked every `integrity_natural_period`.
    f.run(SimConfig::default().integrity_natural_period + 2);
    let bodies = f.sim.take_detached();
    assert!(
        bodies.iter().any(|b| b.count >= 5 * 5 * 5),
        "the block above the eaten neck detaches: {:?}",
        bodies.iter().map(|b| b.count).collect::<Vec<_>>()
    );
}

#[test]
fn a_hot_piece_takes_its_energy_with_it() {
    let mut f = fixture(SimConfig::default());
    f.put(w(10, 1, 10), w(17, 20, 17), "stone");
    // Heat the tower (and its surroundings) through a temperature field.
    let pos = ChunkPos::new(0, 0, 0);
    f.sim.ensure_field(&f.world, pos);
    for t in f.sim.thermal.get_mut(&pos).unwrap().temp.iter_mut() {
        *t = 500.0;
    }
    // Edits are external (not accounted, ADR 007): measure after digging.
    f.dig(w(10, 1, 10), w(17, 1, 17));
    let before = f.sim.total_energy(&f.world);
    let imbalance = f.sim.energy_imbalance();
    f.run(1);
    assert_eq!(f.sim.take_detached().len(), 1);
    let after = f.sim.total_energy(&f.world);
    let accounted = f.sim.energy_imbalance() - imbalance;
    assert!(
        (after - before - accounted).abs() < 1e-3 * before.abs(),
        "drift {:.3e}",
        after - before - accounted
    );
    let t = f.sim.temperature_at(w(12, 10, 12));
    assert!(t <= 500.0 + 1.0, "no latent heat released: {t}");
}
