# Halo Clipper

A fast, native Linux video editor with a focus on getting clips out the door — not learning a DAW. Built in Rust + egui. Single ~11 MB binary. No bloat.

> Formerly "Brett's Simple Clipper". The app migrates your old config and proxy cache automatically on first launch, and `./install.sh` cleans up the old menu entry.

Two modes for two workflows:

- **Simple Clip** — drop a video in, mark In/Out, export. The fastest way to grab a 20-second highlight.
- **Advanced** — multi-track NLE: split, delete, drag clips, per-clip audio gain, per-clip speed, burned-in text overlays. Still simpler than Resolve, way faster than Premiere.

## What it does

### Editing

- Open any video ffmpeg can read (`mp4`, `mkv`, `mov`, `webm`, `avi`, `m4v`) — via File → Open, the file dialog, "Open With" from a file manager, or `halo-clipper clip.mp4` on the command line
- **Simple Clip mode**: Set In / Set Out / Clear, exports one range. Stream-copies when keyframe-aligned, re-encodes when not.
- **Advanced mode**:
  - Multi-track timeline with V1 + one A track per source audio stream
  - **Split** at the playhead, **Delete** selected clips (with linked-partner cascade), **Detach Audio** to expose all source audio lanes
  - Drag clips horizontally with magnetic snap to clip boundaries
  - Timeline gaps are first-class: they play (and export) as black + silence
  - **Per-clip** audio gain (0 → 2.0, slider + preset chips in the right-hand Edit panel) — split a clip and one segment can be quiet while the rest stays loud. Applies to the selected clip and its linked V/A partner, never the whole track.
  - Per-clip speed 0.25× → 4.0× with live mpv preview (chained `atempo` so audio stays in sync)
  - Per-segment text overlays — font, color, size, **outline, drop shadow, opacity**; drag the caption around on the live preview; burned in only during that segment via `drawtext` at export (splits copy the caption to independently editable halves), with pixel sizes scaled to the output resolution so the export matches the preview
  - Linked-by-default V+A: edits propagate together until you press **D** to detach
  - Full undo/redo (Ctrl+Z / Ctrl+Shift+Z) on every mutation
  - Click any clip → the Edit side panel appears with gain / speed / caption controls, all scoped to that clip; click empty lane (or the ✕) to dismiss

### Playback

- libmpv embedded directly (no subprocess) — frames render straight into the egui GL context via an FBO blit
- Editing-tuned mpv config: 30 s decoded cache, 20 s readahead, 2 GiB demuxer cache, hr-seek framedrop, all CPU cores for SW decode
- **Pop Out** opens the source in a real mpv window if you want full-screen / pixel-perfect playback
- Volume slider, mute toggle, play/pause via the spacebar
- **Scrub proxy** (see below) — long-GOP sources scrub as smoothly as a native mpv window

### Export modes (one modal, three presets)

| Mode | Behavior |
|---|---|
| **Auto** | Stream-copies in Simple Clip mode if the In mark is keyframe-aligned; otherwise re-encodes at libx264 CRF 18 to preserve source quality. Always re-encodes in Advanced mode (the filter graph requires decoded frames). |
| **Discord** | Size-targeted CBR encode. Tier chips for Discord's upload caps — **Free (20 MB), Nitro Basic (50 MB), Nitro (500 MB)** — plus a 10 → 475 MB slider for anything in between. The encoder auto-picks codec, resolution, and bitrate from the clip's duration, and steps the audio bitrate down on tight budgets so a long clip still fits the free tier. See **Codec auto-selection** below. |
| **Custom** | Resolution preset (Source / 480p / 720p / 1080p / 1440p / 4K), frame-rate cap (Source / 24 / 30 / 60), codec (any HW encoder ffmpeg advertised, plus libx264 / libx265 / SVT-AV1), and **rate control**: constant quality (CRF / QP / CQ / ICQ — whichever the codec calls it, with a codec-appropriate slider range) or target bitrate with a live size estimate. "Advanced" reveals the per-codec encoder preset, audio codec (AAC / Opus), and audio bitrate. |

The active GPU is auto-detected from `/sys/class/drm` and the recommended codec is starred in the codec dropdown. Encoder detection runs `ffmpeg -encoders` once at startup and only offers what's actually available. Hardware **AV1** encoders get a second check — a 2-frame test encode — because ffmpeg advertises `av1_vaapi` / `av1_nvenc` on machines whose silicon has no AV1 encode block, and a phantom entry would only fail once an export was already running. Opus in an `.mp4` container automatically gets the `-strict -2` flag ffmpeg still requires.

#### Codec auto-selection (Discord mode)

Discord plays AV1 in MP4, and AV1 needs roughly 40% less bitrate than H.264 for the same picture — which is exactly what a hard size cap is short of. Halo Clipper switches to it only when it actually helps:

