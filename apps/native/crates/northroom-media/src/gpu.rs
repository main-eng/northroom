//! GPU colour conversion with wgpu on Metal (Direct3D 12 on Windows, Vulkan on Linux). Pixel
//! buffers are wrapped as textures on their IOSurface (no copy) on a Mac and uploaded from memory
//! elsewhere. Decoded frames are converted from their stream's colour into the working space,
//! and the working texture is converted into an encoder surface's Y'CbCr planes.

use crate::color::{
    input_transform_uniform, output_transform_uniform, ColorDescription, OutputColor,
    TRANSFORM_UNIFORM_FLOATS,
};
use crate::pixel_buffer::{PixelBuffer, PixelLayout};
use crate::{media_error, MediaError, MediaResult};
#[cfg(target_os = "macos")]
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTextureDescriptor, MTLTextureType,
    MTLTextureUsage,
};

/// How a working texture holds the working space's linear light (see `color`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkingFormat {
    /// Half floats. Keeps light above SDR white and colours outside BT.709, for HDR display and
    /// encoding.
    Float,
    /// 8-bit sRGB, clipped to SDR white and BT.709, at half the memory. For display on an 8-bit
    /// sRGB screen, which clips there anyway.
    Srgb8,
}

impl WorkingFormat {
    const ALL: [Self; 2] = [Self::Float, Self::Srgb8];

    pub fn texture_format(self) -> wgpu::TextureFormat {
        match self {
            Self::Float => wgpu::TextureFormat::Rgba16Float,
            Self::Srgb8 => wgpu::TextureFormat::Rgba8UnormSrgb,
        }
    }
}
/// Full-resolution chroma (signed, around zero) between encoding a frame and subsampling it.
const FULL_CHROMA_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

/// The uniform block and the curves both transforms use. `color::pack_uniform` fills the block.
const TRANSFORM_UNIFORM: &str = r"
struct Transform {
    // Stored luma: offset and scale (into 0..1 on input, out of it on output).
    luma: vec2<f32>,
    // Stored chroma: offset and scale (into -0.5..0.5 on input, out of it on output).
    chroma: vec2<f32>,
    // Red and blue luma weights of the Y'CbCr matrix.
    luma_weights: vec2<f32>,
    // Chroma lookup shift for the stream's chroma siting, in chroma texels (input only).
    chroma_shift: vec2<f32>,
    // 0 BT.709, 1 sRGB, 2 PQ, 3 HLG.
    transfer: f32,
    // Working-space value of the curve's 1.0 on input; its reciprocal on output.
    white_scale: f32,
    // Linear RGB between the stream's primaries and the working primaries.
    primaries: mat3x3<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: VertexOutput;
    output.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    output.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return output;
}

// The SDR curves are odd functions here, so colours outside the gamut (negative) survive.
// A stream coded with the BT.709 curve is read and written as the Mac shows it: a plain power
// of 1.961 (see `Transfer::Bt709`).
fn bt709_to_linear(value: vec3<f32>) -> vec3<f32> {
    return sign(value) * pow(abs(value), vec3<f32>(1.961));
}

fn linear_to_bt709(value: vec3<f32>) -> vec3<f32> {
    return sign(value) * pow(abs(value), vec3<f32>(1.0 / 1.961));
}

fn srgb_to_linear(value: vec3<f32>) -> vec3<f32> {
    let size = abs(value);
    let toe = size / 12.92;
    let curve = pow((size + 0.055) / 1.055, vec3<f32>(2.4));
    return sign(value) * select(curve, toe, size <= vec3<f32>(0.04045));
}

fn linear_to_srgb(value: vec3<f32>) -> vec3<f32> {
    let size = abs(value);
    let toe = size * 12.92;
    let curve = 1.055 * pow(size, vec3<f32>(1.0 / 2.4)) - 0.055;
    return sign(value) * select(curve, toe, size <= vec3<f32>(0.0031308));
}

// PQ (SMPTE ST 2084): 1.0 is 10 000 nits.
fn pq_to_linear(value: vec3<f32>) -> vec3<f32> {
    let power = pow(max(value, vec3<f32>(0.0)), vec3<f32>(1.0 / 78.84375));
    let ratio = max(power - 0.8359375, vec3<f32>(0.0)) / (18.8515625 - 18.6875 * power);
    return pow(ratio, vec3<f32>(1.0 / 0.1593017578125));
}

