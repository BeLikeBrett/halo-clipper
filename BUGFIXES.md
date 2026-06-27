# Bug Fixes

Running log of bugs found and fixed in brett-clipper. Newest at top.

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
