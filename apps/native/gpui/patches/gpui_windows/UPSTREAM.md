# gpui_windows: draw glyphs as Core Text does, and draw groups faded as one layer

This folder is Zed's `crates/gpui_windows` at 125ca8672e928b19a62c2e4b761fb29fe2a34ae3 with three
changes: `DirectWriteState::rendering_mode` in `src/direct_write.rs`,
`DirectXRenderer::get_font_info` in `src/directx_renderer.rs`, and the Direct3D 11 renderer
drawing the opacity groups of the patched `gpui` (`../gpui`) in `src/directx_renderer.rs`,
`src/shaders.hlsl` and `build.rs`. The manifest has its `workspace = true` entries written out.
Nothing else differs. No pull request has been opened.

Windows drew grey text visibly heavier than the Mac and Linux, in pictures of the same screen:
the faint edge pixels of each glyph are lifted most (an edge the Mac draws at 15% coverage drawn
at 33%). Fitted against Linux's pictures, Windows's text reads as gamma 2.2 with an enhanced
contrast near 3.5, against the 1.8 and 1.0 that match the Mac's text (the same fit of Linux
against the Mac gives exactly 1.8 and 1.0).

## Unhinted, antialiased both ways

`rendering_mode` asked `GetRecommendedRenderingMode`, which follows a font's `gasp` table and
size and may answer `NATURAL` (antialiased across only) or grid fitting. Core Text never hints
and antialiases both ways, so every glyph is drawn `NATURAL_SYMMETRIC` with grid fitting off, as
the Linux patch (`../gpui_wgpu`) turns swash's hinting off. Which of the two changes moves the
gate's pictures most is not measured: only a Windows runner draws with DirectWrite.

## The wgpu renderer's gamma and contrast

`get_font_info` read the gamma and grayscale enhanced contrast from the system's rendering
parameters, which ClearType tuning sets, so text looked different from one machine to the
next. It now reads them as
`gpui_wgpu`'s `RenderingParameters` does: `ZED_FONTS_GAMMA` (default 1.8, within 1.0 to 2.2) and
`ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST` (default 1.0). Subpixel contrast and pixel geometry are
still the system's.

## Groups faded as one layer

The patched `gpui` paints a group (`Window::paint_with_opacity`) into a scene of its own,
`Scene::groups`, and `Scene::batches` yields `PrimitiveBatch::Groups` where the group is drawn.
The renderer draws them as the Metal renderer in `../gpui_apple` does:

- `DirectXRenderer::draw_scene` first draws each group's scene (its own groups first) into a
  texture of its own, the frame's size and format, cleared to transparent, so it holds the group
  premultiplied. Each pass writes its own global parameters and instance buffers before it
  draws, and clears the shared depth buffer.
- The parent scene is then drawn, and at a `Groups` batch each group's texture is drawn over the
  group's bounds by `group_vertex` and `group_fragment` in `src/shaders.hlsl`: the texel at the
  fragment's pixel times the group's opacity, blended premultiplied (source one, destination one
  minus source alpha), at the fixed depth every batch but quads gets, so the opaque-quad
  pre-pass still hides what is under it.
- The textures are kept for the next frame and reused when its size is the same; a frame
  without groups lets them go, and a lost device drops them.

`render_to_image` draws through the same path, so a check's pictures include groups.
The `group` shader module is listed in `build.rs` with the others, for release builds' fxc.

Unverified here: the HLSL is not compiled on the Mac that made this change (no fxc), so only a
Windows build checks it. Subpixel glyphs inside a group are drawn with the subpixel blend,
which does not write alpha, so where nothing opaque in the group is under them they fade
wrongly; GPUI picks subpixel glyphs only in opaque windows.