1. Work out the video bitrate the target size allows, and the resolution H.264 would be forced down to.
2. Compute **bits per pixel per frame** at that resolution. Below **0.10 bpp** H.264 starts smearing detail — and a 60 fps clip crosses that line at twice the bitrate a 30 fps one does, since the same bits are split over twice as many frames.
3. Under the threshold, switch to the best available AV1 encoder (GPU-matched first — VAAPI on AMD, NVENC on NVIDIA, QSV on Intel — then any hardware AV1, then software SVT-AV1). The extra efficiency is spent on resolution: the same 20 MB that only carried 720p on H.264 carries 1080p on AV1.
4. Over the threshold, stay on H.264 — it embeds in every Discord client and encodes several times faster, and the efficiency would buy nothing.

Software SVT-AV1 scales its preset by clip length (6 for ≤1 min, up to 12 past 15 min) so a long VOD doesn't turn into a coffee break. The stats panel names the chosen codec and says what the choice bought.

**Override:** the panel's codec row is **Auto / H.264 / AV1**. Auto runs the decision above; the other two pin the family and let bitrate and resolution follow from it. Forcing H.264 when Auto wanted AV1 tells you what it costs ("Auto would have held 1080p instead of 720p"), and the AV1 button is disabled outright on machines with no usable AV1 encoder.

### Performance

The four levers we pull for snappy editing on long, high-bitrate sources:

1. **Hardware decode (VAAPI)** — When the encoder is VAAPI and the source codec is H.264/HEVC/VP9/MPEG-2/VC-1, frames decode straight onto the GPU, flow through `scale_vaapi`, and into the encoder without ever leaving VRAM. ~2–3× the throughput of SW decode on 4K sources. Falls back transparently for codecs VAAPI can't decode (AV1, VP8, exotic).

2. **Scrub proxies** — On file open, a short-GOP H.264 proxy (GOP=12, 1080p max, libx264 veryfast CRF 22 — or h264_vaapi if available) generates in the background to `~/.cache/halo-clipper/proxies/<hash>.mp4`. mpv swaps to the proxy when it's ready. Result: timeline scrubbing snaps to 5+ keyframes per second instead of the 0.2 keyframes/sec a typical OBS recording offers. Cache-keyed by `(canonical_path, size, mtime)` so re-opening the same file is instant. Settings offers an **All-intra** profile (every frame a keyframe — frame-exact scrubs, bigger proxies) and an auto-clear-on-close toggle.

3. **mpv tuned for editing, not playback** — The defaults assume "press play and watch a movie." We override with `cache=yes`, `cache-secs=30`, `demuxer-readahead-secs=20`, `demuxer-max-bytes=2GiB`, `demuxer-max-back-bytes=2GiB`, `hr-seek-framedrop=yes`, `vd-lavc-fast=yes`, `video-latency-hacks=yes`. Re-scrubbing visited regions is instant.

4. **Continuous repaint while dragging** — egui's default is "repaint on input only," which freezes the preview if you hold the cursor still mid-drag. While `user_dragging_seek` is true we force `ctx.request_repaint()` every frame so mpv's latest decoded frame always shows.

### Visual polish

- Custom button system (`primary_button` / `secondary_button` / `accent_button` / `pill_button` / `speed_preset_chip`) — all painted directly so they actually react to hover/press states, all with proper hand-cursor + consistent typography
- Mode tabs are pill buttons that switch between Simple Clip and Advanced
- Big monospace playhead readout above the timeline (Resolve/Premiere convention) — current position bold, total muted
- Export progress modal with animated indeterminate ribbon for the initialization phase (no more "0%" looking broken)
- Selected cards in the Export Options modal carry the "selected" state via a subtle border + radio dot, not a saturated blue wash that fights body text

## Install (Linux desktop app)

Quick path — get it into your app launcher:

```bash
cd ~/Projects/Brett
./install.sh
```

That:
- Builds the release binary if not already built
- Copies it to `~/.local/bin/halo-clipper`
- Drops icons into `~/.local/share/icons/hicolor/{48,64,128,256,512}x.../apps/` plus the scalable SVG
- Writes `~/.local/share/applications/halo-clipper.desktop` with absolute paths substituted (so it works in any compositor regardless of session PATH)
- Removes any leftover `brett-clipper` install from before the rename
- Refreshes the desktop database + icon cache

Launch via your app menu / fuzzel / wofi / file-manager "Open With" — or from a shell as `halo-clipper` (optionally with a file path argument).

Right-click any `.mp4` / `.mkv` / `.mov` / `.webm` / `.avi` / `.m4v` in your file manager → "Open With" → Halo Clipper.

Uninstall:

```bash
./install.sh uninstall
```

### Runtime dependencies

