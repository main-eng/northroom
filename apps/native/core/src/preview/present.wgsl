// Draws the working texture (linear light, BT.709 primaries, 1.0 = SDR white) to the preview
// layer as it is. The layer is extended linear sRGB, so values above 1.0 reach the display as
// HDR and nothing here clamps them.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
  let corner = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
  var out: VsOut;
  out.pos = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
  out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
  return out;
}

@fragment
fn fs(v: VsOut) -> @location(0) vec4<f32> {
  return vec4<f32>(textureSampleLevel(src, src_sampler, v.uv, 0.0).rgb, 1.0);
}
