#!/usr/bin/env python3
"""Archive exactly the tagged source tree; local artifacts are never included."""
import argparse
import gzip
import hashlib
import io
from pathlib import Path
import subprocess
import tarfile
import tomllib
import zipfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--ref', default='HEAD')
parser.add_argument('--output-dir', type=Path, default=root / 'dist')
args = parser.parse_args()
commit = subprocess.check_output(['git', 'rev-parse', '--verify', args.ref + '^{commit}'], cwd=root, text=True).strip()
manifest = subprocess.check_output(['git', 'show', commit + ':Cargo.toml'], cwd=root, text=True)
version = tomllib.loads(manifest)['package']['version']
prefix = f'feedr-{version}/'
source = subprocess.check_output(['git', 'archive', '--format=tar', '--prefix=' + prefix, commit], cwd=root)
with tarfile.open(fileobj=io.BytesIO(source)) as archive:
    names = archive.getnames()
    assert prefix + 'Cargo.lock' in names
    assert all(not set(Path(name).parts[1:]).intersection({'.git', 'target', 'dist', 'ref', '.artifacts', '.feedr'}) for name in names)
args.output_dir.mkdir(parents=True, exist_ok=True)
tar = args.output_dir / f'feedr-v{version}-src.tar.gz'
tar.write_bytes(gzip.compress(source, mtime=0))
zip_path = args.output_dir / f'feedr-v{version}-src.zip'
with zipfile.ZipFile(zip_path, 'w') as output:
    zipped = subprocess.check_output(['git', 'archive', '--format=zip', '--prefix=' + prefix, commit], cwd=root)
    with zipfile.ZipFile(io.BytesIO(zipped)) as input_archive:
        for info in input_archive.infolist():
            output.writestr(info, input_archive.read(info.filename))
for path in [tar, zip_path]:
    path.with_name(path.name + '.sha256').write_text(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n')
    print(path)
