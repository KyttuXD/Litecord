# 🍎 Litecord on macOS (Intel)

This fork adds a build for **Intel Macs (x86_64) on macOS 10.12 Sierra or later**. It needs no AVX and
no particular GPU (the interface is drawn in software by default), so it is meant for old machines too.

| Machine | Works with |
| :--- | :--- |
| MacBook Air / Pro, iMac, Mac mini from 2010–2020 (for example the 2015 MacBook Air) | The macOS they already run (10.12 or later). |
| Mac Pro 3,1 (2008) and other Macs whose last official system is 10.11 El Capitan or older | Only with a patched newer macOS (dosdude1 patchers or OpenCore Legacy Patcher). 10.11 itself cannot run it: the Rust standard library needs 10.12. |
| Apple Silicon Macs | Through Rosetta 2. |

## Mac Pro 3,1 (2008) and other Macs on 10.6 – 10.11

The app cannot start on these systems as they are, and no build option changes that:

- The Rust standard library needs macOS 10.12 (older Rust reached 10.7, never 10.6, and cannot build this project).
- The window, tray and camera libraries call AppKit / AVFoundation APIs that old systems do not have.
- Discord only accepts TLS 1.2, which the system TLS of 10.6 – 10.8 does not offer.

What works on that hardware (two 64-bit Xeons without AVX are fine for Litecord):

| Route | Keeps the old macOS? | Runs |
| :--- | :--- | :--- |
| Boot a current Linux from a USB stick or a second drive | Yes, the internal system is not touched | `Litecord-x86_64.AppImage` |
| Install macOS 10.12 – 10.13 with a dosdude1 patcher on another drive or partition | Yes, if it goes on its own drive | `Litecord-macos-x64.zip` (drawn in software, no Metal needed) |
| OpenCore Legacy Patcher with a newer macOS | Yes, on its own drive | `Litecord-macos-x64.zip` |

## Install

1. Download `Litecord-macos-x64.zip` from the [nightly release](https://github.com/KyttuXD/Litecord/releases/tag/nightly),
   unzip it and move `Litecord.app` to `/Applications`.
2. The app is not signed with an Apple developer account, so the first time **right-click it and choose
   Open** (or run `xattr -dr com.apple.quarantine /Applications/Litecord.app`).
3. Allow the **Microphone** when asked (and **Camera** / **Screen Recording** when you share them).

## What differs from Windows and Linux

- No global mute/deafen shortcuts.
- Screen sharing sends whole displays: no single windows, no system audio, no mouse pointer.
- Video is encoded on the CPU (OpenH264).
- Updates open the nightly page in the browser instead of installing in place.
- Started from Finder, settings and logs live in `~/Library/Application Support/Litecord`.

## Build it yourself

Needs the Xcode Command Line Tools, Rust with `rustup target add x86_64-apple-darwin`, and `brew install nasm`.

```bash
MACOSX_DEPLOYMENT_TARGET=10.12 cargo build --release --bin litecord --target x86_64-apple-darwin
```

```bash
scripts/macos/bundle.sh
```

The second command creates `dist/Litecord.app` (with `packaging/macos/Info.plist`, the icon and an ad hoc
signature) and `dist/Litecord-macos-x64.zip`. Build only `--bin litecord`: the test binaries under `src/bin`
are Windows tools. The build needs a recent Rust, so for a very old Mac it is easier to build on a newer one
and copy the app over.

## Where the macOS code is

- `src/screen_capture/sources/macos.rs`: screen capture with `CGDisplayCreateImage`.
- `#[cfg(target_os = "macos")]` blocks in `src/main.rs`, `src/utils/updater.rs`, `src/utils/hardware_monitor.rs`,
  `src/audio/sound_effects.rs`, `src/auth/vault.rs` and `src/screen_capture/audio_loopback.rs`.
- `libc` is a dependency on every Unix target (`Cargo.toml`), not only Linux.
