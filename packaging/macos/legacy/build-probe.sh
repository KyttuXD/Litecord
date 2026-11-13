#!/bin/bash
# build-probe.sh BOOTSTRAP_DIRECTORY [CRATE]: builds legacy/CRATE (default: probe) for OS X 10.5 and later (Intel 64-bit)
# with the MacPorts Rust 1.77 compiler fetched by bootstrap-runtime.sh + bootstrap-leopard.sh.
# The recipe (compiler, compatibility library, flat namespace, old program format) comes from
# KyttuXD/TheLastRoute, packaging/macos/legacy. Run on an Intel macOS build host.
set -euo pipefail
ROOT=$(git rev-parse --show-toplevel)
TOOLCHAIN=${1:?usage: build-probe.sh BOOTSTRAP_DIRECTORY [CRATE]}
CRATE=${2:-probe}
NAME=$(sed -n 's/^name = "\(.*\)"$/\1/p' "$ROOT/legacy/$CRATE/Cargo.toml" | head -n 1)
test -n "$NAME"
COMPILER="$TOOLCHAIN/compiler-leopard"
SUPPORT="$TOOLCHAIN/support-leopard/opt/local/lib"
test -x "$COMPILER/bin/rustc"
test -f "$SUPPORT/libMacportsLegacySystem.B.dylib"
# The compiler program itself is a Darwin 10 binary and needs its own compatibility library.
export DYLD_LIBRARY_PATH="$TOOLCHAIN/support/opt/local/lib:$COMPILER/lib:/usr/lib"
export RUSTC="$COMPILER/bin/rustc"
export MACOSX_DEPLOYMENT_TARGET=10.5
# Switches the miniquad of TheLastRoute to its old-Mac window and OpenGL 2.1 code.
export LR_MACOS_LEGACY=1
# Flat namespace: the old system then finds clock_gettime, getentropy and the like in
# libMacportsLegacySystem instead of looking for them (in vain) in its own libSystem.
# mio_unsupported_force_waker_pipe: mio wakes its event loop through kqueue's EVFILT_USER on macOS,
# and on OS X 10.6 that wake-up never arrives (a tokio program hung in kevent on a Mac Pro 3,1).
# With this switch mio uses a pipe instead, which every system has.
export RUSTFLAGS="-L native=$SUPPORT -C link-arg=-lMacportsLegacySystem.B -C link-arg=-Wl,-flat_namespace --cfg mio_unsupported_force_waker_pipe"
# The voice crates in the form Rust 1.77 can build (only the legacy edition uses them).
if [ "$CRATE" = app ]; then python3 "$ROOT/legacy/vendor/prepare.py"; fi
cd "$ROOT/legacy/$CRATE"
# Cargo 1.77 drives the build: a newer Cargo passes switches this compiler does not know.
rustup run 1.77.0 cargo build --locked --release --target x86_64-apple-darwin
BIN="$ROOT/legacy/$CRATE/target/x86_64-apple-darwin/release/$NAME"
OUT="$ROOT/dist/$NAME"
rm -rf "$OUT"
mkdir -p "$OUT"
cp "$BIN" "$OUT/"
cp "$SUPPORT/libMacportsLegacySystem.B.dylib" "$OUT/"
install_name_tool -change /opt/local/lib/libMacportsLegacySystem.B.dylib \
    @executable_path/libMacportsLegacySystem.B.dylib "$OUT/$NAME"
otool -L "$OUT/$NAME"
otool -l "$OUT/$NAME" | grep -A4 -E 'LC_VERSION_MIN_MACOSX|LC_BUILD_VERSION' || true
python3 "$ROOT/packaging/macos/legacy/machobind.py" "$OUT/$NAME" | head -n 2
python3 "$ROOT/packaging/macos/legacy/machobind.py" "$OUT/$NAME" | head -n 1 | grep -q FLAT \
    || { echo "not linked with a flat namespace" >&2; exit 1; }
# OS X before 10.8 can only start the old program format (LC_UNIXTHREAD, load command 5).
python3 -c 'import struct,sys;d=open(sys.argv[1],"rb").read();n=struct.unpack_from("<I",d,16)[0];o=32;c=[]
for _ in range(n):
    k,s=struct.unpack_from("<II",d,o);c.append(k);o+=s
print("program format:", "LC_UNIXTHREAD (runs on 10.5+)" if 5 in c else "LC_MAIN (needs 10.8+)")
sys.exit(0 if 5 in c else 1)' "$OUT/$NAME"
(cd "$ROOT/dist" && rm -f "$NAME.zip" && zip -qry "$NAME.zip" "$NAME")
echo "Built dist/$NAME.zip"
# The legacy edition also as a double-clickable app, with the compatibility library inside.
if [ "$CRATE" = app ]; then
    VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/legacy/app/Cargo.toml" | head -n 1)
    APP="$ROOT/dist/Litecord Legacy.app"
    rm -rf "$APP"
    mkdir -p "$APP/Contents/MacOS"
    cp "$OUT/$NAME" "$OUT/libMacportsLegacySystem.B.dylib" "$APP/Contents/MacOS/"
    sed "s/@VERSION@/$VERSION/" "$ROOT/packaging/macos/legacy/Info.plist" > "$APP/Contents/Info.plist"
    (cd "$ROOT/dist" && rm -f Litecord-Legacy-macos-10.5.zip && zip -qry Litecord-Legacy-macos-10.5.zip "Litecord Legacy.app")
    echo "Built dist/Litecord-Legacy-macos-10.5.zip"
fi
