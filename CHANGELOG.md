# Changelog

## 0.10.14

- `.haloedit` files now have their own icon and open Halo Clipper when you
  double-click them, on both Windows and Linux — no more "it's just a text
  file" problem.
- "Save Edit" is hidden from the menu when there's nothing to save (no video
  or empty timeline).

## 0.10.13

- Save Edit / Open Edit: save your whole timeline to a file and reopen it
  exactly where you left off. It links to your source videos, so if one is
  moved or deleted, opening tells you which file is missing.
- The Auto export tab now shows what it will do (copy vs re-encode), the
  settings it will use, and a projected file size — no more blank screen.
- Disabling a track now only hides it in the preview; the export still
  includes it. The menu reads "Hide/Show track in preview."
- Timeline tracks are numbered sensibly per kind — V1, V2, V3, T1, T2,
  A1, A2 — instead of climbing internal IDs.
- Dragging a video from the file manager onto the timeline shows the same
  accurate clip preview as dragging from the media bin.
- The Updates window is cleaner and now shows the new version's changelog
  before you install.
- Halo Clipper can update itself whenever it's installed in a writable
  location (e.g. your home folder), not only a downloaded build.

## 0.10.12

- Discord exports stay under the chosen size limit, and a failed GPU
  encoder automatically falls back to the CPU encoder.

## 0.10.11

- New text layers last 10 seconds (up to the end of your clips) instead of
  3, so they're big enough on the timeline to grab and resize.

## 0.10.10

- Narrow windows (e.g. half a screen): the preview toolbar wraps instead of
  stretching the layout, and the video can no longer paint under the Edit
  panel.
- Clearer selection on the timeline: a soft-white ring with trim grips on
  each end; the clip keeps its own color.
- Export window is much shorter: Auto / Discord / Custom is a switch, the
  Discord settings sit on aligned rows, and the result is one summary card.

## 0.10.9

- Panel sizes (Media, Edit, timeline height) are remembered between
  sessions; Settings → Reset panel sizes restores the defaults.
- Switching the output shape (e.g. Original → Reels / TikTok) fits every
  video to the new shape; one undo step.
- Text no longer vanishes when deselected with the playhead on its first
  frame (the intro's empty frame); paused previews show it settled.
- The Media hint hides while a file is dragged over the window.
- Linux: the app icon shows in GTK launchers at small sizes again.

## 0.10.8

- Edit panel split into tabs: Text / Look / Animate for text, Frame /
  Animate / Playback for video clips.
- Cleaner sliders: thin rail, small knob.
- Halo logo menu in the top-left holds Open Video, Settings, Help & updates
  and Quit; plain-text Simple Clip / Advanced switch.
- Fit to screen is now first in the preview toolbar.
- Files dropped on an empty timeline start at the very beginning.
- The Media drop highlight stays inside the list.
- Text style presets removed.

## 0.10.7

- Linux: dragging files into the app now works on Wayland desktops (niri,
  GNOME, KDE), onto Media or straight onto the timeline.
- Moving clips and text between layers is predictable and smooth: lanes slide
  apart to open a "New layer" row, and the dragged item glides into the row it
  will land in. Dropping on a free lane of the same kind uses that lane.
- Shift+Enter adds a new line while typing on the preview.
- Gradient colors: pick Start / End, with direction tiles that show the result.
- "Fit to screen" replaces Reset frame and the Fit/Fill switch; it lights up
  when the video has been moved or resized. Also in the clip panel.
- Bigger Simple Clip / Advanced switch; File and Help moved to the left corner.

## 0.10.6

- Advanced: fix echoing and clipping audio when playing, pausing or moving
  clips. Streams of one file now share one decoder with the export's limiter,
  and the Simple player is stopped when entering Advanced.
- Advanced: instant cuts play smoothly; the next clip on each layer is
  preloaded and swapped in at the cut.
- Text is a normal layer you can put on any lane (new text goes on top); the
  Text band is gone. Type directly on the preview.
- Cleaner text settings (gradient stop bar, no presets) and clip Frame /
  Animation panel with a proper Preview button.
- G closes gaps (selection-aware ripple).
- Drop files onto the Advanced timeline; cleaner drop highlight on Media.
- Switching to Advanced places the first clip on the timeline.
- Minimal timeline scrollbar with a playhead marker when zoomed.

## 0.10.5

- Windows: if the app can't start (for example a graphics driver without
  OpenGL 2.0, or a Remote Desktop session), show an error box explaining why
  instead of closing silently.
- Windows: a typed export name without an extension (e.g. "my clip") no longer
  fails with "Unable to choose an output format"; the save dialog now offers an
  MP4 filter and the extension is added automatically.
- Windows: "Open Folder" after an export now opens Explorer with the file
  selected (it called the Linux-only xdg-open before).
- Small screens: the window starts maximized when 1280x800 doesn't fit (1366x768
  laptops), and the Export Options dialog scrolls so Begin Export stays visible.
- Discord export card now says Free 20 MB, matching the tier buttons.

## 0.10.4

- Correct the Windows native import library to reference the bundled libmpv-2.dll.

- Bundle the verified Vulkan loader needed by Windows libmpv and test the
  complete portable payload without developer runtime paths.

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
