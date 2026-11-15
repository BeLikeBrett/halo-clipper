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

## Remembered layout, aspect refit, intro first frame (2026-11-15)

- Panel sizes are saved in `config.json` (`layout`) as soon as a resize
  ends: Media width, Edit width, and the timeline's offset from its fitted
  height (so lanes being added or removed keep the user's extra room). A
  resize is a size change while the pointer is held; window clamping never
  counts. Saving writes only `layout`, never unsaved Settings edits.
  Settings → Layout → Reset panel sizes clears it.
- The timeline panel id changes with its row count; on a change the stored
  egui panel state for the new id is dropped so it starts from fitted + offset.
- Changing the output shape fits every video clip (Fit to screen rules), in
  the same undo step as the shape change.
- While paused, text whose intro hasn't drawn anything yet (Fade, Pop, Slide,
  Words on their first frame) shows settled, like selected text, so it can
  be seen and grabbed. Playback and export still animate from blank.
- `assets/icon.svg`: the comment sits inside `<svg>`. Before the root element
  it pushed `<svg` past the format sniffer, so GTK failed to load the
  scalable icon (used at 32 px in launchers) and showed a broken image.

## Inspector tabs, app menu, slim sliders (2026-11-15)

- The edit panel is split into tabs (`widgets::tab_bar`: a sliding accent
  underline, no extra click layers). Text: Text (field, Font, Align) · Look
  (Color, Outline, Shadow, Box) · Animate (in/out). Video clips: Frame (Fit to
  screen, Zoom, Opacity) · Animate · Playback (Detach/Attach audio, Gain,
  Speed). Audio-only clips show Playback with no tab bar.
- Text style presets are gone from the UI (`STYLE_PRESETS[1]` still seeds the
  default style).
- Sliders are `widgets::Slim`: a 4 px rail with an accent fill and a small
  white knob (ring while dragging) beside an editable value box. egui's stock
  slider sizes its knob from the row height, which looked oversized.
- Top bar: the Halo logo + name in the left corner is one menu (Open Video…,
  Settings…, Help & updates… / Install update…, Quit, with an update dot).
  The Simple Clip / Advanced switch is text only.
- Fit to screen is the first preview toolbar button, before the aspect menu.
- Files dropped from outside onto an empty timeline stick to 00:00 until the
  pointer moves more than 28 px sideways.
- The Media drop highlight stays inside the list under the Media / Import
  header; narrow cards stack the badge above the label.

## Lane drags, Wayland drops, frame fitting (2026-11-15, 0.10.7)

- One rule set for every vertical drag (clips, media from the bin, text
  bars), in `editor::resolve_lane`: the middle half of a lane of the item's
  kind targets that lane when the item fits there; the outer quarters open a
  new lane at that boundary; an occupied lane opens a new layer above it
  (below it for audio). Its own lane keeps an item while it fits, so a
  sideways move never opens a lane, and a new lane right beside an item's
  own lane, when it's the only thing there, is the same as staying.
- The decision reads the layout as drawn: once a gap opens under the
  pointer it stays open while the pointer is inside it. The lanes below the
  gap slide down (0.14 s), revealing a "New video/text/audio layer" row, and
  the dragged item glides into the exact row it will land in. The timeline
  keeps one spare row under the lanes so a gap never needs a panel resize.
