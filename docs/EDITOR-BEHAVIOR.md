# Advanced editing behavior

Changes in 0.9 and 0.9.1 preserve the app's existing theme and live overlapping preview.
Simple Clip keeps its mark-and-export workflow.

## Reversible attachment

A media group stores member clip IDs and whether the timeline displays it as a
compound bar. Attachment does not merge samples, change source ranges, fill gaps,
restore deleted clips, reset gain/speed, or change track order. Playback/export
always consume the original underlying clips. Undo snapshots include groups.

New video placements start attached to their audio streams. Detaching exposes
independent pieces. Splits made while detached retain their remembered group;
reattaching the video restores that group, including edited audio pieces. Several
selected videos and audio pieces can be attached together. Splitting an attached
compound produces independent left/right compounds, including at an internal gap.
Moving a compound applies the same time delta to all its members.

## Canvas shape and placement

The largest video placed on the timeline supplies the default source dimensions.
Unplaced imports do not affect it. Aspect modes use that source's short edge;
for example, 3840 × 2160 becomes 2160 × 3840 in vertical mode. No numeric resolution
picker appears in the preview. Custom export settings can override output size.

Fit preserves the whole frame. Fill covers the canvas and crops overflow. Zoom,
position, opacity, and canvas background apply in preview and export. Blur fills
the canvas from the lowest visible video at that moment; it falls back to the
next video through gaps, and to black where no video is active. It is independent
of foreground placement. The live GPU blur approximates FFmpeg's Gaussian blur.

Official editor references informing these choices:

- [Final Cut Pro: spatial conform](https://support.apple.com/en-ae/guide/final-cut-pro/ver3363b44e/mac)
  separates Fit/Fill behavior from project aspect ratio.
- [Adobe Premiere: Auto Reframe overview](https://helpx.adobe.com/premiere/desktop/add-video-effects/commonly-used-effects/auto-reframe-overview.html)
  treats sequence shape separately from individual framing. Halo's framing is
  manual; this change does not add subject tracking.
- [CapCut: portrait/landscape conversion](https://www.capcut.com/resource/convert-portrait-videos-to-landscape)
  combines ratio selection with a canvas background/blur option.

## Validation

Model tests cover default combined bars, repeated attach/detach after cuts,
deletions, gaps, speed/gain edits, multiple sources and external audio, undo,
moving compounds, and splitting through internal gaps. Real FFmpeg tests verify
portrait output dimensions, Fit/Fill, zoom, solid/blurred backgrounds, empty gaps,
video ordering/opacity, and independent audio timing/gain/speed. Thumbnail tests
extract actual decoded frames. Native UI checks use a separate Xvfb display and
isolated configuration/cache directories.


## Interaction refinements in 0.9.1

- A new media drag reserves a real timeline row. Existing rows shift aside over
  100 ms. Row selection uses the original positions, so shifting the preview
  does not make the drop boundary oscillate. Time guides show magnetic alignment.
- The first hover into an empty timeline anchors the clip at zero until the
  pointer moves sideways by 28 px; Shift bypasses that hold and normal snapping.
- The timeline panel chooses its initial height from the visible row count,
  including the insertion row for an imported asset. Existing clip drags keep
  the divider stable. Each row count remembers manual divider resizing.
  Overflow scrolls when the window cannot show every row plus a usable preview.
- The shortcut footer has an explicit height. Its earlier implicit minimum
  caused the parent panel to grow a few pixels on each redraw.
- The Edit panel has a bounded scroll viewport; read-only clip details are removed.
- Background color editing uses an inline HSV picker in a persistent floating
  window, avoiding nested popup dismissal. A pointer drag is one undo action.
- Wheel zoom is anchored at the pointer and grouped into undo gestures. Undo and
  reset stop lingering wheel inertia. Idle selection decoration fades after
  1.8 seconds; the clip remains selected. Reset frame fills and centers the clip.
- Simple Clip shares imports but keeps per-source marks, positions, and undo/redo
  histories. Advanced keeps its timeline, selection, and playhead. Simple undo
  restores source marks without restoring an older copy of Advanced's edit.

## Native rendering

The render adapter passes the generated native `mpv_opengl_fbo`, including its
fourth `internal_format` field, through stack-owned parameter arrays. The
high-level 6.0.0 binding's FBO has three integers and no C representation. The
adapter also owns the player, GL loader, and repaint callback for the full
native context lifetime, removing the previous lifetime transmute.

Each native call starts with standard GL state and restores the caller's pixel
transfer, buffer, texture, framebuffer, blend, and raster state. Decoder
copy-back (`auto-copy`) retains hardware decode where available and avoids direct
surface imports into incompatible GL renderers. Software GL renderers use
software decoding, since hardware-decoded frames were corrupted under llvmpipe.
Exports still use originals.
See [mpv's OpenGL interop contract](https://raw.githubusercontent.com/mpv-player/mpv/master/include/mpv/render_gl.h)
and [native render definitions](https://raw.githubusercontent.com/mpv-player/mpv/master/include/mpv/render.h).

Decoder frames are prepared offscreen before any layer is composited to the
canvas. This prevents subsequent decoder renders from erasing earlier layers
and renders a blurred background from the same frame as its foreground.

Simple Clip keeps its decoded framebuffer at a stable source-aspect size, up
to 1920 × 1080, and scales it into the viewport. Framebuffer allocation is
included in the GL state guard. This avoids reallocating on divider/window
resize and avoids fractional-size upscaler artifacts on software graphics.