fn linear_to_pq(value: vec3<f32>) -> vec3<f32> {
    let power = pow(max(value, vec3<f32>(0.0)), vec3<f32>(0.1593017578125));
    return pow((0.8359375 + 18.8515625 * power) / (1.0 + 18.6875 * power), vec3<f32>(78.84375));
}

// HLG (ARIB STD-B67): scene light, 1.0 at full signal.
fn hlg_to_linear(value: vec3<f32>) -> vec3<f32> {
    let signal = max(value, vec3<f32>(0.0));
    let low = signal * signal / 3.0;
    let high = (exp((signal - 0.55991073) / 0.17883277) + 0.28466892) / 12.0;
    return select(high, low, signal <= vec3<f32>(0.5));
}

fn linear_to_hlg(value: vec3<f32>) -> vec3<f32> {
    let light = max(value, vec3<f32>(0.0));
    let low = sqrt(3.0 * light);
    let high = 0.17883277 * log(max(12.0 * light - 0.28466892, vec3<f32>(0.000001))) + 0.55991073;
    return select(high, low, light <= vec3<f32>(1.0 / 12.0));
}

fn to_linear(value: vec3<f32>, transfer: f32) -> vec3<f32> {
    if transfer < 0.5 { return bt709_to_linear(value); }
    if transfer < 1.5 { return srgb_to_linear(value); }
    if transfer < 2.5 { return pq_to_linear(value); }
    return hlg_to_linear(value);
}

fn from_linear(value: vec3<f32>, transfer: f32) -> vec3<f32> {
    if transfer < 0.5 { return linear_to_bt709(value); }
    if transfer < 1.5 { return linear_to_srgb(value); }
    if transfer < 2.5 { return linear_to_pq(value); }
    return linear_to_hlg(value);
}
";

/// Stored Y'CbCr planes to the working space.
const INPUT_SHADER: &str = r"
@group(0) @binding(0) var luma_plane: texture_2d<f32>;
@group(0) @binding(1) var chroma_plane: texture_2d<f32>;
@group(0) @binding(2) var plane_sampler: sampler;
@group(1) @binding(0) var<uniform> transform: Transform;

@fragment
fn video_fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let luma = (textureSample(luma_plane, plane_sampler, input.uv).r - transform.luma.x) * transform.luma.y;
    // Chroma is interpolated by the sampler (bilinear), moved to where the stream sites it.
    let chroma_uv = input.uv + transform.chroma_shift / vec2<f32>(textureDimensions(chroma_plane));
    let chroma = (textureSample(chroma_plane, plane_sampler, chroma_uv).rg - transform.chroma.x) * transform.chroma.y;
    let red_weight = transform.luma_weights.x;
    let blue_weight = transform.luma_weights.y;
    let red = luma + 2.0 * (1.0 - red_weight) * chroma.y;
    let blue = luma + 2.0 * (1.0 - blue_weight) * chroma.x;
    let green = (luma - red_weight * red - blue_weight * blue) / (1.0 - red_weight - blue_weight);
    let light = to_linear(vec3<f32>(red, green, blue), transform.transfer) * transform.white_scale;
    return vec4<f32>(transform.primaries * light, 1.0);
}
";

/// The working space to Y'CbCr: stored luma, and chroma at full resolution for `SUBSAMPLE_SHADER`.
const ENCODE_SHADER: &str = r"
@group(0) @binding(0) var working: texture_2d<f32>;
@group(0) @binding(1) var<uniform> transform: Transform;

struct EncodedPixel {
    // The stored luma sample.
    @location(0) luma: vec4<f32>,
    // Cb and Cr in -0.5..0.5.
    @location(1) chroma: vec4<f32>,
}

@fragment
fn encode_fragment(input: VertexOutput) -> EncodedPixel {
    let pixel = vec2<i32>(input.position.xy);
    let light = (transform.primaries * textureLoad(working, pixel, 0).rgb) * transform.white_scale;
    let rgb = from_linear(light, transform.transfer);
    let red_weight = transform.luma_weights.x;
    let blue_weight = transform.luma_weights.y;
    let luma = red_weight * rgb.r + (1.0 - red_weight - blue_weight) * rgb.g + blue_weight * rgb.b;
    var encoded: EncodedPixel;
    encoded.luma = vec4<f32>(transform.luma.x + luma * transform.luma.y, 0.0, 0.0, 1.0);
    encoded.chroma = vec4<f32>(
        (rgb.b - luma) / (2.0 * (1.0 - blue_weight)),
        (rgb.r - luma) / (2.0 * (1.0 - red_weight)),
        0.0,
        1.0,
    );
    return encoded;
}
";

