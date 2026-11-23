//! Draws the working texture onto the preview layer unchanged, with no grade or look.

const PRESENT_WGSL: &str = include_str!("present.wgsl");

/// A surface on the `CAMetalLayer` a Mac shell hands the core. Windows and Linux shells hand
/// over no layer; there the core draws into images the shell shows in its own frame.
///
/// # Safety
/// `layer` is a live `CAMetalLayer` that outlives the surface.
#[cfg(target_os = "macos")]
pub(crate) unsafe fn layer_surface(
    instance: &wgpu::Instance,
    layer: *mut std::ffi::c_void,
) -> Result<wgpu::Surface<'static>, String> {
    // SAFETY: the caller's promise.
    unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer)) }
        .map_err(|cause| cause.to_string())
}

pub struct PresentPass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl PresentPass {
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview present"),
            source: wgpu::ShaderSource::Wgsl(PRESENT_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview present"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview present"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview present"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(target.into())],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("preview present"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            pipeline,
            sampler,
        }
    }

    /// The bind group that reads `working`. Made once per working texture.
    pub fn bind(&self, device: &wgpu::Device, working: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview present"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(working),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Records the working texture into `target`, fitted inside it with black bars.
    pub fn draw(
        &self,
        commands: &mut wgpu::CommandEncoder,
        bind: &wgpu::BindGroup,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
        source_size: (u32, u32),
    ) {
        let (target_width, target_height) = (target_size.0 as f32, target_size.1 as f32);
        let scale = (target_width / source_size.0 as f32).min(target_height / source_size.1 as f32);
        let (width, height) = (source_size.0 as f32 * scale, source_size.1 as f32 * scale);
        let mut pass = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("preview present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_viewport(
            (target_width - width) / 2.0,
            (target_height - height) / 2.0,
            width.max(1.0),
            height.max(1.0),
            0.0,
            1.0,
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
