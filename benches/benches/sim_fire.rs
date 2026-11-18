//! Milestone 5 scenario: a 6×6 forest (trunks 3×3, canopies 13 wide, touching) on
//! a stone floor, lit at one corner. Reports per-tick timings of the whole
//! simulation (automaton + reactions + diffusion + combustion + integrity + rigid
//! bodies) and how the fire progresses.
//!
//! `cargo bench -p mv_benches --bench sim_fire`

use std::time::{Duration, Instant};

use mv_core::config::{PhysicsConfig, SimConfig};
use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry, ReactionSet};
use mv_physics::Physics;
use mv_sim::Simulation;
use mv_world::edit::fill_box;
use mv_world::{Chunk, World};

const TICKS: u32 = 1800;

fn w(x: i64, y: i64, z: i64) -> WorldPos {
    WorldPos::new(x, y, z)
}

/// Places material the way the editor does (only into air, sets base temperature).
fn place(
    world: &mut World,
    sim: &mut Simulation,
    a: WorldPos,
    b: WorldPos,
    m: MaterialId,
) -> usize {
    let n = fill_box(world, a, b, m, MaterialId::is_air);
    sim.edited(world, a, b, Some(m));
    n
}

fn main() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../assets");
    let root = std::path::Path::new(root);
    let registry = MaterialRegistry::load_dir(&root.join("materials")).expect("materials");
    let reactions = ReactionSet::load_dir(&root.join("reactions"), &registry).expect("reactions");
    let id = |n: &str| registry.id(n).unwrap_or_else(|| panic!("material {n}"));

    let mut world = World::new();
    for z in -2..2 {
        for y in -1..3 {
            for x in -2..2 {
                world.insert(ChunkPos::new(x, y, z), Chunk::uniform(MaterialId::AIR));
            }
        }
    }
    fill_box(
        &mut world,
        w(-128, 0, -128),
        w(127, 0, 127),
        id("stone"),
        |_| true,
    );
    let mut sim = Simulation::new(&registry, &reactions, &SimConfig::default());
    // Like the game's terrain anchor: the ground (y <= 0) never falls.
    sim.set_anchor(Some(Box::new(|p: WorldPos| p.0.y <= 0)));

    // Same layout as the in-game scenario (crates/mv_game/src/drop_test.rs), on flat
    // ground: trunks, then canopies, then the fire.
    let (side, spacing) = (6i64, 12i64);
    let mut fuel = 0;
    let mut trees = Vec::new();
    for i in 0..side {
        for j in 0..side {
            let x = -side * spacing / 2 + i * spacing;
            let z = -side * spacing / 2 + j * spacing;
            let top = 32 + (i * 7 + j * 5) % 13;
            trees.push((x, z, top));
        }
    }
    for &(x, z, top) in &trees {
        fuel += place(
            &mut world,
            &mut sim,
            w(x - 1, 1, z - 1),
            w(x + 1, top, z + 1),
            id("wood"),
        );
    }
    for &(x, z, top) in &trees {
        fuel += place(
            &mut world,
            &mut sim,
            w(x - 6, top - 6, z - 6),
            w(x + 6, top - 1, z + 6),
            id("leaves"),
        );
        fuel += place(
            &mut world,
            &mut sim,
            w(x - 4, top, z - 4),
            w(x + 4, top + 3, z + 4),
            id("leaves"),
        );
    }
    let (x0, z0, _) = trees[0];
    place(
        &mut world,
        &mut sim,
        w(x0 - 4, 1, z0 - 1),
        w(x0 - 2, 6, z0 + 1),
        id("fire"),
    );
    println!(
        "sim_fire: {} trees, {fuel} fuel voxels, {} rayon threads",
        trees.len(),
        rayon::current_num_threads()
    );

    let count = |world: &World, names: &[&str]| {
        let ms: Vec<MaterialId> = names.iter().map(|n| id(n)).collect();
        let mut n = 0;
        for (_, c) in world.iter() {
            for b in 0..512 {
                n += match c.brick(b) {
                    mv_world::BrickRef::Uniform(u) => 512 * usize::from(ms.contains(&u)),
                    mv_world::BrickRef::Dense(v) => v.iter().filter(|x| ms.contains(x)).count(),
                };
            }
        }
        n
    };

    // Burnt trunks drop their canopies: they fall as rigid bodies.
    let mut physics = Physics::new(&registry, &PhysicsConfig::default(), 1.0 / 30.0);
    let mut max_bodies = 0;
    let mut times = Vec::with_capacity(TICKS as usize);
    let (mut max_thermal, mut max_active, mut max_flames, mut max_reactions) = (0, 0, 0, 0);
    for tick in 1..=TICKS {
        let t = Instant::now();
        sim.step(&mut world);
        physics.step(&mut world, &mut sim);
        max_bodies = max_bodies.max(physics.stats().bodies);
        times.push(t.elapsed());
        world.take_edited();
        let s = sim.stats();
        max_thermal = max_thermal.max(s.thermal_chunks);
        max_active = max_active.max(s.active_chunks);
        max_flames = max_flames.max(s.flames);
        max_reactions = max_reactions.max(s.reactions);
        if tick % 150 == 0 {
            println!(
                "sim_fire: tick {tick}: fuel {} fire {} smoke {} ash {} | {} thermal / {} active chunks, {:.2} ms",
                count(&world, &["wood", "leaves"]),
                count(&world, &["fire"]),
                count(&world, &["smoke"]),
                count(&world, &["ash"]),
                s.thermal_chunks,
                s.active_chunks,
                times.last().unwrap().as_secs_f64() * 1000.0,
            );
        }
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut sorted = times.clone();
    sorted.sort();
    let pct = |p: f64| ms(sorted[((sorted.len() - 1) as f64 * p).round() as usize]);
    println!(
        "sim_fire: {TICKS} ticks: avg {:.2} ms, p50 {:.2}, p95 {:.2}, max {:.2} ms/tick; max {max_active} active / {max_thermal} thermal chunks, max {max_flames} flames and {max_reactions} reactions per tick, max {max_bodies} rigid bodies ({} detached); {} of {fuel} fuel voxels left",
        ms(times.iter().sum::<Duration>()) / times.len() as f64,
        pct(0.5),
        pct(0.95),
        pct(1.0),
        physics.stats().created,
        count(&world, &["wood", "leaves"]),
    );
}
