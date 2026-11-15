# Bug Fixes

Running log of bugs found and fixed in Halo Clipper (formerly
brett-clipper). Newest at top.

---

## 2026-11-15 — Advanced: echoing / clipping audio while playing and editing

### Symptom

- Playing, pausing or moving clips in Advanced sometimes doubled the audio
  for a moment (echo), and a clip with two audio streams could distort.
- Once, after editing in Simple Clip and switching to Advanced, the audio
  sounded clipped, as if both editors were playing.

### Root cause

- Every audio stream had its own mpv decoder. Two streams of the same file
  drifted apart independently, which is heard as an echo.
- The decoders' outputs were summed by the sound server with no limiter,
  while the export runs `amix` + `alimiter`; loud game audio plus mic clipped
  only in preview.
- After a seek or resume, a decoder that came back late was only corrected by
  the periodic drift check (up to ~750 ms of overlap).
- Splitting a clip reloaded the same file at the cut, which briefly overlapped
  the old and new decoders.
- Entering Advanced paused but did not unload the Simple Clip player, so
  nothing guaranteed it stayed silent.

### Fix

- `live.rs`: group streams by file and timing; one decoder per group with a
  lavfi-complex `volume`/`amix`/`alimiter` graph matching the export (unit
  test pins the graph string). Lone streams fold their gain into mpv's cubic
  volume.
- An epoch counter forces immediate realignment after seek/pause/play.
  Same-file clip changes seek instead of reloading. Standby decoders are
  parked on the next cut and swapped in.
- `set_mode` calls `Player::stop()` when entering Advanced.
- Verified in the Xvfb rig with a two-stream test file: steady drift −9 ms
  video / +13 ms audio, and −3 / −25 ms across a cut.

---

## 2026-11-13 — Windows: app did nothing when launched; export/UI gaps on Windows

Found by installing the CI-built `Halo-Clipper-Setup.exe` (0.10.4) on a clean
Windows 10 22H2 VM (`/satassd/winiso/clipper-test.qcow2`, an overlay over
`win10.qcow2`).

### Symptom

- Finish → "Open Halo Clipper", the desktop shortcut, and running the exe
  all did nothing: no window, no message, process gone in ~2 s.
- With the window working: typing an export name without ".mp4" failed
  with ffmpeg "Unable to choose an output format".
- "Open Folder" after an export did nothing.
- On a 1024x768 / 1366x768 screen the 1280x800 window and the Export
  Options dialog ran off the bottom; Begin Export was unreachable.

### Root cause

- The log said `egui_glow requires opengl 2.0+`: the VM's Basic Display
  Adapter only has GDI OpenGL 1.1. `run_native` returned Err, `main`
  returned it, and a `windows_subsystem = "windows"` exe has no console,
  so the user saw nothing. Same for real users without a vendor GPU
  driver or on Remote Desktop.
- `rfd` returns exactly what was typed; the Windows dialog filter was "All
  Files", so nothing appended an extension.
- Open Folder spawned `xdg-open` unconditionally.
- The export modal used a fixed height with no scroll area.
- CI only runs `--self-check` on Windows, which never creates a window, so
  none of this was covered.

### Fix

- `platform::show_startup_error` shows a MessageBox (and logs) when
  `run_native` fails, with an OpenGL/driver hint when relevant.
- `export::ensure_extension` appends the suggested extension when the
  chosen path has none; the dialog also gets a "<EXT> video" filter.
- `platform::reveal_in_file_manager`: `explorer.exe /select,"<file>"` on
  Windows, `xdg-open <dir>` elsewhere.
- Export modal height is capped to the window and its body is a
  `ScrollArea`; `fit_window_to_monitor` maximizes on the first frame if the
  window doesn't fit the monitor.

### Verification

