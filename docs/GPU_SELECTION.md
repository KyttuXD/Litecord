# Streaming GPU on Windows

Under **Settings → Video & Interface → Video encoder**, the app lists
the graphics cards found on this machine, along with **Automatic** and **CPU (OpenH264)**.
The choice applies to the next stream; changing the preference while someone
is watching does not recreate the capture or the encoder of the current session.

## Automatic selection

The app creates the D3D11 device on the card connected to the captured monitor. For a
window, it uses the monitor where the window is when the stream starts. On laptops with the screen
on the Intel GPU, this allows using Quick Sync and leaving the NVIDIA card available for the game.
On a monitor connected to the NVIDIA card, Automatic may use NVENC on the NVIDIA card itself.
Moving the window to another monitor during the session does not change the device.

To capture a monitor, it first tries **DXGI Desktop Duplication**, which requires a
device from the card that monitor is connected to. If that fails, it tries
**Windows Graphics Capture (WGC)** on the same device. To capture a window, it
uses WGC. This DXGI requirement is in the [Microsoft documentation](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput).

Automatic avoids a transfer between cards in the usual setup; it is not a
guarantee against driver failures, monitor changes, protected content or lack of
encoder support. If capture with an explicit device does not start, the
app falls back to the previous capture, including the GDI fallback. If texture
conversion or encoding fails, the session continues on the path that copies to the
CPU. On AMD, textures straight into AMF remain restricted to manual selection until
they are validated on a real card.

## Manual selection

- **Intel:** uses Quick Sync through Media Foundation on the chosen device.
- **NVIDIA:** uses the driver's NVENC API on the chosen device.
- **AMD:** tries AMF by texture; it still needs validation on AMD hardware.
- **CPU (OpenH264):** keeps the compatible capture and encodes in software.

When the chosen card is not the monitor's, capture uses WGC on that card. Windows
may transfer the image between the GPUs. The conversion and encoding steps
stay on the capture device, without the app reading the full frames
back into memory;
the local thumbnail is still read at a reduced size. This does not mean
the GPUs stop sharing bandwidth or power with the game.

During capture with an explicit device, the settings show the card
name, **DXGI/WGC** and whether processing stays on the GPU or has moved to
compatibility mode. The information changes on start, fallback and stop events, with no
refresh timer when the app is idle.

Preferences are saved as `gpu:VVVV:DDDD` (PCI vendor and device IDs).
Old values are still accepted: `nvenc` and `ffmpeg` choose NVIDIA, `wmf` chooses
Intel, `amf` chooses AMD and `openh264` chooses CPU. Preferences are not rewritten
automatically. If the saved card does not exist, it appears as unavailable in the list
and capture uses Automatic. Two cards of the same model share the same
option; choosing a specific instance of that model is not implemented.

## Local validation

Build with `--features automation`, with the local worker already running at `127.0.0.1:8787`:

```powershell
cargo build --release --target-dir target --features automation -j 2
node scripts/perf/gpu_selection.cjs
# Optional: compare with a copy of the previous integrated executable.
node scripts/perf/gpu_selection.cjs --baseline=D:/path/litecord.exe --seconds=10
```

The script uses the Marido/Henrique test accounts only on the server
`Ricogames_ e MariaJulia_`, channel `fofocas`. It mutes the app's output, does not
capture screen audio and only terminates the processes it started. A Chrome window
shows real animated content for the monitor capture. It does not publish the worker.

It checks real streaming and decoding with automatic selection, each GPU
found, CPU and a nonexistent saved GPU; window capture through WGC; preference
change during the session; state cleanup and response time on stop;
and the options translated into seven languages. It measures the CPU cycles
of the sender and memory, saving the results and the screenshots of the
settings in `target/gpu-selection/`. The percentage is of **one 3 GHz core**,
as in the other performance scripts, not of the whole processor.

The script requires each listed GPU to have working texture encoding. For
cards without support, the fallback must be validated manually. It does not cover HDR, monitor
rotation, GPU switching by physical removal, device loss, several cards of the
same model, Linux or behavior inside a specific game.

## Result on 19/10/2026

Windows 11, i5-13450HX, 8 GB of RAM, Intel UHD connected to the 1920×1080 screen and RTX 3050
Laptop 6 GB. Two accounts on the same machine, real animated capture, a 1080p60 request,
output muted and the sending app in the tray. Eight-second samples,
CPU counted in cycles and normalized to a 3 GHz core:

| Selection | Observed capture | Sender CPU | FPS received |
|---|---|---:|---:|
| Automatic | Intel / DXGI / Quick Sync | 14.2% | 59.9 |
| Intel manual | Intel / DXGI / Quick Sync | 15.7% | 59.9 |
| NVIDIA manual | NVIDIA / WGC / NVENC | 11.2% | 60.0 |
| Nonexistent saved GPU | Intel / DXGI / Quick Sync | 16.6% | 60.0 |
| CPU | compatible capture / OpenH264 | 76.6% | 58.1 |
| Window, automatic | Intel / WGC / Quick Sync | 15.0% | 59.7 |
| Not streaming, in a call | — | 1.3% | 0 |
| After stopping NVIDIA | — | 1.6% | 0 |

The previous copy of the executable (identified as `5efda85`) streamed at 16.1%
of one core and approximately 60 FPS. In another round of the new selection, automatic
came in at 12.5% and Intel manual at 15.4%. These are short samples, with variation between
rounds; they do not demonstrate a gain in games or that one GPU will always be more economical.
The main result is the predictable choice of the card with the hardware paths
working. NVIDIA may compete with the game for resources and power.

In the final round, six stops responded in about 7–12 ms through the local control,
including the window stop. No image arrived after stopping the stream
during the sample; GPU processing was disabled and the capture state was
cleared. A preference change during a session was kept for the next one.
The seven languages had their option names checked and interface screenshots saved.

85 normal unit tests passed, along with six hardware tests run
explicitly (conversion, colors, scaling, NVENC, Quick Sync and keyframes). The
browser fixture marked as ignored was not run in this round.
Local artifacts: `target/gpu-selection/2026-10-19T04-00-19-548Z` (initial comparison)
and `target/gpu-selection/2026-10-19T04-09-29-350Z` (final version). AMD and Linux remain
without local validation, as do performance and latency during a heavy game.
