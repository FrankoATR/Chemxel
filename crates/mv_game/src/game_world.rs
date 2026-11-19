//! The streamed voxel world and its synchronisation with the GPU.
//!
//! Each frame: stream chunks around the camera, then mirror world changes on the GPU
//! in priority order: unloads, edits (never delayed, so they show up the same
//! frame), then newly loaded chunks up to the per-frame upload budget.

use std::sync::Arc;

use anyhow::Context as _;
use glam::{DVec3, IVec3, UVec3};
use mv_core::{ChunkPos, Config, WorldPos};
use mv_materials::MaterialRegistry;
use mv_render::{MaterialGpu, Renderer, UploadStats, VolumeDesc};
use mv_world::World;
use mv_world::stream::{StreamSettings, StreamStats, Streamer};
use mv_world::terrain::{TerrainGenerator, TerrainPalette};

/// What one [`GameWorld::update`] did, for the debug overlay.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncStats {
    pub upload: UploadStats,
    pub chunks_uploaded: u32,
    pub pending_uploads: usize,
    pub stream: StreamStats,
}

#[derive(Debug)]
pub struct GameWorld {
    pub world: World,
    streamer: Streamer,
    upload_budget: u64,
    render_radius: UVec3,
}

impl GameWorld {
    pub fn new(config: &Config, registry: &MaterialRegistry) -> anyhow::Result<Self> {
        let palette = TerrainPalette::from_registry(registry)
            .map_err(|name| anyhow::anyhow!("terrain needs material \"{name}\""))
            .context("building the terrain generator")?;
        let generator = Arc::new(TerrainGenerator::new(
            &config.world,
            config.sim.seed,
            palette,
        ));
        let r = &config.render;
        let settings = StreamSettings {
            radius: r.render_distance_chunks,
            vertical_radius: r.render_height_chunks,
            unload_margin: config.world.unload_margin_chunks,
            max_inflight: config.world.max_inflight_jobs as usize,
        };
        Ok(Self {
            world: World::new(),
            streamer: Streamer::new(generator, settings),
            upload_budget: u64::from(r.upload_budget_kib) * 1024,
            render_radius: UVec3::new(
                r.render_distance_chunks,
                r.render_height_chunks,
                r.render_distance_chunks,
            ),
        })
    }

    /// Saves modified chunks to `store` on unload and loads saved ones from it.
    pub fn set_store(&mut self, store: Arc<mv_world::store::ChunkStore>) {
        self.streamer.set_store(store);
    }

    /// Modified chunks that came back (from disk or memory) since the last call.
    pub fn take_restored(&mut self) -> Vec<ChunkPos> {
        self.streamer.take_restored()
    }

    /// Writes every loaded chunk with unsaved changes to the store. Returns the
    /// number of chunks and bytes written.
    pub fn save_chunks(&mut self) -> anyhow::Result<(usize, usize)> {
        let Some(store) = self.streamer.store().cloned() else {
            return Ok((0, 0));
        };
        let positions: Vec<ChunkPos> = self
            .world
            .iter()
            .filter(|(_, c)| c.is_unsaved())
            .map(|(p, _)| p)
            .collect();
        let (mut chunks, mut bytes) = (0, 0);
        for pos in positions {
            if let Some(chunk) = self.world.chunk_mut(pos) {
                bytes += store
                    .save(pos, chunk)
                    .with_context(|| format!("saving chunk {pos:?}"))?;
                chunks += 1;
            }
        }
        Ok((chunks, bytes))
    }

    /// GPU volume that covers the render distance around the camera.
    pub fn volume_desc(config: &Config) -> VolumeDesc {
        let r = &config.render;
        let h = r.render_distance_chunks * 2 + 1;
        VolumeDesc {
            dims: UVec3::new(h, r.render_height_chunks * 2 + 1, h),
            chunk_capacity: r.chunk_slot_capacity,
            brick_capacity: r.brick_pool_capacity,
            heat_capacity: r.heat_view_capacity,
            body_capacity: config.physics.max_bodies,
            body_voxel_capacity: config.physics.max_body_voxels,
        }
    }

