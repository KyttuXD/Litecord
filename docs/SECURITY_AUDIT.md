# Pre-Release Security Audit Report (Security Audit)

**Date**: September 2026  
**Repository**: Litecord (`Ak4ai/Litecord`)  
**Branch**: `dev`  
**Scope**: Analysis of vulnerabilities, attack surface, credential storage, media/network processing and hardening recommendations for production versions.

---

## 1. Executive Summary

**Litecord** is a lightweight, native Discord client built with **Rust** and the **Slint GUI**. The pre-release security audit evaluated the application's security posture, focusing on:
1. Storage and handling of credentials / tokens.
2. Attack surface compared with Electron / Web based clients.
3. Handling of external links and operating system URI schemes.
4. Processing of attachments, images and custom emojis (DoS / OOM / Decompression Bomb risks).
5. Network, audio and real-time voice security (RTC / WebRTC / DAVE E2EE).
6. Automatic update mechanism (Self-Updater).
7. Interaction with native APIs and FFI (Linux X11/Wayland/PulseAudio and Windows Win32/WASAPI/D3D11).

### Audit Status
- **Overall Risk Level**: **Very Low**.
- **Critical Vulnerabilities / RCE**: None found.
- **Mitigations Applied Immediately**: Implemented protections against *Decompression Bombs* in image/emoji caches and fixed a buffer read in the Windows profiler.
- **Conclusion**: The system shows an excellent defensive posture and is fit for a release once the applied hardening is in place.

---

## 2. Architectural Analysis and Attack Surface

### 2.1. Rust + Slint vs. Electron / Chromium
Most alternative clients, and the official client, use Electron (Node.js + Chromium). Litecord eliminates this entire class of vulnerabilities:
- **No Web/DOM Engine**: There is no free-form HTML/CSS rendering, eliminating **Cross-Site Scripting (XSS)** vectors.
- **No JavaScript Interpreter**: Prototype pollution attacks, `eval()`, code injection via JS or insecure `nodeIntegration` bridging do not exist in Litecord.
- **Memory Safety**: The code is compiled with the strict memory and concurrency safety guarantees of the Rust compiler (`rustc`), preventing *use-after-free*, *double-free* and the vast majority of *buffer overflows*.

### 2.2. Token Management and Storage
- **Windows**:
  - Session tokens are encrypted using the native **Windows DPAPI** (`CryptProtectData`), binding the cryptographic keys to the user's account on the operating system.
  - Only processes running under the same local Windows user can decrypt the session.
- **Linux**:
  - Storage in a directory with strict POSIX permission `0700` (`~/.config/litecord/`) and a `0600` file (`session.vault`).
  - Authenticated encryption using **AES-256-GCM**, with key derivation via PBKDF2/SHA256 using the machine identifier (`/etc/machine-id` or `/var/lib/dbus/machine-id`), the user's UID and a static salt.
- **Zero-Token Logging**:
  - Filters at every log layer and in `println!`/`eprintln!` ensure that credentials and authentication tokens are never written to disk (`litecord_app.log`) or printed to the terminal.
  - Atomic purge on logout: the token file is deleted and removed from memory when the session ends.

---

## 3. Audited Vulnerabilities and Implemented Mitigations

During the audit, the following points of attention were identified and mitigated:

### 3.1. Decompression Bomb Mitigation in Attachments
- **File**: `src/attachment_cache.rs`
- **Scenario**: Manipulated images with disproportionate dimensions (e.g. 50,000 x 50,000 pixels with few compressed bytes) could force the allocation of gigabytes of RAM during decoding, causing *Out of Memory (OOM)* or freezing the user's machine.
- **Action Taken**: Added a prior dimension check (`ImageReader::into_dimensions()`) before full decoding:
  - Maximum limit of **8192px** for each of width and height.
  - Maximum limit of **25,000,000 pixels** (25 Megapixels) in the width x height product.
  - Files exceeding the limit are safely rejected without overloading memory.

### 3.2. Mitigation of Malicious Emojis
- **File**: `src/emoji_cache.rs`
- **Scenario**: Servers with custom emojis with inflated dimensions could cause severe performance degradation in chat.
- **Action Taken**: Added validation that rejects emojis with width or height above **1024px** before the full decode on the GPU/CPU.

### 3.3. Bounds Check in the Windows Thread Reader
- **File**: `src/cpu_profiler.rs`
- **Scenario**: The function that retrieves thread names on Windows (`GetThreadDescription`) used a length cast without checking the upper bound of the static 256-byte buffer, allowing an overread in atypical cases.
- **Action Taken**: Added an explicit `len < 256` clamp before iterating or indexing the buffer, ensuring full safety when processing native strings.

