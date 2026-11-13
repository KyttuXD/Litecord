# Litecord on old Intel Macs (OS X 10.5 – 10.11): status

Target machine: a Mac Pro 3,1 (2008, two Xeon E5462, 64-bit, SSE4.1) on **OS X 10.6.8**.
The normal macOS build ([MACOS.md](MACOS.md)) needs 10.12 and cannot start there.
A separate **legacy edition** (`legacy/app`) exists for these systems. It starts on the Mac Pro under
10.6.8, opens its window and gets a QR login code from Discord; logging in and chatting have not been
tried by a person yet. It has text chat only: no voice, pictures, settings or notifications.
This page records what has been proven and what is left.

## The legacy edition

- Download `Litecord-Legacy-macos-10.5.zip` from the [nightly release](https://github.com/KyttuXD/Litecord/releases/tag/nightly),
  unzip it and open `Litecord Legacy.app` (or run `Contents/MacOS/litecord-legacy` from Terminal to see its log).
- Scan the QR code with the Discord app on a phone (Settings > Scan QR Code). The login is kept in
  `~/Library/Application Support/Litecord Legacy/session`, readable by your user only, **not encrypted**.
- Pick a server on the left, then a channel; type and press Return to send. The mouse wheel scrolls each list.
- It is Litecord's own gateway, HTTP and QR-login code, compiled unchanged from `src/` (see
  `legacy/app/src/lib.rs`), with a small CPU-drawn interface in `legacy/app/src/main.rs`.

Two things OS X 10.6 does differently had to be worked around (both found on the Mac Pro):

- `fcntl(F_DUPFD_CLOEXEC)` does not exist before 10.7 and the Rust standard library needs it for every
  `try_clone`: `legacy/compat` answers it with `F_DUPFD` + `FD_CLOEXEC`.
- kqueue's `EVFILT_USER` wake-ups never arrive, so tokio hung: the build sets
  `--cfg mio_unsupported_force_waker_pipe`, which makes mio wake through a pipe.

## What works on the real machine (2026-11-12)

`legacy/probe` is a small test program. Built by the `macos-legacy.yml` workflow and run on the Mac Pro
(OS X 10.6.8) it printed:

```
PASS  rust runtime: threads, thread-locals, clocks (20 ms, ...), hash map of 100
PASS  tls to discord: TLSv1_3, "HTTP/1.1 200 OK", the reply names the gateway
PASS  audio output: "Built-in Output", 44100 Hz, 2 ch, 17408 frames played
PASS  audio input: "Built-in Line Input", 44100 Hz, 2 ch, 17408 frames recorded
RESULT: all checks passed
```

`legacy/window-probe` (same workflow) opens a window with the patched miniquad of TheLastRoute and shows a
picture drawn on the CPU and uploaded as a texture every frame. On the same Mac Pro:

```
PASS  window and OpenGL context created, shader compiled
PASS  first frame drawn, window is 960 x 600
PASS  363 frames in 6.0 s (60 per second), 0 mouse events, 0 key events
RESULT: the window test finished
```

(It logs one harmless `invalid drawable` line before the first frame. Mouse and keyboard were not touched
during that run, so input is not verified yet.)

So on 10.6.8: a window with a CPU-drawn interface at 60 frames per second is possible, a Rust program runs, reaches Discord over modern TLS (rustls; the system TLS stops at 1.0),
and plays and records sound through CoreAudio (cpal).

## How it is built

The recipe comes from [KyttuXD/TheLastRoute](https://github.com/KyttuXD/TheLastRoute) (`packaging/macos/legacy`):

- MacPorts' patched **Rust 1.77** compiler (emulated thread-locals) with the standard library built for Darwin 9.
- MacPorts **legacy-support** (`libMacportsLegacySystem.B.dylib`) shipped beside the program, for system
  calls old systems lack.
- Deployment target **10.5**, linked with a **flat namespace**, old program format (`LC_UNIXTHREAD`).
- Cargo 1.77 drives the build; dependency versions are picked for Rust 1.77
  (`CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback`, lockfile version 3, `coreaudio-sys` 0.2.16).

Scripts: `packaging/macos/legacy/bootstrap-runtime.sh`, `bootstrap-leopard.sh`, `build-probe.sh`.
Run the workflow from the Actions tab (macOS Legacy experiments) and download the `litecord-legacy-probe` artifact.

## What stands between this and Litecord

Measured by resolving Litecord's dependencies for Rust 1.77:

| Part | Problem | Way forward |
| :--- | :--- | :--- |
| Interface (Slint) | Every Slint version Litecord accepts needs Rust newer than 1.77 (1.14 needs 1.88) | An older Slint with its software renderer, if the `.slint` files can be made to compile with it, or a separate small interface |
| Window (winit) | Written for macOS 10.11+ | The patched miniquad of TheLastRoute (OpenGL 2.1): proven by `legacy/window-probe` |
| Video, screen share, web viewers | `openh264`, `str0m`, `h264-reader`, `jpeg-encoder` need Rust 1.83 – 1.87; `openh264` is C++ | Left out of a legacy edition |
| Camera, tray icon | `cameras` needs Rust 1.85 and AVFoundation (10.7+); tray needs 10.10+ | Left out |
| Crypto versions | `sha2` 0.11 and `x25519-dalek` 3 need Rust 1.85 – 1.88 | Their previous major versions |
| Litecord's own code | Uses language features newer than 1.77 in places | Rewritten where the legacy edition uses that code |

The legacy edition therefore has its own window layer and starts with text chat; voice is the next step
(audio in and out already work in the probe).