    /// Structural integrity anchor: solids more than `depth` voxels below the
    /// generated terrain surface never fall (ADR 009). Uses the generator's cached
    /// column heights, with the last column kept at hand.
    pub fn anchor(&self, depth: i32) -> mv_sim::Anchor {
        let generator = std::sync::Arc::clone(self.streamer.generator());
        let last: std::sync::Mutex<
            Option<(
                glam::IVec2,
                std::sync::Arc<mv_world::terrain::ColumnHeights>,
            )>,
        > = std::sync::Mutex::new(None);
        Box::new(move |p: mv_core::WorldPos| {
            let column = glam::IVec2::new((p.0.x >> 6) as i32, (p.0.z >> 6) as i32);
            let mut cache = last.lock().unwrap_or_else(|e| e.into_inner());
            if cache.as_ref().is_none_or(|(c, _)| *c != column) {
                *cache = Some((column, generator.column(column)));
            }
            let Some((_, heights)) = cache.as_ref() else {
                return false;
            };
            let surface = heights.get((p.0.x & 63) as usize, (p.0.z & 63) as usize);
            p.0.y < i64::from(surface) - i64::from(depth)
        })
    }

    /// A camera position a few metres above the terrain at world column `(x, z)`.
    pub fn spawn_point(&self, x: i64, z: i64) -> DVec3 {
        let g = self.streamer.generator();
        let ground = g.height_at(x, z).max(g.sea_level());
        DVec3::new(x as f64 + 0.5, f64::from(ground) + 48.0, z as f64 + 0.5)
    }

    /// Whether the region around the camera is fully loaded and uploaded.
    pub fn is_idle(&self) -> bool {
        self.streamer.is_idle() && self.world.pending_loaded() == 0
    }

    /// Streams around `camera` and uploads changes to the renderer.
    pub fn update(&mut self, camera: DVec3, renderer: &mut Renderer) -> SyncStats {
        let _span = tracing::trace_span!("world_update").entered();
        let center = WorldPos(camera.floor().as_i64vec3()).chunk();
        self.streamer.update(center, &mut self.world);
        renderer.set_volume_origin(ChunkPos(center.0 - self.render_radius.as_ivec3()));

        let mut stats = SyncStats::default();
        for pos in self.world.take_unloaded() {
            stats.upload += renderer.remove_chunk(pos);
        }
        for pos in self.world.take_edited() {
            if let Some(chunk) = self.world.chunk_mut(pos) {
                stats.upload += renderer.upload_chunk(pos, chunk);
                stats.chunks_uploaded += 1;
            }
        }
        let budget_end = stats.upload.bytes + self.upload_budget;
        while stats.upload.bytes < budget_end {
            let Some(pos) = self.world.pop_loaded() else {
                break;
            };
            if let Some(chunk) = self.world.chunk_mut(pos) {
                stats.upload += renderer.upload_chunk(pos, chunk);
                stats.chunks_uploaded += 1;
            }
        }
        stats.pending_uploads = self.world.pending_loaded();
        stats.stream = self.streamer.stats();
        stats
    }

    /// Chunk containing `pos`, for display.
    pub fn chunk_of(pos: DVec3) -> IVec3 {
        WorldPos(pos.floor().as_i64vec3()).chunk().0
    }
}

/// Shader-side material table, in `MaterialId` order.
pub fn gpu_materials(registry: &MaterialRegistry) -> Vec<MaterialGpu> {
    registry
        .iter()
        .map(|(_, m)| {
            let mut gpu = MaterialGpu::from_srgba(m.color, m.emissive);
            gpu.params[1] = f32::from(u8::from(m.state == mv_materials::MatterState::Liquid));
            gpu
        })
        .collect()
}
