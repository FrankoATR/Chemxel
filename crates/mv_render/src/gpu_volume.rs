//! GPU-side brickmap: the voxel data the raymarcher reads (ADR 003, ADR 005).
//!
//! Three levels, all in storage buffers (see `assets/shaders/raymarch.wgsl`):
//!
//! 1. `chunk_grid`: one entry per chunk cell of the rendered window. The window is a
//!    box of chunks that follows the camera ([`GpuVolume::set_origin`]).
//! 2. `chunk_bricks`: 512 entries per chunk slot.
//! 3. `brick_pool`: 512 `u16` materials per brick slot, packed two per `u32`.
//!
//! Grid and table entries share one encoding: [`EMPTY`] (air), [`UNIFORM_BIT`] `|`
//! material (a chunk or brick made of a single material, no further storage), or a
//! slot index into the next level. Uniform chunks and bricks therefore cost no pool
//! memory.
//!
//! Chunks are *resident* independently of the window: a chunk keeps its slots while
//! the window moves, and the grid is rebuilt from the resident set when the window
//! origin changes (it is small: one `u32` per chunk cell).

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{IVec3, UVec3};
use mv_core::ChunkPos;
use mv_core::consts::{BRICK_VOLUME, BRICKS_PER_CHUNK};
use mv_world::{BrickRef, Chunk, MaterialId};

use crate::slots::SlotAllocator;

/// "Nothing here" (air) in grid and table entries.
pub const EMPTY: u32 = u32::MAX;
/// Flag for "uniform chunk/brick"; the low 16 bits hold the material.
pub const UNIFORM_BIT: u32 = 1 << 31;
/// Bytes of one brick in the pool (512 × u16).
pub const BRICK_BYTES: u64 = (BRICK_VOLUME * 2) as u64;
/// Bytes of one chunk's brick table (512 × u32).
const TABLE_BYTES: u64 = (BRICKS_PER_CHUNK * 4) as u64;
/// Max entries of the material table.
pub const MAX_MATERIALS: usize = 4096;

/// Per-material data for the shader. Matches `struct Material` in WGSL.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct MaterialGpu {
    /// Linear RGB + alpha.
    pub color: [f32; 4],
    /// x: emissive intensity; y: 1 for liquids (Fresnel sky reflection); zw reserved.
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
    /// Size of the rendered window, in chunks.
    pub dims: UVec3,
    /// Max resident non-uniform chunks (2 KiB of table each).
    pub chunk_capacity: u32,
    /// Bricks the pool can hold (1 KiB each).
    pub brick_capacity: u32,
    /// Temperature fields the heat view can show (16 KiB each).
    pub heat_capacity: u32,
    /// Rigid bodies drawn at once, and their total voxels (2 bytes each).
    pub body_capacity: u32,
    pub body_voxel_capacity: u32,
}

/// Counters from uploads, for the debug overlay and budgets.
#[derive(Debug, Clone, Copy, Default)]
pub struct UploadStats {
    pub bricks_written: u32,
    pub bricks_freed: u32,
    pub bytes: u64,
}

impl std::ops::AddAssign for UploadStats {
    fn add_assign(&mut self, o: Self) {
        self.bricks_written += o.bricks_written;
        self.bricks_freed += o.bricks_freed;
        self.bytes += o.bytes;
    }
}

/// Occupancy of the GPU volume, for the debug overlay.
#[derive(Debug, Clone, Copy, Default)]
pub struct VolumeStats {
    pub resident_chunks: usize,
    pub chunk_slots_in_use: u32,
    pub chunk_slot_capacity: u32,
    pub bricks_in_use: u32,
    pub brick_capacity: u32,
}

impl VolumeStats {
    /// Bytes of brick pool in use.
    pub fn brick_bytes(&self) -> u64 {
        u64::from(self.bricks_in_use) * BRICK_BYTES
    }
}

