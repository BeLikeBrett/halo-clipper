#!/usr/bin/env python3
"""Download pinned, checksum-verified x86_64 media runtimes; extract with 7z."""
import hashlib,json,pathlib,subprocess,sys,urllib.request,shutil
root=pathlib.Path(__file__).resolve().parents[1]
out=pathlib.Path(sys.argv[1] if len(sys.argv)>1 else root/'dist/runtime')
out.mkdir(parents=True,exist_ok=True)
lock=json.loads((root/'packaging/runtime-lock.json').read_text())
for asset in lock['assets']:
    archive=out/asset['name']
    if not archive.exists():
        with urllib.request.urlopen(urllib.request.Request(asset['url'],headers={'User-Agent':'Mozilla/5.0 Halo-Clipper-Release'})) as response, archive.open('wb') as target:
            while chunk:=response.read(1024*1024): target.write(chunk)
    if hashlib.file_digest(archive.open('rb'),'sha256').hexdigest()!=asset['sha256']:
        raise SystemExit(f"Checksum mismatch for {archive.name}")
    folder=out/asset.get('folder',('mpv-dev' if asset['name'].startswith('mpv-dev') else 'ffmpeg' if asset['name'].startswith('ffmpeg') else 'mpv'))
    if folder.exists(): shutil.rmtree(folder)
    folder.mkdir(exist_ok=True)
    subprocess.run(['7z','x','-y',f'-o{folder}',str(archive)],check=True,stdout=subprocess.DEVNULL)
print(out)
