# gpui_wgpu: rasterize Linux glyphs at the requested subpixel offset, unhinted, and draw groups faded as one layer

This folder is Zed's `crates/gpui_wgpu` at 125ca8672e928b19a62c2e4b761fb29fe2a34ae3 with three
changes: two to `CosmicTextSystemState::rasterize_glyph` in `src/cosmic_text_system.rs`, and the
renderer drawing the opacity groups of the patched `gpui` (`../gpui`) in `src/wgpu_renderer.rs`
and `src/shaders.wgsl`. The manifest has its `workspace = true` entries written out. Nothing
else differs. The glyph changes are written to be proposed to Zed as they stand; the group
drawing needs the patched `gpui`. No pull request has been opened.

## Subpixel offset in device pixels

`Window::paint_glyph` quantizes the glyph's origin after scaling it to device pixels, so
`RenderGlyphParams::subpixel_variant` is a fraction of a _device_ pixel. The rasterizer builds
its swash scaler at `font_size * scale_factor`, so it also works in device pixels. But it divided
the offset by `scale_factor` again. At a scale of 2, every glyph was drawn half a variant short of
where its atlas tile is placed, and four horizontal variants gave only two distinct positions.
DirectWrite (`gpui_windows/src/direct_write.rs`) already uses `variant / SUBPIXEL_VARIANTS`.

```diff
         let subpixel_offset = Vector::new(
-            params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32 / params.scale_factor,
-            params.subpixel_variant.y as f32 / SUBPIXEL_VARIANTS_Y as f32 / params.scale_factor,
+            params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32,
+            params.subpixel_variant.y as f32 / SUBPIXEL_VARIANTS_Y as f32,
         );
```

## Unhinted outlines

`.hint(true)` runs swash's hinter. For a font with no TrueType instructions, such as Inter or
most variable fonts, that means skrifa's autohinter. It snaps stems and x-heights to the pixel
grid, so text set in the same font is shorter and heavier on Linux than on macOS. Core Text
never hints, and at 2x the grid-fitting buys no sharpness. With `.hint(false)`, the glyphs Linux
draws match macOS's outline for outline.

```diff
-            .hint(true)
+            .hint(false)
```

If hinting should stay for 1x displays, the same effect comes from
`.hint(params.scale_factor < 2.0)`.

## How it was found

The same GPUI screens were drawn on a Linux runner (Xvfb at scale 2) and on macOS, and the
pictures compared. The Linux text was shorter and sat in the wrong places. A standalone swash rasterizer placed glyphs the way GPUI does. When set to hint and
divide the offset, it reproduced the Linux pictures' glyphs. When set to do neither, it
reproduced the macOS ones.

## Groups faded as one layer

The patched `gpui` paints a group (`Window::paint_with_opacity`) into a scene of its own,
`Scene::groups`, and `Scene::batches` yields `PrimitiveBatch::Groups` where the group is drawn.
The renderer draws them as the Metal renderer in `../gpui_apple` does:

- `WgpuRendererCore::draw_scene` first draws each group's scene (its own groups first) into a
  texture of its own, the frame's size and format, cleared to transparent, so it holds the group
  premultiplied. Each scene is a submission of its own: it writes its instances from the start
  of the instance data and its global parameters into the one globals buffer, which is safe
  because queue writes made after a submission take effect after it. So a scene's quads start
  at instance zero on both instance transports, as the quad depths need. Each pass clears the
  shared depth texture.
- The parent scene is then drawn, and at a `Groups` batch each group's texture is drawn over the
  group's bounds by `vs_group` and `fs_group` in `src/shaders.wgsl`: the texel at the fragment's
  pixel (`textureLoad`, no sampler) times the group's opacity, blended premultiplied (source one,
  destination one minus source alpha) whatever the surface's alpha mode, at the fixed depth
  every batch but quads gets, so the opaque-quad pre-pass still hides what is under it. Each
  group's bounds and opacity are a uniform buffer kept with its texture and bind group.
- The textures are kept for the next frame and reused when its size is the same; a frame
  without groups lets them go, and a lost device drops them with the rest of the renderer.
- The atlas bind groups are prepared once a frame for the sprites of every scene in it.

`WgpuRenderer::draw` and the headless renderer (`render_scene_to_image`, a check's pictures)
both draw through `render_frame`, so both draw groups.

Checked on a Mac: the crate checks for `x86_64-unknown-linux-gnu`; the three shader variants
(storage, WebGL, subpixel) validate with the pinned naga as the crate's tests do; and a copy of
this crate with its instance set to wgpu's Metal backend draws a test scene headless with the
expected pixels (a group's later part covering its earlier one before the fade, a quad painted
after a group over it, nested groups at 0.5 and 0.5 giving 0.25, and textures reused, let go
and made again). Unverified here: Vulkan and GL on Linux, which CI's Linux runner covers.
Subpixel glyphs inside a group are drawn with the subpixel blend, which does not write alpha,
so where nothing opaque in the group is under them they fade wrongly; GPUI picks subpixel
glyphs only in opaque windows, and Northroom's shell asks for grayscale text.

## Re-applying

When Northroom's GPUI pin (`gpui/Cargo.toml`, `[workspace.dependencies]`) moves, copy the new
rev's `crates/gpui_wgpu` here and make the same changes. Then write its `workspace = true`
entries out again from that rev's root `Cargo.toml`, at the versions in `gpui/Cargo.lock`. Once
Zed has the glyph changes and draws groups itself, delete this folder and the `[patch]` entry.
