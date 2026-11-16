//! World editing tools.
//!
//! Edits work chunk by chunk: each affected chunk is fetched once and its voxels are
//! written in memory order, so the per-voxel cost is a bounds check and a store.

use glam::I64Vec3;
use mv_core::consts::CHUNK_SIZE;
use mv_core::{ChunkPos, LocalPos, WorldPos};
use mv_materials::MaterialId;

use crate::World;

/// Writes `material` into every voxel within `radius` of the centre of voxel
/// `center` for which `replace(current)` is true. Unloaded chunks are skipped.
/// Returns the number of voxels that changed.
pub fn fill_sphere(
    world: &mut World,
    center: WorldPos,
    radius: f32,
    material: MaterialId,
    mut replace: impl FnMut(MaterialId) -> bool,
) -> usize {
    let _span = tracing::trace_span!("fill_sphere").entered();
    let r = f64::from(radius.max(0.0));
    let r2 = r * r;
    let reach = r.floor() as i64;
    let lo = center.0 - I64Vec3::splat(reach);
    let hi = center.0 + I64Vec3::splat(reach);
    let (clo, chi) = (WorldPos(lo).chunk().0, WorldPos(hi).chunk().0);
    let size = CHUNK_SIZE as i64;

    let mut changed_total = 0;
    for cz in clo.z..=chi.z {
        for cy in clo.y..=chi.y {
            for cx in clo.x..=chi.x {
                let cpos = ChunkPos::new(cx, cy, cz);
                let Some(chunk) = world.chunk_mut(cpos) else {
                    continue;
                };
                let origin = cpos.origin().0;
                // Local bounds of the sphere's box inside this chunk.
                let a = (lo - origin).max(I64Vec3::ZERO);
                let b = (hi - origin).min(I64Vec3::splat(size - 1));
                let mut changed = 0;
                for z in a.z..=b.z {
                    let dz = (origin.z + z - center.0.z) as f64;
                    for y in a.y..=b.y {
                        let dy = (origin.y + y - center.0.y) as f64;
                        let dyz = dy * dy + dz * dz;
                        if dyz > r2 {
                            continue;
                        }
                        for x in a.x..=b.x {
                            let dx = (origin.x + x - center.0.x) as f64;
                            if dx * dx + dyz > r2 {
                                continue;
                            }
                            let local = LocalPos::new(x as u8, y as u8, z as u8);
                            if replace(chunk.get(local)) && chunk.set(local, material) {
                                changed += 1;
                            }
                        }
                    }
                }
                if changed > 0 {
                    chunk.compact();
                    world.mark_edited(cpos);
                    changed_total += changed;
                }
            }
        }
    }
    changed_total
}

