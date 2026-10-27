#!/usr/bin/env python3
"""Exercise draft publication and fail-closed checks without contacting GitHub."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='feedr-publisher-') as temporary:
    tree = Path(temporary)
    (tree / 'scripts').mkdir()
    (tree / 'packaging').mkdir()
    (tree / 'Cargo.toml').write_text('[package]\nversion = "0.1.0"\n')
    (tree / 'packaging/release-notes.md').write_text('Test release\n')
    shutil.copy2(root / 'scripts/publish-release.py', tree / 'scripts/publish-release.py')
    subprocess.run(['git', 'init', '-q', str(tree)], check=True)
    subprocess.run(['git', 'add', '.'], cwd=tree, check=True)
    subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'Fixture'], cwd=tree, check=True)
    subprocess.run(['git', 'tag', 'v0.1.0'], cwd=tree, check=True)
    dist = tree / 'dist'
    dist.mkdir()
    names = [f'feedr-v0.1.0-macos-{arch}.{ext}' for arch in ['aarch64', 'x86_64'] for ext in ['dmg', 'zip']]
    names += [f'feedr-v0.1.0-linux-{arch}.tar.gz' for arch in ['aarch64', 'x86_64']]
    names += [f'feedr_0.1.0_{arch}.deb' for arch in ['amd64', 'arm64']]
    names += ['feedr-v0.1.0-src.tar.gz', 'feedr-v0.1.0-src.zip']
    for name in names:
        data = name.encode()
        (dist / name).write_bytes(data)
        (dist / (name + '.sha256')).write_text(f'{hashlib.sha256(data).hexdigest()}  {name}\n')
    state = tree / 'state.json'
    bin_dir = tree / 'bin'
    bin_dir.mkdir()
    fake = bin_dir / 'gh'
    fake.write_text('''#!/usr/bin/env python3
import hashlib, json, os, sys
from pathlib import Path
p=Path(os.environ['FEEDR_PUBLISH_TEST_STATE'])
s=json.loads(p.read_text())
args=sys.argv[1:]
s['calls'].append(args)
p.write_text(json.dumps(s))
assert args[0]=='release', 'Drafts must be read using gh release view, not the by-tag REST API'
if args[1]=='list':
    print(json.dumps([{'tagName':'v0.1.0','isDraft':s['draft']}]))
elif args[1]=='upload':
    assert s['draft']
elif args[1]=='view':
    assets=[]
    for path in Path('dist').iterdir():
        digest='sha256:'+hashlib.sha256(path.read_bytes()).hexdigest()
        if s.get('corrupt') and path.suffix=='.deb': digest='sha256:'+'0'*64
        assets.append({'name':path.name,'size':path.stat().st_size,'digest':digest})
    print(json.dumps({'assets':assets,'isDraft':s['draft'],'url':'https://example.invalid/draft'}))
elif args[1]=='edit':
    assert s['draft'] and '--draft=false' in args
    s['draft']=False
    p.write_text(json.dumps(s))
else:
    raise AssertionError(args)
''')
    fake.chmod(0o755)
    env = os.environ | {'PATH': str(bin_dir) + os.pathsep + os.environ['PATH'],
                        'FEEDR_PUBLISH_TEST_STATE': str(state)}
    command = [shutil.which('python3'), 'scripts/publish-release.py', '--tag', 'v0.1.0']
    for draft, corrupt, succeeds in [(True, False, True), (True, True, False), (False, False, False)]:
        state.write_text(json.dumps({'draft': draft, 'corrupt': corrupt, 'calls': []}))
        result = subprocess.run(command, cwd=tree, env=env, capture_output=True, text=True)
        assert (result.returncode == 0) == succeeds, result.stderr
        after = json.loads(state.read_text())
        edits = [call for call in after['calls'] if call[:2] == ['release', 'edit']]
        assert bool(edits) == succeeds
        assert after['draft'] == (draft and not succeeds)
        if not draft:
            assert not any(call[1] == 'upload' for call in after['calls'])
    state.write_text(json.dumps({'draft': True, 'calls': []}))
    (dist / names[0]).write_bytes(b'corrupt download')
    result = subprocess.run(command, cwd=tree, env=env, capture_output=True)
    assert result.returncode != 0 and not json.loads(state.read_text())['calls']
print('PASS: draft lookup and finalization; local/remote corruption and published-release mutation are rejected')
