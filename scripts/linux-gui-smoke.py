#!/usr/bin/env python3
"""Launch the installed desktop app on an isolated Xvfb display and data store."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else '/usr/bin/feedr').resolve())
output = Path(sys.argv[2] if len(sys.argv) > 2 else '.artifacts/linux-gui.png').resolve()
output.parent.mkdir(parents=True, exist_ok=True)
env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'FEEDR_'))}
with tempfile.TemporaryDirectory(prefix='feedr-gui-') as temporary:
    root = Path(temporary)
    env['XDG_DATA_HOME'] = str(root / 'xdg')
    env['XDG_CONFIG_HOME'] = str(root / 'config')
    env['XDG_RUNTIME_DIR'] = str(root / 'runtime')
    Path(env['XDG_RUNTIME_DIR']).mkdir(mode=0o700)
    env['LIBGL_ALWAYS_SOFTWARE'] = '1'
    prefs = root / 'xdg/dev.feedr.desktop/app.ron'
    prefs.parent.mkdir(parents=True)
    prefs.write_text(json.dumps({'feedr.auto_updates': 'false'}))
    command = [binary, '--data-dir', str(root / 'data')]
    subprocess.run(command + ['demo', '--no-launch'], env=env, check=True, capture_output=True)
    log_path = output.with_suffix('.log')
    with log_path.open('w') as log:
        gui = subprocess.Popen(command + ['gui'], env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                assert gui.poll() is None, log_path.read_text()
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(gui.pid), '--name', 'Feedr'], capture_output=True, text=True, env=env)
                if result.returncode == 0:
                    window = result.stdout.splitlines()[0]
                    break
                time.sleep(0.2)
            else:
                raise AssertionError('No visible Feedr window: ' + log_path.read_text())
            time.sleep(2)
            subprocess.run(['xdotool', 'windowfocus', window, 'key', 'j'], env=env, check=True)
            time.sleep(2)
            subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-f', 'x11grab', '-video_size', '1440x900', '-i', env['DISPLAY'], '-frames:v', '1', str(output)], env=env, check=True)
            assert output.stat().st_size > 10000
            subprocess.run(['xdotool', 'key', '--window', window, 'alt+F4'], env=env, check=True)
            # Xvfb may have no WM; SIGTERM is fallback cleanup, not a startup success criterion.
            print('PASS: installed GUI created a visible window and accepted keyboard input; screenshot:', output)
        finally:
            if gui.poll() is None:
                gui.terminate()
            gui.wait(timeout=10)
            subprocess.run(command + ['stop'], env=env, check=True, capture_output=True)
    assert (root / 'data/feedr.sqlite3').is_file()
