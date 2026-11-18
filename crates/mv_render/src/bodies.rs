//! Rigid bodies on the GPU (milestone 6, ADR 009).
//!
//! Each body is a small dense voxel grid with a transform. `body_headers` holds, per
//! body, its inverse rotation (as three rows), its position relative to the volume
//! window and its size plus the offset of its voxels in `body_voxels` (two `u16`
//! material ids per `u32`, like the brick pool). Voxel data is repacked only when the
//! set of bodies changes; transforms are rewritten every frame.

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Mat3, Quat};
use mv_world::MaterialId;

/// A body to draw this frame.
#[derive(Debug, Clone, Copy)]
pub struct BodyInstance<'a> {
    /// Stable id: voxels are only re-uploaded when the set of ids changes.
    pub id: u64,
    /// Grid size in voxels.
    pub size: [u32; 3],
    /// Materials, x fastest (air = empty).
    pub voxels: &'a [MaterialId],
    /// World position (microvoxels) of the grid's minimum corner frame: voxel `g`
    /// is centred at `position + rotation * (g + 0.5)`.
    pub position: DVec3,
    pub rotation: Quat,
}

/// Matches `struct Body` in `raymarch.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BodyGpu {
    /// Rows of the inverse rotation (world → body).
    rows: [[f32; 4]; 3],
    /// xyz: position relative to the volume origin.
    position: [f32; 4],
    /// xyz: size; w: offset of the first voxel (in u16 units).
    size: [u32; 4],
}

#[derive(Debug)]
pub(crate) struct BodyVolume {
    headers: wgpu::Buffer,
    voxels: wgpu::Buffer,
    max_bodies: u32,
    /// Capacity in voxels.
    voxel_capacity: u32,
    /// Uploaded bodies: (id, offset), in upload order.
    resident: Vec<(u64, u32)>,
    count: u32,
    scratch: Vec<u32>,
    warned: bool,
}

impl BodyVolume {
    pub fn new(device: &wgpu::Device, max_bodies: u32, voxel_capacity: u32) -> Self {
        let max_bytes = device.limits().max_storage_buffer_binding_size;
        let voxel_capacity =
            voxel_capacity.clamp(2, (max_bytes * 2).min(u64::from(u32::MAX)) as u32);
        let max_bodies = max_bodies.max(1);
        let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        Self {
            headers: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("body_headers"),
                size: u64::from(max_bodies) * std::mem::size_of::<BodyGpu>() as u64,
                usage,
                mapped_at_creation: false,
            }),
            voxels: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("body_voxels"),
                size: u64::from(voxel_capacity.div_ceil(2)) * 4,
                usage,
                mapped_at_creation: false,
            }),
            max_bodies,
            voxel_capacity,
            resident: Vec::new(),
            count: 0,
            scratch: Vec::new(),
            warned: false,
        }
    }

    pub fn buffers(&self) -> [&wgpu::Buffer; 2] {
        [&self.headers, &self.voxels]
    }

    /// Bodies uploaded by the last call to [`BodyVolume::upload`].
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Uploads this frame's bodies. `origin` is the volume window's minimum corner
    /// in world microvoxels.
    pub fn upload(&mut self, queue: &wgpu::Queue, origin: DVec3, bodies: &[BodyInstance<'_>]) {
        let fits =
            |b: &BodyInstance<'_>| b.voxels.len() == (b.size[0] * b.size[1] * b.size[2]) as usize;
        let same_set = self.resident.len() == bodies.len()
            && self.resident.iter().zip(bodies).all(|(r, b)| r.0 == b.id);
        if !same_set {
            // Repack every body's voxels (only happens when bodies appear or leave).
            self.resident.clear();
            let mut offset = 0u32;
            for b in bodies.iter().take(self.max_bodies as usize) {
                let n = b.voxels.len() as u32;
                if !fits(b) || offset + n > self.voxel_capacity {
                    if !self.warned {
                        tracing::warn!("body voxel storage full ({} voxels)", self.voxel_capacity);
                        self.warned = true;
                    }
                    break;
                }
                self.scratch.clear();
                self.scratch.extend(b.voxels.chunks(2).map(|pair| {
                    u32::from(pair[0].0) | (u32::from(pair.get(1).map_or(0, |m| m.0)) << 16)
                }));
                queue.write_buffer(
                    &self.voxels,
                    u64::from(offset / 2) * 4,
                    bytemuck::cast_slice(&self.scratch),
                );
                self.resident.push((b.id, offset));
                // Keep offsets even so every body starts on a whole u32.
                offset += n + (n & 1);
            }
        }
        let headers: Vec<BodyGpu> = bodies
            .iter()
            .zip(&self.resident)
            .map(|(b, &(_, offset))| {
                let r = Mat3::from_quat(b.rotation);
                let p = (b.position - origin).as_vec3();
                BodyGpu {
                    rows: [
                        [r.x_axis.x, r.x_axis.y, r.x_axis.z, 0.0],
                        [r.y_axis.x, r.y_axis.y, r.y_axis.z, 0.0],
                        [r.z_axis.x, r.z_axis.y, r.z_axis.z, 0.0],
                    ],
                    position: [p.x, p.y, p.z, 0.0],
                    size: [b.size[0], b.size[1], b.size[2], offset],
                }
            })
            .collect();
        self.count = headers.len() as u32;
        if !headers.is_empty() {
            queue.write_buffer(&self.headers, 0, bytemuck::cast_slice(&headers));
        }
    }
}