/// Full-resolution chroma to the stored half-size plane. A chroma sample sits on the left
/// column of its 2×2 luma pixels, between the two rows. Chroma is filtered after encoding, as
/// the standards define subsampling, with half-band filters that keep more detail than a box
/// while still preventing aliasing from full-resolution graphics.
const SUBSAMPLE_SHADER: &str = r"
@group(0) @binding(0) var full_chroma: texture_2d<f32>;
@group(0) @binding(1) var<uniform> transform: Transform;

@fragment
fn chroma_fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    // Across: centred on the left column. Down: centred between the two rows.
    var column_offsets = array<i32, 5>(-3, -1, 0, 1, 3);
    var column_weights = array<f32, 5>(-1.0 / 32.0, 9.0 / 32.0, 16.0 / 32.0, 9.0 / 32.0, -1.0 / 32.0);
    var row_weights = array<f32, 4>(-1.0 / 16.0, 9.0 / 16.0, 9.0 / 16.0, -1.0 / 16.0);
    let last = vec2<i32>(textureDimensions(full_chroma)) - vec2<i32>(1);
    let block = vec2<i32>(input.position.xy) * 2;
    var chroma = vec2<f32>(0.0);
    for (var row = 0; row < 4; row++) {
        for (var column = 0; column < 5; column++) {
            let pixel = clamp(block + vec2<i32>(column_offsets[column], row - 1), vec2<i32>(0), last);
            chroma += column_weights[column] * row_weights[row] * textureLoad(full_chroma, pixel, 0).rg;
        }
    }
    return vec4<f32>(transform.chroma.x + chroma * transform.chroma.y, 0.0, 1.0);
}
";

/// A source's colour, ready to convert its frames into the working space.
pub struct InputTransform {
    bind_group: wgpu::BindGroup,
    layout: PixelLayout,
}

/// An export's colour, bound to the working texture it reads.
pub struct OutputTransform {
    encode_bind_group: wgpu::BindGroup,
    subsample_bind_group: wgpu::BindGroup,
    full_chroma: wgpu::TextureView,
    layout: PixelLayout,
}

/// The luma and chroma planes of a decoded frame as textures on the frame's own surface.
pub struct VideoFrameTextures {
    bind_group: wgpu::BindGroup,
    layout: PixelLayout,
}

/// The luma and chroma planes of an encoder input surface as render targets.
pub struct EncoderSurfaceTextures {
    luma: wgpu::TextureView,
    chroma: wgpu::TextureView,
    layout: PixelLayout,
}

/// How one plane of a pixel buffer stores its samples.
#[derive(Clone, Copy)]
enum PlaneFormat {
    R8,
    Rg8,
    R16,
    Rg16,
}

impl PlaneFormat {
    fn wgpu(self) -> wgpu::TextureFormat {
        match self {
            Self::R8 => wgpu::TextureFormat::R8Unorm,
            Self::Rg8 => wgpu::TextureFormat::Rg8Unorm,
            Self::R16 => wgpu::TextureFormat::R16Unorm,
            Self::Rg16 => wgpu::TextureFormat::Rg16Unorm,
        }
    }

    #[cfg(target_os = "macos")]
    fn metal(self) -> MTLPixelFormat {
        match self {
            Self::R8 => MTLPixelFormat::R8Unorm,
            Self::Rg8 => MTLPixelFormat::RG8Unorm,
            Self::R16 => MTLPixelFormat::R16Unorm,
            Self::Rg16 => MTLPixelFormat::RG16Unorm,
        }
    }
}

/// The formats of a layout's luma and chroma planes.
fn plane_formats(layout: PixelLayout) -> (PlaneFormat, PlaneFormat) {
    match layout {
        PixelLayout::Nv12 { .. } => (PlaneFormat::R8, PlaneFormat::Rg8),
        PixelLayout::P010 { .. } => (PlaneFormat::R16, PlaneFormat::Rg16),
    }
}

/// The pipelines that write one plane layout: `encode` fills the luma plane and the
/// full-resolution chroma, `subsample` fills the chroma plane from that.
struct OutputPipelines {
    bit_depth: u32,
    encode: wgpu::RenderPipeline,
    subsample: wgpu::RenderPipeline,
}

