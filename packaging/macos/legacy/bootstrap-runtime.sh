#!/bin/sh
# Fetch the MacPorts Snow Leopard bootstrap compiler into a NEW private folder.
# Build-time tool only: players must not need Rust or MacPorts installed.
set -eu
if [ "$(uname -s)" != Darwin ]; then
    echo "This compiler bootstrap runs on an Intel macOS build host." >&2
    exit 1
fi
OUT=${1:?usage: bootstrap-runtime.sh NEW_ABSOLUTE_DIRECTORY}
case "$OUT" in /*) ;; *) echo "An absolute output path is required." >&2; exit 1 ;; esac
if [ -e "$OUT" ]; then
    echo "Refusing to overwrite existing output: $OUT" >&2
    exit 1
fi
mkdir -p "$OUT/downloads" "$OUT/unpack" "$OUT/compiler" "$OUT/support"
BASE=https://github.com/MarcusCalhoun-Lopez/rust/releases/download/1.78.0
fetch() {
    curl --fail --location --retry 3 "$1" --output "$OUT/downloads/$2"
    echo "$3  $OUT/downloads/$2" | shasum -a 256 --check
}
# Checksums match MacPorts' rust_build-1.0.tcl for the Darwin 10 bootstrap.
fetch "$BASE/rustc-1.77.0%2B0-x86_64-macports-darwin10.tar.gz" rustc.tar.gz \
    9cbae2651c6319b3cf4c963d3ad8216b511f6559079c73b656b1666bde73d5a1
fetch "$BASE/rust-std-1.77.0%2B0-x86_64-macports-darwin10.tar.gz" rust-std.tar.gz \
    efac56ec008eddc46165feb13780179cf2325d1e1f95f7bd8519e57eb40d7f87
fetch https://packages.macports.org/legacy-support/legacy-support-1.5.2_0.darwin_10.x86_64.tbz2 legacy-support.tbz2 \
    1ffe7783afb9667153dd344964b990285c78aa1a29da46fa212d720e077ecc93
tar -xzf "$OUT/downloads/rustc.tar.gz" -C "$OUT/unpack"
tar -xzf "$OUT/downloads/rust-std.tar.gz" -C "$OUT/unpack"
tar -xjf "$OUT/downloads/legacy-support.tbz2" -C "$OUT/support"
cp -R "$OUT/unpack/rustc-1.77.0-x86_64-apple-darwin/rustc/." "$OUT/compiler/"
cp -R "$OUT/unpack/rust-std-1.77.0-x86_64-apple-darwin/rust-std-x86_64-apple-darwin/." "$OUT/compiler/"
echo "Fetched checked Snow Leopard compiler and runtime into $OUT (no system installation)."