### 3.4. Restriction of Protocol Handlers in External Links
- **File**: `src/main.rs` (`open_url`)
- **Scenario**: Opening arbitrary links via chat could trigger dangerous OS handlers (`file://`, `ms-msdt:`, `powershell:`, `calculator:`, `cmd:`).
- **Status**: Verified that Litecord enforces a strict check:
  ```rust
  if !url.starts_with("http://") && !url.starts_with("https://") {
      log::warn!("URL rejected due to insecure scheme: {}", url);
      return;
  }
### 3.5. Protection against DLL Hijacking / Side-Loading on Windows
- **File**: `src/main.rs` (`main`)
- **Scenario**: Running the application from download folders containing DLLs manipulated by third parties could induce Windows to load untrusted libraries.
- **Action Taken**: Explicit call to `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` as the very first instruction of `main()` on Windows, forcing native DLLs to be resolved exclusively from the protected `System32` system directory.

### 3.6. Strict Isolation of Temporary Directories and Symlink Race Prevention
- **File**: `src/updater.rs` and `src/main.rs`
- **Scenario**: Use of the shared `/tmp` directory on Linux distributions allowed concurrency or symlink race risks from other local users of the same machine.
- **Action Taken**: Implemented the `get_secure_update_dir()` function, prioritizing `$XDG_RUNTIME_DIR/litecord_update` or `~/.cache/litecord/update` with strict POSIX permissions `0700` (`rwx------`).

### 3.7. Elimination of Subshells and Command Injection in Native Audio
- **File**: `src/screen_capture.rs` (`AudioLoopbackIsolation`)
- **Scenario**: PulseAudio/PipeWire utility commands previously used `sh -c` with string interpolation to invoke `awk` and `pactl`.
- **Action Taken**: Replaced with native Rust routines (`std::process::Command::args`), parsing the `pactl` output directly in memory and invoking subcommands with strict vector arguments, eliminating any shell subshell invocation (`/bin/sh`).

### 3.8. Secure Memory Purge and Overwriting of Tokens on Disk
- **File**: `src/main.rs` (`dpapi_protect`, `dpapi_unprotect`, `delete_secure_token`)
- **Action Taken**:
  - Explicit zeroing of memory (`std::ptr::write_bytes`) in the DPAPI decryption buffers before handing the memory blocks back to the system via `LocalFree`.
  - Overwriting token files with zeros before deletion (`fs::remove_file`), preventing recovery of residual data in unallocated clusters.

### 3.9. P2P Privacy Hardening: Strict Channel Validation, Audio Encryption and Unicast
- **File**: `src/screen_capture.rs`
- **Scenario**: P2P streams on local networks (LAN) previously used broadcast (`255.255.255.255`) and an autonomous audio playback fallback, allowing other devices on the same local network, or outside the voice call, to receive the audio and video of streams not intended for them.
- **Action Taken**:
  - **Strict Channel Validation**: Added a check in the receiver (`screen-capture-rx`) that immediately discards any packet (`OP_AUDIO_FRAME`, `OP_VIDEO_CHUNK`, `OP_FEC_PARITY`, `OP_ANNOUNCE`, `OP_HEARTBEAT`, `OP_STOP`) if the user is not connected to exactly the same active voice channel (`pkt_cid != my_current_cid` or `my_current_cid == 0`).
  - **Mandatory AES-256-GCM Encryption**: All loopback audio packets are now encrypted with the voice room key (`get_voice_encryption_key`) before transmission, and the receiver rejects packets that cannot be authenticated with the room key.
  - **Broadcast Replaced by Unicast**: Sending media to universal broadcast addresses (`255.255.255.255`) was eliminated. Audio and video packets are now dispatched strictly via direct Unicast to the verified IP addresses of the call participants (`peers_store` / `LAST_SEEN_PEER_ADDR`).

---

## 4. Communications, Voice and Video Security

1. **Discord Data Transport**:
   - Gateway and REST connections use TLS 1.3 / 1.2 with `rustls` or a secure native backend.
   - Strictly direct communication with Discord's official servers (`gateway.discord.gg`, `discord.com`). There are no intermediary proxies, analytics or external telemetry.
2. **Voice and Stream Encryption (SRTP & DAVE E2EE)**:
   - Media packets are protected with `aead_aes256_gcm_rtpsize`.
   - Support for the **DAVE** protocol (Discord Audio/Video End-to-End Encryption) based on the IETF MLS standard (Messaging Layer Security - RFC 9420), ensuring that only call participants can decode the audio and video frames.
3. **Screen Capture and Fallbacks**:
   - Capture permissions on Linux follow the Wayland security portals (`xdg-desktop-portal`) with explicit user consent, in addition to X11 Shared Memory (`xshm`).
   - On Windows, it uses the DirectX Desktop Duplication API (DXGI) or GDI with device context isolation.

---

## 5. Update Mechanism (Self-Updater)

- **Origin Validation**: The installer and auto-updater check the download URL before any request. Only assets coming from the official GitHub repository (`https://github.com/Ak4ai/Litecord/releases/download/...`) are accepted.
- **Execution Integrity**: There is no execution of unsigned external scripts or command injection via shell for the update. The download happens in a secure, isolated directory with single-user permissions (`0700`).

---

## 6. Ongoing Hardening Recommendations (Next Steps)

For future versions and the continuous security roadmap:
1. **Cryptographic Checksum / Release Signature Validation**:
   - Adding a SHA-256 hash check or digital signature verification of binaries downloaded via the auto-updater before replacing the executable.
2. **Continuous Automated Auditing in CI/CD**:
   - Integrating `cargo audit` and `cargo deny` into the GitHub Actions pipelines for continuous monitoring of CVEs in third-party dependencies.

---

## 7. Audit Conclusion

Litecord presents a hardened, secure and very high-performance architecture. All priority mitigation recommendations were implemented successfully and validated in the build and unit tests. The code on `dev` is approved and ready for release.
