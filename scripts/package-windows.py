#!/usr/bin/env python3
"""Assemble the Windows payload (dist/windows) and the portable zip.

Usage: package-windows.py <runtime-dir> <halo-clipper.exe>

<runtime-dir> is the output of fetch-windows-runtime.py (mpv-dev/, mpv/,
ffmpeg/ subfolders). The payload folder is rebuilt from scratch so files
from an older runtime never ride along into the installer.
"""

import pathlib
import shutil
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
PAYLOAD = ROOT / "dist" / "windows"

# (file name, runtime subfolder it is found in)
RUNTIME_FILES = [
    ("libmpv-2.dll", "mpv-dev"),
    ("vulkan-1.dll", "vulkan"),
    ("ffmpeg.exe", "ffmpeg"),
    ("ffprobe.exe", "ffmpeg"),
    ("mpv.exe", "mpv"),
    ("d3dcompiler_43.dll", "mpv"),
]


def find_runtime_file(runtime: pathlib.Path, name: str, folder: str) -> pathlib.Path:
    matches = sorted((runtime / folder).rglob(name))
    if folder == "vulkan":
        matches = [p for p in matches if "x64" in p.parts]
    if not matches:
        raise SystemExit(f"Missing runtime file: {folder}/**/{name}")
    # The ffmpeg folder can hold both the pinned n8 build and an older
    # single-tool archive; prefer the pinned one.
    pinned = [p for p in matches if "ffmpeg-n8." in str(p)]
    return (pinned or matches)[0]


def is_license(path: pathlib.Path) -> bool:
    upper = path.name.upper()
    return path.is_file() and ("LICENSE" in upper or "COPYING" in upper) and path.suffix not in (".7z", ".zip")


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    runtime = pathlib.Path(sys.argv[1])
    binary = pathlib.Path(sys.argv[2])

    shutil.rmtree(PAYLOAD, ignore_errors=True)
    PAYLOAD.mkdir(parents=True)

    shutil.copy2(binary, PAYLOAD / "halo-clipper.exe")
    for name, folder in RUNTIME_FILES:
        shutil.copy2(find_runtime_file(runtime, name, folder), PAYLOAD / name)
    for name in ("LICENSE", "THIRD-PARTY-NOTICES.md"):
        shutil.copy2(ROOT / name, PAYLOAD / name)
    shutil.copy2(ROOT / "packaging" / "runtime-lock.json", PAYLOAD / "runtime-lock.json")

    licenses = PAYLOAD / "licenses"
    licenses.mkdir()
    fonts = ROOT / "assets" / "fonts"
    shutil.copy2(fonts / "LICENSE", licenses / "DejaVu-LICENSE")
    for font in ("Inter", "JetBrainsMono"):
        shutil.copy2(fonts / f"{font}-LICENSE.txt", licenses / f"{font}-LICENSE")
    for license_file in sorted((fonts / "caption").glob("*.txt")):
        shutil.copy2(license_file, licenses / f"caption-{license_file.name}")
    for source in runtime.rglob("*"):
        if is_license(source):
            shutil.copy2(source, licenses / "-".join(source.relative_to(runtime).parts))

    shutil.make_archive(str(ROOT / "dist" / "Halo-Clipper-windows-x86_64"), "zip", ROOT / "dist", "windows")
    print(PAYLOAD)


if __name__ == "__main__":
    main()
