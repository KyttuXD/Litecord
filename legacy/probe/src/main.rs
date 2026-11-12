//! Checks, on a real old Mac, the parts of Litecord that do not depend on the window:
//! the Rust runtime, a TLS 1.2 connection to Discord (the system TLS of OS X 10.6 stops
//! at TLS 1.0, so this uses rustls), and audio output and input through CoreAudio.
//! Prints one PASS / FAIL / SKIP line per check and exits with 1 if any check failed.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

thread_local! {
    static SLOT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn report(name: &str, result: Result<String, String>) -> bool {
    match result {
        Ok(detail) => {
            println!("PASS  {name}: {detail}");
            true
        }
        Err(detail) if detail.starts_with("SKIP") => {
            println!("{detail}  ({name})");
            true
        }
        Err(detail) => {
            println!("FAIL  {name}: {detail}");
            false
        }
    }
}

fn runtime() -> Result<String, String> {
    let start = Instant::now();
    let handles: Vec<_> = (1..=4u32)
        .map(|i| {
            std::thread::spawn(move || {
                SLOT.with(|s| s.set(i));
                std::thread::sleep(Duration::from_millis(20));
                SLOT.with(|s| s.get())
            })
        })
        .collect();
    let mut seen = Vec::new();
    for h in handles {
        seen.push(h.join().map_err(|_| "a thread panicked".to_string())?);
    }
    if seen != [1, 2, 3, 4] || SLOT.with(|s| s.get()) != 0 {
        return Err(format!("thread-local values leaked between threads: {seen:?}"));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    let map: std::collections::HashMap<u32, u32> = (0..100).map(|i| (i, i * 2)).collect();
    Ok(format!(
        "threads, thread-locals, clocks ({} ms, unix time {}), hash map of {}",
        start.elapsed().as_millis(),
        now.as_secs(),
        map.len()
    ))
}

fn tls() -> Result<String, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from("discord.com").map_err(|e| e.to_string())?;
    let conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    let sock = std::net::TcpStream::connect(("discord.com", 443)).map_err(|e| format!("connect: {e}"))?;
    sock.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let mut stream = rustls::StreamOwned::new(conn, sock);
    stream
        .write_all(
            b"GET /api/v10/gateway HTTP/1.1\r\nHost: discord.com\r\nUser-Agent: litecord-legacy-probe\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        )
        .map_err(|e| format!("write: {e}"))?;
    let mut body = Vec::new();
    // The server may close without a TLS close_notify: what was read still counts.
    let _ = stream.read_to_end(&mut body);
    let text = String::from_utf8_lossy(&body);
    let status = text.lines().next().unwrap_or("").to_string();
    let version = stream.conn.protocol_version().map(|v| format!("{v:?}")).unwrap_or_default();
    if text.contains("wss://") {
        Ok(format!("{version}, \"{status}\", the reply names the gateway"))
    } else {
        Err(format!("{version}, unexpected reply \"{status}\" ({} bytes)", body.len()))
    }
}

#[cfg(target_os = "macos")]
mod audio {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    pub fn output() -> Result<String, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("SKIP  no output device")?;
        let name = device.name().unwrap_or_default();
        let config = device.default_output_config().map_err(|e| format!("config: {e}"))?;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!("\"{name}\" uses {:?}, the probe only writes f32", config.sample_format()));
        }
        let channels = config.channels() as usize;
        let rate = config.sample_rate().0 as f32;
        let frames = Arc::new(AtomicUsize::new(0));
        let frames_cb = Arc::clone(&frames);
        let mut phase = 0f32;
        let stream = device
            .build_output_stream(
                &config.clone().into(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    for frame in data.chunks_mut(channels) {
                        // A quiet 440 Hz tone, so a person at the Mac can hear that it works.
                        phase = (phase + 440.0 / rate) % 1.0;
                        let v = (phase * std::f32::consts::TAU).sin() * 0.05;
                        for s in frame.iter_mut() {
                            *s = v;
                        }
                    }
                    frames_cb.fetch_add(data.len() / channels, Ordering::Relaxed);
                },
                |e| eprintln!("output stream error: {e}"),
                None,
            )
            .map_err(|e| format!("build stream on \"{name}\": {e}"))?;
        stream.play().map_err(|e| format!("play: {e}"))?;
        std::thread::sleep(Duration::from_millis(400));
        drop(stream);
        let played = frames.load(Ordering::Relaxed);
        if played == 0 {
            return Err(format!("\"{name}\" opened but asked for no audio"));
        }
        Ok(format!("\"{name}\", {} Hz, {channels} ch, {played} frames played", rate as u32))
    }

    pub fn input() -> Result<String, String> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or("SKIP  no input device (no microphone connected)")?;
        let name = device.name().unwrap_or_default();
        let config = device.default_input_config().map_err(|e| format!("config: {e}"))?;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!("\"{name}\" uses {:?}, the probe only reads f32", config.sample_format()));
        }
        let channels = config.channels() as usize;
        let frames = Arc::new(AtomicUsize::new(0));
        let frames_cb = Arc::clone(&frames);
        let stream = device
            .build_input_stream(
                &config.clone().into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    frames_cb.fetch_add(data.len() / channels.max(1), Ordering::Relaxed);
                },
                |e| eprintln!("input stream error: {e}"),
                None,
            )
            .map_err(|e| format!("build stream on \"{name}\": {e}"))?;
        stream.play().map_err(|e| format!("record: {e}"))?;
        std::thread::sleep(Duration::from_millis(400));
        drop(stream);
        let got = frames.load(Ordering::Relaxed);
        if got == 0 {
            return Err(format!("\"{name}\" opened but delivered no audio"));
        }
        Ok(format!("\"{name}\", {} Hz, {channels} ch, {got} frames recorded", config.sample_rate().0))
    }
}

fn main() {
    println!("litecord-legacy-probe {}", env!("CARGO_PKG_VERSION"));
    let mut ok = true;
    ok &= report("rust runtime", runtime());
    ok &= report("tls to discord", tls());
    #[cfg(target_os = "macos")]
    {
        ok &= report("audio output", audio::output());
        ok &= report("audio input", audio::input());
    }
    println!("{}", if ok { "RESULT: all checks passed" } else { "RESULT: at least one check failed" });
    std::process::exit(if ok { 0 } else { 1 });
}
