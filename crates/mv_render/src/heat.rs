//! Temperature data for the heat-map debug view (F5).
//!
//! The simulation keeps temperature on a coarse grid (16³ cells per chunk) and only
//! for chunks that deviate from ambient. Each frame the view is on, those fields are
//! packed into `heat_pool` and `heat_index` maps every chunk cell of the rendered
//! window to its field (or `EMPTY` = ambient).

use glam::{IVec3, UVec3};
use mv_core::ChunkPos;
use mv_core::consts::THERMAL_CELLS_PER_CHUNK;

use crate::gpu_volume::EMPTY;

/// Temperatures of one chunk's cells, x-fastest.
pub type HeatField = [f32; THERMAL_CELLS_PER_CHUNK];

const FIELD_BYTES: u64 = (THERMAL_CELLS_PER_CHUNK * 4) as u64;

#[derive(Debug)]
pub(crate) struct HeatVolume {
    index: Vec<u32>,
    index_buf: wgpu::Buffer,
    pool_buf: wgpu::Buffer,
    capacity: u32,
    warned: bool,
}

impl HeatVolume {
    pub fn new(device: &wgpu::Device, dims: UVec3, capacity: u32) -> Self {
        let cells = (dims.x * dims.y * dims.z) as usize;
        let max = (device.limits().max_storage_buffer_binding_size / FIELD_BYTES) as u32;
        let capacity = capacity.clamp(1, max.max(1));
        let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            index: vec![EMPTY; cells],
            index_buf: buffer("heat_index", (cells * 4) as u64),
            pool_buf: buffer("heat_pool", u64::from(capacity) * FIELD_BYTES),
            capacity,
            warned: false,
        }
    }

    /// Replaces the uploaded fields with `fields` (those outside the window at
    /// `origin` with size `dims` are skipped).
    pub fn upload<'a>(
        &mut self,
        queue: &wgpu::Queue,
        origin: ChunkPos,
        dims: UVec3,
        fields: impl Iterator<Item = (ChunkPos, &'a HeatField)>,
    ) {
        self.index.fill(EMPTY);
        let d = dims.as_ivec3();
        let mut slot = 0u32;
        for (pos, field) in fields {
            let rel = pos.0 - origin.0;
            if rel.cmplt(IVec3::ZERO).any() || rel.cmpge(d).any() {
                continue;
            }
            if slot >= self.capacity {
                if !self.warned {
                    tracing::warn!("heat view: more than {} thermal chunks", self.capacity);
                    self.warned = true;
                }
                break;
            }
            let cell = (rel.x + rel.y * d.x + rel.z * d.x * d.y) as usize;
            self.index[cell] = slot;
            queue.write_buffer(
                &self.pool_buf,
                u64::from(slot) * FIELD_BYTES,
                bytemuck::cast_slice(field),
            );
            slot += 1;
        }
        queue.write_buffer(&self.index_buf, 0, bytemuck::cast_slice(&self.index));
    }

    pub fn buffers(&self) -> [&wgpu::Buffer; 2] {
        [&self.index_buf, &self.pool_buf]
    }
}
