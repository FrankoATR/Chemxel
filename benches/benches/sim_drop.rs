//! Milestone 3 acceptance scenario (not a Criterion bench: the state evolves, so
//! every tick is different). Drops a 40×32×40 block of sand and one of water
//! (102 400 voxels) into a 160×160 stone basin, runs the simulation until it
//! settles, and reports per-tick timings plus the final shape.
//!
//! `cargo bench -p mv_benches --bench sim_drop`

use std::time::{Duration, Instant};

use mv_core::config::SimConfig;
use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry, ReactionSet};
use mv_sim::Simulation;
use mv_world::edit::fill_box;
use mv_world::{Chunk, World};

const MAX_TICKS: u32 = 3000;

fn w(x: i64, y: i64, z: i64) -> WorldPos {
    WorldPos::new(x, y, z)
}

fn main() {
    let assets = concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/materials");
    let registry = MaterialRegistry::load_dir(assets.as_ref()).expect("materials");
    let id = |n| registry.id(n).expect(n);
    let (stone, sand, water) = (id("stone"), id("sand"), id("water"));

    // 4×3×4 chunks of air; basin floor at y = 0, walls up to y = 40.
    let mut world = World::new();
    for z in -2..2 {
        for y in -1..2 {
            for x in -2..2 {
                world.insert(ChunkPos::new(x, y, z), Chunk::uniform(MaterialId::AIR));
            }
        }
    }
    fill_box(&mut world, w(-81, 0, -81), w(80, 40, 80), stone, |_| true);
    fill_box(
        &mut world,
        w(-80, 1, -80),
        w(79, 40, 79),
        MaterialId::AIR,
        |_| true,
    );

    let reactions_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/reactions");
    let reactions = ReactionSet::load_dir(reactions_dir.as_ref(), &registry).expect("reactions");
    let mut sim = Simulation::new(&registry, &reactions, &SimConfig::default());
    let sand_box = (w(-60, 60, -20), w(-21, 91, 19));
    let water_box = (w(20, 60, -20), w(59, 91, 19));
    let placed = fill_box(&mut world, sand_box.0, sand_box.1, sand, |_| true)
        + fill_box(&mut world, water_box.0, water_box.1, water, |_| true);
    sim.wake_box(&world, sand_box.0, sand_box.1);
    sim.wake_box(&world, water_box.0, water_box.1);
    println!(
        "sim_drop: {placed} voxels (sand + water), {} rayon threads",
        rayon::current_num_threads()
    );

    let mut times = Vec::new();
    let mut max_active = 0;
    let mut max_moves = 0;
    let mut settled_at = None;
    for tick in 1..=MAX_TICKS {
        let t = Instant::now();
        sim.step(&mut world);
        times.push(t.elapsed());
        let s = sim.stats();
        max_active = max_active.max(s.active_chunks);
        max_moves = max_moves.max(s.moves);
        // Drain the GPU-upload queue the game would consume.
        world.take_edited();
        if sim.active_chunks() == 0 {
            settled_at = Some(tick);
            break;
        }
    }

    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut sorted = times.clone();
    sorted.sort();
    let pct = |p: f64| ms(sorted[((sorted.len() - 1) as f64 * p).round() as usize]);
    let busy: Vec<_> = times.iter().take(300).copied().collect();
    println!(
        "sim_drop: {} ticks, avg {:.2} ms, p50 {:.2}, p95 {:.2}, max {:.2} ms; first 300 ticks avg {:.2} ms",
        times.len(),
        ms(times.iter().sum::<Duration>()) / times.len() as f64,
        pct(0.5),
        pct(0.95),
        pct(1.0),
        ms(busy.iter().sum::<Duration>()) / busy.len() as f64,
    );
    println!(
        "sim_drop: max active chunks {max_active}, max moves/tick {max_moves}, settled: {}",
        settled_at.map_or("no".to_owned(), |t| format!(
            "after {t} ticks ({:.1} s of game time)",
            f64::from(t) / 30.0
        ))
    );

    // Final shape: sand pile height and water surface flatness.
    let mut sand_count = 0;
    let mut water_count = 0;
    let mut sand_top = 0;
    let mut water_tops = Vec::new();
    for z in -80..80 {
        for x in -80..80 {
            let mut top_water = None;
            for y in 1..41 {
                match world.voxel(w(x, y, z)) {
                    Some(m) if m == sand => {
                        sand_count += 1;
                        sand_top = sand_top.max(y);
                    }
                    Some(m) if m == water => {
                        water_count += 1;
                        top_water = Some(y);
                    }
                    _ => {}
                }
            }
            if let Some(y) = top_water {
                water_tops.push(y);
            }
        }
    }
    let (lo, hi) = (
        water_tops.iter().min().copied().unwrap_or(0),
        water_tops.iter().max().copied().unwrap_or(0),
    );
    println!(
        "sim_drop: sand {sand_count} (pile height {sand_top}), water {water_count} over {} columns, surface y {lo}..={hi}",
        water_tops.len()
    );
}
