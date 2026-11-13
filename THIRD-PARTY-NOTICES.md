# Third-party components

Halo Clipper source is available under the MIT license in LICENSE. Binary
packages also contain third-party software under its respective licenses.

- Rust dependencies: exact versions are recorded in Cargo.lock; their package
  metadata links to source and license files at https://crates.io.
- mpv / libmpv: https://github.com/mpv-player/mpv, including FFmpeg and the
  dependencies selected by https://github.com/shinchiro/mpv-winbuild-cmake.
  Pinned binary provenance and SHA-256 hashes are in packaging/runtime-lock.json.
  The pinned upstream release links the build revisions; corresponding source
  recipes and dependency revision locks are in that upstream repository.
  mpv licensing: https://github.com/mpv-player/mpv/blob/master/Copyright.
- FFmpeg: https://ffmpeg.org and https://github.com/FFmpeg/FFmpeg. Windows binaries
  are GPL builds from https://github.com/BtbN/FFmpeg-Builds; build recipes,
  dependency sources and license terms are provided by that project. The
  downloaded package's license files are included under licenses/.
- DejaVu fonts: https://dejavu-fonts.github.io, license in assets/fonts/LICENSE
  and licenses/DejaVu-LICENSE in Windows packages.
- Inter (https://github.com/rsms/inter) and JetBrains Mono
  (https://github.com/JetBrains/JetBrainsMono) interface fonts, SIL Open Font
  License 1.1. Embedded as Latin subsets; licenses in
  assets/fonts/Inter-LICENSE.txt and assets/fonts/JetBrainsMono-LICENSE.txt
  (licenses/ in Windows packages).

Runtime binaries are redistributed without modifying their contents. They are
not covered by the Halo Clipper source license. Retain runtime notices when
redistributing a package. Linux uses system media libraries and programs.
