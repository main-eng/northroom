# gpui_wgpu: Linux glyphs at the subpixel offset GPUI asks for

This folder is Zed's `crates/gpui_wgpu` at 125ca8672e928b19a62c2e4b761fb29fe2a34ae3 with one
change: two lines of `CosmicTextSystemState::render_glyph_image` in
`src/cosmic_text_system.rs`. The manifest has its `workspace = true` entries written out.
Nothing else differs (`diff -r` against that rev's `crates/gpui_wgpu/src` shows only these
lines). It is kept here and not proposed to Zed, at the owner's request.

## The bug

`Window::paint_glyph` scales a glyph's origin to device pixels before it quantizes it, so
`RenderGlyphParams::subpixel_variant` is a fraction of a _device_ pixel. This rasterizer builds
its swash scaler at `font_size * scale_factor`, so it draws in device pixels too, yet it divided
the offset by `scale_factor` once more. At a scale of 2 every glyph is drawn half a variant
short of where its atlas tile is placed, and the four horizontal variants give two positions.
At a scale of 1 the division changes nothing.

The other two rasterizers agree with the fix. DirectWrite (`gpui_windows/src/direct_write.rs`)
uses `variant / SUBPIXEL_VARIANTS` as it stands. Core Text (`gpui_apple/src/text_system.rs`)
does divide by the scale, but only after scaling its drawing context by the same factor.

```diff
         let subpixel_offset = Vector::new(
-            params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32 / params.scale_factor,
-            params.subpixel_variant.y as f32 / SUBPIXEL_VARIANTS_Y as f32 / params.scale_factor,
+            params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32,
+            params.subpixel_variant.y as f32 / SUBPIXEL_VARIANTS_Y as f32,
         );
```

## Re-applying

When Northroom's GPUI pin (`gpui/Cargo.toml`, `[workspace.dependencies]`) moves, first look at
the new rev's two lines. If the division is gone, delete this folder and its `[patch]` entry.
Otherwise copy the new rev's `crates/gpui_wgpu` here, change the two lines, and write its
`workspace = true` entries out again from that rev's root `Cargo.toml`, at the versions in
`gpui/Cargo.lock`.
