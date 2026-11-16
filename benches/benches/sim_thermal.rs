//! Milestone 4 scenario: a 32×8×32 lava pool poured onto a 64×8×64 ice slab
//! inside a stone basin. Reports per-tick timings of the whole simulation
//! (automaton + diffusion + phase changes) and the evolution of the materials.
//!
//! `cargo bench -p mv_benches --bench sim_thermal`

use std::time::{Duration, Instant};

use mv_core::config::SimConfig;
use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry};
use mv_sim::Simulation;
use mv_world::edit::fill_box;
use mv_world::{Chunk, World};

const TICKS: u32 = 900;

fn w(x: i64, y: i64, z: i64) -> WorldPos {
    WorldPos::new(x, y, z)
}

/// Places material the way the editor does (sets its base temperature).
fn place(
    world: &mut World,
    sim: &mut Simulation,
    a: WorldPos,
    b: WorldPos,
    m: MaterialId,
) -> usize {
    let n = fill_box(world, a, b, m, |_| true);
    sim.edited(world, a, b, Some(m));
    n
}

fn main() {
    let assets = concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/materials");
    let registry = MaterialRegistry::load_dir(assets.as_ref()).expect("materials");
    let id = |n: &str| registry.id(n).unwrap_or_else(|| panic!("material {n}"));

    let mut world = World::new();
    for z in -2..2 {
        for y in -1..3 {
            for x in -2..2 {
                world.insert(ChunkPos::new(x, y, z), Chunk::uniform(MaterialId::AIR));
            }
        }
    }
    let stone = id("stone");
    fill_box(&mut world, w(-81, 0, -81), w(80, 30, 80), stone, |_| true);
    fill_box(
        &mut world,
        w(-80, 1, -80),
        w(79, 30, 79),
        MaterialId::AIR,
        |_| true,
    );

    let mut sim = Simulation::new(&registry, &SimConfig::default());
    let ice = place(
        &mut world,
        &mut sim,
        w(-32, 1, -32),
        w(31, 8, 31),
        id("ice"),
    );
    let lava = place(
        &mut world,
        &mut sim,
        w(-16, 9, -16),
        w(15, 16, 15),
        id("lava"),
    );
    println!(
        "sim_thermal: {ice} ice + {lava} lava voxels, {} rayon threads",
        rayon::current_num_threads()
    );

    let count = |world: &World, name: &str| {
        let m = id(name);
        let mut n = 0;
        for (_, c) in world.iter() {
            for b in 0..512 {
                n += match c.brick(b) {
                    mv_world::BrickRef::Uniform(u) => 512 * usize::from(u == m),
                    mv_world::BrickRef::Dense(v) => v.iter().filter(|&&x| x == m).count(),
                };
            }
        }
        n
    };

    let mut times = Vec::with_capacity(TICKS as usize);
    let mut max_thermal = 0;
    let mut max_transitions = 0;
    for tick in 1..=TICKS {
        let t = Instant::now();
        sim.step(&mut world);
        times.push(t.elapsed());
        world.take_edited();
        let s = sim.stats();
        max_thermal = max_thermal.max(s.thermal_chunks);
        max_transitions = max_transitions.max(s.transitions);
        if tick % 150 == 0 {
            println!(
                "sim_thermal: tick {tick}: ice {} water {} steam {} lava {} | {} thermal / {} active chunks",
                count(&world, "ice"),
                count(&world, "water"),
                count(&world, "steam"),
                count(&world, "lava"),
                s.thermal_chunks,
                s.active_chunks
            );
        }
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut sorted = times.clone();
    sorted.sort();
    let pct = |p: f64| ms(sorted[((sorted.len() - 1) as f64 * p).round() as usize]);
    println!(
        "sim_thermal: {TICKS} ticks: avg {:.2} ms, p50 {:.2}, p95 {:.2}, max {:.2} ms/tick; max {max_thermal} thermal chunks, max {max_transitions} transitions/tick",
        ms(times.iter().sum::<Duration>()) / times.len() as f64,
        pct(0.5),
        pct(0.95),
        pct(1.0),
    );
}
