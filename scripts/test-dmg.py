#!/usr/bin/env python3
"""Verify a read-only mounted DMG, without opening the user's GUI or data."""
import os
from pathlib import Path
import plistlib
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
        print('PASS: read-only DMG contains a verified, executable app, Applications link, installation guide and notices')
    finally:
        subprocess.run(['hdiutil', 'detach', str(mount)], check=True)
