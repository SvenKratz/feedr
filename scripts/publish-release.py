#!/usr/bin/env python3
"""Publish only a complete, checksum-verified release, initially uploaded as a draft."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tomllib

root = Path(__file__).resolve().parents[1]
version = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--tag', required=True)
parser.add_argument('--dist', type=Path, default=root / 'dist')
parser.add_argument('--repo', default='SvenKratz/feedr')
args = parser.parse_args()
assert args.tag == 'v' + version, 'Tag must match Cargo version'
assert subprocess.check_output(['git', 'rev-parse', args.tag + '^{commit}'], cwd=root) == subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root), 'Build checkout must match release tag'
names = [f'feedr-v{version}-macos-{arch}.{ext}' for arch in ['aarch64', 'x86_64'] for ext in ['dmg', 'zip']]
names += [f'feedr-v{version}-linux-{arch}.tar.gz' for arch in ['aarch64', 'x86_64']]
names += [f'feedr_{version}_{arch}.deb' for arch in ['amd64', 'arm64']]
names += [f'feedr-v{version}-src.{ext}' for ext in ['tar.gz', 'zip']]
digests = {}
for name in sorted(names):
    path = args.dist / name
    assert path.is_file() and path.stat().st_size > 0, f'Missing package: {name}'
    with path.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    expected = f'{digest}  {name}\n'
    assert path.with_name(name + '.sha256').read_text() == expected, f'Checksum mismatch: {name}'
    digests[name] = digest
(args.dist / 'SHA256SUMS').write_text(''.join(f'{digest}  {name}\n' for name, digest in digests.items()))
assets = names + [name + '.sha256' for name in names] + ['SHA256SUMS']
os.environ['GH_REPO'] = args.repo
gh = ['gh']
# Check existence via the release list rather than interpreting an auth/network error as 404.
releases = json.loads(subprocess.check_output(gh + ['release', 'list', '--limit', '100', '--json', 'tagName,isDraft']))
existing = next((release for release in releases if release['tagName'] == args.tag), None)
if existing:
    assert existing['isDraft'], 'Never mutate a published release; increment the version instead'
else:
    subprocess.run(gh + ['release', 'create', args.tag, '--verify-tag', '--draft', '--title', f'Feedr {args.tag}',
                         '--notes-file', str(root / 'packaging/release-notes.md')], check=True)
subprocess.run(gh + ['release', 'upload', args.tag, '--clobber'] + [str(args.dist / name) for name in assets], check=True)
release = json.loads(subprocess.check_output(gh + ['release', 'view', args.tag, '--json', 'assets,isDraft,url']))
assert release['isDraft'], 'Only a draft may be finalized'
remote = {asset['name']: asset for asset in release['assets']}
assert set(remote) == set(assets), 'Unexpected or missing remote assets'
for name in assets:
    path = args.dist / name
    with path.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    assert remote[name]['size'] == path.stat().st_size
    assert remote[name]['digest'] == 'sha256:' + digest, f'Remote digest mismatch: {name}'
subprocess.run(gh + ['release', 'edit', args.tag, '--draft=false', '--latest'], check=True)
print(f'https://github.com/{args.repo}/releases/tag/{args.tag}')
