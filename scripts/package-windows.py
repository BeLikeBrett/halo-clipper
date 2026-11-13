#!/usr/bin/env python3
import json,pathlib,shutil,sys
root=pathlib.Path(__file__).resolve().parents[1]
runtime=pathlib.Path(sys.argv[1]);binary=pathlib.Path(sys.argv[2]);payload=root/'dist/windows'
payload.mkdir(parents=True,exist_ok=True)
shutil.copy2(binary,payload/'halo-clipper.exe')
for name,folder in [('libmpv-2.dll','mpv-dev'),('ffmpeg.exe','ffmpeg'),('ffprobe.exe','ffmpeg'),('mpv.exe','mpv'),('d3dcompiler_43.dll','mpv')]:
    matches=list((runtime/folder).rglob(name))
    # ffmpeg folder may contain the old shinchiro single-tool archive as well.
    source=next((p for p in matches if 'ffmpeg-n8.' in str(p)),matches[0] if matches else None)
    if not source: raise SystemExit(f'Missing runtime: {name}')
    shutil.copy2(source,payload/name)
for name in ['LICENSE','THIRD-PARTY-NOTICES.md']:shutil.copy2(root/name,payload/name)
shutil.copy2(root/'packaging/runtime-lock.json',payload/'runtime-lock.json')
licenses=payload/'licenses';licenses.mkdir(exist_ok=True)
shutil.copy2(root/'assets/fonts/LICENSE',licenses/'DejaVu-LICENSE')
for source in runtime.rglob('*'):
    if source.is_file() and ('LICENSE' in source.name.upper() or 'COPYING' in source.name.upper()) and source.suffix not in ['.7z','.zip']:
        name='-'.join(source.relative_to(runtime).parts)
        shutil.copy2(source,licenses/name)
shutil.make_archive(str(root/'dist/Halo-Clipper-windows-x86_64'),'zip',root/'dist','windows')
print(payload)
