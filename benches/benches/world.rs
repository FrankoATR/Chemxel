//! Milestone 2: terrain generation, editing and picking.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use glam::{DVec3, IVec2};
use mv_core::config::WorldConfig;
use mv_core::consts::CHUNK_SIZE;
use mv_core::{ChunkPos, WorldPos};
use mv_world::edit::fill_sphere;
use mv_world::raycast::raycast;
use mv_world::terrain::{TerrainGenerator, TerrainPalette};
use mv_world::{Chunk, MaterialId, World};

fn generator() -> TerrainGenerator {
    let palette = TerrainPalette::synthetic();
    TerrainGenerator::new(&WorldConfig::default(), 12_648_430, palette)
}

/// Chunk containing the surface at column (0, 0).
fn surface_chunk(g: &TerrainGenerator) -> ChunkPos {
    let h = g.height_at(32, 32);
    ChunkPos::new(0, h.div_euclid(CHUNK_SIZE as i32), 0)
}

fn terrain(c: &mut Criterion) {
    let g = generator();
    let pos = surface_chunk(&g);
    g.column(IVec2::new(pos.0.x, pos.0.z)); // cached, as in a real column
    c.bench_function("terrain_surface_chunk_cached_heights", |b| {
        b.iter(|| g.generate(black_box(pos)));
    });
    let mut i = 0;
    c.bench_function("terrain_column_heights_64x64", |b| {
        b.iter(|| {
            // A fresh column every time (the cache is keyed by column).
            i += 1;
            let col = g.column(IVec2::new(i, 1_000));
            g.retain_columns(IVec2::new(i, 1_000), 0);
            col
        });
    });
}

/// Ground of uniform stone below y = 0, 4×2×4 chunks.
fn flat_world() -> World {
    let mut w = World::new();
    for z in -2..2 {
        for x in -2..2 {
            w.insert(ChunkPos::new(x, -1, z), Chunk::uniform(MaterialId(1)));
            w.insert(ChunkPos::new(x, 0, z), Chunk::uniform(MaterialId::AIR));
        }
    }
    w
}

fn editing(c: &mut Criterion) {
    let mut w = flat_world();
    let center = WorldPos::new(0, 0, 0);
    c.bench_function("fill_sphere_r16_dig_and_refill", |b| {
        b.iter(|| {
            let dug = fill_sphere(&mut w, center, 16.0, MaterialId::AIR, |_| true);
            let filled = fill_sphere(&mut w, center, 16.0, MaterialId(1), MaterialId::is_air);
            w.take_edited();
            (dug, filled)
        });
    });
}

fn picking(c: &mut Criterion) {
    let w = flat_world();
    // Shallow ray: crosses ~120 voxels of air before hitting the ground.
    let origin = DVec3::new(-100.5, 10.5, -3.5);
    let dir = DVec3::new(1.0, -0.09, 0.02);
    c.bench_function("raycast_shallow_120_voxels", |b| {
        b.iter(|| raycast(&w, black_box(origin), black_box(dir), 1024.0));
    });
}

criterion_group!(benches, terrain, editing, picking);
criterion_main!(benches);
