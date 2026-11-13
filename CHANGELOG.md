# Changelog

## 0.10.2

- Fix Linux updates failing to reopen on HaloOS / Arch: build against the
  supported mpv client API 2 on Ubuntu 24.04.
- Add a real mpv initialization check to Linux/Windows release validation.
- Check Linux update startup before touching the current binary or backup;
  restore the previous version on immediate restart failure and keep the old UI open.

## 0.10.1

- Keep animated clip dimensions correct across Windows and Linux media runtimes.

- Keep text export timing accurate on older Linux FFmpeg releases as well
  as the bundled Windows runtime.

- Subtle, stable import-order numbers on Media names, drag labels, and
  timeline clips (1 · filename, 2 · filename).

- Media: native file drops on Windows and Linux, with a temporary drop cue,
  multi-file import, duplicate handling, and preservation of the active edit.

- Text: independent, precise In/Out timing, custom outline and box colors,
  two-color gradients with direction and palette shortcuts, and box opacity.
  Extra color controls expand on demand; removed the bottom preview/help/delete block.
- Per-segment video intro/outro effects in Edit: Fade, Snap zoom, Pull back,
  Whip, and Flash, with separate durations, Amount, and edge playback.
  Preview and exports share timing; splits preserve effects on the outer edges.

- Rebuilt burned-in text as independent canvas layers: drag anywhere, including
  portrait canvas bars; resize with corner handles or the scroll wheel.
- Added eleven bundled fonts, six style presets, color and outline controls,
  intro/outro animations, typewriter and word reveals.
- Text has independent timeline bars for moving, trimming, splitting, duplicating,
  and deleting; typing and drag gestures undo together.
- Preview text renders locally without rebuilding mpv filters. Export uses the
  same Rust renderer with frame-rate-correct animation and cancellable preparation.
- Removed redundant empty-editor “Open a video to begin” messages.

- Interface typography (Inter + JetBrains Mono), painted icons, refined
  graphite palette, segmented mode switch, menus that unroll from their button.
- Advanced timeline: pinned ruler with live time, large playhead grab handle,
  clearer split seams, attached-audio strip on V+A bars, preview-only track
  mute buttons, clips leave their old lane immediately when dragged.
- Canvas: magnetic snapping to canvas and layer edges with guides (Alt to
  bypass); Reset frame fits with bars when the canvas shape differs (9:16).
- Transport: go-to-start button, Home/End shortcuts.
- Advanced timeline: Ctrl+scroll zooms around the pointer (fit-to-width stays
  the default); footer shows the zoom factor and a Fit button.
- Canvas toolbar: aspect and Background are drop-downs; a compact color
  picker (square, hue strip, presets, hex) replaces the floating window.
- Palette: soft-white playhead, teal audio clips, blue snap guides.
- Keyboard shortcuts keep working after clicking Play, Split or any button.
- Windows: multi-size icon and version info embedded in the exe (crisp in
  the title bar, taskbar, Start menu and Apps list); installer and
  uninstaller use the app icon.

## 0.8.0

- Replace the Layers button panel with a media bin and drag-focused timeline.
- Import without automatic placement; remove media, including the final item,
  along with its timeline instances.
- Move and resize overlapping videos directly on the preview using corner handles.
- Replace background timeline encoding with live video/audio decoders and GPU
  composition. Transform edits update without preparing another preview.
- Keep the inspector closed until requested, remove duplicate transport rows,
  and make the compact timeline resizable.
- Follow the largest video on the timeline for canvas and export dimensions,
  with explicit manual overrides.
- Preserve clip source identity when moving clips across tracks and restore
  imported media state through undo / redo.

## 0.7.0

- Advanced: import video and audio into independently ordered layers.
- Add layer visibility / mute, duplication with linked audio, timing and trim
  controls, video opacity, size, and placement, and undo / redo support.
- Share a compositor between original-quality exports and debounced background
  previews, keeping layer order, gaps, independent audio, and speed in sync.
- Ship a Windows GUI executable, portable bundle, and per-user installer with
  pinned media runtimes and embedded caption fonts.
- Add signed GitHub release updates, optional startup checks, rotating local
  logs, path redaction, support report export, and issue templates.
- Preserve preview initialization and HaloOS MangoHud crash fixes.
- Fix parallel test-source generation and bound FFmpeg error buffering.