On the VM: no-GL launch shows the error box; with Mesa llvmpipe
(`opengl32.dll` + `libgallium_wgl.dll` next to the exe and
`GALLIUM_DRIVER=llvmpipe` — without that env var Mesa picks its D3D12
backend, which dies with exit 0x80070057 in a VM without D3D12) the app
opens maximized, plays, exports via Discord and Auto, a typed
"typed_name" saves as typed_name.mp4, and Open Folder selects it in
Explorer. Unit test `typed_name_without_extension_gets_suggested_one`.

---

## 2026-11-13 — Shortcuts stopped after clicking Play or Split

### Symptom

After clicking Play (or Split) with the mouse, pressing S or Space did
nothing until the user clicked an empty part of the window.

### Root cause

egui buttons take keyboard focus when clicked, and a focused button claims
Space/Enter/arrow keys. Split also kept the original (left) clip selected,
so the next S press looked for a selected clip under the playhead and found
none.

### Fix

Painted controls use a non-focusable click sense (`widgets::CLICK`), and
`release_mouse_focus` drops focus left on non-text widgets by a mouse press
at the end of each frame. Split now selects the right-hand pieces.

---

## 2026-11-13 — Windows light mode turned stock widgets light

### Symptom

Under Windows (seen in Wine) labels, the Import button and separators
rendered in egui's light style on the dark panels: near-black "Media" title,
white buttons.

### Root cause

egui 0.29 follows the OS theme by default. `set_visuals` only styled the
active theme, so when the OS reported light mode egui switched to its
default light style for every stock widget.

### Fix

`theme::apply` pins `Theme::Dark` and sets the dark visuals/style
explicitly with `set_visuals_of` / `set_style_of`.

---

## 2026-11-13 — Duplicate "Halo Clipper" launcher entry with a gear icon

### Symptom

The app menu listed Halo Clipper twice, one entry with a generic icon.

### Root cause

Testing the Windows installer under Wine let winemenubuilder mirror the
Start-menu shortcut into ~/.local/share/applications/wine/. Separately, the
long-running launcher daemon had cached a failed icon lookup.

### Fix

Removed the Wine-generated entries, refreshed the icon cache and restarted
the launcher daemon. winemenubuilder is disabled in the build prefix
(`HKCU\Software\Wine\DllOverrides`, `winemenubuilder.exe` = "").

---

## 2026-11-13 — Advanced: dragged clip stayed visible in its old lane

### Symptom

Dragging a clip (or a V+A compound) to another track drew the drop ghost
at the target while the original bar kept rendering at its old position until
release, so the clip looked duplicated rather than moved.

### Root cause

The lane loop painted every clip unconditionally; the drag payload only added
a ghost on top.

### Fix

`show_editor_timeline` skips the clip whose id matches an `EditorDrag::Clip`
payload once `placement` resolves a drop position, and the ghost is painted
with the full clip renderer (thumbnail, label, audio strip). Seam markers
ignore the moving clip, so a split partner doesn't show a cut against a gap.

### Verification

Headless Xvfb rig: split a clip, dragged the right half to a new lane, and
captured mid-drag. The old lane was empty; only the ghost was drawn.

---

## 2026-11-13 — Advanced timeline panel shrank 8 px per frame

### Symptom

While building the pinned ruler, the bottom panel shrank on every frame
until the floor height, ignoring the row-count default.

### Root cause

egui's resizable `TopBottomPanel` stores the height its content actually
used. The lane `ScrollArea` budget subtracted two item spacings while only one
followed it, so content came up 8 px short each frame.

### Fix

Budget exactly one spacing, and end the timeline with
`ui.expand_to_include_rect(ui.max_rect())` so a short frame can never
ratchet the panel down.

### Verification

Rig: dropped a second clip; panel grew to the 2-row default and stayed put.

---

## 2026-08-27 — Edit panel: audio gain applied to the whole track, not the clip

### Symptom

Pull the Gain slider down on one loud segment and *every* segment on
that audio track got quieter — in the preview and in the export. There
was no way to duck a single moment (say, a spot where your voice spikes)
without ducking the entire clip. Speed, sitting right below it in the
same panel, was correctly per-clip, which made the gain behavior read as
a bug rather than a design choice. It was.