- Media dragged from the bin can land on an existing free lane too.
- Files dropped from outside: the middle of a video lane drops onto it (a new
  lane only if they don't fit); its edges insert a new lane.
- Native Wayland (niri, GNOME, KDE…): winit only reports file drops on X11,
  so `wayland_drop.rs` binds `wl_data_device` on winit's own display with a
  private queue on a small thread, accepts `text/uri-list`, and reads the
  drop. It also reports the pointer during the drag, so timeline drops work
  on Wayland. The thread is stopped in `on_exit` before the display closes.
- "Fit to screen" replaces Reset frame and the Fit/Fill switch: centered,
  filling the canvas when the shapes match, whole with bars when they don't.
  It lights up (accent) only when the selected video's drawn frame differs
  from that; it's in the preview toolbar, the clip panel, and the preview's
  right-click menu.
- Gradients: two color wells (Start ⇄ End; click one, then a swatch; click it
  again for the full picker) and four square direction tiles that each show
  the actual gradient running that way.
- Inline text: Shift+Enter adds a line (egui's `consume_key` matched Enter
  with Shift held too, so it used to finish editing).
- Top bar: a large Simple Clip / Advanced switch with a sliding highlight in
  the center (menus: see the section above).

## Layers, audio sync and cut playback (2026-11-15)

- Text is a layer like any other: the Text band is gone. Text lanes stack with
  video lanes above the audio lanes; empty text lanes are pruned with the
  others. Muted text lanes hide their text in preview and export.
- Inline text editing: click selected text (or double-click any text) on the
  preview to type in a bubble under it. Enter finishes, Shift+Enter adds a
  line, clicking away closes it; one editing session is one undo step. The
  selection frame hugs the drawn glyphs and outline (or the box when on).
- Text inspector: Font, Align, Color (Solid/Gradient), Outline, Shadow, Box,
  Animate in/out (now in tabs, see above). No named gradient presets.
- Clip panel: Frame (Fit to screen, Zoom, Opacity) and Animation (Start of
  clip / End of clip chips, a Preview pill, Length, and Strength where it
  applies).
- G closes gaps. With a selection, the selection and everything that starts at
  or after it slide left until something on one of their lanes touches (groups
  move together). With nothing selected, every empty stretch of the timeline,
  including the lead-in, is removed across all lanes so layers stay aligned.
  Nothing to close is a no-op without an undo entry.
- External drops in Advanced: hovering the timeline shows an insertion line
  and an "Add at" chip on the target lane (or a new lane); dropping imports
  the files and places them back-to-back from that time as one undo step.
  Hovering Media shows a soft rounded drop card instead of the old outline.
- Switching to Advanced with an empty timeline places the Simple Clip source
  (or the first imported video) at 0:00 as one undo step. Entering Advanced
  stops the Simple player so it can never play underneath.
- Live audio: streams of one file with the same timing share one mpv decoder
  and are mixed with the export's amix + alimiter graph, so they can't drift
  apart (echo) or sum past full scale (clipping). After a seek or pause, every
  decoder realigns on the next frame instead of waiting for the drift check.
  Clip changes within the same file seek rather than reload.
- Cut playback: each lane keeps a standby decoder parked on its next cut
  (2.5 s lookahead) and swaps it in at the boundary, so instant cuts play
  without a reload gap. Set `HALO_CLIPPER_TRACE_LIVE=1` to log sync decisions.
- Zoomed timeline: a thin scrollbar under the lanes; drag the thumb, or click
  the track to glide there. A white tick marks the playhead, and the view
  follows the playhead while it plays.

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
  bars around a fitted video. Text lives on Text lanes (T1, T2…) and paints in
  lane order with video, so a lower text lane sits under the video above it.
- T or Add text adds a three-second layer at the playhead on the topmost free
  text lane (a new top lane if none is free), pauses playback, selects it, and
  opens the inline editor on the preview. Double-click an empty spot on a text
  lane to add one there. A new layer inherits
  the previous look. Empty layers show a preview placeholder and never export it.
- Drag the preview text to position it. Canvas and video edges/centers snap;
  Alt bypasses snapping. Drag a corner or scroll over selected text to resize.
  Position changes reuse the texture; rasterization happens only for a change
  in text, style, size, canvas size, or animation state. No mpv filter rebuilds.
- The inspector has eleven bundled font choices, swatches and custom color,
  alignment, outline, shadow, and box controls. Position has no numeric
  sliders.
- Intros: None, Fade, Pop, Rise, Type, Words. Outros: None, Fade, Pop, Drop.
  Separate In/Out time sliders control duration (50 ms–5 seconds), including
  the complete Type/Words reveal. Short layers cap each transition at half
  their length. Paused selection shows settled text for editing;
  selected text outside its range is ghosted only while paused.
- Text bars sit on their lanes like clips; dragging one vertically moves it to
  another lane or a new one. Drag the body to move or an end to trim,
  with snapping to playhead and neighboring edit points (Alt bypasses it).
  S splits selected text and selects the right half; Del removes it. The bar
  context menu offers Duplicate and Delete.
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

## Color mixing and clip motion (0.10.1)

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
without switching the current Simple Clip source. In Advanced, dropping on the
timeline also places the files there (see the 2026-11-15 section).
The first video initializes an empty Simple workspace; an empty Advanced
workspace keeps its mode. Duplicate paths are skipped, invalid files report
an error while valid files in the same batch still import. An Advanced batch
uses one undo step. The Import button remains available.

Import numbers are stable, one-based, independent of track IDs, and preserved
in undo snapshots. Timeline and drag labels retain the same media number.
