//! Coordinate conversion micro-benchmarks.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use mv_core::consts::CHUNK_VOLUME;
use mv_core::{LocalPos, WorldPos};

fn world_to_chunk_local(c: &mut Criterion) {
    c.bench_function("world_to_chunk_local_4096", |b| {
        b.iter(|| {
            let mut acc = 0usize;
            for i in -2048i64..2048 {
                let (chunk, local) = black_box(WorldPos::new(i, -i, i * 3)).to_chunk_local();
                acc = acc
                    .wrapping_add(chunk.0.x as usize)
                    .wrapping_add(local.index());
            }
            acc
        });
    });
}

fn local_index_roundtrip(c: &mut Criterion) {
    c.bench_function("local_index_roundtrip_chunk", |b| {
        b.iter(|| {
            let mut acc = 0usize;
            for i in 0..CHUNK_VOLUME {
                acc = acc.wrapping_add(LocalPos::from_index(black_box(i)).index());
            }
            acc
        });
    });
}

criterion_group!(benches, world_to_chunk_local, local_index_roundtrip);
criterion_main!(benches);
