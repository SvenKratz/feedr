#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
profile="${1:-debug}"
case "$profile" in
  release) cargo build --locked --release ;;
  debug) cargo build --locked ;;
  *) echo 'Usage: scripts/package-macos.sh [debug|release]' >&2; exit 1 ;;
esac
bundle="dist/Feedr.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "target/$profile/feedr" "$bundle/Contents/MacOS/feedr"
cp packaging/Info.plist "$bundle/Contents/Info.plist"
if [ -f packaging/Feedr.icns ]; then cp packaging/Feedr.icns "$bundle/Contents/Resources/Feedr.icns"; fi
codesign --force --sign - "$bundle"
printf 'Built %s/dist/Feedr.app\n' "$PWD"
