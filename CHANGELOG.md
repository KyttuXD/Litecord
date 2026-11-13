# Changelog

All notable changes to **Litecord** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v1.3.1-beta] - 2026-10-26

**Summary:** streams no longer stutter on laptops with two GPUs or for phone viewers; voice recovers by itself when the audio device, signaling or DAVE encryption fail; a new settings window; the main window reopens where it was left; invite and event cards in chat; and many interface fixes from contributors. While idle, Litecord still sits at ~40 MB and 0.2% CPU with the window open. Thanks to @stickyfran, @starzynhobr and @PedroZamecki for their contributions.

### 🔄 Updates
- **The update notice did not appear for versions that only change the last number**: "1.3.1-beta" was read as 1.3.0, because the "1-beta" part did not turn into a number. For that reason this version is published as `v1.3.1`, which 1.3.0 recognizes.

### 🎙️ Voice and calls
- **Incoming audio no longer stops playing** (#64). The audio output is recreated when the device reports an error or stops asking for sound (recovers in ~3 s); a broken DAVE group is rebuilt after repeated failures (at most once every 10 s); and the call no longer tries to reconnect when Discord ends it for good.
- **Audio without DAVE while the call moves to version 0 of the protocol**: participants who had already switched versions were discarded as decryption failures. As libdave does, those packets now play from the moment the change is announced until 10 s after DAVE comes back; outside that window they are still rejected. Based on #65 by @stickyfran.
- **Optional voice processing** (RNNoise noise suppression, echo cancellation and automatic gain), off by default. Contributed by @stickyfran (#41).
- The console log is written by a dedicated thread: a paused console, or one with selected text, used to freeze the whole app.

### 📡 Streaming
- **No stutter on hybrid laptops**: with the other GPU busy, Quick Sync stalled because capture waited inside `AcquireNextFrame` while holding the device. Capture now polls every 4 ms without waiting, encoding has its own thread (a slow frame is dropped instead of freezing the send), and the streaming threads run one level above normal. With the RTX at 100% and the CPU full, Automatic holds ~57–60 FPS.
- **Web viewers (mostly on phones)** lost ~28% of packets with the default send buffer; it is now a 4 MB send buffer, a queue for whatever the socket refuses, and in-order resending.
- Web streaming survives signaling drops (up to 2 min) and short viewer connection drops (15 s); the page keeps playing while the media arrives.
- A still screen is repeated 10 times per second instead of on every pass of the send loop, and the first viewer receives a complete image even when the screen is still.
- **Sharing on the web opens a window** with the link, copy, end and close, instead of the copy button in the top bar.

### 🪟 Window and interface
- **The window reopens where it was left**: position, size and maximized state are saved (once, 1 s after you stop moving it, or when closing or going to the tray). When maximized, the close button works in the corner of the screen. Based on #44 by @stickyfran.
- **New settings screen**, with grouped navigation, one column per page and the explanation under each option. Contributed by @starzynhobr (#59).
- A single font family and whole-number sizes (#47), the Windows shadow on the frameless window (#53), a channel list with adjustable width (#57), colored emoji at the start of channel names (#58), smooth server icons in the CPU renderer (#51), channel names with "decorated" letters without squares (#48). Contributions by @starzynhobr.
- The Privacy screen said there was no server besides Discord; it now says that the Litecord server only puts the participants of a stream in contact with each other and that GitHub is queried for updates.
- On Linux, the window reports `app_id` on Wayland and `WM_CLASS` on X11, avoiding duplicate icons in the taskbar. Contributed by @stickyfran (#45).
- **Shortcuts with mouse buttons on Linux**. Contributed by @PedroZamecki (#43).

### 💬 Chat and servers
- **Server invite and event cards** in chat, with icon, cover, online members and a join button. Contributed by @starzynhobr (#54, #55); the cache keeps the last 32 codes with the images already at card size and is emptied in the tray.
- **Forwarded messages**, media banners, voice messages and the other Discord message types. Contributed by @stickyfran (#62).
- Channel mentions with the real name and clickable (#56), `[text](<url>)` and `<url>` links and text that wrapped letter by letter (#46), day separators that change with the language (#50) and a retry of the server list when Discord fails for a moment (#52). Contributions by @starzynhobr.
- **Opening an already visited server got faster**: the permissions of the last 8 servers are kept for up to 5 min and discarded when roles or the member itself change; the channel list is checked in ~390 ms instead of ~530 ms. Based on #61 by @stickyfran.
- **Stylized letters (𝗥𝗨𝗟𝗘𝗦, 𝘋𝘪𝘤𝘢𝘴) now appear in messages, embeds, author names and channels that start with an emoji**; before, they were blank.
- Scrolling the server list no longer drops to ~26 FPS with the mouse over it.
- `build.rs` only runs again when the commit changes (#49, @starzynhobr).

## [v1.3.0-beta] - 2026-10-20 — Light on resources for gamers (#27)

> **Required update.** Streams are now sent only to those who ask to watch, with a key drawn for each stream (protocol 3). Earlier versions can neither watch nor be watched by this one, so the signaling server now asks for the update.

**Summary:** this is a required update (screen sharing protocol 3). In the tray Litecord wakes up less than once a second; sending 1080p60 costs ~13% of one CPU core (was 45–49%) and watching ~8% (was 68%), because frames go from the screen to the GPU encoder and from the GPU decoder to the screen without touching the CPU. Nothing is sent or decoded while nobody is watching; every stream has its own key, and only the sender can end it. Also new: watch on demand, sharing a stream to browsers, GPU selection, a network diagnostic, servers in your Discord order with drag-and-drop, mouse buttons as mute/deafen shortcuts, richer bot messages and polls, and calls that survive Discord's reconnect requests. In a 5-hour session streaming and watching 1080p60, the CPU stayed stable and no frame was dropped. Thanks to @stickyfran and @PedroZamecki for their contributions.

### ⚡ Truly idle in the tray
- **Infinite UI loop with the window hidden**: any animation that started while the app was in the tray (someone joining a voice channel, an unread counter changing) made Slint request frames forever, ~130 times per second, until the window was reopened. The cause is in Slint's winit backend (also in 1.18.1); Litecord now uses a local copy of version 1.17.1 with the fix (`vendor/`).
- **The interface is drawn by the CPU by default.** Without a GPU context, the video driver does not keep threads waking up next to the game and private memory is cut in half. GPU rendering is still available in Settings → Video & Interface (takes effect on restart).
- Stopped waking up periodically: global shortcuts (they read the keyboard every 20 ms; now they wait for the key), the tray icon (120 ms) and its tooltip (500 ms), the popout bar (300 ms), the hidden hardware monitor, P2P reception, the decoder and signaling outside a call, the timer governor, the memory cleanup every 60 s.
- With the window hidden, messages, voice counters and DM order are only noted down; on restore, everything is applied at once and the open channel is reloaded (before, this only happened during a call).
- The Discord connection reads only the type of each event and discards without parsing the ones Litecord does not use (presences, typing, reactions from other servers...).
- Result (Windows 11, outside a call): in the tray, from 338 to **0.9 wake-ups per second** and 0.00% CPU; window open at 0.08% CPU; ~10 MB of RAM in the tray and 16–25 MB with the window open.

### 🎙️ Voice and calls
- **Quiet speech preserved on the receiver**: removed the cutoff that zeroed whispers and word endings, along with the spectrum calculation and the per-packet log. The speech indicator keeps its own separate calculation. Contributed by @stickyfran (#36).
- **Less accumulated old audio**: the per-participant queue is now limited to 600 ms, including loss-recovery frames, keeping the 40 ms pre-buffer. The voice and microphone queues are cleared when leaving the call. Contributed by @stickyfran (#36), adapted to preserve the audio after playback stutters.
- **Voice buffers on Linux**: nominal period of 20 ms, aligned to 64 frames and to the device limits, with probing and a fallback to the driver default when refused. The Windows buffer configuration stays the same. Contributed by @stickyfran (#36).
- **Calls no longer drop when Discord asks for a reconnect.** Litecord used to reconnect by opening a new session, and about 1m45s later Discord removed the account from the call (and ended the stream) because the session that had joined it was gone. This happens on its own in long calls, with no network failure at all. The session is now resumed: in a test cutting the connection on both sides in the middle of a stream, it came back in 2 s and nobody left the call.
- **Voice sending at the microphone's pace** instead of a 20 ms timer: the Windows 1 ms timer is no longer used during calls. In the end-to-end test, the longest interval between speech packets dropped from 131 ms to 22 ms.
- **Microphone closed on leaving the call**: it kept capturing until the app was closed, with the Windows microphone indicator on.
- **Joining a channel no longer freezes the interface**: the microphone is opened on its own thread (with a wireless headset this could take seconds).
- The voice panel is only redrawn when someone joins, leaves or changes volume; the microphone meter runs at 15 FPS and only with settings open, and noise below the voice threshold no longer wakes the interface.
- With no audio output device, the call searches again with growing wait times (up to 5 s) and warns once, instead of every half second.
- The 1 ms timer and the stream audio output stayed on forever after the first call; they are now released on leaving.

### 📡 Streaming and signaling
- **GPU choice on Windows.** Automatic uses the card connected to the captured monitor, leaving the dedicated one available for the game when the screen is on the integrated one. Settings also let you choose an Intel, NVIDIA or AMD card present in the computer, or the CPU; they show the card and method actually used and explain when the image may cross between GPUs. The choice takes effect on the next stream, and a saved card that no longer exists goes back to Automatic.
- **Streaming without the CPU touching the image.** The captured screen stays on the GPU: the card itself resizes it, converts the colors and hands it to the encoder, without the two GPU → memory → GPU trips per frame. Whoever streams 1080p60 with someone watching went from 45–49% to **11–19% of a core** (NVENC or Quick Sync); the image arrives with the same colors as the screen (maximum difference of 1 level). NVENC is now used directly through the driver API, without FFmpeg, and Intel-only laptops use Quick Sync. If anything fails, the stream falls back by itself to the previous path.
- Media Foundation (Quick Sync) streams now declare their colors in the stream; browsers and Litecord used to guess the matrix and one of the two saw slightly wrong colors. A frame that took longer than expected came out stuck to the next one and the receiver lost one of the two.
- **Watch on demand.** A new stream appears with the LIVE badge and the **Watch** button, as on Discord; nothing is received before that. The sender only sends to those who asked, and with nobody watching it encodes nothing and captures the screen only at the pace of its own preview: **from 25–33% to 5% of a core**. Hiding the stream, or closing or leaving Litecord in the tray or minimized, stops receiving; on return, the image reappears in 0.1–0.45 s.
- **Key per stream.** Before, video and audio were encrypted with a key derived from the channel ID, and anyone who knew it could join the signaling room, start receiving the stream and decrypt it; the receiver also accepted unencrypted images. Now each stream draws a key, delivered only to whoever asked to watch and is in the voice channel, encrypted for them (X25519). Details and limits in `docs/WATCH_ON_DEMAND.md`.
- **Quality that climbs back up.** The bitrate only dropped when there was loss and never came back. Now it starts full on each stream, drops 20% with loss above 8% and rises 0.5 Mbps every 5 s without loss, following the worst viewer at the moment; whoever leaves stops counting.
- Apart from the viewer and the sender, nobody exchanges heartbeats in the call; the local network broadcast only happens when the signaling server is down.
- **Share on the web**: link with a per-session key, H.264 video and Opus stream audio over WebRTC, up to three viewers without login. The page and signaling use the existing worker; the media goes directly to the browsers. Ending the web share, stopping the stream or leaving the call disconnects the viewers. No TURN: restrictive networks may not connect. The worker update must be published before use over the internet.
- Outside a call P2P reception sleeps completely (no heartbeat or broadcast); in a call without a stream, it wakes once per second.
- After leaving a call, signaling would rejoin the old room and the privacy filter accepted packets from it.
- Signaling wakes on events instead of checking every 500 ms, and the keepalive went from 15 s to 40 s.
- Dropped-frame warnings became a one-per-second summary.
- **With nobody watching and the preview hidden, screen capture stops completely** (the Windows Graphics Capture session is closed and DXGI duplication stops requesting frames): the sender stays at 1.7–2.2% of a core with Litecord in the tray. The encoder is opened as soon as capture starts, and the first image arrives 0.24–0.54 s after someone clicks Watch.
- **Stream audio goes out as Opus** (64 kbit/s) instead of uncompressed PCM (768 kbit/s).
- **Only the sender can end their own stream.** The end notice now carries a proof made with the stream key; before, anyone who knew the channel ID and a user ID could take their stream down for everyone with a single packet. A presence announcement saying someone stopped, or a notice from the signaling server that they left, are also ignored while their video keeps arriving with the right key.
- Browsers watching over the web heard the stream audio at the wrong volume and distorted: the mono Opus encoder of the library in use is broken. The audio is now encoded in stereo (with both channels equal).

### 🩺 Network diagnostic
- Settings → Network & Proxy gained a **network diagnostic**: it tests whether streaming with someone on another network can connect directly (NAT type via the STUN of three providers, blocked UDP, signaling server) and copies a report to paste into an issue. The report hides the last two numbers of the public IP.

### 🎞️ Watching streams
- **On Windows, the stream is decoded and displayed by the GPU without going through the CPU.** The card's decoder (Intel, NVIDIA or AMD) receives the video directly, without Media Foundation, and the image goes to the screen on a Windows layer beneath the interface, with the stream controls drawn on top. Watching a 1080p stream at 60 FPS went from **68% to 8% of a core** (fullscreen, from 106% to 8%). It applies to the main window and the detached window.
- **Nothing is decoded when no window shows the stream** (Litecord in the tray or minimized during a game): 2–3% of a core, against 53% before. On return, the image reappears in about 0.1 s (0.5 s coming out of the tray).
- When the GPU cannot decode the stream (virtual machine, driver without an H.264 decoder, interlaced video), it continues on the CPU as before, now converted already at the size it is displayed (from 68% to 46% of a core). The same applies to those using GPU interface rendering, which does not use layers.
- The colors of streams made with NVENC were declared as full-range BT.709, but Litecord converts them to limited-range BT.601; anyone trusting the declaration (browsers watching over the web, the GPU decoder) saw grayish blacks. The declaration was fixed, and the receiver handles streams from old versions.
- **Streaming with the Media Foundation encoder** (chosen by hand in Settings → Video) did not work: it wrote an MP4 file and sent the new bytes, and the header with the video parameters only exists when the file ends. Now the card's encoder (Intel Quick Sync, NVIDIA or AMD) is used directly, with keyframes on demand.
- The quality label shows the stream resolution again (it was showing the size at which it was drawn).
- **Hiding and watching again no longer accumulates memory, and hiding gives everything back.** The GPU device was released before everything created on it (layers, swap chain, video processor), so it was never destroyed: each time you went back to watching left 30–50 MB behind, and in 10 cycles the receiver went from 154 to 686 MB. Also, the main window re-enabled the layers of a stream that had just been hidden. Now everything is destroyed in the right order, waiting for the Windows compositor to release the layer tree: on hiding, memory goes back from 174 to 66 MB in a little over a second (56 MB before watching), and in 15 hide-and-watch cycles it stays between 187 and 193 MB. Opening and closing the popout also stopped accumulating (~10 MB each time before).
- **Less memory overall**: the allocator now reserves memory only when it is used. With a 1080p60 stream, the viewer uses 164–190 MB instead of 228–247 MB and the sender 271 MB instead of 310 MB, with the same CPU; outside a call the app starts with 40 MB instead of 55 MB.
- The decode queue dropped from 120 to 30 frames (0.5 s): when the receiver falls behind, the image is no longer up to 2 s behind the stream. Stutters of up to half a second pass without dropping anything.
- When frames are dropped, the keyframe request now goes to **the sender** (it used to go to the local encoder), and the receiver waits for that frame instead of decoding corrupted images.

### 💬 Interface
- **Servers in the same order and folders as Discord**, and servers and channels can be reordered by dragging (the order is saved on this computer). Contributed by @stickyfran (#39).
- **Opening a server became instant.** The chat used to wait for the member list and the bot commands, which are not needed for reading: messages now start loading together with the channel list (from ~1.1 s to ~1 ms after it), and members and commands come in the background. An already visited server shows the channels and the chat immediately, and Discord's response only corrects the list. Channels and permissions are fetched at the same time. Switching servers before the previous one finishes loading no longer leaves the previous one's channels on screen, and a channel opened during loading is not replaced by the first channel. Idea by @stickyfran (#38).
- **Mouse buttons as shortcuts** (right, middle and side buttons) to mute and deafen, on Windows. With no mouse shortcut configured, nothing changes: the shortcuts thread keeps sleeping until a key. With one, the buttons are read every 30 ms (0.1% of a core); listening to the mouse through Raw Input would wake the app on every movement, up to 1000 times per second on a gaming mouse. Idea by @PedroZamecki (#31).
- **User bar like Discord's**: avatar with status indicator, display name and @username. Contributed by @stickyfran (#40).
- **Bot messages, polls and markdown**: messages with only embeds, images or links no longer appear as "[Special content]"; polls show the question and the options; headings, quotes, lists, strikethrough text and bold links are displayed; Discord dates (`<t:...>`) follow the local time zone and the app language. Contributed by @stickyfran (#37).
- Avatars and server icons are rounded on the loaded image, fixing the square clipping of the software renderer.
- Server channels no longer jump to the top of the list on every message (only DMs are sorted by activity).
- Updating voice counters no longer undoes the DM order or erases unread counts.
- With CPU rendering (the new default), the window came back from the tray or from minimized transparent, showing only what moved (the call cards, the CPU and RAM counters). Windows erases the contents of a hidden window and does not tell the renderer; it is now redrawn entirely on return.
- The border of the stream card is drawn over the video: before, the image covered the border where it touched, and the video corners stuck out of the rounded border.

### 🐧 Linux
- Global shortcuts wait for keyboard events (`poll` on evdev) instead of reading the keys every 20 ms.
- 20 ms voice buffers negotiated with the device (see Voice and calls). Contributed by @stickyfran (#36).
- These changes compile and pass CI, but have not yet been tested on a real Linux machine: reports are welcome.

### 🧪 How this version was tested
- **New tools** (`scripts/perf/`, test builds only): per-thread wake-up counting, idle and in-call cost, and end-to-end tests that run two accounts side by side on the same PC.
- **End-to-end voice**: a reference speech sample goes through Discord and is compared with what the sender captured: median similarity 0.99, 2 of 283 segments damaged, 0.2% loss; one packet every 20 ms (maximum 21 ms). At the old timer-driven pace, the longest interval reached 131 ms.
- **In a call with a real microphone**: CPU from 13.6% to 0.6% of a core with the window open and from 4.0% to 0.4% in the tray.
- **Slow receiver**: simulated stutters of 300 ms pass without dropping anything; at 800 ms, a single keyframe request and back to 60 FPS in the following second.
- **Streaming**: opens instantly on the other client, 1080p at 60 FPS. Receiver CPU counted in cycles (Windows time accounting leaves out threads that wake up briefly, such as a 60 FPS one): 68% of a core before, 46% with conversion at the displayed size, 34% decoding on the GPU and copying back, **8% with layers**; fullscreen, 103–110% on the CPU against 8%. In the tray or minimized, 2–3%.
- **GPU decoder**: compared frame by frame with OpenH264 and ffmpeg on Intel and NVIDIA cards, with videos from x264 (several profiles, B-frames, multiple references and slices), NVENC, Quick Sync, Media Foundation and OpenH264 configured as in the app: all identical. On packet loss, it stops showing images until the next keyframe, without showing corrupted images.
- **Long session**: 5 hours streaming and watching 1080p60, with 60 trips to the tray and popout openings: the viewer stayed at 8–9% of a core and the sender at 12–13% from start to finish, 60 FPS and no dropped frames in any measurement; the sender's memory stayed flat (268–273 MB) and the viewer's oscillated between 171 and 198 MB.
- **Memory**: hiding and watching again, opening and closing the popout, and going to the tray, 15 times each, with no growth (`watch_memory.ps1`); on hiding the stream, memory goes back from 174 to 66 MB. The Direct3D debug layer confirms that nothing of the video device stays alive afterwards.
- **Security**: forged stream-end packets, forged presence and forged leave on signaling (on a local signaling server) while the stream is being watched: the image continues; the real end closes in 0.01 s (`stream_stop_auth.ps1`).
- **Colors**: from the screen to the receiver, maximum difference of 1 level out of 255.
- 105 unit tests (including counting H.264 reference frames with synthetic videos, web streaming negotiation, voice queue dropping and channel dragging) and 6 worker tests. The scroll animation fix in Slint's core has a test in the crate itself; the hidden-window loop fix was verified by measuring the app (132 → 0 wake-ups/s).

### ⚠️ Known limitations
- Measured only on a laptop with Intel and NVIDIA: the AMD GPU encoder and decoder were not tested on a real AMD card (without a compatible GPU, Litecord uses the CPU, as before).
- Streams between different networks depend on the direct connection getting through the NAT on both sides; there is no relay server (TURN). The network diagnostic helps show why, if it does not connect.
- Sharing on the web: up to three viewers, and very restrictive networks may not connect.

## [v1.2.0-beta] - 2026-10-12 — New P2P signaling, grouped chat, profile pictures & fixed Linux install

> **Required update for screen sharing over the internet.** The signaling server now only accepts this version's protocol; v1.1.0 and older clients keep voice, chat and LAN streaming, but cannot find peers over the internet until they update.

---

### 📡 P2P signaling and screen streaming
- **Event-driven presence**: the client announced presence every 2 s throughout the call, which exhausted the signaling server. It now announces on joining, when another participant joins, when the stream starts or stops, and when the network address changes.
- **Signaling server** (`cloudflare/`): the code is now part of the repository, uses WebSocket hibernation and notifies the room when someone joins or leaves. The ID of the user who left arrived corrupted (a JSON number above 2^53) and is now sent as text.
- **Stream end**: frames still in transit after stopping reopened the preview frozen on the last frame, with nothing to close it.
- **Slow receivers**: a receiver unable to decode could request several keyframes per second and bring down the sender's frame rate; requests are now limited to one per second on both sides.
- **Logs**: the record of every received packet (93% of a 48 MB log) became a summary every 10 s; heartbeat and public address are only logged when they change.
- Reconnection to the signaling server with growing wait times (2 s to 60 s) and an address configurable via `LITECORD_SIGNALING_URL`.

### 💬 Chat & profile pictures
- **Grouped messages** (PR #26, by **Starzinho — [@starzynhobr](https://github.com/starzynhobr)**): consecutive messages from the same author are grouped, with day separators and persistent avatars; rows are updated incrementally, without recreating the list. All the design and implementation of the grouping and of the avatar/name hierarchy are his.
- **Profile pictures in more places** (#25): DM list, voice members in the sidebar and the voice panel. Reuses the existing avatar cache (40x40 thumbnails, memory and disk limits) and makes no new requests.

### ⚡ Performance and background (#27)
- The hardware monitor only queries the GPU counter with the window visible and the HUD on; in the tray it samples CPU/RAM every 10 s.
- The system thread profiler became optional (`LITECORD_CPU_PROFILER=1`).
- The Windows 1 ms timer resolution is only kept during voice calls and streams; the stream audio output is only opened when audio is arriving.
- The P2P receive thread reduces polling when idle.
- Measured in a synthetic benchmark: idle cost dropped from +2.0% to +1.2% over the baseline; in voice it stayed at ~+5.3% (v1.1.0-beta: +6.0%). Effect in real games not yet measured.

### 🪪 Client identification (#30)
- REST, Gateway and QR login declared three different environments (Electron/Chrome 120, Chrome 127 and Chrome 130), always as Windows and `pt-BR`. A single module (`utils/client_identity`) now generates the `User-Agent`, the Gateway properties and a matching `X-Super-Properties` header from the same constants, as a web client on Chrome, with the real operating system and language.
- Optional fields that the official clients generate on every launch (`launch_signature`, session identifiers) were left out on purpose.

### 🌍 Languages (#29)
- About 190 texts that were hard-coded in Portuguese (login/QR, settings, screen sharing, accounts, updates and error messages) now follow the chosen language.
- English and Portuguese are complete; Spanish, German, French, Russian and Japanese are included as machine translation, to be reviewed by native speakers.

### 🐧 Linux
- **install.sh on Fedora/Nobara** (#28): packages are now installed one by one, with the correct names; before, a failure in an atomic `dnf` install left everything uninstalled, silently.
- **AnyLinux AppImage** (#21, #28): workflow rewritten according to the guide (Arch container, `quick-sharun`, OpenGL/PipeWire/GTK/GStreamer bundled). The AppImage generated by GitHub Actions was validated on a Fedora 43 KDE Wayland VM (it opens and starts).

### 🔧 Dependencies
- `futures-util` 0.3.34 (PR #15), `windows-sys` 0.61.2 (PR #17), `x25519-dalek` 3.0.0 (PR #14), `sha2` 0.11 (PR #13) and `openh264` 0.9 (PR #16).

### 🧪 How this version was tested
Tests run on 11 and 12/10/2026, with Windows 11 (RTX 3050, NVENC) on one side and a Fedora 43 KDE Wayland VM (no GPU, 2 vCPUs) on the other, on different accounts.

- **Automated tests**: 28 unit tests (language tables, date format per language, message grouping, client identity).
- **Signaling server**: 16 protocol tests against the production server (room, participants joining and leaving, relay without echo, keepalive, isolation between rooms, old clients parked).
- **Signaling server consumption** (measured on Cloudflare): from ~960 GB-s to under 0.01 GB-s per 15 minutes; a room with two connected clients stayed at thousandths of a GB-s.
- **Windows ↔ Linux screen streaming**: in both directions, with start, stop and an immediate restart; continuous sessions of 12 and 7 minutes, with no memory growth and the Windows send steady at 60 FPS.
- **CPU cost of streaming on Windows**: 92% of a core with OpenH264 (CPU), ~1–2% with NVENC after the log reduction.
- **Log size**: from 48 MB to 0.27 MB in an equivalent session.
- **Login**: saved token, token detection and QR code (QR validated on Linux), with the new identity, no authentication error.
- **Voice**: calls in a server between the two systems.
- **Linux**: `install.sh` and the AppImage generated by GitHub Actions open and connect on Fedora 43 KDE Wayland.
- **Languages**: language switching checked on real screens in German, Russian and Japanese.
- Optional build feature `automation` (off by default and absent from published binaries), used to drive the interface in these tests: window capture, clicks, keyboard and state reading, only on `127.0.0.1`.

### ⚠️ Not validated
- Peers on different networks (both sides of the test went out through the same public IP) and more than two participants in the same room.
- Linux with a graphics card: the VM renders and decodes in software, so the Linux performance numbers (memory, receive FPS) do not represent a real machine.
- QR login on Windows with the new identity, voice in DMs and prolonged use of `openh264` 0.9.
- Effect of the #27 changes in real games (only measured in a synthetic benchmark).
- The Spanish, German, French, Russian and Japanese translations were not reviewed by native speakers.

### 🙏 Credits
- **Starzinho ([@starzynhobr](https://github.com/starzynhobr))** — author of PR #26 (grouped messages, day separators and persistent avatars).
- Everyone who reported the problems fixed in this version: #21, #25, #27, #28, #29 and #30.

---

## [v1.1.0-beta] - 2026-09-23 — Direct Messages (DMs), Private Voice Calls & AnyLinux AppImage

This version introduces full native support for **Direct Messages (DMs)**, **Private Voice Calls (DM Voice Calls)**, unread message counters and badges with real-time synchronization, and the universal **AnyLinux (distro-agnostic) AppImage** packaging via GitHub Actions.

---

### 💬 Direct Messages (DMs) & Group Conversations
- **DM Navigation and Interface**:
  - New DMs button at the top of the server sidebar with a dedicated vector icon (`assets/dm-chat.svg`).
  - Side list with support for individual (1:1) channels and groups (Group DMs).
  - Avatars with colored initials for each user/group, presence status and updated names.
- **Real-Time Gateway Synchronization**:
  - Private channels loaded via the REST API with dynamic message updates through Discord Gateway v10.
  - Automatic reordering of the list when new messages arrive.

### 📞 Private Voice Calls in DMs (Ring & Call Management)
- **1-Click Voice Call**:
  - Phone call button (`assets/phone.svg`) placed directly in the header of the active DM.
- **Incoming Call Modal**:
  - Animated visual notification overlay showing the caller's name and avatar.
  - Dedicated **Answer** (green) and **Decline** (red, `assets/phone-slash.svg`) buttons.
  - Loop/self-call prevention: when starting a call, the originator joins the voice channel directly without triggering the modal for themselves.
  - Sound feedback for ringing and for answered/ended calls.

### 🔴 Unread Message Badges & Accurate Synchronization
- **Aggregated Global Badge**:
  - Counter shown over the DMs icon in the server bar indicating the total unread messages across all conversations.
- **Individual Badges per DM**:
  - Red pill-shaped badge with the individual count next to the conversation name.
  - Smart formatting for high numbers (`99+`).
  - Geometric alignment and optical centering identical to the user-count badge of voice channels.
- **Smart Message Deduplication**:
  - Control cache to prevent spurious counts caused by multiple firings of `MESSAGE_CREATE` events.
  - Automatic reset and immediate removal of the badge when selecting and viewing the conversation.

### 🎨 Light Theme, Participants in Voice Channels & Permission Filter (by @starzynhobr — PRs #22, #23)
- **Native Light Theme**:
  - Light Theme selector in the app Settings with automatic persistence in the user's settings file.
  - Centralization of semantic color tokens in Slint's `AppTheme` module for visual consistency.
- **Participant View in Voice Rooms**:
  - Users present in voice channels are shown directly in the sidebar before connecting.
  - Compact rows with an initials avatar and the user's name.
  - Clicking the participant's row automatically connects to the corresponding voice room.
  - Fingerprinting mechanism to detect members joining/leaving without unnecessary reloading.
- **Channel Permission Filter (`VIEW_CHANNEL`)**:
  - Local calculation of Discord permissions respecting the official precedence (owner/administrator, roles and channel/member overwrites).
  - Inaccessible channels are automatically hidden from the sidebar and empty categories are removed, eliminating visual clutter.
- **Visual Clarity and UX Improvements**:
  - Flatter, cleaner surfaces on channels and messages, eliminating the effect of excessive boxes.
  - Hand pointer cursors applied to buttons and interactive controls.
  - Native draggable scroll bar (`ScrollView`) in the channel sidebar.
  - Modularization of Slint components into dedicated files (`channel_sidebar.slint`, `chat_messages.slint`, `settings.slint`, `icon_button.slint`).

### 🐧 AnyLinux Universal AppImage & CI/CD
- **Universal Distribution for Linux**:
  - Automated generation of the `Litecord-x86_64.AppImage` package on GitHub Actions through `quick-sharun`.
  - Guaranteed to run on any modern or old x86_64 Linux distribution (Ubuntu, Debian, Fedora, Arch Linux, Void, etc.), without `glibc` conflicts.
  - All shared dynamic libraries included directly in the bundle (including `libxdo.so.3`, `libayatana-appindicator3`, `libfontconfig`, `libasound`).
  - Desktop integration with a `.desktop` file (`assets/litecord.desktop`) and the official high-resolution icon (`assets/litecord.png`).

---

## [v1.0.0-beta] - 2026-09-08 — Litecord Beta 1.0.0

This version marks the **official transition from the Alpha cycle (v0.1.0 – v0.3.9) to the Beta 1.0.0 phase**, consolidating a deep restructuring of the application architecture, a new zero-latency hardware screen streaming engine (Sunshine / Moonlight standard), process audio isolation, native proxy support, global shortcuts and a real-time consumption monitor.

---

### 🚀 Modular Architecture & Hardware Video Engine (Sunshine / Moonlight Standard)
- **Architectural Restructuring into Domain Modules**:
  - Source code fully modularized from monolithic modules into cohesive domain directories: `src/audio/`, `src/auth/`, `src/encoder/`, `src/gateway/`, `src/screen_capture/`, `src/ui/` and `src/utils/`.
  - FFmpeg/NVENC runtime libraries embedded directly for standalone builds and execution without externally installed dependencies.
- **Universal GPU Video Acceleration**:
  - Native hardware encoding engines: **NVIDIA NVENC** (via Direct3D 11 and dedicated libraries), **AMD AMF** (with low-latency tuning), **Intel QuickSync (QSV)**, **Windows Media Foundation (WMF)** and an optimized fallback via **OpenH264 SIMD AVX2**.
  - **Zero-Latency Tuning (Sunshine / OBS Standard)**:
    - *Constrained Baseline* profile configuration with `max_b_frames = 0`, `filler_data = 0` and AUDs disabled.
    - Keyframe interval (IDR/GOP) set to 30 frames (500ms to 1s), eliminating accumulated image artifacts.
    - Strict Annex B NAL normalization with two-ended extraction, persistent caching of SPS/PPS headers and atomic injection **exclusively on IDR frames** (preserving the integrity of DPB references on P frames).
    - Instant recovery mechanism via Picture Loss Indication (PLI) and Full Intra Request (FIR) through `OP_KEYFRAME_REQ` routed to the active peers.
    - Fault-tolerant video decoder that survives network fluctuations and frame drops without screen freezes or destructive restarts.
    - Continuous 60 FPS pacer (CFR - *Constant Frame Rate*) and adaptive dynamic bitrate (default 4.5 Mbps with micro-pacing).
    - Parallel BGRA to NV12 color space conversion vectorized with Rayon/SIMD.

---

### 🌐 High-Performance P2P WAN Streaming & New Signaling
- **Migration to Cloudflare Worker WebSocket Signaling**:
  - Replacement of traditional MQTT signaling with ultra-low-latency WebSockets on Cloudflare Workers, eliminating broker overhead, disconnections and *ghost streams*.
- **Dynamic WAN Discovery via STUN & P2P Connectivity**:
  - Precise discovery of public addresses and NAT-mapped ports via dynamic STUN, eliminating port guessing and improper overlaps.
  - Safe MTU enforced at 1200 bytes per UDP packet to avoid fragmentation on WAN routers and VPNs.
  - Proactive filtering of virtual network adapters (VPN, Hyper-V, WSL) and dropping of loopback packets carrying the own UID in the P2P mesh receiver.
  - Authenticated end-to-end encryption (E2EE) with ephemeral X25519 ECDH + AES-256-GCM keys, adding less than 0.07ms of overhead per frame.

---

### 🔊 Advanced Audio Subsystem & Sound Isolation (Loopback)
- **Share Audio Isolation on Windows (WASAPI Process Loopback)**:
  - Native audio capture of games and shared windows isolating specific processes via WASAPI Process Loopback: it streams the game/video sound without capturing the voice call or the microphone itself (zero echo).
- **Audio Routing on Linux (PipeWire / ALSA Virtual Sink)**:
  - Isolation of desktop system audio in screen streams by creating a dynamic virtual sink, separating the audio monitor from the app's voice playback.
  - Direct routing of Litecord's ALSA streams to the physical output (`PIPEWIRE_NODE`).
- **Dynamic Speaker Switching During a Call**:
  - Switch between headphones, speakers and USB DACs in the middle of a call without disconnecting, without dropping the room and without restarting the app.
  - Atomic recreation of the CPAL playback stream in the voice channel, preserving the jitter buffer state and the DAVE E2EE session.
- **Microphone Gain & Sensitivity Calibration**:
  - Real-time software microphone gain slider adjustable from 0% to 200%.
  - Intuitive reorganization of the Voice and Audio tab in settings with a live VU monitor and VAD sensitivity.
- **Tactile Sound Feedback (Sound Effects)**:
  - Tactile sound synthesis engine (`sound_effects.rs`) running on a dedicated low-latency thread via CPAL (with native fallback on Windows):
    - **Mute**: descending harmonic beep (440 Hz → 220 Hz) confirming muting.
    - **Unmute**: ascending harmonic beep (330 Hz → 660 Hz) confirming microphone reactivation.
    - **Deafen / Undeafen**: dedicated sound sequences for deafening and restoring audio.
    - **Joining and Leaving a Call**: sound alerts on joining or leaving voice rooms.

---

### ⌨️ Global Keyboard Shortcuts (Global Keybinds)
- **Game-Focused Global Capture (Win32 / Linux evdev)**:
  - `src/utils/keybinds.rs` module with a background listener working on Windows (Win32 hooks and `GetAsyncKeyState`) and on Linux (`evdev`), responding even with games running in exclusive fullscreen.
  - Default shortcuts: **`Ctrl + Shift + M`** (Toggle Microphone) and **`Ctrl + Shift + D`** (Toggle Audio).
  - Interactive shortcut recorder and persistence of custom combinations in `.litecord_keybinds.json`.
  - Real-time synchronization between the UI state, sound confirmation and dispatch to the Discord Voice Gateway (`GatewayCommand::UpdateVoiceState`).

---

### 🌐 Native HTTP/HTTPS, SOCKS5 & System Proxy Support
- **Multi-Protocol Network Engine**:
  - Dynamic proxy injection in Rust into `reqwest::Client` with support for **HTTP/HTTPS** (`CONNECT` tunnel), **SOCKS5** (compatible with Shadowsocks, V2Ray, Xray, Tor) and **System Proxy**.
  - Basic Auth (username and password) for corporate and private proxies.
  - Integrated **"Test Connection"** button in settings with real-time verification of latency and connectivity to the Discord gateway endpoint (`discord.com/api/v10/gateway`).
  - Hot reload (*zero downtime*): saving settings rebuilds the HTTP client instantly without closing the application.
- **Smart Connection Diagnostics on the Login Screen**:
  - Elastic proxy failure warning banner with a clear distinction between network failures (`10061`, connection refused, DNS) and credential rejection (`401 Unauthorized`).
  - 1-click shortcut in the banner leading directly to the Network & Proxy tab in settings.

---

### 📊 Real-Time Hardware Monitor & Compact HUD
- **Faithful Process Metrics (Real Litecord Consumption)**:
  - **CPU**: Collects the kernel and user time of the Litecord process via `GetProcessTimes` relative to the total elapsed system time (`GetSystemTimes`), matching the Windows Task Manager calculation faithfully.
  - **RAM (in MB)**: Collection of the real Working Set consumed by Litecord via `K32GetProcessMemoryInfo` (returns in MB, e.g. `42 MB`), without a confusing global percentage.
  - **GPU**: Collection via PDH (`\GPU Engine(*)\Utilization Percentage`) filtering strictly by the engine instances belonging to the current PID (`pid_<PID>_...`), isolating the app's real usage.
- **Taskbar and Tray Integration**:
  - Dynamic, compact title in the Windows Taskbar: `Litecord - x%/ymb` (e.g. `Litecord - 2%/45MB`), without being cut off in the taskbar.
  - Live tooltip in the system tray: `Litecord - CPU x% | RAM y MB | GPU z%`.
  - Minimalist, elegant HUD integrated into the top bar next to the settings icon, with on/off toggle support in settings.

---

### 🧹 Advanced Memory Management & Linux Parity
- **Cross-Platform `trim_process_memory()` Routine**:
  - On Windows: working set emptying via `K32EmptyWorkingSet()`, dropping RAM usage to under 5 MB when minimizing to the tray or going idle.
  - On Linux: calls to `malloc_trim(0)` and `mi_collect(true)` to force the immediate return of defragmented heap memory pages to `glibc`.
  - Deactivation of background timers in Slint when closing popout windows and minimizing, allowing the application to hibernate in `epoll_wait()`.

---

### 📹 Isolated-Thread Camera Pipeline
- **Safe Webcam Initialization**:
  - Camera devices initialized in an isolated worker thread with their own COM context, preventing apartment model collisions with the main thread.
  - Dynamic buffer sizing for the display canvas, safe timeouts and prevention of improper GDI capture fallbacks during video streaming.

---

### 🎨 Design, SVG Vector Icons & Cyber Sapphire Identity
- **Standardization of SVG Vector Icons**:
  - Replacement of emojis and unicode characters prone to rendering failures (tofus/empty squares) with crisp SVG vector icons (`microphone.svg`, `headphones.svg`, `rocket.svg`, `chevron-down.svg`, etc.).
  - Microphone test button with an animated visual toggle between idle and testing states.
- **Usability & Style Improvements**:
  - **Cyber Sapphire** palette with Discord-classic squircle-style badges and a compact (20px) economical preview notice with smart truncation (*elide*).
  - Automatic chat scroll to the bottom when switching voice rooms or text channels.
  - Direct access button to Settings in the title bar on all screens (including Login).
  - 1-click **"Regenerate QR Code"** button to restart expired mobile authentications without closing the app.

---

## [v0.3.9] - 2026-08-30

### 👥 Multi-Account Vault & Instant Account Switching
- **Encrypted Multi-Account Vault (`AccountVault`)**:
  - Securely stores multiple Discord accounts simultaneously in `%APPDATA%/Litecord/session.vault` (Windows DPAPI) and `~/.config/litecord/session.vault` (Linux AES-256-GCM).
  - Preserves user metadata (User ID, Global Name, Username, Tag, Avatar Initials, and Active state).
- **Interactive Account Switcher Modal**:
  - Clicking the user profile bar in the bottom-left now opens the **Account Manager** modal.
  - Lists all saved accounts with instantaneous 1-click **"Switch"** without re-scanning QR codes or re-authenticating.
  - Includes **"➕ Add Another Account"** to easily authenticate and register secondary accounts into the vault.
  - Includes single-account removal trash buttons and **"Sign Out of All Accounts"** to wipe all credentials on full logout.

### 🛡️ End-to-End Encryption (E2EE) & Security
- **X25519 ECDH + AES-256-GCM P2P Cryptography**:
  - Integrated ephemeral Curve25519 Diffie-Hellman (`x25519-dalek`) key exchange directly into P2P video streaming signaling.
  - Generates zero-knowledge session keys in RAM: third parties or eavesdroppers with the Discord Channel ID (`cid`) cannot decrypt video packets.
  - Authenticated AEAD encryption (AES-256-GCM) with 12-byte random nonces and 16-byte integrity tags, rejecting forged or tampered packets instantly.
  - Sub-0.07ms encryption/decryption per frame utilizing hardware AES-NI instructions (< 0.05% CPU impact at 1080p 60 FPS).
- **Anonymous & Encrypted MQTT Signaling**:
  - MQTT topics are derived via SHA-256 hashes (`litecord/sig/<hash>`), keeping room identity obscured from outside observers.
  - Presence payloads (IPs, ports, user IDs) are 100% encrypted with AES-256-GCM before transmission over the signaling broker.

### ⚡ Screen Share Engine & Stability
- **Sub-0.2ms Local Preview Downsampler**:
  - Replaced scalar pixel loops with a fast SIMD box-downsampler to 480w, eliminating UI thread latency and preview stuttering.
- **Strict Bounds Clamping (`fit_bgra_to_canvas`)**:
  - Added strict coordinate and destination clamping in the DXGI frame scaler, eliminating slice out-of-bounds panics on odd-aligned monitor resolutions and DPI scaling.
- **Resilient Full-Mesh UDP Routing**:
  - Transmitters broadcast simultaneously to local port clusters (`127.0.0.1:50005..=50007`), LAN broadcasts, and remote WAN endpoints.
  - Extended watchdog timeout to 3000ms with per-chunk activity renewal, ensuring rock-solid stream persistence.

### 🎨 UI & Design Fixes
- **Unified Vector SVG Collapse Chevrons (`chevron-down.svg`, `chevron-right.svg`, `chevron-up.svg`)**:
  - Replaced system Unicode arrows (`▸` / `▾` / `▼`) with crisp SVG vector icons for categories and message links.
  - Fixes missing font glyph boxes (`□`) on Windows systems and provides smooth color transitions across all platforms.
- **Fixed Chat Header Channel Title Duplication**:
  - Sanitized active channel name formatting to prevent duplicate hashtag prefixing (`## channel` -> `# channel`).
- **Remote Stream Viewport Expansion**:
  - Decoupled remote video card visibility in Slint UI, guaranteeing instant video viewport rendering upon incoming frame arrival.

### 📦 Windows Installer & Updater
- **Automated Restart & Installer Relaunch**:
  - Removed `skipifsilent` flag in Inno Setup (`installer.iss`), ensuring the installer launches `litecord.exe` immediately after installation and in-app updates.
- **Continuous Logging (`litecord_app.log`)**:
  - Switched log file handle to append mode to preserve complete multi-instance telemetry diagnostics.

---

## [v0.3.8] - 2026-08-29

### 🛡️ Security & Token Hardening
- **Linux Encrypted Token Vault (`~/.config/litecord/session.vault`)**:
  - Implemented secure credential storage on Linux strictly following the XDG Base Directory specification.
  - Directories are created with `0700` and vault files with `0600` (exclusive user read/write access).
  - Credentials are encrypted at rest with **AES-256-GCM** using keys derived from `/etc/machine-id` and the user's UID (preventing token theft even if storage files are exfiltrated).
  - Seamless automatic migration from legacy `.litecord_token` files.
- **Windows DPAPI Vault Alignment**:
  - Encrypted tokens on Windows are now stored in `%APPDATA%/Litecord/session.vault` using Windows DPAPI (`CryptProtectData`).
- **P2P Video Streaming Cryptographic Authentication**:
  - Derived AES-256-GCM E2EE keys now mix Discord's authenticated `voice_secret_key` (negotiated via Voice Gateway Opcode 4).
  - Anonymous MQTT signaling topics are derived using SHA-256 hashes of the authenticated session key, ensuring only verified participants in the voice room can discover or decrypt screen share streams.
- **Strict Log Sanitization**:
  - Removed all token and session key prefixes from console and file logs (`litecord_app.log`).
- **Atomic Logout Credential Cleanup**:
  - Centralized `delete_secure_token()` handler to guarantee zero residual credentials on user sign-out.

### 🖥️ UI & Popout Window Improvements
- **Popout Window Responsive Controls Hierarchy**:
  - Decreased minimum window resize limits down to **180px × 120px** (mini Picture-in-Picture mode).
  - Implemented progressive hiding order as the window is shrunk:
    1. Volume slider (hides when width <= 420px).
    2. "LIVE" Live Badge (hides when width <= 340px).
    3. FPS Counter Badge (hides when width <= 270px).
    4. Username (hides when width <= 200px).
    5. Action control buttons (Ghost mode, Pin, Minimize, Maximize, Close) remain **100% visible and accessible**.
- **Embedded Video Card Dynamic Expansion**:
  - Clicking maximize/focus on the embedded stream card in a voice room now dynamically expands the video container to fill all available viewport height and width (`stage_height - 52px`) with zero clipping or overflow.
- **Main Window Maximize/Restore Button Toggle**:
  - Synchronized `is_maximized` state between the window manager and titlebar.
  - The maximize icon dynamically toggles between single square (`chrome-maximize.svg`) and restore double-square (`chrome-restore.svg`).

### ⚡ Portability & Robustness
- **Static CRT on Windows (`+crt-static`)**:
  - Windows release builds now statically link the C runtime (`target-feature=+crt-static`), allowing Litecord to run immediately on fresh Windows installs and VMs without requiring the Visual C++ Redistributable (`vcruntime140.dll`).
- **Automatic Software Renderer Fallback**:
  - Added automatic detection and fallback to Slint software renderer (`SLINT_BACKEND=software`) when hardware OpenGL/GPU drivers fail to initialize (common in Virtual Machines and RDP sessions).

---

## [v0.3.7] - 2026-08-28

### 🎙️ Audio & Voice Pipeline
- Enhanced Opus packet decoding and jitter buffer synchronization.
- Real-time VAD threshold testing with visual audio meters.
- Squad Leader / IGL Dynamic Speech Priority Ducking engine.

### 📺 P2P Video Streaming & DRM Bypass
- DXGI Desktop Duplication & Direct3D11 zero-copy hardware framebuffer capture.
- Isolated DRM-free browser launcher profile for movie nights.
- WASAPI in-game audio loopback mixer.

---

## [v0.3.0] - 2026-08-25

### ✨ Initial Release
- Native Slint UI with sub-30 MB RAM footprint and sub-0.1% CPU usage.
- DeepSleep System Tray background suspension (< 5 MB RAM).
- Discord Gateway v9 & DAVE voice encryption support.
- QR Code mobile login via Remote Auth v2.
- Smart slash commands autocomplete with keyboard navigation.
- Ephemeral image attachments and Twemoji Unicode support.
