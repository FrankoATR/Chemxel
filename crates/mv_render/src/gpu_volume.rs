//! GPU-side brickmap: the voxel data the raymarcher reads.
//!
//! Three levels, all in storage buffers (see `assets/shaders/raymarch.wgsl`):
//!
//! 1. `chunk_grid`: one entry per chunk cell of the rendered volume → chunk slot or
//!    [`EMPTY`].
//! 2. `chunk_bricks`: 512 entries per chunk slot → brick slot or [`EMPTY`].
//! 3. `brick_pool`: 512 `u16` materials per brick slot, packed two per `u32`.
//!
//! Only bricks marked dirty in the CPU [`Chunk`] are re-uploaded. Empty bricks never
//! occupy pool memory.

use bytemuck::{Pod, Zeroable};
use glam::{IVec3, UVec3};
use mv_core::ChunkPos;
use mv_core::consts::{BRICK_VOLUME, BRICKS_PER_CHUNK};
use mv_world::{BrickRef, Chunk, MaterialId};

use crate::slots::SlotAllocator;

/// Sentinel for "no chunk / no brick" in GPU tables.
pub const EMPTY: u32 = u32::MAX;
/// Bytes of one brick in the pool (512 × u16).
const BRICK_BYTES: u64 = (BRICK_VOLUME * 2) as u64;
/// Max entries of the material table.
pub const MAX_MATERIALS: usize = 4096;

/// Per-material data for the shader. Matches `struct Material` in WGSL.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct MaterialGpu {
    /// Linear RGB + alpha.
    pub color: [f32; 4],
    /// x: emissive intensity; yzw reserved.
    pub params: [f32; 4],
}

impl MaterialGpu {
    /// Converts an sRGB colour to the linear values the shader works with.
    pub fn from_srgba(rgba: [u8; 4], emissive: f32) -> Self {
        let lin = |c: u8| {
            let c = f32::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        Self {
            color: [
                lin(rgba[0]),
                lin(rgba[1]),
                lin(rgba[2]),
                f32::from(rgba[3]) / 255.0,
            ],
            params: [emissive, 0.0, 0.0, 0.0],
        }
    }
}

/// Sizing of the GPU volume.
#[derive(Debug, Clone, Copy)]
pub struct VolumeDesc {
    /// Chunk at the volume's minimum corner.
    pub origin: ChunkPos,
    /// Volume size in chunks.
    pub dims: UVec3,
    /// Bricks the pool can hold (1 KiB each).
    pub brick_capacity: u32,
}

/// Counters from one upload, for the debug overlay and budgets.
#[derive(Debug, Clone, Copy, Default)]
pub struct UploadStats {
    pub bricks_written: u32,
    pub bricks_freed: u32,
    pub bytes: u64,
}

#[derive(Debug)]
pub struct GpuVolume {
    origin: ChunkPos,
    dims: UVec3,

    chunk_grid: Vec<u32>,
    chunk_grid_buf: wgpu::Buffer,
    chunk_grid_dirty: bool,

    chunk_slots: SlotAllocator,
    chunk_bricks: Vec<u32>,
    chunk_bricks_buf: wgpu::Buffer,

    brick_slots: SlotAllocator,
    brick_pool_buf: wgpu::Buffer,

    materials_buf: wgpu::Buffer,

