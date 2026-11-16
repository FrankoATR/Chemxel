//! Compute pass that raymarches the [`GpuVolume`] into an HDR texture.

use bytemuck::{Pod, Zeroable};

use crate::context::SurfaceSize;
use crate::gpu_volume::GpuVolume;

const SHADER: &str = include_str!("../../../assets/shaders/raymarch.wgsl");
const WORKGROUP: u32 = 8;
/// HDR intermediate format; also readable by the blit pass.
pub(crate) const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Per-frame constants. Matches `struct Frame` in `raymarch.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct FrameUniform {
    pub camera_pos: [f32; 4],
    pub forward: [f32; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
    /// x: tan(fov_y/2), y: aspect, z: width, w: height.
    pub projection: [f32; 4],
    pub sun_dir: [f32; 4],
    /// xyz: brush centre relative to the volume origin, w: radius (<= 0: no brush).
    pub brush: [f32; 4],
    /// x: fog coefficient k, y: fog start distance s, in `1 - exp(-((t - s) * k)^2)`.
    pub fog: [f32; 4],
    pub volume_dims: [u32; 4],
}

#[derive(Debug)]
pub(crate) struct RaymarchPass {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    output_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    size: SurfaceSize,
}

impl RaymarchPass {
    pub fn new(device: &wgpu::Device, volume: &GpuVolume, size: SurfaceSize) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("raymarch.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("raymarch"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(1),
                storage(2),
                storage(3),
                storage(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: OUTPUT_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("raymarch"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("raymarch"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame_uniform"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let output_view = create_output(device, size);
        let bind_group = create_bind_group(device, &layout, &uniform, volume, &output_view);
        Self {
            pipeline,
            layout,
            uniform,
            output_view,
            bind_group,
            size,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, volume: &GpuVolume, size: SurfaceSize) {
        if size == self.size || size.is_empty() {
            return;
        }
        self.size = size;
        self.output_view = create_output(device, size);
        self.bind_group = create_bind_group(
            device,
            &self.layout,
            &self.uniform,
            volume,
            &self.output_view,
        );
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output_view
    }

    pub fn dispatch(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &FrameUniform,
    ) {
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(frame));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("raymarch"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.dispatch_workgroups(
            self.size.width.div_ceil(WORKGROUP),
            self.size.height.div_ceil(WORKGROUP),
            1,
        );
    }
}

fn create_output(device: &wgpu::Device, size: SurfaceSize) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("raymarch_output"),
            size: wgpu::Extent3d {
                width: size.width.max(1),
                height: size.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    volume: &GpuVolume,
    output: &wgpu::TextureView,
) -> wgpu::BindGroup {
    let [grid, bricks, pool, materials] = volume.buffers();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("raymarch"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: grid.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: bricks.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pool.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: materials.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(output),
            },
        ],
    })
}
