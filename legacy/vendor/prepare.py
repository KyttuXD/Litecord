#!/usr/bin/env python3
"""Fetches the crates the legacy edition needs in a patched form and patches them.

Voice needs Discord's DAVE encryption (the `davey` crate, on OpenMLS) and Opus (`opus-rs`).
They are written for newer Rust than the 1.77 compiler old OS X has: two are edition 2024,
which Cargo 1.77 cannot even read. This script downloads the exact published versions
(sha256 checked) into this folder and applies the few changes below. Nothing else is touched;
the folders it creates are not committed (see .gitignore here).

    python3 legacy/vendor/prepare.py          # safe to run again: existing folders are kept
    python3 legacy/vendor/prepare.py --force  # download and patch again

The changes are all "same meaning, older spelling":
  edition 2024 -> 2021, `&raw mut x` -> `addr_of_mut!(x)`, `n.is_multiple_of(k)` -> `n % k == 0`,
  `Option::is_none_or`, `iter::repeat_n`, inline `const { }`, a let-chain, `core::error::Error`,
  `size_of`/`size_of_val` imports, exhaustive matches on `Result<_, Infallible>`.
"""
import hashlib
import io
import os
import re
import shutil
import sys
import tarfile
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))

CRATES = {
    "opus-rs": ("0.1.42", "bafbcaf2ed381cdf68ced4ed8d16517de90ffd7424e36f0b9e45d67fbe30b1f9"),
    "davey": ("0.1.5", "27466c95d1885cc506b29cec686a91ba7c472376cca09850d026cceb9c29a888"),
    "openmls": ("0.8.1", "dcb512bfe6a55777518853ea535c6241f069cb0e8984678c117151d2a1e7e903"),
    "rand_core": ("0.10.1", "63b8176103e19a2643978565ca18b50549f6101881c443590420e4dc998a3c69"),
}


def fetch(name, version, sha256):
    url = f"https://static.crates.io/crates/{name}/{name}-{version}.crate"
    request = urllib.request.Request(url, headers={"User-Agent": "litecord-legacy-vendor"})
    data = urllib.request.urlopen(request, timeout=120).read()
    got = hashlib.sha256(data).hexdigest()
    if got != sha256:
        sys.exit(f"{name} {version}: sha256 is {got}, expected {sha256}")
    target = os.path.join(HERE, name)
    shutil.rmtree(target, ignore_errors=True)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive.getmembers():
            # Only plain files and folders below "<name>-<version>/".
            if not (member.isfile() or member.isdir()) or not member.name.startswith(f"{name}-{version}/"):
                continue
            if ".." in member.name.split("/"):
                sys.exit(f"{name}: unexpected path {member.name}")
            archive.extract(member, HERE)
    os.rename(os.path.join(HERE, f"{name}-{version}"), target)
    for extra in ("Cargo.toml.orig", "Cargo.lock", ".cargo_vcs_info.json", "benches", "examples", "tests"):
        path = os.path.join(target, extra)
        if os.path.isdir(path):
            shutil.rmtree(path)
        elif os.path.exists(path):
            os.remove(path)
    return target


def edit(root, relative, changes):
    """changes: list of (pattern, replacement, expected count or None, regex?)."""
    path = os.path.join(root, relative)
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    for pattern, replacement, expected, is_regex in changes:
        if is_regex:
            text, count = re.subn(pattern, replacement, text)
        else:
            count = text.count(pattern)
            text = text.replace(pattern, replacement)
        if count == 0 or (expected is not None and count != expected):
            sys.exit(f"{relative}: expected {expected} of {pattern!r}, found {count}: the crate changed, update prepare.py")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


def manifest(root, extra_changes=()):
    """Edition 2021, no rust-version, no bench/example/test targets (their folders are removed)."""
    path = os.path.join(root, "Cargo.toml")
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    text = text.replace('edition = "2024"', 'edition = "2021"')
    text = re.sub(r'(?m)^rust-version = .*\n', "", text)
    text = re.sub(r'(?ms)^\[\[(bench|example|test)\]\]\n.*?(?=^\[|\Z)', "", text)
    text = re.sub(r'(?m)^auto(lib|bins|examples|tests|benches) = .*\n', "", text)  # keys Cargo 1.77 does not know
    for pattern, replacement in extra_changes:
        text, count = re.subn(pattern, replacement, text)
        if count == 0:
            sys.exit(f"{path}: {pattern!r} not found: the crate changed, update prepare.py")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


def each_source(root):
    for folder, _, files in os.walk(os.path.join(root, "src")):
        for name in files:
            if name.endswith(".rs"):
                yield os.path.relpath(os.path.join(folder, name), root)


def patch_everywhere(root, changes):
    """Regex changes applied to every source file; returns how many were made."""
    total = 0
    for relative in each_source(root):
        path = os.path.join(root, relative)
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        new = text
        for pattern, replacement in changes:
            new, count = re.subn(pattern, replacement, new)
            total += count
        if new != text:
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(new)
    return total


