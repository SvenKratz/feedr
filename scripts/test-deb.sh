#!/bin/sh
# Run only on an isolated Linux release builder, as root.
set -eu
package=$(realpath "$1")
test "$(id -u)" = 0
test "$(dpkg-deb -f "$package" Architecture)" = "$(dpkg --print-architecture)"
apt-get install -y "$package"
desktop-file-validate /usr/share/applications/feedr.desktop
dpkg --verify feedr
feedr --version
python3 scripts/smoke.py /usr/bin/feedr
FEEDR_TEST_NULL_AUDIO=1 python3 scripts/media-smoke.py /usr/bin/feedr
xvfb-run -a -s '-screen 0 1440x900x24' python3 scripts/linux-gui-smoke.py /usr/bin/feedr
# Removal must not touch the database in the user's separate data directory.
data_dir=$(mktemp -d /tmp/feedr-deb.XXXXXX)
trap 'rm -rf "$data_dir"' EXIT
feedr --data-dir "$data_dir" demo --no-launch > /dev/null
feedr --data-dir "$data_dir" stop > /dev/null
test -s "$data_dir/feedr.sqlite3"
apt-get remove -y feedr
test -s "$data_dir/feedr.sqlite3"
test ! -e /usr/bin/feedr
echo 'PASS: Debian installation, desktop entry, installed CLI/MCP/media/GUI and removal preserving data'
