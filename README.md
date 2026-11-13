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

- Add video or audio with **Add video / audio…** in the Layers panel. Imports
  begin at the playhead and keep their own original source files.
- Video lanes are ordered from front to back. **Up / Down** in Layers, or
  **Above / Below** in Edit, changes that order. Audio lanes mix together.
- Select a segment to set its start time and source In/Out. Drag horizontally
  to move it, with snapping. Linked video and audio move and trim together;
  **Detach Audio** makes them independent.
- Set video size, horizontal / vertical placement, and opacity for picture in
  picture, transparent overlays, and full-frame cuts. **Duplicate** copies the
  selected segment and its linked audio into new layers above.
- Toggle video visibility or audio mute by layer. Empty lanes can be removed.
- Split, delete, change speed, adjust gain, and add per-segment captions.
  Layer operations participate in undo / redo.
- Timeline gaps remain black or silent. Layered previews are prepared in the
  background at up to 640×360 / 30fps with a progress message. Changes are
  debounced; playback is available after the current preview is ready. Exports
  always use original media at the chosen output quality.

The timeline scrolls vertically as you add lanes. The Edit panel also scrolls
when its controls do not fit. Simple Clip stays focused on the quick-cut workflow.
Sessions are currently in memory: **export your edit before closing or updating**.

### Export

**Auto** preserves source quality. **Discord** targets an upload size and chooses
resolution / codec accordingly. **Custom** exposes resolution, frame rate, codec,
quality or bitrate, and audio settings. Available hardware encoders are detected
at startup; layered composition runs on the CPU, with an upload to the GPU when
the selected encoder requires it. Proxy media never replaces export originals.

### Shortcuts

| Key | Action |
|---|---|
| Space | Play / pause |
| I / O | Set In / Out in Simple Clip |
| S | Cycle Simple Clip marks; split selected / intersecting Advanced clips |
| Delete | Delete selected Advanced segments and linked partners |
| D | Detach linked audio |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+click | Add / remove a segment from selection |

Command line: `halo-clipper video.mp4`, or
`halo-clipper video.mp4 --advanced --layer overlay.mp4 --layer music.wav`.

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
