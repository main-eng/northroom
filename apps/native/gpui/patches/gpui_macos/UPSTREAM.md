# gpui_macos: a window paced by a display link it leaves while idle

This folder is Zed's `crates/gpui_macos` at 125ca8672e928b19a62c2e4b761fb29fe2a34ae3 with one
change: every window's frames are paced by the shared `CVDisplayLink` registry, and a window leaves
its display's link while it is idle. Zed paces windows on macOS 14 and later with a
`CAMetalDisplayLink` on the window's layer instead. The manifest has its `workspace = true`
entries written out. Nothing else differs. Written to be proposed to Zed as it stands. No pull
request has been opened.

## Why

A `CAMetalDisplayLink` keeps five of a window's drawables (each 16 MB for a 2560×1600 window), and
they stay after the link pauses, is invalidated or the layer is resized. Drawing into the layer's
own `nextDrawable` keeps three, the layer's `maximumDrawableCount`. Measured on an M5 MacBook Pro
(2026-11-17, a 2560×1600 window, two runs each, the 60 fps SDR clip): playing used 224 MB with the
Metal link and 185 MB without it, the peak while scrubbing 241 MB and 202 MB, and the board after
one menu 157 MB and 114 MB. The video controls' and menus' fades held 112–120 frames a second
either way.

Zed's `CVDisplayLink` path, used before macOS 14, stays subscribed while the window is visible,
so an idle window still wakes 120 times a second. Hence the second half of the change.

## The change (`src/display_link.rs`, `src/window.rs`)

- `WindowFrames` replaces `MetalFrames` and the `FrameSource` choice. It owns the window's
  `WindowFrameSource`, the display it was started on, and whether GPUI drew or asked for a frame
  during the current request.
- `step` brackets GPUI's frame request with `begin_frame` and `end_frame`. A request in which GPUI
  neither drew (`PlatformWindow::draw`, `note_drawn`) nor asked again (the frame waker or
  `schedule_frame`, `wake`) unsubscribes the window, so a display with no busy window stops its
  link. The next `wake` subscribes it again on the same display.
- `start_display_link` and `stop_display_link` only start or stop `WindowFrames`; occlusion, a
  screen change and the synchronous draws in `display_layer` and on activation are unchanged.
- `MacWindow::frame_waker` returns a waker on every system, so GPUI's demand reaches an idle
  window as it reached a paused Metal link.
- `WindowFrameSource::debug_state` became `is_subscribed`; `WindowFrames::debug_state` reports the
  display, whether the window is subscribed and whether it was woken.
- The Metal link's frame (`metal_frame`) and its renderer attachment are gone. The renderer in
  `gpui_apple` keeps its attached mode, unused here.

## Re-applying

When Northroom's GPUI pin (`gpui/Cargo.toml`, `[workspace.dependencies]`) moves, copy the new
rev's `crates/gpui_macos` here and make the same change; write its `workspace = true` entries out
again from that rev's root `Cargo.toml`. Once Zed paces a window without keeping more drawables
than the layer's own, delete this folder and its `[patch]` entry.