    scratch: Box<[MaterialId; BRICK_VOLUME]>,
    packed: Vec<u32>,
}

impl GpuVolume {
    pub fn new(device: &wgpu::Device, desc: &VolumeDesc) -> Self {
        let cells = (desc.dims.x * desc.dims.y * desc.dims.z) as usize;
        let max_chunks = cells as u32;
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let buffer = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: storage,
                mapped_at_creation: false,
            })
        };

        tracing::info!(
            "GPU volume: {}x{}x{} chunks, brick pool {} bricks ({} MiB)",
            desc.dims.x,
            desc.dims.y,
            desc.dims.z,
            desc.brick_capacity,
            (u64::from(desc.brick_capacity) * BRICK_BYTES) >> 20
        );

        Self {
            origin: desc.origin,
            dims: desc.dims,
            chunk_grid: vec![EMPTY; cells],
            chunk_grid_buf: buffer("chunk_grid", (cells * 4) as u64),
            chunk_grid_dirty: true,
            chunk_slots: SlotAllocator::new(max_chunks),
            chunk_bricks: vec![EMPTY; cells * BRICKS_PER_CHUNK],
            chunk_bricks_buf: buffer("chunk_bricks", (cells * BRICKS_PER_CHUNK * 4) as u64),
            brick_slots: SlotAllocator::new(desc.brick_capacity),
            brick_pool_buf: buffer(
                "brick_pool",
                u64::from(desc.brick_capacity.max(1)) * BRICK_BYTES,
            ),
            materials_buf: buffer(
                "materials",
                (MAX_MATERIALS * std::mem::size_of::<MaterialGpu>()) as u64,
            ),
            scratch: Box::new([MaterialId::AIR; BRICK_VOLUME]),
            packed: Vec::with_capacity(BRICK_VOLUME / 2),
        }
    }

    pub fn origin(&self) -> ChunkPos {
        self.origin
    }

    pub fn dims(&self) -> UVec3 {
        self.dims
    }

    pub fn bricks_in_use(&self) -> u32 {
        self.brick_slots.in_use()
    }

    pub fn brick_capacity(&self) -> u32 {
        self.brick_slots.capacity()
    }

    /// Uploads the material table. Entries beyond [`MAX_MATERIALS`] are ignored.
    pub fn set_materials(&mut self, queue: &wgpu::Queue, materials: &[MaterialGpu]) {
        if materials.len() > MAX_MATERIALS {
            tracing::error!(
                "{} materials exceed the GPU table size {MAX_MATERIALS}",
                materials.len()
            );
        }
        let n = materials.len().min(MAX_MATERIALS);
        queue.write_buffer(
            &self.materials_buf,
            0,
            bytemuck::cast_slice(&materials[..n]),
        );
    }

    fn cell_index(&self, pos: ChunkPos) -> Option<usize> {
        let rel = pos.0 - self.origin.0;
        let dims = self.dims.as_ivec3();
        if rel.cmplt(IVec3::ZERO).any() || rel.cmpge(dims).any() {
            return None;
        }
        Some((rel.x + rel.y * dims.x + rel.z * dims.x * dims.y) as usize)
    }

    /// Uploads the dirty bricks of `chunk` (and clears its dirty mask).
    /// Chunks outside the volume are ignored.
    pub fn upload_chunk(
        &mut self,
        queue: &wgpu::Queue,
        pos: ChunkPos,
        chunk: &mut Chunk,
    ) -> UploadStats {
        let mut stats = UploadStats::default();
        let Some(cell) = self.cell_index(pos) else {
            return stats;
        };
        let dirty = chunk.take_dirty_bricks();
        if dirty.is_empty() {
            return stats;
        }

        let mut slot = self.chunk_grid[cell];
        let mut new_chunk = false;
        if slot == EMPTY {
            if chunk.as_uniform().is_some_and(MaterialId::is_air) {
                return stats;
            }
            let Some(new_slot) = self.chunk_slots.alloc() else {
                tracing::warn!("no free chunk slot for {pos:?}");
                return stats;
            };
            slot = new_slot;
            self.chunk_grid[cell] = slot;
            self.chunk_grid_dirty = true;
            new_chunk = true;
        }

        let table_base = slot as usize * BRICKS_PER_CHUNK;
        // A freshly allocated slot must have its whole table written once, otherwise the
        // GPU would read stale or zeroed entries (zero is a valid brick slot).
        let mut table_changed = new_chunk;
        for brick in dirty.iter() {
            let occupied = match chunk.brick(brick) {
                BrickRef::Uniform(m) => {
                    self.scratch.fill(m);
                    !m.is_air()
                }
                BrickRef::Dense(voxels) => {
                    self.scratch.copy_from_slice(voxels);
                    voxels.iter().any(|m| !m.is_air())
                }
            };
            let entry = &mut self.chunk_bricks[table_base + brick];
            if occupied {
                if *entry == EMPTY {
                    let Some(brick_slot) = self.brick_slots.alloc() else {
                        tracing::warn!("brick pool full; dropping brick {brick} of {pos:?}");
                        continue;
                    };
                    *entry = brick_slot;
                    table_changed = true;
                }
                self.packed.clear();
                self.packed.extend(
                    self.scratch
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|[lo, hi]| u32::from(lo.0) | (u32::from(hi.0) << 16)),
                );
                queue.write_buffer(
                    &self.brick_pool_buf,
                    u64::from(*entry) * BRICK_BYTES,
                    bytemuck::cast_slice(&self.packed),
                );
                stats.bricks_written += 1;
                stats.bytes += BRICK_BYTES;
            } else if *entry != EMPTY {
                self.brick_slots.free(*entry);
                *entry = EMPTY;
                table_changed = true;
                stats.bricks_freed += 1;
            }
        }

        if table_changed {
            let table = &self.chunk_bricks[table_base..table_base + BRICKS_PER_CHUNK];
            queue.write_buffer(
                &self.chunk_bricks_buf,
                (table_base * 4) as u64,
                bytemuck::cast_slice(table),
            );
            stats.bytes += (BRICKS_PER_CHUNK * 4) as u64;
        }
        stats
    }

    /// Writes pending table changes. Call once per frame after the uploads.
    pub fn flush(&mut self, queue: &wgpu::Queue) {
        if self.chunk_grid_dirty {
            queue.write_buffer(
                &self.chunk_grid_buf,
                0,
                bytemuck::cast_slice(&self.chunk_grid),
            );
            self.chunk_grid_dirty = false;
        }
    }

    /// Bind group entries for bindings 1–4 of the raymarch shader.
    pub(crate) fn buffers(&self) -> [&wgpu::Buffer; 4] {
        [
            &self.chunk_grid_buf,
            &self.chunk_bricks_buf,
            &self.brick_pool_buf,
            &self.materials_buf,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_to_linear() {
        let m = MaterialGpu::from_srgba([0, 255, 188, 128], 2.0);
        assert_eq!(m.color[0], 0.0);
        assert!((m.color[1] - 1.0).abs() < 1e-6);
        assert!((m.color[2] - 0.5029).abs() < 1e-3);
        assert!((m.color[3] - 0.502).abs() < 1e-3);
        assert_eq!(m.params[0], 2.0);
    }
}