pub struct Gpu {
    /// Kept so a host can create a surface for its own layer on this device.
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    sampler: wgpu::Sampler,
    planes_layout: wgpu::BindGroupLayout,
    input_transform_layout: wgpu::BindGroupLayout,
    output_layout: wgpu::BindGroupLayout,
    /// One per working format, in `WorkingFormat::ALL` order.
    input_pipelines: [wgpu::RenderPipeline; 2],
    output_pipelines: Vec<OutputPipelines>,
}

impl Gpu {
    pub fn new() -> MediaResult<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|cause| MediaError(format!("no graphics adapter: {cause}")))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("northroom-media"),
            // 10-bit planes are 16-bit normalised textures. WebGPU only promises to sample
            // those; Metal can also render into them, which the adapter-specific flag allows.
            required_features: wgpu::Features::TEXTURE_FORMAT_16BIT_NORM
                | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
            ..Default::default()
        }))
        .map_err(|cause| MediaError(format!("open GPU device: {cause}")))?;

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("plane sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let planes_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video planes"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let input_transform_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("input transform"),
                entries: &[uniform_entry(0)],
            });
        let output_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("output transform"),
            entries: &[texture_entry(0), uniform_entry(1)],
        });

        let input_source = format!("{TRANSFORM_UNIFORM}{INPUT_SHADER}");
        let input_pipelines = WorkingFormat::ALL.map(|format| {
            create_fullscreen_pipeline(
                &device,
                &[&planes_layout, &input_transform_layout],
                &input_source,
                "video_fragment",
                &[format.texture_format()],
            )
        });
        let encode_source = format!("{TRANSFORM_UNIFORM}{ENCODE_SHADER}");
        let subsample_source = format!("{TRANSFORM_UNIFORM}{SUBSAMPLE_SHADER}");
        let output_pipelines = [
            PixelLayout::Nv12 { full_range: false },
            PixelLayout::P010 { full_range: false },
        ]
        .map(|layout| {
            let (luma, chroma) = plane_formats(layout);
            let (luma_format, chroma_format) = (luma.wgpu(), chroma.wgpu());
            OutputPipelines {
                bit_depth: layout.bit_depth(),
                encode: create_fullscreen_pipeline(
                    &device,
                    &[&output_layout],
                    &encode_source,
                    "encode_fragment",
                    &[luma_format, FULL_CHROMA_FORMAT],
                ),
                subsample: create_fullscreen_pipeline(
                    &device,
                    &[&output_layout],
                    &subsample_source,
                    "chroma_fragment",
                    &[chroma_format],
                ),
            }
        })
        .into();

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            sampler,
            planes_layout,
            input_transform_layout,
            output_layout,
            input_pipelines,
            output_pipelines,
        })
    }

    /// A working texture of the given size in `format`.
    pub fn create_working_texture(
        &self,
        width: u32,
        height: u32,
        format: WorkingFormat,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("working"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: format.texture_format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    fn create_transform_buffer(&self, block: [f32; TRANSFORM_UNIFORM_FLOATS]) -> wgpu::Buffer {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("colour transform"),
            size: size_of_val(&block) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bytes: Vec<u8> = block.iter().flat_map(|value| value.to_ne_bytes()).collect();
        self.queue.write_buffer(&buffer, 0, &bytes);
        buffer
    }

    /// The conversion from frames of a source into the working space. `layout` is how the
    /// source's frames are stored (see `PixelLayout::for_stream`).
    pub fn create_input_transform(
        &self,
        color: &ColorDescription,
        layout: PixelLayout,
    ) -> MediaResult<InputTransform> {
        if color.full_range != layout.full_range() {
            return media_error("the frame layout's range differs from the colour description's");
        }
        let buffer =
            self.create_transform_buffer(input_transform_uniform(color, layout.bit_depth()));
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("input transform"),
            layout: &self.input_transform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Ok(InputTransform { bind_group, layout })
    }

    /// The conversion from `working` into encoder surfaces of `color`.
    pub fn create_output_transform(
        &self,
        color: OutputColor,
        working: &wgpu::Texture,
    ) -> MediaResult<OutputTransform> {
        let description = color.description();
        let layout = PixelLayout::for_stream(color.bit_depth(), description.full_range)?;
        let buffer =
            self.create_transform_buffer(output_transform_uniform(&description, color.bit_depth()));
        let full_chroma = self
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("full-resolution chroma"),
                size: working.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FULL_CHROMA_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let bind = |label, texture: &wgpu::TextureView| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.output_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(texture),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            })
        };
        Ok(OutputTransform {
            encode_bind_group: bind("encode", &working.create_view(&Default::default())),
            subsample_bind_group: bind("subsample", &full_chroma),
            full_chroma,
            layout,
        })
    }

    /// Off the Mac, copies one in-memory plane into a new texture. Encoder input surfaces exist
    /// only on a Mac, so nothing renders into a plane here.
    #[cfg(not(target_os = "macos"))]
    fn wrap_plane(
        &self,
        buffer: &PixelBuffer,
        plane: usize,
        plane_format: PlaneFormat,
        usage: wgpu::TextureUsages,
    ) -> MediaResult<wgpu::Texture> {
        if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
            return media_error("encoder surfaces are not made on this platform");
        }
        let (width, height) = buffer.plane_size(plane);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame plane"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: plane_format.wgpu(),
            usage: usage | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            buffer.plane(plane),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(buffer.row_bytes(plane) as u32),
                rows_per_image: Some(height),
            },
            size,
        );
        Ok(texture)
    }

    /// Wraps one plane of `buffer`'s IOSurface as a texture. The texture keeps the buffer alive,
    /// so its pool cannot hand the surface out again while the GPU still uses it.
    #[cfg(target_os = "macos")]
    fn wrap_plane(
        &self,
        buffer: &PixelBuffer,
        plane: usize,
        plane_format: PlaneFormat,
        usage: wgpu::TextureUsages,
    ) -> MediaResult<wgpu::Texture> {
        let format = plane_format.wgpu();
        let surface = buffer.io_surface().ok_or_else(|| {
            MediaError("pixel buffer has no IOSurface to share with the GPU".into())
        })?;
        let size = buffer.plane_size(plane);
        let mut metal_usage = MTLTextureUsage::empty();
        if usage.contains(wgpu::TextureUsages::TEXTURE_BINDING) {
            metal_usage |= MTLTextureUsage::ShaderRead;
        }
        if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
            metal_usage |= MTLTextureUsage::RenderTarget;
        }
        // SAFETY: the Metal calls follow their documented contracts: the descriptor matches the
        // plane's size and format, and the surface outlives the call. The hal texture is created
        // for the same device that made the Metal texture.
        let texture = unsafe {
            let hal_device = self
                .device
                .as_hal::<wgpu::hal::api::Metal>()
                .ok_or_else(|| MediaError("the GPU device is not a Metal device".into()))?;
            let descriptor =
                MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                    plane_format.metal(),
                    size.0 as usize,
                    size.1 as usize,
                    false,
                );
            descriptor.setUsage(metal_usage);
            descriptor.setStorageMode(MTLStorageMode::Shared);
            let raw = hal_device
                .raw_device()
                .newTextureWithDescriptor_iosurface_plane(&descriptor, surface, plane)
                .ok_or_else(|| MediaError("Metal refused to wrap the IOSurface plane".into()))?;
            let keep_alive = buffer.clone();
            let hal_texture = wgpu::hal::metal::Device::texture_from_raw(
                raw,
                format,
                MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent {
                    width: size.0,
                    height: size.1,
                    depth: 1,
                },
                Some(Box::new(move || drop(keep_alive))),
            );
            self.device
                .create_texture_from_hal::<wgpu::hal::api::Metal>(
                    hal_texture,
                    &wgpu::TextureDescriptor {
                        label: Some("pixel buffer plane"),
                        size: wgpu::Extent3d {
                            width: size.0,
                            height: size.1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage,
                        view_formats: &[],
                    },
                    wgpu::TextureUses::RESOURCE,
                )
        };
        Ok(texture)
    }

    /// Both planes of `buffer` as texture views.
    fn wrap_planes(
        &self,
        buffer: &PixelBuffer,
        usage: wgpu::TextureUsages,
    ) -> MediaResult<(wgpu::TextureView, wgpu::TextureView, PixelLayout)> {
        let layout = buffer.layout().ok_or_else(|| {
            MediaError(format!(
                "unsupported pixel format {:#x}",
                buffer.format_type()
            ))
        })?;
        let (luma_formats, chroma_formats) = plane_formats(layout);
        let luma = self.wrap_plane(buffer, 0, luma_formats, usage)?;
        let chroma = self.wrap_plane(buffer, 1, chroma_formats, usage)?;
        Ok((
            luma.create_view(&Default::default()),
            chroma.create_view(&Default::default()),
            layout,
        ))
    }

    /// The planes of a decoded frame, ready to sample.
    pub fn wrap_video_frame(&self, buffer: &PixelBuffer) -> MediaResult<VideoFrameTextures> {
        let (luma, chroma, layout) =
            self.wrap_planes(buffer, wgpu::TextureUsages::TEXTURE_BINDING)?;
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video planes"),
            layout: &self.planes_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&luma),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&chroma),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Ok(VideoFrameTextures { bind_group, layout })
    }

    /// The planes of an encoder input surface, ready to render into.
    pub fn wrap_encoder_surface(
        &self,
        buffer: &PixelBuffer,
    ) -> MediaResult<EncoderSurfaceTextures> {
        let (luma, chroma, layout) =
            self.wrap_planes(buffer, wgpu::TextureUsages::RENDER_ATTACHMENT)?;
        Ok(EncoderSurfaceTextures {
            luma,
            chroma,
            layout,
        })
    }

    /// Draws a video frame over the whole of `target` (a working texture's view), in
    /// working-space colour.
    pub fn draw_video_frame(
        &self,
        commands: &mut wgpu::CommandEncoder,
        frame: &VideoFrameTextures,
        transform: &InputTransform,
        target: &wgpu::TextureView,
    ) -> MediaResult<()> {
        if frame.layout != transform.layout {
            return media_error(format!(
                "frame is {} but its source was described as {}",
                frame.layout.name(),
                transform.layout.name()
            ));
        }
        let target_format = target.texture().format();
        let Some(pipeline) = WorkingFormat::ALL
            .iter()
            .position(|format| format.texture_format() == target_format)
            .map(|index| &self.input_pipelines[index])
        else {
            return media_error(format!("{target_format:?} is no working format"));
        };
        draw_fullscreen(
            commands,
            pipeline,
            &[&frame.bind_group, &transform.bind_group],
            &[target],
        );
        Ok(())
    }

    /// Writes the working texture into both planes of an encoder surface.
    pub fn draw_output(
        &self,
        commands: &mut wgpu::CommandEncoder,
        transform: &OutputTransform,
        surface: &EncoderSurfaceTextures,
    ) -> MediaResult<()> {
        if surface.layout != transform.layout {
            return media_error(format!(
                "encoder surface is {} but the output colour needs {}",
                surface.layout.name(),
                transform.layout.name()
            ));
        }
        let pipelines = self
            .output_pipelines
            .iter()
            .find(|pipelines| pipelines.bit_depth == surface.layout.bit_depth())
            .ok_or_else(|| MediaError("no output pipeline for this bit depth".into()))?;
        draw_fullscreen(
            commands,
            &pipelines.encode,
            &[&transform.encode_bind_group],
            &[&surface.luma, &transform.full_chroma],
        );
        draw_fullscreen(
            commands,
            &pipelines.subsample,
            &[&transform.subsample_bind_group],
            &[&surface.chroma],
        );
        Ok(())
    }

    /// Blocks until the GPU has finished `submission`, or everything submitted when `None`.
    pub fn wait_for(&self, submission: Option<wgpu::SubmissionIndex>) -> MediaResult<()> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: submission,
                timeout: None,
            })
            .map(|_| ())
            .map_err(|cause| MediaError(format!("wait for the GPU: {cause}")))
    }
}

fn create_fullscreen_pipeline(
    device: &wgpu::Device,
    bind_layouts: &[&wgpu::BindGroupLayout],
    source: &str,
    fragment_entry: &str,
    target_formats: &[wgpu::TextureFormat],
) -> wgpu::RenderPipeline {
    let targets: Vec<Option<wgpu::ColorTargetState>> = target_formats
        .iter()
        .map(|format| Some((*format).into()))
        .collect();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fullscreen"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let bind_layouts: Vec<Option<&wgpu::BindGroupLayout>> =
        bind_layouts.iter().copied().map(Some).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fullscreen"),
        bind_group_layouts: &bind_layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fullscreen"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn draw_fullscreen(
    commands: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    bind_groups: &[&wgpu::BindGroup],
    targets: &[&wgpu::TextureView],
) {
    let attachments: Vec<Option<wgpu::RenderPassColorAttachment<'_>>> = targets
        .iter()
        .map(|view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })
        })
        .collect();
    let mut pass = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("fullscreen"),
        color_attachments: &attachments,
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    for (index, bind_group) in bind_groups.iter().enumerate() {
        pass.set_bind_group(index as u32, *bind_group, &[]);
    }
    pass.draw(0..3, 0..1);
}