### Root cause

`Clip` already carried a per-clip `gain` field, `export.rs` already
emitted `volume=` per clip inside the concat, and `split_clip` already
copied gain to both halves. The Edit panel was the only piece that never
got the message: its snapshot collected `audio_track_ids` and the slider
wrote `Track::gain`, so `Clip::gain` was dead weight everywhere except
the export filter graph, which faithfully applied a value nothing could
change. The preview mix (`build_audio_chain`) read track gain too.

### Fix

`src/app.rs`:

1. The Edit panel snapshot now collects `audio_clip_ids` — the selected
   clip itself for an audio clip, or the audio partners in that clip's
   link group for a video clip — and the slider writes `Clip::gain` on
   each. Track gain is left alone (still applied after the concat, still
   available for a future mixer).
2. `build_audio_chain` multiplies each track's gain by the gain of the
   clip currently under the playhead, so the preview follows the segment
   being played. mpv's volume filter is a single scalar, so this is the
   same edge-triggered trick `sync_clip_speed` already used for speed —
   the chain string changes at a clip boundary and `sync_audio_mix`
   pushes it.
3. Added Mute / −6 dB / 1× / +3.5 dB preset chips next to the slider,
   matching the speed row.

### Note

Gain is per *clip*, so ducking part of a take still starts with a split
(**S**) — the panel now says so.

---

## 2026-07-16 — Advanced export: A/V desync after any timeline gap

### Symptom

In Advanced mode, any project with a gap on the timeline (split + delete
a middle piece, drag clips apart, delete the first clip) exported with
video and audio out of sync from the gap onward. The preview looked
right; the exported file drifted by exactly the gap length.

### Root cause

`run_multitrack` built the two chains with different timeline models.
The video chain concatenated the trimmed segments back-to-back, which
*squashes* gaps out of the output. The audio chain inserted `anullsrc`
silence for every gap, which *keeps* them. Video came out shorter than
audio, and every clip after the first gap played against the wrong
sound. (A leading gap or trailing gap also desynced the tail because the
audio's final padding used the sum of clip durations rather than the
timeline extent.)

### Fix

`src/export.rs`:

1. Video gaps (leading, between clips, and trailing up to the shared
   timeline end) are synthesized as black `color=` segments and fed into
   the concat in cut order — matching both the preview (which paints
   black in gaps) and the audio chain's silence. On the VAAPI path the
   gap source is bridged with `format=nv12,hwupload` so concat sees one
   surface type.
2. New `timeline_end_ms()` — the rightmost cut position across video
   clips and emitting audio tracks — is now the padding target for both
   chains, so their durations always match.
3. `total_output_ms()` (progress-bar total) now uses the real output
   duration: gaps kept, per-clip speed applied (a 2× clip renders half
   as long). Discord's bitrate budget uses the same math, so its size
   targeting is no longer wrong for sped-up or gapped cuts.

### Verification

New end-to-end tests in `export::tests` run the real pipeline against a
generated source: `e2e_gap_exports_black_and_keeps_av_sync` asserts both
streams come out at the full timeline length and `blackdetect` finds the
black interval at the gap position; `e2e_speed_clip_shortens_both_streams`
asserts a 2× clip halves both streams and the progress total;
`e2e_vaapi_gap_graph_when_hardware_present` runs the same graph through
the full VAAPI GPU path (verified locally on the 7800 XT).

---

## 2026-07-16 — Advanced export fails outright on videos with no audio stream

### Symptom

Opening a soundless video (screen capture without mic, some phone
clips), switching to Advanced, and exporting failed with an ffmpeg
"matches no streams" error.

### Root cause

`ensure_tracks_initialized` guarantees at least one audio lane so the
data model keeps a consistent shape, giving it `source_audio_index = 0`
even when `audio_stream_count == 0`. The export plan then emitted
`[0:a:0]atrim=...` against a stream that doesn't exist, which aborts the
whole ffmpeg run. The preview had the same latent problem via mpv's
`lavfi-complex` (`[aid1]` with no audio track).

