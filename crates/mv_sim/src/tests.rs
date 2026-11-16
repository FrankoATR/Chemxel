//! Rule tests on small deterministic worlds.

use mv_core::config::SimConfig;
use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry};
use mv_world::edit::fill_box;
use mv_world::{Chunk, World};

use super::*;

const MATERIALS: &str = r#"[
    Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
    Material(name: "stone", state: Solid,  density: 2600, color: (1, 1, 1, 255)),
    Material(name: "sand",  state: Powder, density: 1600, color: (2, 2, 2, 255)),
    Material(name: "water", state: Liquid, density: 1000, color: (3, 3, 3, 255), dispersion: 5),
    Material(name: "oil",   state: Liquid, density: 800,  color: (4, 4, 4, 255), dispersion: 3),
    Material(name: "steam", state: Gas,    density: 0.6,  color: (5, 5, 5, 255)),
]"#;

struct Fixture {
    world: World,
    sim: Simulation,
    stone: MaterialId,
    sand: MaterialId,
    water: MaterialId,
    oil: MaterialId,
    steam: MaterialId,
}

fn w(x: i64, y: i64, z: i64) -> WorldPos {
    WorldPos::new(x, y, z)
}

/// Air chunks `x, y, z in -1..=1` with a stone floor at `y = 0` (so y >= 1 is open).
fn fixture(seed: u64) -> Fixture {
    let registry = MaterialRegistry::from_sources([("test", MATERIALS)]).unwrap();
    let id = |n| registry.id(n).unwrap();
    let mut world = World::new();
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                world.insert(ChunkPos::new(x, y, z), Chunk::uniform(MaterialId::AIR));
            }
        }
    }
    let stone = id("stone");
    fill_box(&mut world, w(-64, 0, -64), w(127, 0, 127), stone, |_| true);
    let config = SimConfig {
        seed,
        ..SimConfig::default()
    };
    Fixture {
        sim: Simulation::new(&registry, &config),
        world,
        stone,
        sand: id("sand"),
        water: id("water"),
        oil: id("oil"),
        steam: id("steam"),
    }
}

impl Fixture {
    fn put(&mut self, min: WorldPos, max: WorldPos, m: MaterialId) {
        fill_box(&mut self.world, min, max, m, |_| true);
        self.sim.wake_box(&self.world, min, max);
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.sim.step(&mut self.world);
        }
    }

    fn at(&self, x: i64, y: i64, z: i64) -> MaterialId {
        self.world.voxel(w(x, y, z)).unwrap()
    }

    fn count(&self, m: MaterialId, min: WorldPos, max: WorldPos) -> usize {
        let mut n = 0;
        for z in min.0.z..=max.0.z {
            for y in min.0.y..=max.0.y {
                for x in min.0.x..=max.0.x {
                    n += usize::from(self.at(x, y, z) == m);
                }
            }
        }
        n
    }

    /// Height of the topmost `m` voxel in column (x, z), searching y in 1..64.
    fn top(&self, m: MaterialId, x: i64, z: i64) -> Option<i64> {
        (1..64).rev().find(|&y| self.at(x, y, z) == m)
    }
}

#[test]
fn sand_falls_one_cell_per_tick_and_rests_on_the_floor() {
    let mut f = fixture(1);
    f.put(w(5, 10, 5), w(5, 10, 5), f.sand);
    f.run(1);
    assert_eq!(f.at(5, 9, 5), f.sand);
    assert_eq!(f.at(5, 10, 5), MaterialId::AIR);
    f.run(20);
    assert_eq!(f.at(5, 1, 5), f.sand);
}

#[test]
fn a_falling_column_moves_as_a_whole() {
    let mut f = fixture(1);
    f.put(w(5, 10, 5), w(5, 14, 5), f.sand);
    f.run(1);
    assert_eq!(f.count(f.sand, w(5, 9, 5), w(5, 13, 5)), 5);
    assert_eq!(f.at(5, 14, 5), MaterialId::AIR);
}

#[test]
fn powder_slides_off_a_stack() {
    let mut f = fixture(2);
    f.put(w(5, 1, 5), w(5, 2, 5), f.sand);
    f.run(3);
    assert_eq!(f.at(5, 2, 5), MaterialId::AIR, "top grain must slide down");
    assert_eq!(f.count(f.sand, w(4, 1, 4), w(6, 1, 6)), 2);
}

#[test]
fn static_solids_do_not_move() {
    let mut f = fixture(1);
    f.put(w(5, 20, 5), w(6, 21, 6), f.stone);
    f.run(10);
    assert_eq!(f.count(f.stone, w(5, 20, 5), w(6, 21, 6)), 8);
}

#[test]
fn sand_forms_a_mound() {
    let mut f = fixture(3);
    // A tall column released at once.
    f.put(w(0, 1, 0), w(0, 60, 0), f.sand);
    f.run(400);
    assert_eq!(f.count(f.sand, w(-30, 1, -30), w(30, 63, 30)), 60);
    let center = f.top(f.sand, 0, 0).expect("pile at the centre");
    assert!(
        center < 15,
        "a 60-voxel column must spread out (height {center})"
    );
    // Heights do not increase away from the centre (a cone, not a column).
    let ring = [(4, 0), (-4, 0), (0, 4), (0, -4)];
    for (x, z) in ring {
        assert!(f.top(f.sand, x, z).unwrap_or(0) <= center);
    }
    assert!(f.top(f.sand, 2, 0).is_some() || f.top(f.sand, -2, 0).is_some());
}

