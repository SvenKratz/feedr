"""Collect notices from the resolved native dependency graph, never local artifacts."""
import json
from pathlib import Path
import shutil
import subprocess


def collect(root: Path, destination: Path):
    destination.mkdir(parents=True)
    host = next(line.split(': ', 1)[1] for line in subprocess.check_output(
        ['rustc', '-vV'], text=True).splitlines() if line.startswith('host: '))
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', host], cwd=root))
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    selected, pending = set(), [metadata['resolve']['root']]
    while pending:
        node = pending.pop()
        if node in selected:
            continue
        selected.add(node)
        pending.extend(nodes[node]['dependencies'])
    notices = ['Feedr third-party notices', '', 'Native media runtimes are separate system dependencies.', '']
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        if package['id'] not in selected:
            continue
        folder = Path(package['manifest_path']).parent
        target = destination / f"{package['name']}-{package['version']}"
        # Never walk target/, dist/, reference repositories or local user artifacts.
        candidates = folder.iterdir() if folder == root else folder.rglob('*')
        for source in candidates:
            name = source.name.lower()
            legal = (any(word in name for word in ('license', 'licence', 'copying', 'copyright', 'notice'))
                     or name.startswith(('ofl', 'ufl'))
                     or ('fonts' in source.parts and source.suffix == '.txt'))
            if source.is_file() and legal:
                output = target / source.relative_to(folder)
                output.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, output)
        explicit = package.get('license_file')
        if explicit:
            source = folder / explicit
            if source.is_file():
                target.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target / source.name)
        notices.extend([f"{package['name']} {package['version']}",
                        f"License: {package.get('license') or 'See included license file'}",
                        package.get('repository') or '', ''])
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    rust_licenses = sysroot / 'share/doc/rust/licenses'
    if rust_licenses.is_dir():
        shutil.copytree(rust_licenses, destination / 'rust-standard-library')
    for name in ['COPYRIGHT.html', 'COPYRIGHT-library.html']:
        source = sysroot / 'share/doc/rust' / name
        if source.is_file():
            output = destination / 'rust-standard-library' / name
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, output)
    (destination / 'THIRD-PARTY-NOTICES.txt').write_text('\n'.join(notices))