| Required | Used for |
|---|---|
| `ffmpeg` ≥ 5 | Encode, decode, filter graph, all subprocess work |
| `ffprobe` | Probing source metadata |
| `libmpv` ≥ 2 | Embedded preview |
| `mpv` | "Pop Out" external window |
| `librsvg` (only at install) | Rasterizing PNG icons for the install step |

VAAPI hardware encode/decode wants `mesa-va-drivers` (AMD/Intel). NVENC wants `nvidia-utils`.

## Keyboard shortcuts

### Simple Clip

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `I` | Set In point at playhead |
| `O` | Set Out point at playhead |
| `S` | Set In, then Out, then restart cycle |
| `J` / `←` | Seek back 1s (Shift = 5s) |
| `L` / `→` | Seek forward 1s (Shift = 5s) |
| `M` | Toggle audio mute on V1 |

### Advanced

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `S` | Split every clip the playhead is on |
| `Del` | Delete selected clip(s) + linked partners |
| `D` | Detach selected clip(s) — exposes audio lanes |
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z` | Redo |
| Ctrl+Click | Add a clip to the selection (multi-select) |

(Text inputs swallow Delete and Ctrl+Z when focused — egui handles its own editing inside text fields, so accidentally backspacing in a caption box doesn't delete the clip.)

## Architecture

Module-by-module map of `src/`:

| File | Purpose |
|---|---|
| `main.rs` | eframe entry, viewport + icon setup, DejaVu font registration, CLI file argument |
| `app.rs` | All app state, UI layout, action dispatch, keyboard handling, modals |
| `theme.rs` | Color palette + global egui style |
| `track.rs` | `Clip` / `Track` / `TrackId` / `ClipId` / `LinkId` data model, `TextOverlay`, split/delete/detach logic, project_cut_duration |
| `timeline.rs` | Custom timeline widgets — Simple Clip rail, Advanced multi-track with drag-and-snap, scrubber |
| `player.rs` | libmpv FFI wrapper, render-context lifecycle, event thread, editing-tuned init properties |
| `media.rs` | ffprobe JSON → `MediaInfo` (duration, dims, fps, codec, audio stream count) |
| `proxy.rs` | Scrub-proxy generation (Standard / All-intra profiles), cache-keyed by `(path, size, mtime)`, clear_cache + cache_size_bytes for settings UI |
| `settings.rs` | Persistent prefs loaded/saved at `~/.config/halo-clipper/config.json` (atomic write via .tmp + rename; auto-migrates from the old brett-clipper path) |
| `encode.rs` | `EncodeSettings`, `VideoCodec` (with per-codec quality name/range/flags + `bitrate_efficiency`), `ResolutionPreset`, `EncodeQuality`, `DiscordNitro` tier math + AV1 codec picker, `NitroPlan` (one plan shared by the stats panel and the export), GPU vendor detection, encoder probe + AV1 capability verification, `HwDecode` strategy + `is_vaapi_decodable` gate |
| `export.rs` | Async export pipeline — `ExportPlan::{Simple, MultiTrack}`, ffmpeg subprocess with `-progress pipe:1` parsing, VAAPI device binding, black-gap synthesis, state-machine `build_video_post_chain`, `build_drawtext` with output-resolution scaling |
| `gl_loader.rs` | OpenGL function loader for libmpv's render context |
| `video_fbo.rs` | Helper FBO that mpv renders into; we then blit-copy into the egui rect |

### File-open flow

1. `media::probe()` runs ffprobe, returns `MediaInfo`
2. mpv loads the **source** immediately so editing can start right away
3. If a proxy for the current quality profile is cached, mpv re-loads it at playhead 0 (cache hit, instant)
4. Otherwise `proxy::spawn_transcode()` runs in the background; `update()` polls progress and updates the status bar
5. When the proxy finishes, `tick_proxy()` swaps mpv to the proxy at the current playhead position
6. `Project.proxy` stores the proxy path. **Export pipelines always read `Project.source`** — proxies are preview-only and have zero impact on output quality.

### Export filter-graph model

The multi-track graph is built from the cut timeline:

- **Video**: gaps between clips (and before the first / after the last, up to the shared timeline end) are synthesized as black `color=` segments — hwuploaded when the surrounding frames are VAAPI surfaces — then everything concats in cut order. Per-clip speed is applied per segment via `setpts`. Each segment is scaled and its own caption is burned before concat, so captions never extend into another segment or a black gap.
- **Audio**: per-track `atrim` + `atempo`/`volume` chains with `anullsrc` silence in the gaps, padded to the same timeline end, then `amix` across unmuted tracks. Video and audio always come out the same length.
- **Segment post-chain**: `build_video_post_chain(hw_decode, resolution, drawtext_expr, encoder_wants_gpu)` tracks a single `on_gpu` bool through scale → drawtext → encoder handoff, inserting `hwdownload`/`hwupload` bridges exactly where needed. Adding a new filter step means extending this one helper.

## Build from source

```bash
cd ~/Projects/Brett
cargo build --release          # ~9 sec recompile, ~1 min cold
cargo test --release           # 69 tests, incl. real-ffmpeg end-to-end exports
cargo clippy --release --all-targets
```

The test suite shells out to `ffmpeg`/`ffprobe` for its end-to-end cases (synthetic sources in `$TMPDIR/halo-clipper-tests/`), and exercises the full VAAPI pipeline automatically when a render node + VAAPI-capable ffmpeg are present.

Profile is already tuned for shipping:

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true
```

