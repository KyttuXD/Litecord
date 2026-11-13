# 📚 Litecord Documentation & Wiki Hub

Welcome to the **Litecord** documentation and engineering hub!

This directory contains the platform's technical specifications, architecture plans, audit reports and development guidelines.

---

## 📑 Documentation Index

### 🚀 Architecture & High-Performance Pipelines
* **[High-Performance Network & Proxy](NETWORK_AND_PROXY.md)**:
  * Native support for HTTP/HTTPS proxies, SOCKS5 and the operating system proxy.
  * Dynamic injection into the `reqwest` client and HTTPS `CONNECT` tunnels.
  * Smart failure and timeout protection with 1-click shortcuts on the login screen.
  * Latency diagnostics (real-time ping to the Discord Gateway).
* **[P2P Video Streaming Architecture](P2P_VIDEO_STREAMING_PLAN.md)**:
  * Diagnosis of the Full HD (1080p @ 60 FPS) streaming system.
  * Signaling protocol via the Discord Gateway and NAT Hole Punching (STUN RFC 5389 / Google / Cloudflare).
  * RFC 6184 NAL Unit fragmentation for H.264 over low-latency P2P UDP.
  * GPU acceleration pipeline (NVENC, AMD AMF, Intel/Windows Media Foundation MFT and FFmpeg fallback).

### 🛡️ Security, Encryption & Audits
* **[Security Audit Report](SECURITY_AUDIT.md)**:
  * In-depth analysis of the attack surface and threat models.
  * E2EE encryption (Discord DAVE Protocol & MLS - Messaging Layer Security).
  * Secure token storage with DPAPI encryption on Windows.
  * Memory validation and safe routines in native Rust.
* **[Technical Code Audit Report](CODE_AUDIT_REPORT.md)**:
  * Concurrency diagnosis, mutex mapping and *mutex poisoning* mitigation.
  * Thread lifecycle management and dynamic isolation of graphics libraries.
  * WASAPI audio pipeline optimizations and low-latency VAD.

### 👥 Governance & Community
* **[Security Policy](../SECURITY.md)**: Vulnerability reporting guidelines and security support.
* **[Contributing Guidelines](../CONTRIBUTING.md)**: Development guide, code standards and Pull Request submission.
* **[Code of Conduct](../CODE_OF_CONDUCT.md)**: Code of conduct for contributors and community members.
* **[Changelog](../CHANGELOG.md)**: Complete history of versions, releases and release notes.

---

## 🏗️ Repository Structure

```text
Litecord/
├── src/                      # Native Rust source code
│   ├── encoder/              # Hardware acceleration engines (NVENC, AMF, WMF, FFmpeg)
│   ├── gateway/              # Discord Gateway client (WebSocket, REST, DAVE E2EE)
│   ├── p2p/                  # P2P video/audio protocol and STUN NAT Hole Punching
│   ├── screen_capture/       # DXGI Desktop Duplication & GDI screen capture
│   ├── voice/                # WASAPI audio pipeline, Loopback and Opus codec
│   └── main.rs               # Entry point and Slint UI orchestration
├── ui/                       # Native Slint graphical interface (GPU-accelerated)
│   └── appwindow.slint       # UI design, tabs, video room and visual components
├── assets/                   # SVG vector icons, graphic assets and fonts
├── docs/                     # Technical documentation and Wiki articles
└── Cargo.toml                # Dependencies and project manifest
```
