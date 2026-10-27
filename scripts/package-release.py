#!/usr/bin/env python3
"""Build native installers and compatible, verified updater archives."""
import argparse
import hashlib
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile
from release_notices import collect

ROOT = Path(__file__).resolve().parents[1]
VERSION = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def checksum(path):
    with path.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    path.with_name(path.name + '.sha256').write_text(f'{digest}  {path.name}\n')


def sign(path, identity, entitlements=False):
    args = ['codesign', '--force', '--sign', identity]
    if identity != '-':
        args += ['--options', 'runtime', '--timestamp']
        if entitlements:
            args += ['--entitlements', ROOT / 'packaging/release.entitlements.plist']
    run(*args, path)
    run('codesign', '--verify', '--deep', '--strict', path)


def notarize(path, profile):
    run('xcrun', 'notarytool', 'submit', path, '--keychain-profile', profile, '--wait')


def background(path, arch):
    from PIL import Image, ImageDraw, ImageFont
    image = Image.new('RGB', (1280, 840), '#0d171e')
    draw = ImageDraw.Draw(image)
    font = '/System/Library/Fonts/Menlo.ttc'
    draw.text((64, 48), 'feedr', font=ImageFont.truetype(font, 76), fill='#83c6c4')
    draw.text((68, 151), 'Your agents’ resources, within reach.', font=ImageFont.truetype(font, 25), fill='#a6b7c2')
    draw.line((68, 210, 1212, 210), fill='#30444b', width=2)
    draw.line((560, 457, 720, 457), fill='#83c6c4', width=4)
    draw.line((695, 435, 720, 457, 695, 479), fill='#83c6c4', width=4)
    draw.text((320, 625), 'Drag Feedr to Applications', font=ImageFont.truetype(font, 29), fill='#d5e1e7')
    draw.text((68, 770), f'v{VERSION} · {arch}', font=ImageFont.truetype(font, 20), fill='#829ca7')
    image.save(path)


def macos(binary, dist, stage, arch, notices):
    import dmgbuild
    bundle = stage / 'Feedr.app'
    contents = bundle / 'Contents'
    (contents / 'MacOS').mkdir(parents=True)
    (contents / 'Resources').mkdir()
    shutil.copy2(binary, contents / 'MacOS/feedr')
    shutil.copy2(ROOT / 'packaging/Feedr.icns', contents / 'Resources/Feedr.icns')
    shutil.copy2(ROOT / 'LICENSE', contents / 'Resources/LICENSE')
    shutil.copytree(notices, contents / 'Resources/licenses')
    info = plistlib.loads((ROOT / 'packaging/Info.plist').read_bytes())
    info.update(CFBundleShortVersionString=VERSION, CFBundleVersion=VERSION, LSMinimumSystemVersion='14.0')
    (contents / 'Info.plist').write_bytes(plistlib.dumps(info))
    identity = os.environ.get('FEEDR_CODESIGN_IDENTITY', '-')
    profile = os.environ.get('FEEDR_NOTARY_PROFILE')
    if profile and identity == '-':
        raise RuntimeError('Notarization requires a Developer ID signing identity')
    sign(bundle, identity, entitlements=True)
    if profile:
        submission = stage / 'notarization.zip'
        run('ditto', '-c', '-k', '--keepParent', bundle, submission)
        notarize(submission, profile)
        run('xcrun', 'stapler', 'staple', bundle)
        run('xcrun', 'stapler', 'validate', bundle)
    archive = dist / f'feedr-v{VERSION}-macos-{arch}.zip'
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as output:
        for path in sorted(bundle.rglob('*')):
            if path.is_file():
                output.write(path, path.relative_to(stage))
    run(binary, 'update-verify', '--archive', archive)
    instructions = stage / 'Read before installing.txt'
    text = (ROOT / 'packaging/macos-install.txt').read_text()
    if profile:
        text = text.replace('This release is ad-hoc signed, without Apple Developer ID signing or notarization.\nGatekeeper may block it. Review the source before authorizing an unsigned download.\nIf you prefer, build from source using the instructions in README.md.',
                            'This release is Developer ID signed and notarized by Apple.')
    instructions.write_text(text)
    wallpaper = stage / 'background.png'
    background(wallpaper, arch)
    dmg = dist / f'feedr-v{VERSION}-macos-{arch}.dmg'
    if dmg.exists():
        dmg.unlink()
    dmgbuild.build_dmg(str(dmg), 'Feedr', settings={
        'files': [str(bundle), str(instructions)],
        'symlinks': {'Applications': '/Applications'},
        'icon': str(ROOT / 'packaging/Feedr.icns'),
        'background': str(wallpaper), 'format': 'UDZO',
        'window_rect': ((100, 100), (640, 420)),
        'icon_locations': {'Feedr.app': (170, 230), 'Applications': (470, 230),
                           'Read before installing.txt': (550, 360)},
        'default_view': 'icon-view', 'icon_size': 96, 'text_size': 13,
        'show_toolbar': False, 'show_status_bar': False, 'show_sidebar': False,
        'show_pathbar': False, 'show_tab_view': False,
    })
    sign(dmg, identity)
    if profile:
        notarize(dmg, profile)
        run('xcrun', 'stapler', 'staple', dmg)
        run('xcrun', 'stapler', 'validate', dmg)
    run('hdiutil', 'verify', dmg)
    return [archive, dmg]


