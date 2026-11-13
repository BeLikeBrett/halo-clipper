# Halo Clipper

A native Rust video and audio editor for Windows and Linux. Grab a quick highlight
in **Simple Clip**, or build a layered edit in **Advanced**.

[Download the latest release](https://github.com/BeLikeBrett/halo-clipper/releases/latest)
· [Report an issue](https://github.com/BeLikeBrett/halo-clipper/issues/new?template=bug_report.yml)

## Install

**Windows 10 / 11, 64-bit:** Download `Halo-Clipper-Setup.exe`. It installs for your
user account without administrator access and includes FFmpeg, FFprobe, libmpv,
and caption fonts. The portable ZIP contains `windows/halo-clipper.exe` and its
runtime files; keep those files together.

**HaloOS / Arch Linux:** Use the HaloOS package, or build and run `./install.sh`.
The Linux release binary needs `ffmpeg` and `mpv` installed through your package
manager. Ubuntu / Debian: `sudo apt install ffmpeg libmpv2 mpv` (older releases
may name the library `libmpv1`).

## Edit

**Simple Clip:** Open a video, mark In and Out, and export. Auto mode copies
keyframe-aligned clips when possible and re-encodes when needed.

**Advanced:**

- **Import** adds video and audio to the media bin. Nothing is placed on the
  timeline automatically. Drag a media item to the time and lane you want; a
  full clip preview shows where it will land. Imported clips snap into a new
  layer above or below the lane under the pointer; hold Shift to bypass time snapping.
- Drag clips horizontally to change timing or vertically to another lane.
  Drag a track name up or down to change its layer order. Video at the top
  appears in front; audio lanes mix together.
- Click a video on the preview to select it. Drag the video to move it, and
  drag its corner handles to resize it while keeping its aspect ratio. Videos
  can extend outside the canvas. Transform edits update immediately on the GPU.
- The canvas automatically follows the largest-resolution video **placed on
  the timeline**. Importing alone does not change it. Choose Original, 16:9,
  9:16 (Reels / TikTok), 1:1, or 4:5 above the preview. These formats retain
  native source detail without a resolution picker. Fit shows the whole video;
  Fill crops to the canvas. Zoom, placement, and black / color / blurred video
  backgrounds carry through to export. Custom export can still override size.
- Hover a media item and click **×** to remove it and its timeline instances.
  Removing the last item leaves a blank editor. Import again whenever you want.
- Video and its audio start as one combined bar. **Detach Audio** (**D**) exposes
  the audio. **Attach Audio** (**Shift+D**) collapses it again, preserving source
  trims, cuts, gaps, offsets, speed, gain, and deleted pieces. Attachment changes
  grouping, never the rendered result. Detached splits remember their association.
  Ctrl+click multiple video/audio pieces to attach them as one compound bar.
- Split with **S**, delete with **Delete**, and undo / redo with **Ctrl+Z** /
  **Ctrl+Shift+Z**. Attached pieces edit together; detached pieces edit independently.
- Click a clip to show its controls for framing, audio, speed, opacity, and
  captions. Right-click a clip for edit actions, or a track name to mute / hide
  that track. Read-only clip details stay collapsed until needed.
- Each active video and audio lane uses an independent live decoder. Timeline
  playback, scrubbing and transform edits do not rebuild a rendered timeline.
  Exports always read the original media through the shared composition model.

The preview keeps most of the window. The compact timeline can be resized by
its upper divider and scrolls only when more lanes are present than fit.
Simple Clip retains its focused mark-and-export workflow. Sessions are currently
in memory: **export your edit before closing or updating**.

### Export

**Auto** preserves source quality. **Discord** targets an upload size and chooses
resolution / codec accordingly. **Custom** exposes resolution, frame rate, codec,
quality or bitrate, and audio settings. Available hardware encoders are detected
at startup. Preview layers composite on the GPU; export composition runs through
FFmpeg, with a GPU upload when the selected encoder requires it. Proxy media never replaces export originals.

### Shortcuts

| Key | Action |
|---|---|
| Space | Play / pause |
| I / O | Set In / Out in Simple Clip |
| S | Cycle Simple Clip marks; split selected / intersecting Advanced clips |
| Delete | Delete selected Advanced segments and linked partners |
| D / Shift+D | Detach / attach audio, preserving edits |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+click | Add / remove a segment from selection |

Command line: `halo-clipper video.mp4`, or
`halo-clipper video.mp4 --advanced --layer overlay.mp4 --layer music.wav`.
The Advanced command imports those files into the bin without placing them.

## Updates and support

Open **Help / Updates**. Startup checks query the project's public GitHub release
feed and can be disabled there. Updates require an explicit download and install
click. The app verifies an Ed25519-signed manifest, SHA-256 checksum, and file size
before installation. Windows installs into the current application directory;
user-local Linux binaries can replace themselves, retaining a `.previous` backup.
System-managed Linux installs should update through their package manager.

Logs stay on the user's device: two files, at most about 2 MiB each, under
`%LOCALAPPDATA%\Halo Clipper\logs` on Windows or
`${XDG_STATE_HOME:-~/.local/state}/halo-clipper/logs` on Linux. They include startup,
encoder checks, export results, preview / update errors, and Rust panics.
Known media paths and filenames are redacted. The app does not upload logs or
media automatically. **Export support report…** creates a text report for the
user to review and attach to a public GitHub issue. Review it before sharing;
third-party error messages can include user-provided text.

## Develop

Linux build dependencies include Rust, FFmpeg, libmpv development files, OpenGL,
Wayland, and xkbcommon. On Ubuntu:

```sh
sudo apt install build-essential pkg-config libmpv-dev libwayland-dev \
  libxkbcommon-dev libegl1-mesa-dev libgl-dev ffmpeg fonts-dejavu-core
cargo build --release --locked
cargo test --release --locked
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Tests exercise model editing, captions, encoders, signed metadata, redaction,
real exports, layer ordering / opacity, and independently placed audio. VAAPI
end-to-end coverage runs when compatible hardware is present.

See [release and Windows build instructions](docs/RELEASING.md) for publishing
updates, native Windows builds, signing, and dependency refreshes.

## License

Halo Clipper source: MIT. Bundled runtimes and fonts retain their own licenses;
see [third-party notices](THIRD-PARTY-NOTICES.md) and the package's `licenses/` folder.
