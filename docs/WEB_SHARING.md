# Streaming by link in the browser

During a stream, use **Share on the web → Copy link**. Anyone with the link
watches without a Discord account. **End web** disconnects the browsers and keeps the
native stream; stopping the stream or leaving the call also ends the web share.
The sound is the audio of the computer/window selected in the stream, without adding the
microphone or capturing the call through a separate path. If the computer audio capture
includes a call's output, that sound may also go into it.

The first visit starts the video muted to respect browsers' autoplay rules. Use **Enable sound**. There are volume, playback and fullscreen controls.
Up to three viewers can watch at the same time, with one copy of the upload per
viewer. The link changes with every session and stops working when it ends.

## Architecture

- Litecord ↔ Litecord keeps using the existing protocol.
- `str0m` sends the existing encoder's H.264 over a WebRTC track. There is no second
  video encoder nor WebCodecs on the page; the browser decodes a `<video>`.
- The existing loopback feeds a shared mono Opus encoder, created
  only while there are web viewers. It supports 44.1 and 48 kHz input.
- The existing worker serves `/watch` and routes SDP on `/web/ws`. No audio/video
  goes through the worker. There is no need to host a second page on GitHub Pages.
- The AES-256-GCM key lives in the link's `#` fragment. Signaling is encrypted
  before sending. The owner token is independent of that key; knowing the
  viewer link does not allow registering a sender in that room.
- Before reserving a slot, the worker verifies a proof derived from the key and the
  room. The proof is `SHA-256("litecord-web-viewer-v1\0" || 32-byte key || ASCII
  room)`; the sender registers only the SHA-256 hash of that proof. The AES key
  does not go to the worker. A missing or incorrect proof is refused with status 403 without
  registering a socket in the room or relaying an offer to the sender. Capacity is
  checked again after the asynchronous hash to respect the limit under concurrency.
- The public address is discovered through STUN on the WebRTC socket itself. A connection
  after a period without viewers repeats this discovery, with no idle STUN heartbeat.
- Off: no WebRTC socket, thread or timer. On with no viewers: only the
  signaling connection, with a 40-second keepalive; no frame is copied and
  no audio is encoded. The interface counters are updated by event.
- Bounded queues avoid blocking capture/encoding; a local loss of a frame
  invalidates the references and sending waits for another keyframe.

## Local validation (Windows)

Build the tests, start the worker in another terminal and run the browser test:

```powershell
cargo test --release --lib screen_capture::web_share::tests --no-run
cd cloudflare
npx wrangler dev --local --ip 127.0.0.1 --port 8787
```

From the root, pass the test executable reported by Cargo:

```powershell
node scripts/perf/web_share_e2e.cjs target/release/deps/litecord-<hash>.exe
```

The test uses headless Chrome, Playwright and Python, a synthetic H.264 source of
640×360 at approximately 12.5 FPS and a 440 Hz Opus sine wave. It does not use accounts,
a microphone or screen capture. It checks video/audio decoding, wrong key,
three viewers, refusal of the fourth, viewer reconnection, shutdown and an invalid
link. Artifacts and CPU/memory samples go to `target/web-share-e2e/`.
The samples include the synthetic generation/encoding: they are not a game benchmark
nor an estimate of the cost of 1080p60.

After authentication before admission, **48 Rust tests** passed (one fixture
ignored by default), **six worker tests** and the full flow in Chrome with a
local worker: H.264/Opus, wrong key, three viewers, refusal of the fourth,
viewer replacement and shutdown. The native signaling regression
also passed. Attempts with a missing, wrong or malformed proof do not register a
viewer, and ten simultaneous admissions keep the limit of three.

```powershell
cargo test --release --lib screen_capture::web_share::tests -- --skip browser_fixture
cd cloudflare
npm test
```

To test the application with the local worker, set
`LITECORD_WEB_SIGNALING_URL=ws://127.0.0.1:8787` when starting a test build. The
`web_share_start` automation returns immediately with `pending`; query `state`
until `stream.web_url` is available, or `stream.web_error` indicates the failure.