#[test]
fn sand_sinks_through_water() {
    let mut f = fixture(4);
    f.put(w(0, 1, 0), w(0, 6, 0), f.water);
    // Walls so the water stays in a one-voxel column.
    for (x, z) in [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ] {
        f.put(w(x, 1, z), w(x, 12, z), f.stone);
    }
    f.put(w(0, 7, 0), w(0, 8, 0), f.sand);
    f.run(30);
    assert_eq!(f.at(0, 1, 0), f.sand);
    assert_eq!(f.at(0, 2, 0), f.sand);
    assert_eq!(f.count(f.water, w(0, 3, 0), w(0, 8, 0)), 6);
}

#[test]
fn water_spreads_and_levels_in_a_basin() {
    let mut f = fixture(5);
    // 12×12 basin with walls up to y = 20.
    f.put(w(-1, 1, -1), w(12, 20, 12), f.stone);
    f.put(w(0, 1, 0), w(11, 20, 11), MaterialId::AIR);
    // A 4×9×4 block of water (144 voxels) dropped in a corner: one full layer of 144.
    f.put(w(0, 6, 0), w(3, 14, 3), f.water);
    f.run(600);
    let total = f.count(f.water, w(0, 1, 0), w(11, 20, 11));
    assert_eq!(total, 144);
    let mut heights = Vec::new();
    for z in 0..12 {
        for x in 0..12 {
            heights.push(f.top(f.water, x, z).unwrap_or(0));
        }
    }
    let (lo, hi) = (heights.iter().min().unwrap(), heights.iter().max().unwrap());
    assert!(
        hi - lo <= 1,
        "water surface must be level (heights {lo}..={hi})"
    );
}

#[test]
fn lighter_liquid_floats_on_denser() {
    let mut f = fixture(6);
    f.put(w(-1, 1, -1), w(3, 12, 3), f.stone);
    f.put(w(0, 1, 0), w(2, 12, 2), MaterialId::AIR);
    f.put(w(0, 1, 0), w(2, 2, 2), f.oil);
    f.put(w(0, 3, 0), w(2, 4, 2), f.water);
    f.run(200);
    assert_eq!(
        f.count(f.water, w(0, 1, 0), w(2, 2, 2)),
        18,
        "water at the bottom"
    );
    assert_eq!(f.count(f.oil, w(0, 3, 0), w(2, 4, 2)), 18, "oil on top");
}

#[test]
fn gas_rises_to_the_ceiling() {
    let mut f = fixture(7);
    f.put(w(-1, 1, -1), w(1, 30, 1), f.stone);
    f.put(w(0, 1, 0), w(0, 29, 0), MaterialId::AIR);
    f.put(w(0, 1, 0), w(0, 1, 0), f.steam);
    f.run(40);
    assert_eq!(f.at(0, 29, 0), f.steam);
}

#[test]
fn moves_cross_chunk_borders() {
    let mut f = fixture(8);
    // Sand above the y = 64 border falls into the chunk below it.
    f.put(w(10, 66, 10), w(12, 68, 12), f.sand);
    // Water flows across the x = 0 border (chunk -1 to chunk 0).
    f.put(w(-3, 1, 20), w(-1, 8, 22), f.water);
    f.run(300);
    assert_eq!(f.count(f.sand, w(0, 1, 0), w(30, 63, 30)), 27);
    assert_eq!(f.count(f.sand, w(0, 64, 0), w(30, 127, 30)), 0);
    // The water spreads into a thin film over the open floor: count all of it.
    let water = f.count(f.water, w(-64, 1, -64), w(127, 10, 127));
    assert_eq!(water, 72, "water is conserved across borders");
    assert!(
        f.count(f.water, w(0, 1, -64), w(127, 10, 127)) > 0,
        "some water crossed x = 0"
    );
}

#[test]
fn unloaded_neighbours_are_walls() {
    let mut f = fixture(9);
    // Remove the floor under chunk (1, -1, 1)... the lowest loaded layer is y = -64.
    f.put(w(70, -64, 70), w(70, -64, 70), f.sand);
    f.put(w(70, -63, 70), w(70, -63, 70), f.sand);
    f.run(10);
    assert_eq!(f.at(70, -64, 70), f.sand, "nothing below y = -64 is loaded");
}

#[test]
fn settled_chunks_sleep_and_edits_wake_them() {
    let mut f = fixture(10);
    f.put(w(5, 1, 5), w(5, 4, 5), f.sand);
    assert!(f.sim.active_chunks() > 0);
    f.run(50);
    assert_eq!(f.sim.active_chunks(), 0, "everything settled");
    f.put(w(5, 30, 5), w(5, 30, 5), f.sand);
    assert!(f.sim.active_chunks() > 0);
    f.run(1);
    assert_eq!(f.at(5, 29, 5), f.sand);
}

#[test]
fn deterministic_regardless_of_thread_count() {
    let scenario = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let mut f = fixture(42);
            f.put(w(-20, 20, -20), w(20, 30, 20), f.sand);
            f.put(w(-20, 31, -20), w(20, 40, 20), f.water);
            f.run(60);
            let mut snapshot = Vec::new();
            for z in -1..=1 {
                for y in -1..=1 {
                    for x in -1..=1 {
                        let c = f.world.chunk(ChunkPos::new(x, y, z)).unwrap();
                        snapshot.push(c.data().clone());
                    }
                }
            }
            snapshot
        })
    };
    assert!(
        scenario(1) == scenario(4),
        "results depend on the thread count"
    );
}

#[test]
fn different_seeds_diverge() {
    let run = |seed| {
        let mut f = fixture(seed);
        f.put(w(0, 1, 0), w(0, 40, 0), f.sand);
        f.run(100);
        (0..=8).map(|x| f.top(f.sand, x - 4, 0)).collect::<Vec<_>>()
    };
    let shapes: Vec<_> = (0..4).map(run).collect();
    assert!(shapes.windows(2).any(|w| w[0] != w[1]));
}
