# Watch on demand (protocol 3)

The sender only sends to whoever is watching, and only encodes while someone
is watching (in the app or in the browser). Anyone in the call who is not watching receives
nothing of the stream.

## How it was (protocol 2)

- The sender sent every frame and the stream audio to **all** peers
  that had announced themselves in the signaling room in the last 60 s, watching or not.
- Video, audio and presence were encrypted with a key derived from the **channel ID**.
  The ID is not a secret: anyone who knew it could join the worker's room,
  announce presence, start receiving the stream and decrypt it. The X25519
  exchange that existed was computed and never used.
- The bitrate only decreased (loss > 8% → −20%), never went back up and was not
  reset between streams.
- In a call, everyone exchanged heartbeats every 1.5 s and broadcast on the local network.

## How it works now

### Watch request

- A new stream appears **without video**, with the **Watch** button (as on
  Discord). Watching, closing the stream and "hiding" now mean starting
  or stopping reception.
- While the viewer wants the stream **and some window can show it**, the app
  sends `OP_WATCH` to the sender once per second, over the P2P socket. In the tray,
  minimized or with the stream hidden, the request stops; on return, it starts again and the
  sender generates a keyframe immediately.
- `OP_WATCH` carries the viewer's X25519 public key. The sender only accepts
  people who are **in the same Discord voice channel** (gateway voice state).
- The request is valid for 4 s. Without renewal, the viewer leaves the list. `OP_UNWATCH`
  ends it immediately.

### Per-stream key

- Each stream draws a new AES-256 key. Video, FEC and stream audio
  are encrypted with it, once per packet, for all viewers.
- The sender answers each request with `OP_WATCH_ACK`: its public key and the
  stream key encrypted with the peer key (X25519 + SHA-256, the same on both
  sides). Only whoever made the request can open it.
- Whoever did not ask, or was refused, receives no packets and would have no way to decrypt them.

**Limit:** Discord offers no way for one client to prove to another who it is. An
attacker who knows the channel ID can still pretend to be someone **who is in the voice
channel** and request the stream on their behalf. The sender associates each person with the
first public key that shows up for the stream and refuses another key for the
same ID. This closes the common case (someone who is not even in the call), but it is not a proof of
identity.

### Bitrate

- Reset to the configured value on every stream.
- Decided by the **worst viewer** with a report in the last 3 s. A viewer who leaves stops
  counting when the request expires.
- Loss above 8%: −20% (minimum 3 Mbps). Loss below 2% for 5 s in a row:
  +0.5 Mbps, up to the configured value.

### Timers

- With no viewer: the sender neither encodes nor sends video and audio, and capture
  drops to the pace of the local preview (4 frames per second). The sender's
  announcement (every 1.5 s) continues, so that newcomers can find it.
- Heartbeats between peers exist only where there is a stream: between the viewer and the
  sender (the `OP_WATCH` itself). The local network broadcast only happens when
  signaling is down.
- The loss report is only sent for streams that are being watched.

### Browser

Web streaming is already on demand: the video is only copied into the WebRTC session
when viewers are connected. It now also counts as a viewer for the
encoder: with only the browser watching, the app encodes and sends nothing over P2P.

## Compatibility

Old versions send to everyone with the channel key and do not know how to ask. They cannot
watch anyone using protocol 3, and vice versa. That is why the version that
brings protocol 3 is a **mandatory update**: the worker now requires `v=3`
(`MIN_PROTOCOL = 3`) and parks old versions, as in 1.2.0. The old app
stops finding peers over the internet until it updates.

Publishing the worker before the new version would leave everyone without streaming until
they update; the worker must go out together with the release.

## Measured (Windows 11, 1080p60, two accounts on the same PC)

`scripts/perf/watch_on_demand.ps1`:

| Situation | What the viewer receives | Sender CPU |
|---|---|---|
| Nobody watching | nothing | 4.5–5.9% of a core (before: 25–33%, capturing at 60 FPS) |
| Watching | ~60 FPS | 45–49% |
| Stream hidden | nothing | 5–6% |
| Viewer in the tray | nothing | 5–6% |

The first image arrives 0.2–0.7 s after **Watch**; when returning from the tray or
showing it again, 0.1–0.45 s (before 0.12 s, because the video kept arriving).
The viewer spends 1.3–1.5% of a core in the tray (before 2.2%), because nothing arrives.
With only a browser watching, the app encodes normally (Chrome, Edge and Firefox
receive 1080p60 from real capture); with nobody, native or web, capture stays at 4 FPS.

## Limits that remain

- Presence in signaling (IP addresses and "I'm streaming") is still encrypted
  with the channel key: anyone who knows the channel ID and joins the worker's room sees the
  addresses of those in it, but does not receive or decrypt the stream.
- `OP_STOP` is not authenticated: someone on the same network could make another
  person's stream disappear from the viewer's screen until the next announcement.
- No TURN: networks that block UDP or have symmetric NAT on both sides do not connect.

## Tests

- Unit: peer key derivation (the same on both sides), wrapping of the stream
  key, request expiry, refusal of anyone not in the channel or who changes
  key, bitrate adjustment.
- End to end (`scripts/perf/watch_on_demand.ps1`): without watching, the sender does not
  encode and the receiver receives nothing; on watching, the image arrives; on hiding, it
  stops; on returning, the image reappears; and the `stream_e2e.ps1` regression.
