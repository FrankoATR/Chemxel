//! The voxel content currently shown. Milestone 1: a single test chunk.
//! Milestone 2 replaces this with a streamed, procedurally generated world.

use anyhow::Context as _;
use mv_core::ChunkPos;
use mv_materials::MaterialRegistry;
use mv_render::{MaterialGpu, Renderer, VolumeDesc};
use mv_world::Chunk;
use mv_world::testgen::{TestPalette, sine_terrain_scene};

#[derive(Debug)]
pub struct Scene {
    chunks: Vec<(ChunkPos, Chunk)>,
}

impl Scene {
    /// Builds the milestone 1 test scene.
    pub fn test_scene(registry: &MaterialRegistry) -> anyhow::Result<Self> {
        let palette = TestPalette::from_registry(registry)
            .map_err(|name| anyhow::anyhow!("test scene needs material \"{name}\""))
            .context("building the test scene")?;
        let pos = ChunkPos::new(0, 0, 0);
        Ok(Self {
            chunks: vec![(pos, sine_terrain_scene(pos, &palette))],
        })
    }

    /// GPU volume that covers this scene.
    pub fn volume_desc(&self, brick_capacity: u32) -> VolumeDesc {
        VolumeDesc {
            dims: glam::UVec3::ONE,
            chunk_capacity: 1,
            brick_capacity,
        }
    }

    /// Uploads every chunk's dirty bricks. Cheap when nothing changed.
    pub fn upload_dirty(&mut self, renderer: &mut Renderer) {
        for (pos, chunk) in &mut self.chunks {
            renderer.upload_chunk(*pos, chunk);
        }
    }
}

/// Shader-side material table, in `MaterialId` order.
pub fn gpu_materials(registry: &MaterialRegistry) -> Vec<MaterialGpu> {
    registry
        .iter()
        .map(|(_, m)| MaterialGpu::from_srgba(m.color, m.emissive))
        .collect()
}
