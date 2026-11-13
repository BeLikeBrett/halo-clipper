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
- Background color editing uses the toolbar drop-down and an inline HSV picker.
  A pointer drag is one undo action.
- Wheel zoom is anchored at the pointer and grouped into undo gestures. Undo and
  reset stop lingering wheel inertia. Idle selection decoration fades after
  1.8 seconds; the clip remains selected. Reset frame uses the aspect-aware rule below.
- Simple Clip shares imports but keeps per-source marks, positions, and undo/redo
  histories. Advanced keeps its timeline, selection, and playhead. Simple undo
  restores source marks without restoring an older copy of Advanced's edit.

## Interface pass (2026-11-13)

- Typography: Inter (Regular/Medium/SemiBold) for the interface and
  JetBrains Mono for timecodes, embedded as small Latin subsets. DejaVu stays
  as a fallback. Weights are separate font families
  because egui has no synthetic bold; use `theme::medium/semibold/mono`.
- Icons are painted (`widgets.rs`), never emoji glyphs, so weight and
  alignment don't depend on the fallback font.
- Top-bar menus unroll out of their button (shared fill, square inner corner,
  160 ms ease-out reveal). Hovering a sibling while one is open switches menus.
- Advanced timeline: the ruler is pinned above the scrolling lanes and its
  header cell shows the live playhead time. The whole ruler scrubs; the
  playhead head is a large grab handle and the line itself is grabbable ±5 px.
- A clip being dragged disappears from its old spot as soon as a drop
  position exists; only the full-fidelity ghost (thumbnail, label, audio
  strip) is drawn.
- Abutting clips (splits) are separated by a 2 px gap with splice notches at
  the seam. Selection outlines are drawn inside the clip so they never
  overlap a neighbour.
- V+A compounds show their attached audio as a teal strip along the bottom
  of the bar instead of a "· V+A" suffix.
- Track headers have a monitor (speaker) toggle on audio tracks and on video
  rows that carry attached audio. It mutes the preview only:
  `LivePreview::monitor_muted` is outside the project, undo history and the
  export. The right-click "Disable track" still affects export.
- Canvas move/resize snaps magnetically (10 px) to canvas edges and center
  and to other layers' edges/centers, with guides; dragging further breaks
  free, Alt disables snapping.
- Reset frame is aspect-aware: when the source and canvas shapes differ by more
  than 12 % (e.g. 16:9 gameplay in 9:16) it fits whole and centered with bars;
  otherwise it fills.
- The timeline panel's content claims the full panel height. egui stores a
  resizable panel's height from its content, so any shortfall in the height
  budget made the panel shrink a few pixels every frame.
- Palette: one blue accent (selection, snap guides, primary buttons), video
  clips blue, audio clips teal, playhead soft white. No warm hues outside
  the In/Out marks and errors. The app pins egui to its dark theme so a
  light-mode OS can't restyle the stock widgets.
- Advanced timeline zoom: fit-to-width is the default. Ctrl+scroll (or a
  trackpad pinch) zooms around the pointer down to a 400 ms window;
  Shift+scroll or a horizontal trackpad swipe pans while zoomed (plain
  scroll still scrolls the track list). The footer shows the zoom factor
  and a Fit button. Zoom is view state only (egui temp memory), not project
  data.
- Keyboard flow: painted controls use a non-focusable click sense, and any
  focus a mouse click leaves on a stock egui button is released at the end
  of the frame (text fields keep theirs). Clicking Play or Split and then
  pressing S / Space keeps working without clicking empty space first.
- Split targets the selected clips under the playhead (or every clip under
  it when nothing selected there) and moves the selection to the right-hand
  pieces, so repeated S presses during playback keep cutting the same clip.
- Canvas toolbar: the aspect and Background controls are drop-downs that
  unroll from their button like the top-bar menus. Background holds a
  Black / Blur video / Color switch and, for Color, the compact picker in
  `picker.rs` (saturation/brightness square, hue strip, presets, hex field).
  One drag in the picker is one undo step.

## Independent text layers (2026-11-13)

- Advanced text is stored in `Project::texts`, independently of media clips.
  Its position is normalized to the output canvas, so titles can sit in the
  bars around a fitted video. Text paints above video in list order.