MULTIPLE_OF = (r"!\s*([A-Za-z_][A-Za-z0-9_\.]*(?:\(\))?)\.is_multiple_of\(([^()]+(?:\([^()]*\))?)\)", r"(\1 % (\2) != 0)")
MULTIPLE_OF_PLAIN = (r"([A-Za-z_][A-Za-z0-9_\.]*(?:\(\))?)\.is_multiple_of\(([^()]+(?:\([^()]*\))?)\)", r"(\1 % (\2) == 0)")


def patch_opus(root):
    manifest(root)
    patch_everywhere(root, [
        (r"&raw mut (\(\*[A-Za-z_]+\)\.[A-Za-z_0-9]+)", r"core::ptr::addr_of_mut!(\1)"),
        (r"&raw const (\(\*[A-Za-z_]+\)\.[A-Za-z_0-9]+)", r"core::ptr::addr_of!(\1)"),
        MULTIPLE_OF, MULTIPLE_OF_PLAIN,
        (r"([A-Za-z_][A-Za-z0-9_]*)\.is_none_or\(", r"\1.map_or(true, "),
        (r"std::iter::repeat_n\(([^,()]+), ([^,()]+)\)", r"std::iter::repeat(\1).take(\2)"),
    ])
    edit(root, "src/fixedvec.rs", [
        ("buf: [const { MaybeUninit::uninit() }; N],",
         "// SAFETY: an array of MaybeUninit needs no initialization.\n            buf: unsafe { MaybeUninit::<[MaybeUninit<T>; N]>::uninit().assume_init() },", 1, False),
    ])


def patch_davey(root):
    manifest(root, [
        # The Node.js and Python bindings: optional, and they need Rust 1.83 - 1.88.
        (r'(?ms)^\[dependencies\.(napi|napi-derive|pyo3)\]\n.*?(?=^\[)', ""),
        (r'(?ms)^napi = \[\n.*?\]\n', ""),
        (r'(?m)^pyo3 = \["dep:pyo3"\]\n', ""),
    ])
    edit(root, "src/displayable_code.rs", [
        ("if !desired_length.is_multiple_of(group_size) {", "if desired_length % group_size != 0 {", 1, False),
    ])
    edit(root, "src/cryptor/codec_utils.rs", [
        ('#[allow(\n  clippy::unusual_byte_groupings,\n  reason = "following upstream libdave byte groupings"\n)]',
         "#[allow(clippy::unusual_byte_groupings)] // following upstream libdave byte groupings", 1, False),
        ("use core::matches;", "use core::matches;\nuse core::mem::size_of_val;", 1, False),
    ])
    edit(root, "src/session.rs", [
        ("              if let Some(ids) = expected_user_ids\n                && !ids.contains(&incoming_user_id)\n              {\n"
         "                return Err(ProcessProposalsError::UnexpectedUser(incoming_user_id));\n              }",
         "              if let Some(ids) = expected_user_ids {\n                if !ids.contains(&incoming_user_id) {\n"
         "                  return Err(ProcessProposalsError::UnexpectedUser(incoming_user_id));\n                }\n              }", 1, False),
    ])


def patch_openmls(root):
    manifest(root)
    patch_everywhere(root, [MULTIPLE_OF, MULTIPLE_OF_PLAIN])


def patch_rand_core(root):
    manifest(root)
    edit(root, "src/lib.rs", [
        ("core::error::Error", "std::error::Error", 2, False),
        ("    html_favicon_url = \"https://www.rust-lang.org/favicon.ico\"\n)]\n",
         "    html_favicon_url = \"https://www.rust-lang.org/favicon.ico\"\n)]\nextern crate std; // core::error::Error needs Rust 1.81\n", 1, False),
        ("            Ok(x) => x,\n        }", "            Ok(x) => x,\n            Err(never) => match never {},\n        }", 2, False),
        ("            Ok(()) => (),\n        }", "            Ok(()) => (),\n            Err(never) => match never {},\n        }", 1, False),
    ])
    edit(root, "src/unwrap_err.rs", [("core::error::Error", "std::error::Error", 1, False)])
    edit(root, "src/utils.rs", [
        (r"\A((?:.*\n)*?)use ", r"\1#[allow(unused_imports)]\nuse core::mem::{size_of, size_of_val}; // in the prelude only since Rust 1.80\nuse ", 1, True),
    ])


def quiet(root):
    """These are other people's crates in a rewritten spelling: their warnings (extra
    parentheses from the rewrites, lints newer compilers dropped) are not ours to act on."""
    path = os.path.join(root, "src", "lib.rs")
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("#![allow(warnings)]\n" + text)


PATCHES = {"opus-rs": patch_opus, "davey": patch_davey, "openmls": patch_openmls, "rand_core": patch_rand_core}


def main():
    force = "--force" in sys.argv[1:]
    for name, (version, sha256) in CRATES.items():
        target = os.path.join(HERE, name)
        if os.path.isdir(target) and not force:
            print(f"{name} {version}: already here")
            continue
        root = fetch(name, version, sha256)
        PATCHES[name](root)
        quiet(root)
        print(f"{name} {version}: fetched and patched")


if __name__ == "__main__":
    main()