/// Writes `material` into every voxel of the inclusive box `[min, max]` for which
/// `replace(current)` is true. Unloaded chunks are skipped. Returns the number of
/// voxels that changed.
pub fn fill_box(
    world: &mut World,
    min: WorldPos,
    max: WorldPos,
    material: MaterialId,
    mut replace: impl FnMut(MaterialId) -> bool,
) -> usize {
    let _span = tracing::trace_span!("fill_box").entered();
    let (lo, hi) = (min.0.min(max.0), min.0.max(max.0));
    let (clo, chi) = (WorldPos(lo).chunk().0, WorldPos(hi).chunk().0);
    let size = CHUNK_SIZE as i64;
    let mut changed_total = 0;
    for cz in clo.z..=chi.z {
        for cy in clo.y..=chi.y {
            for cx in clo.x..=chi.x {
                let cpos = ChunkPos::new(cx, cy, cz);
                let Some(chunk) = world.chunk_mut(cpos) else {
                    continue;
                };
                let origin = cpos.origin().0;
                let a = (lo - origin).max(I64Vec3::ZERO);
                let b = (hi - origin).min(I64Vec3::splat(size - 1));
                let mut changed = 0;
                for z in a.z..=b.z {
                    for y in a.y..=b.y {
                        for x in a.x..=b.x {
                            let local = LocalPos::new(x as u8, y as u8, z as u8);
                            if replace(chunk.get(local)) && chunk.set(local, material) {
                                changed += 1;
                            }
                        }
                    }
                }
                if changed > 0 {
                    chunk.compact();
                    world.mark_edited(cpos);
                    changed_total += changed;
                }
            }
        }
    }
    changed_total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Chunk;

    const STONE: MaterialId = MaterialId(1);
    const SAND: MaterialId = MaterialId(2);

    fn world_3x3x3(fill: MaterialId) -> World {
        let mut w = World::new();
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    w.insert(ChunkPos::new(x, y, z), Chunk::uniform(fill));
                }
            }
        }
        w
    }

    #[test]
    fn sphere_volume_across_chunk_corner() {
        let mut w = world_3x3x3(MaterialId::AIR);
        // Centred on a chunk corner: touches 8 chunks.
        let n = fill_sphere(&mut w, WorldPos::new(0, 0, 0), 5.0, SAND, |_| true);
        // Lattice points inside a radius-5 ball.
        let expected = (-5i64..=5)
            .flat_map(|z| (-5i64..=5).flat_map(move |y| (-5i64..=5).map(move |x| (x, y, z))))
            .filter(|(x, y, z)| x * x + y * y + z * z <= 25)
            .count();
        assert_eq!(n, expected);
        assert_eq!(w.take_edited().len(), 8);
        assert_eq!(w.voxel(WorldPos::new(-5, 0, 0)), Some(SAND));
        assert_eq!(w.voxel(WorldPos::new(4, 3, 0)), Some(SAND));
        assert_eq!(w.voxel(WorldPos::new(4, 3, 1)), Some(MaterialId::AIR));
    }

    #[test]
    fn replace_filter_and_idempotence() {
        let mut w = world_3x3x3(STONE);
        let c = WorldPos::new(10, 10, 10);
        // Placing only into air changes nothing inside solid stone.
        assert_eq!(fill_sphere(&mut w, c, 3.0, SAND, MaterialId::is_air), 0);
        assert!(w.take_edited().is_empty());
        let dug = fill_sphere(&mut w, c, 3.0, MaterialId::AIR, |_| true);
        assert!(dug > 0);
        assert_eq!(fill_sphere(&mut w, c, 3.0, MaterialId::AIR, |_| true), 0);
        assert_eq!(fill_sphere(&mut w, c, 3.0, SAND, MaterialId::is_air), dug);
    }

    #[test]
    fn clearing_a_chunk_compacts_it() {
        let mut w = world_3x3x3(MaterialId::AIR);
        let c = WorldPos::new(5, 5, 5);
        fill_sphere(&mut w, c, 2.0, SAND, |_| true);
        assert!(
            w.chunk(ChunkPos::new(0, 0, 0))
                .unwrap()
                .as_uniform()
                .is_none()
        );
        fill_sphere(&mut w, c, 2.0, MaterialId::AIR, |_| true);
        assert_eq!(
            w.chunk(ChunkPos::new(0, 0, 0)).unwrap().as_uniform(),
            Some(MaterialId::AIR)
        );
    }

    #[test]
    fn box_across_chunks() {
        let mut w = world_3x3x3(MaterialId::AIR);
        let n = fill_box(
            &mut w,
            WorldPos::new(-2, -2, -2),
            WorldPos::new(1, 1, 1),
            SAND,
            |_| true,
        );
        assert_eq!(n, 64);
        assert_eq!(w.take_edited().len(), 8);
        assert_eq!(w.voxel(WorldPos::new(1, 1, 1)), Some(SAND));
        assert_eq!(w.voxel(WorldPos::new(2, 1, 1)), Some(MaterialId::AIR));
    }

    #[test]
    fn unloaded_chunks_are_skipped() {
        let mut w = world_3x3x3(MaterialId::AIR);
        // The loaded chunks end at x = 127; the half of the ball beyond is skipped.
        let n = fill_sphere(&mut w, WorldPos::new(127, 0, 0), 2.0, SAND, |_| true);
        // Ball of radius 2: 13 points at dx = 0, 9 at dx = -1, 1 at dx = -2.
        assert_eq!(n, 13 + 9 + 1);
        assert_eq!(w.voxel(WorldPos::new(125, 0, 0)), Some(SAND));
        assert_eq!(w.voxel(WorldPos::new(128, 0, 0)), None);
        let n = fill_sphere(&mut w, WorldPos::new(500, 0, 0), 2.0, SAND, |_| true);
        assert_eq!(n, 0);
    }
}