/// How a resident chunk is stored on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resident {
    /// Single non-air material; lives only in the grid entry.
    Uniform(MaterialId),
    /// Has a brick table at this chunk slot.
    Slot(u32),
}

impl Resident {
    fn grid_entry(self) -> u32 {
        match self {
            Self::Uniform(m) => uniform_entry(m),
            Self::Slot(s) => s,
        }
    }
}

#[inline]
fn uniform_entry(m: MaterialId) -> u32 {
    if m.is_air() {
        EMPTY
    } else {
        UNIFORM_BIT | u32::from(m.0)
    }
}

#[inline]
fn is_slot(entry: u32) -> bool {
    entry & UNIFORM_BIT == 0
}

#[derive(Debug)]
pub struct GpuVolume {
    origin: ChunkPos,
    dims: UVec3,

    chunk_grid: Vec<u32>,
    chunk_grid_buf: wgpu::Buffer,
    chunk_grid_dirty: bool,

    resident: HashMap<ChunkPos, Resident>,
    chunk_slots: SlotAllocator,
    chunk_bricks: Vec<u32>,
    chunk_bricks_buf: wgpu::Buffer,

    brick_slots: SlotAllocator,
    brick_pool_buf: wgpu::Buffer,

    materials_buf: wgpu::Buffer,

    packed: Vec<u32>,
    pool_full_warned: bool,
}

/// Fill level as the shader sees it: 0 = full, else 1..=15 sixteenths of the voxel.
fn gpu_level(stored: u8) -> u32 {
    if stored == 0 {
        0
    } else {
        ((u32::from(stored) * 16 + 128) / u32::from(mv_world::LEVELS)).clamp(1, 15)
    }
}

impl GpuVolume {
    pub fn new(device: &wgpu::Device, desc: &VolumeDesc) -> Self {
        let limits = device.limits();
        let max_binding = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let clamp_capacity = |what: &str, wanted: u32, unit: u64| {
            let max = u32::try_from(max_binding / unit)
                .unwrap_or(u32::MAX)
                .min(UNIFORM_BIT - 1);
            if wanted > max {
                tracing::warn!("{what} capacity {wanted} exceeds the GPU limit; using {max}");
            }
            wanted.clamp(1, max)
        };
        let brick_capacity = clamp_capacity("brick pool", desc.brick_capacity, BRICK_BYTES);
        let chunk_capacity = clamp_capacity("chunk slot", desc.chunk_capacity, TABLE_BYTES);

        let cells = (desc.dims.x * desc.dims.y * desc.dims.z) as usize;
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
            "GPU volume: window {}x{}x{} chunks, {} chunk slots ({} MiB), brick pool {} bricks ({} MiB)",
            desc.dims.x,
            desc.dims.y,
            desc.dims.z,
            chunk_capacity,
            (u64::from(chunk_capacity) * TABLE_BYTES) >> 20,
            brick_capacity,
            (u64::from(brick_capacity) * BRICK_BYTES) >> 20
        );

