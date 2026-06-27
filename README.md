# Brett's Simple Clipper

A fast, native Linux video editor with a focus on getting clips out the door — not learning a DAW. Built in Rust + egui. Single ~11 MB binary. No bloat.

Two modes for two workflows:

- **Simple Clip** — drop a video in, mark In/Out, export. The fastest way to grab a 20-second highlight.
- **Advanced** — multi-track NLE: split, delete, drag clips, per-track audio gain, per-clip speed, burned-in text overlays. Still simpler than Resolve, way faster than Premiere.

## What it does

### Editing

- Open any video ffmpeg can read (`mp4`, `mkv`, `mov`, `webm`, `avi`, `m4v`)
- **Simple Clip mode**: Set In / Set Out / Clear, exports one range. Stream-copies when keyframe-aligned, re-encodes when not.
- **Advanced mode**:
  - Multi-track timeline with V1 + one A track per source audio stream
  - **Split** at the playhead, **Delete** selected clips (with linked-partner cascade), **Detach Audio** to expose all source audio lanes
  - Drag clips horizontally with magnetic snap to clip boundaries
  - Per-track audio gain (0 → 2.0, slider lives in the right-hand Edit panel)
  - Per-clip speed 0.25× → 4.0× with live mpv preview (chained `atempo` so audio stays in sync)
  - Per-track text overlays — pick a font, color, size; drag the caption around on the live preview; burned in via `drawtext` at export
  - Linked-by-default V+A: edits propagate together until you press **D** to detach
  - Full undo/redo (Ctrl+Z / Ctrl+Shift+Z) on every mutation
  - Right-click any clip → Edit, opens the side panel for fine control

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
| **Discord Nitro** | Size-targeted CBR encode. A slider (50 MB → 475 MB at 25 MB steps) lets you pick how big the output should be. The encoder auto-picks resolution + bitrate based on duration so the final file lands just under target. Optimized for Discord's 500 MB Nitro upload cap. |
| **Custom** | Pick your own resolution preset (Source / 480p / 720p / 1080p / 1440p / 4K), codec (any HW encoder ffmpeg advertised + libx264 / libx265), CRF or bitrate, preset, audio bitrate. "Advanced" toggle reveals the per-codec preset dropdown and audio bitrate slider. |

The active GPU is auto-detected from `/sys/class/drm` and the recommended codec is starred in the codec dropdown. Hardware encoder detection runs `ffmpeg -encoders` once at startup and only offers what's actually available.

### Performance

The four levers we pull for snappy editing on long, high-bitrate sources:

1. **Hardware decode (VAAPI)** — When the encoder is VAAPI and the source codec is H.264/HEVC/VP9/MPEG-2/VC-1, frames decode straight onto the GPU, flow through `scale_vaapi`, and into the encoder without ever leaving VRAM. ~2–3× the throughput of SW decode on 4K sources. Falls back transparently for codecs VAAPI can't decode (AV1, VP8, exotic).

2. **Scrub proxies** — On file open, a short-GOP H.264 proxy (GOP=12, 1080p max, libx264 veryfast CRF 22 — or h264_vaapi if available) generates in the background to `~/.cache/brett-clipper/proxies/<hash>.mp4`. mpv swaps to the proxy when it's ready. Result: timeline scrubbing snaps to 5+ keyframes per second instead of the 0.2 keyframes/sec a typical OBS recording offers. Cache-keyed by `(canonical_path, size, mtime)` so re-opening the same file is instant. Toggle "auto-clear on close" in File → Settings if disk space is tight.

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
- Copies it to `~/.local/bin/brett-clipper`
- Drops icons into `~/.local/share/icons/hicolor/{48,64,128,256,512}x.../apps/` plus the scalable SVG
- Writes `~/.local/share/applications/brett-clipper.desktop` with absolute paths substituted (so it works in any compositor regardless of session PATH)
- Refreshes the desktop database + icon cache

Launch via your app menu / fuzzel / wofi / file-manager "Open With" — or from a shell as `brett-clipper`.

