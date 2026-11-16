//! Chunk generation and brick extraction (the CPU side of a GPU upload).

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use mv_core::ChunkPos;
use mv_core::consts::{BRICK_VOLUME, BRICKS_PER_CHUNK};
use mv_world::testgen::{TestPalette, sine_terrain_scene};
use mv_world::{Chunk, MaterialId};

fn palette() -> TestPalette {
    TestPalette {
        stone: MaterialId(1),
        dirt: MaterialId(2),
        grass: MaterialId(3),
        sand: MaterialId(4),
        water: MaterialId(5),
        lava: MaterialId(6),
    }
}

fn scene() -> Chunk {
    sine_terrain_scene(ChunkPos::new(0, 0, 0), &palette())
}

fn generate(c: &mut Criterion) {
    let p = palette();
    c.bench_function("chunk_generate_sine_scene", |b| {
        b.iter(|| sine_terrain_scene(black_box(ChunkPos::new(0, 0, 0)), &p));
    });
}

fn copy_bricks(c: &mut Criterion) {
    let chunk = scene();
    let mut out = [MaterialId::AIR; BRICK_VOLUME];
    c.bench_function("chunk_copy_all_512_bricks", |b| {
        b.iter(|| {
            let mut occupied = 0;
            for brick in 0..BRICKS_PER_CHUNK {
                occupied += u32::from(chunk.copy_brick(black_box(brick), &mut out));
            }
            occupied
        });
    });
}

fn occupancy(c: &mut Criterion) {
    let chunk = scene();
    c.bench_function("chunk_occupancy", |b| {
        b.iter(|| black_box(&chunk).occupancy())
    });
}

criterion_group!(benches, generate, copy_bricks, occupancy);
criterion_main!(benches);
