#!/bin/sh
# Adds what a Leopard (OS X 10.5, Intel 64-bit) build needs to a folder bootstrap-runtime.sh made:
#   compiler-leopard/   the Darwin 10 compiler program with the standard library MacPorts built for Darwin 9
#   support-leopard/    MacPorts legacy-support built from source for 10.5 (MacPorts publishes no 10.5 binary)
# Build-time only; nothing is installed on the system. Then: build-experiment.sh FOLDER 10.5
set -eu
OUT=${1:?usage: bootstrap-leopard.sh BOOTSTRAP_DIRECTORY}
test -x "$OUT/compiler/bin/rustc" || { echo "run bootstrap-runtime.sh first" >&2; exit 1; }
if [ -e "$OUT/compiler-leopard" ] || [ -e "$OUT/support-leopard" ]; then
    echo "Refusing to overwrite an existing Leopard bootstrap in $OUT" >&2
    exit 1
fi
fetch() {
    curl --fail --location --retry 3 "$1" --output "$OUT/downloads/$2"
    echo "$3  $OUT/downloads/$2" | shasum -a 256 --check
}
# Checksum from MacPorts' rust_build-1.0.tcl for the Darwin 9 bootstrap.
fetch "https://github.com/MarcusCalhoun-Lopez/rust/releases/download/1.78.0/rust-std-1.77.0%2B0-x86_64-macports-darwin9.tar.gz" rust-std-darwin9.tar.gz \
    03dba50acbc3ed3ac5c72ef710cb6fbe659975f4658b5af12ae36b269f1f68fa
# Checksum from MacPorts' devel/legacy-support Portfile (1.5.2).
fetch https://github.com/macports/macports-legacy-support/archive/v1.5.2/macports-legacy-support-1.5.2.tar.gz legacy-support-src.tar.gz \
    16da882281d26d8ddfb434acfde0828fc3e4119aab332445f65688ae879e9962
mkdir -p "$OUT/unpack-leopard" "$OUT/compiler-leopard" "$OUT/support-leopard"
tar -xzf "$OUT/downloads/rust-std-darwin9.tar.gz" -C "$OUT/unpack-leopard"
tar -xzf "$OUT/downloads/legacy-support-src.tar.gz" -C "$OUT/unpack-leopard"
# the compiler program as it is, the target libraries from the Darwin 9 build
cp -R "$OUT/unpack/rustc-1.77.0-x86_64-apple-darwin/rustc/." "$OUT/compiler-leopard/"
cp -R "$OUT/unpack-leopard/rust-std-1.77.0-x86_64-apple-darwin/rust-std-x86_64-apple-darwin/." "$OUT/compiler-leopard/"
# legacy-support chooses its wrappers from the deployment target, so a 10.5 target gives the 10.5 set
SRC=$(ls -d "$OUT"/unpack-leopard/macports-legacy-support-*)
make -C "$SRC" ARCHS=x86_64 PREFIX=/opt/local \
    CFLAGS=-mmacosx-version-min=10.5 LDFLAGS=-mmacosx-version-min=10.5 MACOSX_DEPLOYMENT_TARGET=10.5 syslib
make -C "$SRC" ARCHS=x86_64 PREFIX=/opt/local DESTDIR="$OUT/support-leopard" \
    CFLAGS=-mmacosx-version-min=10.5 LDFLAGS=-mmacosx-version-min=10.5 MACOSX_DEPLOYMENT_TARGET=10.5 install-syslib
otool -L "$OUT/support-leopard/opt/local/lib/libMacportsLegacySystem.B.dylib"
otool -l "$OUT/support-leopard/opt/local/lib/libMacportsLegacySystem.B.dylib" | grep -A4 -E 'LC_VERSION_MIN_MACOSX|LC_BUILD_VERSION'
echo "Leopard compiler libraries and compatibility library are in $OUT."