Right-click any `.mp4` / `.mkv` / `.mov` / `.webm` / `.avi` / `.m4v` in your file manager → "Open With" → Brett's Simple Clipper.

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
| `main.rs` | eframe entry, viewport + icon setup, DejaVu font registration |
| `app.rs` | All app state, UI layout, action dispatch, keyboard handling, modals |
| `theme.rs` | Color palette + global egui style |
| `track.rs` | `Clip` / `Track` / `TrackId` / `ClipId` / `LinkId` data model, split/delete/detach logic, project_cut_duration |
| `timeline.rs` | Custom timeline widgets — Simple Clip rail, Advanced multi-track with drag-and-snap, scrubber |
| `player.rs` | libmpv FFI wrapper, render-context lifecycle, event thread, editing-tuned init properties |
| `media.rs` | ffprobe JSON → `MediaInfo` (duration, dims, fps, codec, audio stream count) |
| `proxy.rs` | Scrub-proxy generation, cache-keyed by `(path, size, mtime)`, clear_cache + cache_size_bytes for settings UI |
| `settings.rs` | Persistent prefs loaded/saved at `~/.config/brett-clipper/config.json` (atomic write via .tmp + rename) |
| `encode.rs` | `EncodeSettings`, `VideoCodec`, `ResolutionPreset`, `EncodeQuality`, `DiscordNitro` math, GPU vendor detection, HW encoder probe, `HwDecode` strategy + `is_vaapi_decodable` gate |
| `export.rs` | Async export pipeline — `ExportPlan::{Simple, MultiTrack}`, ffmpeg subprocess with `-progress pipe:1` parsing, VAAPI device binding, state-machine `build_video_post_chain` that handles `scale_vaapi` vs `scale`, `hwdownload` around `drawtext`, final `hwupload`-or-`hwdownload` bridge |
| `thumb.rs` | Background worker that pulls preview frames via `ffmpeg` (used as a still fallback when mpv isn't actively rendering) |
| `gl_loader.rs` | OpenGL function loader for libmpv's render context |
| `video_fbo.rs` | Helper FBO that mpv renders into; we then blit-copy into the egui rect |

### File-open flow

1. `media::probe()` runs ffprobe, returns `MediaInfo`
2. mpv loads the **source** immediately so editing can start right away
3. If `proxy::proxy_exists(source)` is true, mpv re-loads the proxy at playhead 0 (cache hit, instant)
4. Otherwise `proxy::spawn_transcode()` runs in the background; `update()` polls progress and updates the status bar
5. When the proxy finishes, `tick_proxy()` swaps mpv to the proxy at the current playhead position
6. `Project.proxy` stores the proxy path. **Export pipelines always read `Project.source`** — proxies are preview-only and have zero impact on output quality.

### Export filter-graph state machine

`build_video_post_chain(hw_decode, resolution, drawtext_expr, encoder_wants_gpu)` in `export.rs` is the single source of truth. Tracks a `on_gpu: bool` through the chain:

- Starts on GPU iff HW decode is enabled
- `scale_vaapi` (GPU) or `scale` (CPU)
- `drawtext` is CPU-only — if currently on GPU, inserts `hwdownload, format=nv12` first
- Final bridge to whatever pixel-domain the encoder expects

Nine permutations covered by unit tests in `export.rs::tests`. Adding a new filter step means extending this one helper, not threading branches through both `run_simple` and `run_multitrack`.

## Build from source

```bash
cd ~/Projects/Brett
cargo build --release          # ~9 sec recompile, ~1 min cold
cargo test --release            # all 46 unit tests
cargo clippy --release --all-targets
```

Profile is already tuned for shipping:

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true
```

Result: 11 MB stripped binary. See "Where the size goes" in the binary-size discussion (or `size target/release/brett-clipper`).

## Test coverage

46 unit tests as of v0.4 across three areas:

- **`track::tests`** (12 tests) — clip splitting, link-group propagation, delete cascades, detach edge cases. Guards the data model against regressions.
- **`encode::tests`** (20 tests) — codec args (`-c:v`/`-crf`/`-b:v`/`-maxrate`/`-bufsize`), CRF-safety coercion, resolution math, Discord Nitro budget math at multiple durations, slider-scales-bitrate proportionality, GPU vendor recommendation logic.
- **`export::tests`** (14 tests) — exhaustive permutations of the filter-chain state machine (`HwDecode × scale × drawtext × encoder placement`), codec gating (`is_vaapi_decodable` accepts h264/hevc/vp9/mpeg2/vc1, rejects av1/vp8/etc.).

Run via `cargo test --release`.

## Settings & cache

User config lives at `~/.config/brett-clipper/config.json` (atomic write). Open via **File → Settings**.

Today's settings:

- **Auto-clear cache on close** — wipes `~/.cache/brett-clipper/proxies/` on app exit. Off by default (caching is the point of the proxy feature). On = free the disk on quit.
- **Clear cache now** — manual button. Shows live cache size, reports MB freed.

Proxy cache location: `~/.cache/brett-clipper/proxies/<16-hex>.mp4`. Each file is keyed by a hash of `(canonical_path, size, mtime_ns)`. Clearing it just regenerates proxies on next open of the relevant sources.

## Design principles

- **Native, not Electron.** No 200 MB framework, no JS runtime. ~11 MB binary, sub-second cold start.
- **Subprocess ffmpeg, dynamic libmpv.** Updating system ffmpeg never requires recompile. mpv is the only direct link (via libmpv2 + libloading), and the linker handles version compatibility through the standard SONAME.
- **Export quality is sacred.** The proxy never reaches the export pipeline. Whatever lands in your output file is encoded straight from the original source.
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
- ~~Right-click → Edit side panel~~
- ~~Auto / Discord Nitro / Custom export modes~~
- ~~Discord Nitro target-size slider~~
- ~~Hardware encoder detection + GPU vendor probe~~
- ~~VAAPI hardware decode for editing-rate scrubbing~~
- ~~Scrub-proxy generation with disk cache~~
- ~~Persistent settings + cache management~~
- ~~Linux desktop integration (icon, .desktop, install.sh)~~
- ~~Coherent custom button system + polished modals~~

Possible next:

- NVENC hardware decode path (currently NVENC users stay on SW decode)
- Proxy-quality preset toggle (current GOP=12 is the default; "all-I" mode for pixel-perfect scrubs at the cost of disk)
- Project file format (save/restore a session)
- Keyboard shortcut editor in Settings
- More text-overlay typography (drop shadow, stroke, opacity)

## License

MIT.