## Publishing and limits

### Session stop and restart

Stopping removes the session's channels from the interface and signals the shutdown without
waiting for the thread or the network. Each start creates its own socket, channels, encoder and state;
cleaning up a previous session does not affect the current one. Closing the
WebSocket has a 50 ms limit in the background.

Local validation on 17/10/2026: with a server that does not acknowledge the close,
`stop()` returned in **0.040 ms**. The test also kept a new session with three
viewers after the previous one was cleaned up. **47 Rust tests** passed (one browser
fixture ignored by default) and the **four worker tests**. This stop test does not
replace validation of real capture and of the browsers.

The production server needs to receive `cloudflare/worker.js`, `web-share.js`, the
page and the HTML module rule from `wrangler.toml`. Wrangler bundles the page
by import. No migration or new Durable Object class is needed:
web rooms use the `web:` prefix in the existing class and attachments that survive
hibernation. The native routes `/ws` and `/health` remain available.

Protocol compatibility does not mean no interruption on deploy:
[Cloudflare states that updating the code restarts the Durable Objects and disconnects
the WebSockets](https://developers.cloudflare.com/durable-objects/best-practices/websockets/).
The native client tries to reconnect. Take this into account when choosing the time to publish.

Decision: keep a single worker and publish its changes together, after finishing
the work related to it. For now, do not publish. The `LITECORD_WEB_SIGNALING_URL`
variable allows using a local worker only for the web tests,
without changing the native signaling address.

```powershell
cd cloudflare
npx wrangler deploy
```

There is no TURN. Symmetric NAT or networks that block UDP can prevent the connection;
STUN does not guarantee connectivity on every network. A signaling drop ends
the session; generate a new link to reconnect. Still to be validated: a real stream
between distinct networks, other browsers, Linux and the cost at game resolution/FPS.

## Manual offer/answer exchange, without a worker

This is a test mode, enabled explicitly. The stream still comes out of the
app's real encoder. STUN discovers addresses, but no server does signaling.

With the test executable already built at `target/web-share-e2e/litecord-web-test.exe`:

```powershell
powershell -File scripts/perf/start_web_manual.ps1
```

To rebuild this build manually:

```powershell
cargo build --release --bin litecord --features automation
$env:LITECORD_WEB_MANUAL = '1'
$env:LITECORD_AUTOMATION_PORT = '39091'
.\target\release\litecord.exe
```

Start the stream and click **Share on the web → Copy link**. Open the local link
`file:///…#manual` and run the command below in the terminal:

```powershell
powershell -File scripts/perf/web_share_manual.ps1 -Port 39091
```

If the clipboard does not yet hold a valid offer, the script waits:
select all the text in the browser's first field and copy it; go back to the terminal
and press Enter. This way, copying the command itself does not make the test fail. The script
replaces the clipboard with the answer. Paste it into the browser's second field
and click **Connect** and then **Enable sound**. Use Q to cancel.

The automation only listens on
localhost. To test another computer, it can open the same `watch.html#manual`
and send its offer as text; whoever runs Litecord returns the answer. The SDP
contains network addresses: exchange it privately. This does not get around symmetric NAT
or UDP blocking. Without signaling, a remote shutdown is only noticed by the
WebRTC timeout, and closing a tab may take a while to free the slot.

The automatic test of the same flow removes the need to copy and paste by hand:

```powershell
node scripts/perf/web_share_e2e.cjs target/release/deps/litecord-<hash>.exe --manual
```

Do not distribute builds with `automation`. Normal mode keeps using the single worker.

## Real capture and browser matrix

With the local worker started and the browsers already installed, use an
isolated release build to measure the sender. The script opens the indicated test account,
selects by name only the server `Ricogames_ e MariaJulia_` and the channel
`fofocas`, and ends only the process it started. The audio output is muted.

```powershell
cargo build --target-dir target --release --bin litecord --features automation --offline
node scripts/perf/web_share_real.cjs --account=Marido --seconds=20 --rounds=2 --quiet-builds=1
```

Capture uses the real Windows desktop at 1080p60, without audio, with the app in the tray.
A dedicated window sized to the primary monitor shows constant motion;
it is captured through the app's normal path, without injecting frames. It is not a game and
its cost does not represent all possible content. Headless Chrome receives the video
during the measurements; Edge and Firefox also verify real playback at the end.

Each round measures: streaming without web, web without viewers, with one viewer and
with three. There is a final sample without web to observe drift. CPU is a percentage of
**one logical core**, memory is working set and private in MiB. The script calls the existing
collectors `process_sample.py` and `wakeups.ps1`, records the received resolution/FPS
and saves data to `target/web-share-real/<date>/`. A failure of the
wake-up collector is recorded separately. Close other builds and keep the
screen content the same when comparing samples.

`--quiet-builds=1` waits up to ten minutes for compilers to finish before starting
the app and before each sample. If a build starts during collection, the script
keeps the sample and repeats the phase (up to three attempts). Compiler PIDs at the
start/end are in the JSON. FPS uses WebRTC's `framesDecoded`: Firefox 158 returns
zero in `getVideoPlaybackQuality().totalVideoFrames` even though it decodes normally.

The synthetic test, which includes non-silent Opus audio, accepts each installed
browser, without downloading another browser or using the personal profile:

```powershell
node scripts/perf/web_share_e2e.cjs target/debug/deps/litecord-<hash>.exe --browser=chrome
node scripts/perf/web_share_e2e.cjs target/debug/deps/litecord-<hash>.exe --browser=edge
node scripts/perf/web_share_e2e.cjs target/debug/deps/litecord-<hash>.exe --browser=firefox
```

The installed Firefox uses Playwright's `moz-firefox` channel (WebDriver BiDi).
The app announces the local network IPv4 instead of relying on a `127.0.0.1` candidate.
The synthetic results are in `target/web-share-e2e/<browser>/`; they are not useful
for estimating the cost of real capture. None of these scripts publishes the worker.

### Results of 18/10/2026 and pending items

On Windows, the full tests passed with Chrome **155.0.8059.84**, Edge
**155.0.4283.62** and Firefox **158.0**: H.264, non-silent Opus, wrong key,
three viewers, refusal of the fourth, slot release, shutdown and regression
of native signaling. In all three, the selected ICE pair was host/host and the remote
candidate was not loopback. These tests use the 640×360 synthetic source.

Real capture was run with the release build of commit `6146e6e`, on a
Core i5-13450HX (16 logical processors), 8 GB of RAM and NVIDIA RTX 3050 Laptop
6 GB, using NVENC, a 1920×1080 desktop, a target of 60 FPS and 8 Mbps, without audio and with
the app in the tray. Chrome received 1920×1080 with three viewers at **59.3–60.0
FPS** in the stable samples; Edge also played the real capture at 1080p.

Two 20-second rounds were collected per phase. The table below preserves
the observed range; **it is an exploratory series, with interference from
builds, and not a controlled comparison of the additional cost of the web**:

| Real capture | CPU (% of one core) | Working set (MiB) | Private (MiB) |
| --- | ---: | ---: | ---: |
| Web off | 50.78–60.00 | 79.08–93.23 | 517.01–543.71 |
| Web, zero viewers | 52.73–53.83 | 83.67–118.47 | 528.04–581.50 |
| Web, one viewer | 57.89–60.86 | 91.83–125.03 | 544.07–606.32 |
| Web, three viewers | 48.51–58.28 | 98.63–119.73 | 558.13–611.48 |

The data is in `target/web-share-real/2026-10-18T03-11-31-686Z/`. During the
concurrent build, a sample with one viewer had an average reception of
33.87 FPS; the next phase went back to approximately 60 FPS in all three. Memory is
the value at the end of each sample, and the process accumulates caches between phases. The
lower CPU numbers with three viewers cannot be interpreted as a gain.

The repeat without compilers (`2026-10-18T03-17-47-905Z`) obtained one sample without
web at 60 FPS, but was interrupted at **00:20:45, Brasília time**, by a
disconnection of the test account from the `fofocas` channel. The app ended capture and web; the
data after that disconnection is invalid for the comparison. The collector
now validates the call, capture and number of viewers before/after the sample.

**Still to complete: the controlled repeat with the reserved account for about four
minutes, and real capture on Firefox.** The synthetic Firefox test already passed;
the corrected counter uses `framesDecoded`, since the video element's counter
returned zero. No test between distinct networks was done, nor a deploy.

## Cross-network test script, after authorized publishing

1. On the sending computer, open the diagnostic at **Network & Proxy → Test network**
   before and during the call. Keep the report, time and connection type.
2. Start a real capture with the resolution/FPS noted, enable web sharing
   and send the full link privately to the test device.
3. On the phone, turn off Wi-Fi and confirm it is on 4G/5G. Open the link over HTTPS.
   Note the browser/version, carrier and time until the first image appears.
4. Watch for at least two minutes. Check continuous motion, fullscreen,
   rotation and, if the capture includes audio, the **Enable sound** button. Record pauses,
   perceived delay, audio loss and the number of viewers shown in the app.
5. Open two more viewers, try a fourth, close one and try again.
   Confirm the full-capacity notice and the slot release. End the web share in the app and
   confirm the shutdown notice on the devices.
6. Repeat on another Wi-Fi network and, if possible, swapping the sender's network.
   Note each combination separately: a successful local connection does not validate
   NAT between different networks.

If it fails, collect: time, the exact page message, the app's viewer state/count,
the network diagnostic report, versions, network types and whether it works on the
same LAN. On the receiving computer, `chrome://webrtc-internals`,
`edge://webrtc-internals` or `about:webrtc` help record ICE/DTLS states,
selected pair type (host/srflx), codec, bytes received and decoded frames.
Do not publish the link/key, tokens or raw SDP; this data may contain access to the
session or addresses. The app's reports already mask public addresses.

A page unavailable points first to HTTPS/route/deploy. Access refused calls for
a new link of the current session. ICE failing with no media bytes suggests UDP blocking
or an incompatible NAT; the diagnostic distinguishes some of these cases, but does not guarantee
that two NATs are compatible. There is only a direct connection with STUN, without TURN.
Bytes arriving with no decoded frames call for inspection of the codec and keyframes.

## Checklist for publishing the single worker

- Gather all the planned changes for the worker and review the integration on the
  branch chosen by the owner. Integration is not part of these scripts.
- Repeat the Rust tests, `npm test` in `cloudflare/`, the three browsers with a local
  worker and the real samples. Record the commit, numbers and pending items before the release.
- Check `worker.js`, `web-share.js`, the imported page, the HTML rule, the `ROOMS` binding
  and the existing `SignalingRoom` class. Do not add a new-class migration
  to the existing worker. Keep native v2 `/ws` and `/health` working.
- Keep the currently published version and the rollback procedure. Choose
  a window for the WebSocket reconnection; active web links may end and
  need to be recreated. Communicate this effect to the participants.
- Ask for authorization to deploy the reviewed set. Only then run, from the
  `cloudflare/` folder, `npx wrangler deploy` **once**. Push, tag and release also
  depend on their own authorization; they are not done by these tests.
- Verify `/health`, `/watch`, native signaling reconnection and the 4G/5G script
  above. If necessary, roll back the worker version and disable web
  sharing while investigating.
- Publish only the normal build, without `automation`, after the agreed validation.

The test between different networks remains pending until that deploy and the user
runs the script. No publishing is needed to validate the local path.

Local validation on 17/10/2026: manual mode passed on Chrome with the test worker
turned off, including H.264 video and non-silent Opus audio. The
46 Rust tests and the four worker routing tests also passed. The automatic path
was validated earlier with three viewers and a native protocol regression.