def linux(binary, dist, stage, arch, notices):
    portable = stage / 'portable'
    portable.mkdir()
    for source, name in [(binary, 'feedr'), (ROOT / 'README.md', 'README.md'),
                         (ROOT / 'LICENSE', 'LICENSE'), (ROOT / 'packaging/feedr.desktop', 'feedr.desktop'),
                         (ROOT / 'packaging/feedr.svg', 'feedr.svg')]:
        shutil.copy2(source, portable / name)
    shutil.copytree(notices, portable / 'licenses')
    archive = dist / f'feedr-v{VERSION}-linux-{arch}.tar.gz'
    with tarfile.open(archive, 'w:gz') as output:
        for path in sorted(portable.iterdir()):
            output.add(path, arcname=path.name)
    run(binary, 'update-verify', '--archive', archive)
    package = stage / 'debian-package'
    for source, location in [(binary, 'usr/bin/feedr'),
                              (ROOT / 'packaging/feedr.desktop', 'usr/share/applications/feedr.desktop'),
                              (ROOT / 'packaging/feedr.svg', 'usr/share/icons/hicolor/scalable/apps/feedr.svg'),
                              (ROOT / 'LICENSE', 'usr/share/doc/feedr/copyright'),
                              (ROOT / 'README.md', 'usr/share/doc/feedr/README.md')]:
        destination = package / location
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    shutil.copytree(notices, package / 'usr/share/doc/feedr/licenses')
    work = stage / 'shlibdeps'
    (work / 'debian').mkdir(parents=True)
    (work / 'debian/control').write_text('Source: feedr\n\nPackage: feedr\nArchitecture: any\nDescription: Native agent resource feed\n')
    output = subprocess.check_output(['dpkg-shlibdeps', '-O', '-e' + str(binary)], cwd=work, text=True)
    native = next(line.removeprefix('shlibs:Depends=') for line in output.splitlines() if line.startswith('shlibs:Depends='))
    runtime = ('poppler-utils, ffmpeg, libmpv2 | libmpv1, xdg-utils, libxkbcommon0, libxkbcommon-x11-0, '
               'libegl1, libgl1, libwayland-client0, libwayland-egl1, libx11-6, libxcb1, '
               'libxcb-shape0, libxcb-xfixes0, libxcursor1, libxi6, libxrandr2, libxinerama1, libxrender1')
    deb_arch = {'x86_64': 'amd64', 'aarch64': 'arm64'}[arch]
    control = package / 'DEBIAN'
    control.mkdir()
    size = sum(p.stat().st_size for p in package.rglob('*') if p.is_file()) // 1024 + 1
    (control / 'control').write_text(
        f'Package: feedr\nVersion: {VERSION}\nSection: utils\nPriority: optional\nArchitecture: {deb_arch}\n'
        'Maintainer: Sven Kratz <sven@easelapps.ai>\nHomepage: https://github.com/SvenKratz/feedr\n'
        f'Installed-Size: {size}\nDepends: {native}, {runtime}\n'
        'Description: Native resource feed for herdr agent sessions\n'
        ' Preview and open agent artifacts from session feed columns, with MCP publishing,\n'
        ' local history, search and optional deterministic mention discovery.\n')
    payload = [p for p in sorted(package.rglob('*')) if p.is_file() and control not in p.parents]
    (control / 'md5sums').write_text(''.join(
        f"{hashlib.md5(p.read_bytes(), usedforsecurity=False).hexdigest()}  {p.relative_to(package)}\n" for p in payload))
    for path in package.rglob('*'):
        path.chmod(0o755 if path.is_dir() or path == package / 'usr/bin/feedr' else 0o644)
    deb = dist / f'feedr_{VERSION}_{deb_arch}.deb'
    run('dpkg-deb', '--build', '--root-owner-group', '-Zxz', package, deb)
    run('dpkg-deb', '--info', deb)
    return [archive, deb]


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/feedr')
    parser.add_argument('--output-dir', type=Path, default=ROOT / 'dist')
    args = parser.parse_args()
    binary, dist = args.binary.resolve(), args.output_dir.resolve()
    assert subprocess.check_output([str(binary), '--version'], text=True).strip() == f'feedr {VERSION}'
    system = {'Darwin': macos, 'Linux': linux}[platform.system()]
    arch = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'x86_64': 'x86_64'}[platform.machine()]
    dist.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='feedr-release-') as temporary:
        stage = Path(temporary)
        notices = stage / 'notices'
        collect(ROOT, notices)
        artifacts = system(binary, dist, stage, arch, notices)
    for path in artifacts:
        checksum(path)
        print(path)
