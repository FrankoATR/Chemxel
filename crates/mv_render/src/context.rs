//! Ownership of the wgpu instance, adapter, device, queue and presentation surface.

use wgpu::{CurrentSurfaceTexture, SurfaceTarget};

use crate::RenderError;

/// Physical size of the drawable area in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceSize {
    pub width: u32,
    pub height: u32,
}

impl SurfaceSize {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// A zero-sized surface (e.g. minimised window) cannot be configured.
    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// GPU device plus the window surface it presents to.
#[derive(Debug)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl GpuContext {
    /// Creates the device and configures `target` for presentation.
    ///
    /// `target` is anything wgpu can present to (e.g. `Arc<winit::window::Window>`);
    /// the renderer does not depend on a windowing library.
    pub async fn new(
        target: impl Into<SurfaceTarget<'static>>,
        size: SurfaceSize,
        vsync: bool,
    ) -> Result<Self, RenderError> {
        // Backends and flags can be overridden with WGPU_BACKEND, WGPU_DEBUG, etc.
        // The display handle is only needed for GLES/Wayland, which we do not target.
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(target)?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let info = adapter.get_info();
        tracing::info!(
            "GPU adapter: {} ({:?}, {:?} backend, driver: {} {})",
            info.name,
            info.device_type,
            info.backend,
            info.driver,
            info.driver_info
        );

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("chemxel-device"),
                required_features: wgpu::Features::empty(),
                // Desktop-first: request what the hardware offers so large voxel
                // pools are not capped by the conservative WebGPU defaults.
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await?;
        device.on_uncaptured_error(std::sync::Arc::new(|error| {
            tracing::error!("wgpu uncaptured error: {error}");
        }));

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or(RenderError::UnsupportedSurface)?;
        let caps = surface.get_capabilities(&adapter);
        // Prefer an sRGB swapchain so shaders can output linear colour.
        if let Some(srgb) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
            config.format = srgb;
        }
        // Overlays (egui) blend in gamma space and want a non-sRGB view of the same image.
        let overlay = config.format.remove_srgb_suffix();
        if overlay != config.format {
            config.view_formats.push(overlay);
        }
        config.present_mode = if vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        tracing::info!(
            "surface: {}x{} {:?} {:?}",
            config.width,
            config.height,
            config.format,
            config.present_mode
        );
        surface.configure(&device, &config);

        Ok(Self {
            device,
            queue,
            adapter,
            surface,
            config,
        })
    }

    /// Reconfigures the surface after a window resize. Zero sizes are ignored.
    pub fn resize(&mut self, size: SurfaceSize) {
        if size.is_empty() {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn size(&self) -> SurfaceSize {
        SurfaceSize::new(self.config.width, self.config.height)
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Format of the swapchain view given to overlays: the non-sRGB variant of
    /// [`GpuContext::surface_format`].
    pub fn overlay_format(&self) -> wgpu::TextureFormat {
        self.config.format.remove_srgb_suffix()
    }

    /// Acquires the next swapchain image, or `None` if this frame should be skipped.
    /// Recoverable surface problems are handled here by reconfiguring.
    pub fn acquire_frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => Some(frame),
            CurrentSurfaceTexture::Suboptimal(frame) => {
                // Present this one, reconfigure for the next frame.
                self.surface.configure(&self.device, &self.config);
                Some(frame)
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => None,
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                tracing::debug!("surface outdated or lost; reconfiguring");
                self.surface.configure(&self.device, &self.config);
                None
            }
            CurrentSurfaceTexture::Validation => {
                tracing::error!("validation error while acquiring the surface texture");
                None
            }
        }
    }
}
