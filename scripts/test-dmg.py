#!/usr/bin/env python3
"""Verify a read-only mounted DMG, without opening the user's GUI or data."""
import os
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import tomllib

root = Path(__file__).resolve().parents[1]
version = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
dmg = Path(sys.argv[1]).resolve()
subprocess.run(['hdiutil', 'verify', str(dmg)], check=True)
with tempfile.TemporaryDirectory(prefix='feedr-dmg-test-') as directory:
    mount = Path(directory) / 'volume'
    subprocess.run(['hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(dmg)], check=True)
    try:
        app = mount / 'Feedr.app'
        assert (mount / 'Applications').is_symlink()
        assert os.readlink(mount / 'Applications') == '/Applications'
        assert (mount / 'Read before installing.txt').is_file()
        info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
        assert info['CFBundleIdentifier'] == 'dev.feedr.desktop'
        assert info['CFBundleShortVersionString'] == version
        assert (app / 'Contents/Resources/licenses/THIRD-PARTY-NOTICES.txt').stat().st_size > 1000
        subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
        assert subprocess.check_output([str(app / 'Contents/MacOS/feedr'), '--version'], text=True).strip() == 'feedr ' + version
        # A CLI link to the mounted installer would break when the image is ejected.
        cli = Path(directory) / 'bin'
        install = subprocess.run([str(app / 'Contents/MacOS/feedr'), 'install-cli', '--no-modify-path', '--bin-dir', str(cli)], capture_output=True, text=True)
        assert install.returncode != 0 and 'Applications' in install.stderr, install.stderr
        assert not cli.exists()
        installed = Path(directory) / 'Applications/Feedr.app'
        installed.parent.mkdir()
        shutil.copytree(app, installed)
        result = json.loads(subprocess.check_output([str(installed / 'Contents/MacOS/feedr'), 'install-cli', '--no-modify-path', '--bin-dir', str(cli)], text=True))
        assert result['profiles'] == []
        assert (cli / 'feedr').is_symlink()
        assert subprocess.check_output([str(cli / 'feedr'), '--version'], text=True).strip() == 'feedr ' + version
        print('PASS: mounted DMG/app verified; CLI refuses the disk image and installs from a writable app copy without touching shell profiles')
    finally:
        subprocess.run(['hdiutil', 'detach', str(mount)], check=True)