### Fix

- `start_export` drops audio tracks whose `source_audio_index` isn't
  backed by a real source stream; if none remain the export gets `-an`.
- `build_audio_chain` (preview) returns None for soundless sources and
  skips tracks pointing at nonexistent streams.

Verified by `e2e_silent_source_exports_without_audio`.

---

## 2026-07-16 — Burned-in captions render at the wrong size on scaled exports

### Symptom

A caption positioned/sized against the preview exported noticeably
larger than shown whenever the output resolution was below the source
(e.g. 4K source → 1080p export), and smaller when upscaling.

### Root cause

`drawtext` runs *after* the `scale` step, so its `fontsize` is in output
pixels — but the app passed the overlay's `font_size_px`, which is in
source pixels (that's what the preview scales against).

### Fix

`build_drawtext` in `src/export.rs` scales every pixel-denominated value
(font size, outline width, shadow offset) by `output_height /
source_height`. Pinned by `drawtext_scales_pixel_values_to_output_height`.

---

## 2026-07-16 — Dead thumbnail pipeline burned an ffmpeg spawn on every seek

### Symptom

No visible symptom — that's the problem. Every playhead move while
paused spawned an ffmpeg process to extract a 1280px still, decoded it,
and uploaded it as an egui texture… which nothing ever painted. The
embedded mpv preview replaced the still-frame preview long ago, but the
worker was never unplugged.

### Fix

Removed `src/thumb.rs`, the `ThumbWorker` field, and the per-frame
request/drain calls. Scrubbing no longer forks ffmpeg processes at all.

---

## 2026-07-16 — "Open With" from a file manager opened an empty app

### Symptom

The .desktop entry advertises video MIME types and passes `%F`, but
choosing Halo Clipper in a file manager's "Open With" menu launched the
app without loading the file.

### Root cause

`main()` never read `argv`.

### Fix

`main.rs` picks up the first CLI argument and routes it through the new
`ClipperApp::open_path()` (the same path the file dialog uses).

---

## 2026-07-16 — Startup probed the GPU and ffmpeg encoders twice

`ClipperApp::new` ran `detect_hw_encoders()` (an ffmpeg subprocess) and
`detect_gpu()` twice each — once for the stored fields, once inside the
Custom-encode defaults block. Now probed once and shared.

---

## 2026-06-27 — Advanced export: long "warming up" stall before encoding starts

### Symptom

In Advanced (MultiTrack) mode, exporting a clip taken from late in a long
source sits on "Initializing encoder… / warming up…" with progress pinned
at `00:00.000` for a long time before it starts. A real case: a 28s clip
sourced from `14:01.950` of a 15-minute 1440p60 replay warmed up for ~46s.
The further into the source the clip starts, the longer the stall; clips
near the start of a file barely stall at all.

### Root cause

`run_multitrack` (`src/export.rs`) trimmed entirely inside the filter graph
on **absolute** source timestamps (`[0:v]trim=start=841.95:...`) and passed
**no `-ss` before `-i`**. So ffmpeg decoded the source from `t=0` and fed
every frame through `trim`, which discarded everything before the in-point.
For a clip at 14:01 that's ~14 minutes (~50k frames at 1440p60) of pure
decode-and-discard — on the GPU via the VAAPI path, but still wall-clock
time — before a single output frame reached the encoder and `-progress`
could report anything. The Simple path was unaffected: it already fast-seeks
with `-ss` before `-i` (`run_simple`).

### Fix

All in `src/export.rs`:

1. New pure helper `earliest_src_start_ms(video_clips, audio_tracks)` —
   the smallest `src_start_ms` across every clip that actually enters the
   graph (all video clips + audio clips of non-muted, non-empty tracks).
2. `run_multitrack` computes `seek_ms` from it, emits `-ss {seek_ms}`
   before `-i`, and rebases every `trim`/`atrim` start/end by subtracting
   `seek_ms`. Input `-ss` accurate-seeks to the keyframe before the point,
   decode-discards to the exact frame, and rebases output timestamps to 0
   — so the rebased trims line up exactly. `anullsrc` silence durations are
   on the output timeline, so they're untouched. Muted/empty tracks are
   excluded so the seek never reaches back for frames nothing references.

Helper is `≤` every emitted start, so rebased trim values stay non-negative.

### Verification

- `cargo build --release` + `cargo test` clean (50 pass, incl. 4 new
  `earliest_src_start_ms` tests); `cargo clippy --release` warning-free.
- Frame-accuracy: replicated the generated filtergraph old (absolute trim,
  no seek) vs new (`-ss` + rebased trim) on a 20s source, decoding a 3s
  mid-GOP range near the end to deterministic rawvideo. Outputs are
  **byte-identical** (same md5, 90 frames) — the seek selects the exact
  same frames.
- Warm-up: same comparison on the VAAPI path, 5-min 720p30 source, last 5s
  → old **4.77s** vs new **0.32s** wall (~15×), both outputs valid 5.0s
  video+audio. Effect scales with how far the in-point sits into the source.

---

## 2026-05-23 — Advanced mode: preview goes black, Play does nothing after split + delete

### Symptom

In Advanced (MultiTrack) mode, after splitting a clip and deleting one of
the halves, the preview goes black-or-frozen and clicking Play does
nothing. Export of the same project still produces a correct file.

### Root cause

The playhead can land in a *gap* on V1 — i.e. a cut position with no clip
covering it. The classic trigger:

1. Playhead at `30`.
2. Split V1 clip `[0..60]` at `30` → `[0..30]` and `[30..60]`.
3. Delete the right half `[30..60]`.
4. Playhead is now `30`, but `[0..30]`'s `contains_cut` is exclusive of
   the end (`t < cut_end`), so no V1 clip covers `30`.

From here, three pieces of preview/playback code all fall over:

- **`tick_multitrack_playback`**: the "no clip at playhead" branch called
  `set_paused(true)` and snapped the playhead to `0` every frame mpv was
  playing. So pressing Play unpaused mpv for one frame, the next tick saw
  the gap, paused again, snapped to `0` — and from the user's POV "Play
  does nothing."
- **`toggle_play`** only special-cased "playhead at end of canvas → wrap
  to start." It did nothing for a gap in the middle, so it couldn't break
  the pause-loop above.
- **`playhead_to_src`** returns `None` in a gap, so the seek dispatch in
  `update()` issues no seek. mpv's FBO keeps whatever it last rendered.
  If the window/preview had been resized recently, the FBO is cleared to
  black → blank preview.

Export is unaffected because the export pipeline already treats V1 gaps
as black silence — it doesn't go through any of this preview code.

### Fix

All in `src/app.rs`. Three small changes:

1. **`tick_multitrack_playback`** — when the playhead is in a gap during
   playback, skip *forward* to the next clip's `cut_start_ms` and seek
   mpv to its `src_start_ms`. Only pause (cleanly, at `content_end_ms`)
   when nothing follows. No more reset-to-0 on every gap frame.

2. **`toggle_play`** (MultiTrack branch) — before unpausing, if the
   playhead is in a gap, pre-snap forward to the next clip's
   `cut_start_ms` (or wrap to the first clip if nothing's after).
   Guarantees Play lands on real content instead of fighting the tick
   handler.

3. **`show_preview`** — when the playhead is in a V1 gap, paint an
   explicit black rect instead of running the mpv paint callback. The
   preview now reads "black during gaps", which both matches what the
   export produces and makes the gap state visually obvious instead of
   showing a stale frame.

### Verification

- `cargo build --release` clean.
- Open a video → Advanced → Split, Delete one half → Play now skips
  forward to the next clip and plays normally.
- Preview shows black (not frozen) while the playhead sits in a gap.
- Export pipeline is untouched (no behavioural change there).