        let tables = chunk_capacity as usize * BRICKS_PER_CHUNK;
        Self {
            origin: ChunkPos::default(),
            dims: desc.dims,
            chunk_grid: vec![EMPTY; cells],
            chunk_grid_buf: buffer("chunk_grid", (cells * 4) as u64),
            chunk_grid_dirty: true,
            resident: HashMap::new(),
            chunk_slots: SlotAllocator::new(chunk_capacity),
            chunk_bricks: vec![EMPTY; tables],
            chunk_bricks_buf: buffer("chunk_bricks", (tables * 4) as u64),
            brick_slots: SlotAllocator::new(brick_capacity),
            brick_pool_buf: buffer("brick_pool", u64::from(brick_capacity) * BRICK_BYTES),
            materials_buf: buffer(
                "materials",
                (MAX_MATERIALS * std::mem::size_of::<MaterialGpu>()) as u64,
            ),
            packed: Vec::with_capacity(BRICK_VOLUME / 2),
            pool_full_warned: false,
        }
    }

    /// Chunk at the window's minimum corner.
    pub fn origin(&self) -> ChunkPos {
        self.origin
    }

    pub fn dims(&self) -> UVec3 {
        self.dims
    }

    pub fn stats(&self) -> VolumeStats {
        VolumeStats {
            resident_chunks: self.resident.len(),
            chunk_slots_in_use: self.chunk_slots.in_use(),
            chunk_slot_capacity: self.chunk_slots.capacity(),
            bricks_in_use: self.brick_slots.in_use(),
            brick_capacity: self.brick_slots.capacity(),
        }
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

    /// Moves the rendered window so its minimum corner is `origin`.
    pub fn set_origin(&mut self, origin: ChunkPos) {
        if origin == self.origin {
            return;
        }
        self.origin = origin;
        self.chunk_grid.fill(EMPTY);
        for (&pos, &res) in &self.resident {
            if let Some(cell) = cell_index(self.origin, self.dims, pos) {
                self.chunk_grid[cell] = res.grid_entry();
            }
        }
        self.chunk_grid_dirty = true;
    }

    fn set_grid(&mut self, pos: ChunkPos, entry: u32) {
        if let Some(cell) = cell_index(self.origin, self.dims, pos) {
            self.chunk_grid[cell] = entry;
            self.chunk_grid_dirty = true;
        }
    }

    /// Frees the bricks and the table of a chunk slot.
    fn free_slot(&mut self, slot: u32) -> u32 {
        let base = slot as usize * BRICKS_PER_CHUNK;
        let mut freed = 0;
        for entry in &mut self.chunk_bricks[base..base + BRICKS_PER_CHUNK] {
            if is_slot(*entry) {
                self.brick_slots.free(*entry);
                freed += 1;
            }
            *entry = EMPTY;
        }
        self.chunk_slots.free(slot);
        freed
    }

    /// Drops the GPU copy of a chunk (e.g. it was unloaded).
    pub fn remove_chunk(&mut self, pos: ChunkPos) -> UploadStats {
        let mut stats = UploadStats::default();
        if let Some(res) = self.resident.remove(&pos) {
            if let Resident::Slot(slot) = res {
                stats.bricks_freed = self.free_slot(slot);
            }
            self.set_grid(pos, EMPTY);
        }
        stats
    }

    /// Uploads the dirty bricks of `chunk` and clears its dirty mask.
    pub fn upload_chunk(
        &mut self,
        queue: &wgpu::Queue,
        pos: ChunkPos,
        chunk: &mut Chunk,
    ) -> UploadStats {
        let mut stats = UploadStats::default();
        let dirty = chunk.take_dirty_bricks();
        let current = self.resident.get(&pos).copied();

        if let Some(material) = chunk.as_uniform() {
            if dirty.is_empty() && current.is_some() {
                return stats;
            }
            if let Some(Resident::Slot(slot)) = current {
                stats.bricks_freed = self.free_slot(slot);
            }
            if material.is_air() {
                self.resident.remove(&pos);
            } else {
                self.resident.insert(pos, Resident::Uniform(material));
            }
            self.set_grid(pos, uniform_entry(material));
            return stats;
        }

        let (slot, all) = match current {
            Some(Resident::Slot(slot)) => {
                if dirty.is_empty() {
                    return stats;
                }
                (slot, false)
            }
            _ => {
                let Some(slot) = self.chunk_slots.alloc() else {
                    tracing::warn!("no free GPU chunk slot for {pos:?}");
                    chunk.mark_all_dirty();
                    return stats;
                };
                self.resident.insert(pos, Resident::Slot(slot));
                self.set_grid(pos, slot);
                // A new table starts all-EMPTY on the CPU; every brick must be written,
                // not just the dirty ones (the chunk may have been uniform until now).
                (slot, true)
            }
        };

        let base = slot as usize * BRICKS_PER_CHUNK;
        let mut table_changed = all;
        let bricks = if all {
            mv_world::BrickMask::FULL
        } else {
            dirty
        };
        for brick in bricks.iter() {
            let view = chunk.brick(brick);
            let entry = self.chunk_bricks[base + brick];
            // Bricks with partly filled liquid voxels are always uploaded dense.
            let levels = chunk.brick_levels(brick);
            let uniform = if levels.is_some() {
                None
            } else {
                view.uniform_material()
            };
            let new_entry = match uniform {
                Some(m) => {
                    if is_slot(entry) {
                        self.brick_slots.free(entry);
                        stats.bricks_freed += 1;
                    }
                    uniform_entry(m)
                }
                None => {
                    let pool_slot = if is_slot(entry) {
                        entry
                    } else if let Some(s) = self.brick_slots.alloc() {
                        s
                    } else {
                        if !self.pool_full_warned {
                            tracing::warn!(
                                "GPU brick pool full ({} bricks); raise render.brick_pool_capacity",
                                self.brick_slots.capacity()
                            );
                            self.pool_full_warned = true;
                        }
                        continue;
                    };
                    let BrickRef::Dense(voxels) = view else {
                        unreachable!("non-uniform brick view is dense");
                    };
                    // 16 bits per voxel: material in the low 12, fill level in the
                    // high 4 (0 = full; ADR 014).
                    let gpu = |i: usize| {
                        let level = levels.map_or(0, |l| gpu_level(l[i]));
                        u32::from(voxels[i].0 & 0x0fff) | (level << 12)
                    };
                    self.packed.clear();
                    self.packed
                        .extend((0..voxels.len() / 2).map(|k| gpu(2 * k) | (gpu(2 * k + 1) << 16)));
                    queue.write_buffer(
                        &self.brick_pool_buf,
                        u64::from(pool_slot) * BRICK_BYTES,
                        bytemuck::cast_slice(&self.packed),
                    );
                    stats.bricks_written += 1;
                    stats.bytes += BRICK_BYTES;
                    pool_slot
                }
            };
            if new_entry != entry {
                self.chunk_bricks[base + brick] = new_entry;
                table_changed = true;
            }
        }

        if table_changed {
            let table = &self.chunk_bricks[base..base + BRICKS_PER_CHUNK];
            queue.write_buffer(
                &self.chunk_bricks_buf,
                base as u64 * 4,
                bytemuck::cast_slice(table),
            );
            stats.bytes += TABLE_BYTES;
        }
        stats
    }

    /// Writes pending grid changes. Call once per frame after the uploads.
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

/// Grid cell of `pos` in a window starting at `origin`, if inside.
fn cell_index(origin: ChunkPos, dims: UVec3, pos: ChunkPos) -> Option<usize> {
    let rel = pos.0 - origin.0;
    let dims = dims.as_ivec3();
    if rel.cmplt(IVec3::ZERO).any() || rel.cmpge(dims).any() {
        return None;
    }
    Some((rel.x + rel.y * dims.x + rel.z * dims.x * dims.y) as usize)
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

    #[test]
    fn entry_encoding() {
        assert_eq!(uniform_entry(MaterialId::AIR), EMPTY);
        let e = uniform_entry(MaterialId(7));
        assert!(!is_slot(e));
        assert_eq!(e & 0xffff, 7);
        assert!(!is_slot(EMPTY));
        assert!(is_slot(0));
    }

    #[test]
    fn cell_index_window() {
        let dims = UVec3::new(3, 2, 3);
        let origin = ChunkPos::new(-1, -1, -1);
        assert_eq!(cell_index(origin, dims, ChunkPos::new(-1, -1, -1)), Some(0));
        assert_eq!(cell_index(origin, dims, ChunkPos::new(1, 0, 1)), Some(17));
        assert_eq!(cell_index(origin, dims, ChunkPos::new(2, 0, 0)), None);
        assert_eq!(cell_index(origin, dims, ChunkPos::new(0, 1, 0)), None);
        assert_eq!(cell_index(origin, dims, ChunkPos::new(0, 0, -2)), None);
    }
}