- T, Add text, or the Text band’s + adds a three-second layer at the playhead,
  pauses playback, selects it, and focuses the text field. A new layer inherits
  the previous look. Empty layers show a preview placeholder and never export it.
- Drag the preview text to position it. Canvas and video edges/centers snap;
  Alt bypasses snapping. Drag a corner or scroll over selected text to resize.
  Position changes reuse the texture; rasterization happens only for a change
  in text, style, size, canvas size, or animation state. No mpv filter rebuilds.
- The inspector has eleven bundled font choices, six style presets, swatches
  and custom color, alignment, outline, shadow, and box controls. Position has
  no numeric sliders. Presets preserve the chosen font and alignment.
- Intros: None, Fade, Pop, Rise, Type, Words. Outros: None, Fade, Pop, Drop.
  Separate In/Out time sliders control duration (50 ms–5 seconds), including
  the complete Type/Words reveal. Short layers cap each transition at half
  their length. Paused selection shows settled text for editing;
  selected text outside its range is ghosted only while paused.
- Text bars are pinned under the ruler. Drag the body to move or an end to trim,
  with snapping to playhead and neighboring edit points (Alt bypasses it).
  S splits selected text and selects the right half; Del removes it. The bar
  context menu offers Duplicate and Delete. Overlaps stack in up to four rows.
- Text editing is included in Advanced undo history; text and clip selection
  are mutually exclusive. A text-entry gesture or pointer drag is one undo step.
- `text.rs` holds the model/animation curves; `textrender.rs` draws with bundled
  fonts using ab_glyph and tiny-skia; `textui.rs` owns the inspector and interaction.
  Preview and export use that same rasterizer at their respective resolution.
  Export caches held frames in PNG concat inputs and uses the output frame rate
  for timestamps. Preparation checks cancellation between frames and scratch
  assets are deleted on success, failure, or cancellation.
- Removed the initial status and empty-preview “Open a video to begin” messages.
  Actual operation/error status messages remain available.

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

## Color mixing and clip motion (0.10.0)

- Text, outline, and box each support solid custom colors or two-color gradients
  in the collapsed Color mix section. Gradient direction, three palette shortcuts,
  and box opacity appear only when relevant. The shared rasterizer keeps preview,
  animation frames, and exports identical. Gradient edits invalidate texture caches.
- Removed the text inspector's Preview animation button, instructions, and Delete
  button. Transport, timeline context menu, and Delete shortcut remain available.
- Each video segment has an Animation section in Edit: independent In/Out effect
  selectors and durations, one Amount control when applicable, and edge playback.
  None is the default. Effects are Fade, Snap zoom, Pull back, Whip, and Flash.
  Zoom/whip curves use cubic ease-out on entry and cubic acceleration on exit.
  Flash is a single linear white blend; Fade changes the segment's alpha.
- Animation lives on the segment, participates in undo/redo and duplication,
  follows speed-adjusted timeline duration, and leaves attached audio untouched.
  Splitting keeps the intro on the left and outro on the right without adding
  effects at the new internal cut. Short segments cap each edge at half duration.
- Live GPU transforms/white blending and FFmpeg export use shared timing and
  amplitudes from animation.rs. Export zoom dimensions round to even pixels.

References used to select a focused set of short-form effects:

- [Adobe: modern transitions](https://helpx.adobe.com/premiere/desktop/add-video-effects/types-of-effects/transitions.html)
  includes cross zoom, pull motion, and whip.
- [CapCut: transition trends](https://www.capcut.com/help/capcut-transitions)
  covers fade/zoom and swipe/motion styles.
- [CapCut: flash templates](https://www.capcut.com/explore/camera-flash-transition-effect)
  provides examples of flash and zoom edits.

Halo's effects operate on individual segment edges; they do not overlap or shorten
neighboring clips. This release does not add speed ramps or automatic beat syncing.

## Native media drops

Windows OLE drops are explicitly enabled on the viewport; Linux uses winit's
X11/Wayland file-drop events. External drags briefly highlight Media and show
Drop to import. Single or multiple files are probed and staged in the bin,
without placing timeline clips or switching the current Simple Clip source.
The first video initializes an empty Simple workspace; an empty Advanced
workspace keeps its mode. Duplicate paths are skipped, invalid files report
an error while valid files in the same batch still import. An Advanced batch
uses one undo step. The Import button remains available.
