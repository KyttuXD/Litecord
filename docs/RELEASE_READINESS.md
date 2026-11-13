# Pending items for the next release

Audit of 17/10/2026: recorded decisions, plan, code and GitHub.
This document records the state; it does not authorize publishing.

## Current decisions

- Keep a single worker. Finish the changes related to it and publish them together.
- Test the web part with a manual offer/answer or a local worker in the meantime.
- Leave VM/Linux and simulated NAT for the end of the plan.
- Do not measure 1% low, as decided in the plan.
- The v1.2.1 lightness batch was separated from the larger stages 4, 5, 6.2 and 7.
  The web part was brought forward afterwards. The larger stages must not be confused
  with the small fixes already completed, nor considered done just because they are in the plan.

## Already done

- Lightness batch: CPU renderer as the default with a GPU option, microphone on its
  own thread, event/visibility-driven interface updates, fixes for
  timers and devices that stayed active after leaving the call.
- Decode queue from 120 to 30 frames, keyframe request directed at the
  sender and growing wait when there is no output device.
- Network diagnostic with STUN, signaling and report.
- End-to-end voice and a call with a real microphone: PASS recorded in the
  earlier measurements; not repeated in this audit.
- Rounding of avatars/servers in the software renderer.
- Web: H.264 reused, Opus audio, three-viewer limit, per-session
  key, controls in the interface, encrypted signaling and manual test mode.
  Chrome validated locally, with and without the worker; publishing is still pending.
- Web share button reduced to a 32×32 px globe, with a description and
  viewer count on hover and an accessibility label.

## Larger implementations still pending

- [x] **Stage 4 — Receive video with the GPU** (Windows): DXVA decoding and
  DirectComposition layers; OpenH264 remains as the fallback. Linux (VA-API) not done.
- [x] **Stage 5 — Watch on demand** (local, [design](WATCH_ON_DEMAND.md)):
  watch request, sending only to whoever asked, per-stream key delivered with
  X25519, rate adjustment by the worst viewer, encoding paused without viewers.
- [x] Transition decided: protocol 3, mandatory update. The local worker already
  requires `v=3` (`MIN_PROTOCOL = 3`); publish **together** with the release, never before.
- [ ] **Stage 6.2 — Priority of non-audio threads.** Compare
  before/after with a game running; only adopt it if there is a gain without hurting video.

## Pending validations

- [ ] Real streaming between distinct networks, both native and web. Local STUN
  and the NAT test do not prove this scenario. Without TURN, some networks do not connect.
- [ ] VM/Linux: run global shortcuts and simulated NAT, left for the end.
- [ ] Build and test the current state on Linux. The previous CI does not cover the new
  WebRTC/Opus/interface changes that are still local.
- [x] Cost of web streaming with real 1080p60 capture, one and three viewers
  (`web_share_real.cjs`, Chrome/Edge/Firefox).
- [ ] Prolonged 2–3 h test: ran 101 min (stopped for lack of memory on the PC),
  CPU stable; the receiver's private memory rose from 222 to 277 MB, to be investigated.
- [ ] AMD hardware and other browsers: still not validated.
- [ ] Final lightness measurement after the latest changes, using `scripts/perf/`.

The cloud test pair was **option 2** of the diagnosis and remained pending; it is a
tool for testing other networks, not a fix that has already been authorized/implemented.
A direct comparison with TeamSpeak was not done either and is not a defined requirement.

## Closing the release on GitHub

- [ ] Review `CHROME_VERSION` and `CLIENT_BUILD_NUMBER` in
  `src/utils/client_identity.rs` with verified values, without inventing them.
- [ ] Consolidate and version the local changes, review the diff and run Linux/Windows
  CI for the final commit, including the distribution packages.
- [ ] Update the version in `Cargo.toml`, `Cargo.lock`, `installer.iss` and download
  labels; complete the date, tests and final text of the CHANGELOG.
- [ ] Review the performance claims in the READMEs: the CPU/RAM numbers come
  from this PC; the promises of zero FPS loss were not verified in games.
- [ ] Publish the worker once, together with the planned transition; validate native
  and web signaling after the deploy.
- [ ] Generate release artifacts **without the `automation` feature**.
- [ ] Prepare/post the issue about the Slint bugs and the #27 comment requesting
  testing on AMD. The [drafts](RELEASE_COMMUNICATION_DRAFTS.md) were
  preserved in this repository from the temporary scratchpad; review the texts,
  metrics and build link before posting. Nothing was posted in this audit.
- [ ] Create the tag/release only after closing out and validating the artifacts.

State verified on GitHub in this audit: the latest release is still
`v1.2.0-beta` (12/10). CI for the `perf/idle-lightness` branch passed on commit
`8b5933fb70a85756ae401e3c84ac2866c167858b`; that result predates the local HEAD
`871e2dd` and the changes not yet committed.

References: [plan](../plan.md), [changelog](../CHANGELOG.md),
[web streaming tests and usage](WEB_SHARING.md).
