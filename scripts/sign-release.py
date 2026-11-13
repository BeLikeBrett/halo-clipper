#!/usr/bin/env python3
"""Sign exact asset sizes/hashes. Run only from a protected release workflow."""
import base64,hashlib,json,os,pathlib,sys
from cryptography.hazmat.primitives import serialization
root=pathlib.Path(__file__).resolve().parents[1]
version=sys.argv[1].removeprefix('v');assets=pathlib.Path(sys.argv[2])
key=serialization.load_pem_private_key(os.environ['HALO_UPDATE_SIGNING_KEY'].encode(),None)
public=key.public_key().public_bytes(serialization.Encoding.Raw,serialization.PublicFormat.Raw)
if public!=(root/'assets/update-public-key.bin').read_bytes():raise SystemExit('Signing key does not match the application trust key')
entries=[]
for target,name in [('windows-x86_64','Halo-Clipper-Setup.exe'),('linux-x86_64','halo-clipper-linux-x86_64')]:
    file=assets/name
    entries.append(dict(target=target,url=f'https://github.com/BeLikeBrett/halo-clipper/releases/download/v{version}/{name}',sha256=hashlib.file_digest(file.open('rb'),'sha256').hexdigest(),size=file.stat().st_size))
manifest=json.dumps(dict(version=version,assets=entries),indent=2).encode()+b'\n'
(assets/'update.json').write_bytes(manifest)
(assets/'update.json.sig').write_bytes(base64.b64encode(key.sign(manifest))+b'\n')
with (assets/'SHA256SUMS').open('w') as out:
    for file in sorted(assets.iterdir()):
        if file.is_file() and file.name!='SHA256SUMS':out.write(f'{hashlib.file_digest(file.open("rb"),"sha256").hexdigest()}  {file.name}\n')
