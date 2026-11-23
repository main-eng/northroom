# gpui and gpui_apple: groups composited at an opacity

This folder is Zed's `crates/gpui` at 125ca8672e928b19a62c2e4b761fb29fe2a34ae3, and `../gpui_apple`
is that rev's `crates/gpui_apple`, with one change across both: a group of primitives can be drawn
into a texture of its own and that texture drawn at an opacity, as a compositor fades a layer. The
manifests have their `workspace = true` entries written out; `gpui`'s tests, examples, benches and
dev-dependencies are left out (nothing here builds them). Nothing else differs. `../gpui_wgpu`
(Linux) and `../gpui_windows` (Direct3D 11) draw the same groups; their `UPSTREAM.md` describe
their part. Written to be proposed to Zed as it stands. No pull request has been opened.

## Why

`Styled::opacity` (the element opacity) multiplies each primitive's alpha by itself. A panel
faded that way lets a glyph show what is under the panel, since the glyph and the panel are each
half transparent: AppKit, Core Animation and the browsers fade a layer as one, `a·G + (1 − a·α)·B`
for a group `G` of coverage `α` over `B`. Northroom faded its menus, dialogs, tooltips and video
controls as one by drawing the window again with the group at full strength, reading it back
(`render_to_image`) and drawing the group's part of that picture at its opacity. That cost 5.3 ms a
frame (worst 11) on an M5 and held the fades to 79–100 frames a second. Done in the renderer it
costs one more render pass a group.

## The API

`Window::paint_with_opacity(opacity, f)` paints what `f` paints as one group at `opacity`.

## gpui (`src/scene.rs`, `src/window.rs`)

- `Scene::push_group(opacity)` / `Scene::pop_group()`. While a group is open, what is painted goes
  into the group's own `Scene` (the innermost open group's): its primitives, layers, animated
  sprites and nested groups get draw orders of their own there. The scene a window paints into
  still records every operation for replaying a cached view (`PaintOperation::StartGroup`,
  `EndGroup`, and `AnimatedSprite`, which carries the animation the group's scene keeps).
- When a group closes it becomes an `OpacityGroup { order, bounds, opacity, scene }` in the
  enclosing scene's `groups`, ordered like a primitive covering `bounds` (what its primitives
  cover, rounded out to whole device pixels) of a new `PrimitiveKind::Group`, the highest kind,
  so it is drawn after the other kinds of its draw order. Inside a layer it takes the layer's
  order (`PaintOperation::Group`).
- `Scene::batches` yields `PrimitiveBatch::Groups(range)` in paint order like any other kind.
- Animated sprites in a group are advanced and asked for frames with the rest
  (`advance_animations`, `animation_frame_interval`).

## gpui_apple (`src/metal_renderer.rs`, `src/shaders.metal`, `build.rs`)

- `render_frame` draws a scene with `draw_scene`: first each group's scene (its own nested
  groups first) into a texture the size of the frame, cleared to transparent so the texture holds
  the group premultiplied, then the scene itself, whose `PrimitiveBatch::Groups` draw each group's
  texture over its bounds (`draw_groups`).
- The `groups` pipeline reads the group's texture pixel for pixel at the fragment's position and
  multiplies it by the opacity (`group_vertex`, `group_fragment`), blended premultiplied (`One`,
  `OneMinusSourceAlpha`), with the fixed depth every non-quad batch gets, so later opaque quads
  still cover it. `GroupComposite` and `GroupInputIndex` are exported to the shaders.
- The textures (`GroupTextures`) are private, kept for the next frame's groups and let go by a
  frame without groups. All of a frame's scenes share its instance buffer, command buffer and
  depth texture.

## Known limit

Subpixel (ClearType-style) text inside a group blends without writing alpha, so where nothing
opaque in the group is under it, it fades wrongly. The Mac draws no subpixel text and Northroom
asks for grayscale text everywhere.

## Re-applying

When Northroom's GPUI pin (`gpui/Cargo.toml`, `[workspace.dependencies]`) moves, copy the new
rev's `crates/gpui` and `crates/gpui_apple` here and make the same changes; write their
`workspace = true` entries out again from that rev's root `Cargo.toml`, at the versions in
`gpui/Cargo.lock`. Once Zed has group opacity, delete these folders and their `[patch]` entries.