## Test coverage

69 tests as of v0.5:

- **`track::tests`** — clip splitting, link-group propagation, delete cascades, detach edge cases. Guards the data model against regressions.
- **`encode::tests`** — codec args for every rate-control mode (CRF / QP / CQ / ICQ / CBR with maxrate, and the libsvtav1 exception that rejects `-maxrate`), fps caps, resolution math, Discord tier/budget math including the adaptive audio step-down, the AV1-vs-H.264 decision at both frame rates, and GPU vendor recommendation logic.
- **`export::tests`** — exhaustive permutations of the filter-chain state machine (`HwDecode × scale × drawtext × encoder placement`), input-seek rebasing, output-timeline math (gaps + speed), drawtext scaling/opacity, Opus container gating — plus **five end-to-end exports through real ffmpeg**: gap black/sync, speed sync, silent sources, Opus-in-MP4, and the full VAAPI GPU graph.

## Settings & cache

User config lives at `~/.config/halo-clipper/config.json` (atomic write). Open via **File → Settings**.

- **Scrub proxy quality** — Standard (GOP 12; small, smooth) or All-intra (every frame a keyframe; frame-exact scrubbing, bigger files). Applies to the next open; each profile caches separately.
- **Auto-clear cache on close** — wipes `~/.cache/halo-clipper/proxies/` on app exit. Off by default (caching is the point of the proxy feature).
- **Clear cache now** — manual button. Shows live cache size, reports MB freed.

Proxy cache location: `~/.cache/halo-clipper/proxies/<16-hex>[-ai].mp4`, keyed by a hash of `(canonical_path, size, mtime_ns)` plus the quality profile. A cache from the pre-rename app is migrated automatically.

## Design principles

- **Native, not Electron.** No 200 MB framework, no JS runtime. ~11 MB binary, sub-second cold start.
- **Subprocess ffmpeg, dynamic libmpv.** Updating system ffmpeg never requires recompile. mpv is the only direct link (via libmpv2 + libloading), and the linker handles version compatibility through the standard SONAME.
- **Export quality is sacred.** The proxy never reaches the export pipeline. Whatever lands in your output file is encoded straight from the original source.
- **Preview and export agree.** Timeline gaps show black in the preview and export as black. Caption sizing on the preview matches the burned-in result at any output resolution.
- **Editing modes follow the user's intent.** Simple Clip is intentionally limited so it can stay fast. Advanced grows when you need real NLE features but never gets in the way of a quick mark-and-cut.
- **Don't pretend you're a movie studio.** No project files yet — every open is a fresh session. Real users export and move on; we don't want to chase the long tail of "load a project from last week" until someone asks for it.

## Roadmap

Done since v0.1:

- ~~Embedded libmpv preview~~
- ~~Smart-cut (keyframe-aware copy-vs-encode)~~
- ~~Multi-track timeline (Advanced mode)~~
- ~~Per-clip / per-track audio gain~~
- ~~Per-clip speed control~~
- ~~Text overlay with font picker + drag-to-position~~
- ~~Multi-stream audio (one A track per source audio stream)~~
- ~~Click-to-select Edit side panel~~
- ~~Auto / Discord / Custom export modes~~
- ~~Discord tier presets + target-size slider~~
- ~~Hardware encoder detection + GPU vendor probe~~
- ~~VAAPI hardware decode for editing-rate scrubbing~~
- ~~Scrub-proxy generation with disk cache + quality profiles~~
- ~~Persistent settings + cache management~~
- ~~Linux desktop integration (icon, .desktop, install.sh, "Open With" file argument)~~
- ~~Coherent custom button system + polished modals~~
- ~~Constant-quality rate control on every codec family (CRF/QP/CQ/ICQ)~~
- ~~Text overlay typography: outline, drop shadow, opacity~~
- ~~Frame-rate cap in Custom export~~
- ~~Opus audio option~~

Possible next:

- NVENC hardware decode path (currently NVENC users stay on SW decode)
- Project file format (save/restore a session)
- Keyboard shortcut editor in Settings
- Freehand / shape annotations burned into export

## License

MIT.
