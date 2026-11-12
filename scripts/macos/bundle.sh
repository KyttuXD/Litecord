#!/usr/bin/env bash
# Wraps a built litecord binary into Litecord.app and zips it. Run on macOS.
#
#   scripts/macos/bundle.sh [path/to/litecord] [output dir]
#
# Defaults: target/x86_64-apple-darwin/release/litecord (or target/release/litecord)
# and ./dist. The app is signed ad hoc (no Apple developer account): on first launch
# use right click > Open, or `xattr -dr com.apple.quarantine Litecord.app`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

BIN="${1:-}"
if [ -z "$BIN" ]; then
  for candidate in target/x86_64-apple-darwin/release/litecord target/release/litecord; do
    if [ -f "$candidate" ]; then BIN="$candidate"; break; fi
  done
fi
[ -n "$BIN" ] && [ -f "$BIN" ] || { echo "litecord binary not found: build it first (cargo build --release --bin litecord)" >&2; exit 1; }

OUT="${2:-dist}"
VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')"
SHORT_VERSION="${VERSION%%-*}"
APP="$OUT/Litecord.app"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/litecord"
chmod 755 "$APP/Contents/MacOS/litecord"
sed -e "s/@SHORT_VERSION@/$SHORT_VERSION/" -e "s/@VERSION@/$VERSION/" \
  packaging/macos/Info.plist > "$APP/Contents/Info.plist"

# Icon: every size macOS asks for, from the one PNG of the project.
ICONSET="$(mktemp -d)/Litecord.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/app_icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" assets/app_icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Litecord.icns" \
  || echo "warning: could not build the icon, the app keeps the default one" >&2

# Ad hoc signature: lets macOS remember the microphone/camera/screen permissions.
codesign --force --deep --sign - "$APP"

ZIP="$OUT/Litecord-macos-x64.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
echo "Created $APP and $ZIP"
