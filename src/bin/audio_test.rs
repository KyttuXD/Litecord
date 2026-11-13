use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::UdpSocket;
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce
};
use opus_rs::OpusDecoder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    let requested_channel_id = args.get(1).cloned();

    println!("====================================================");
    println!("🧪 LITECORD AUTOMATED AUDIO RECEPTION DIAGNOSTIC TEST");
    println!("====================================================");

    let token = match std::fs::read_to_string(".litecord_token") {
        Ok(t) => t.trim().to_string(),
        Err(_) => {
            println!("❌ ERROR: File .litecord_token not found.");
            return Ok(());
        }
    };

    println!("🔑 Discord token loaded.");

    let client = reqwest::Client::new();
    let mut target_guild_id = String::new();
    let mut target_channel_id = String::new();
    let mut target_channel_name = String::new();

    if let Some(req_cid) = requested_channel_id {
        target_channel_id = req_cid.clone();

        let ch_resp = client.get(format!("https://discord.com/api/v9/channels/{}", req_cid))
            .header("Authorization", &token)
            .send()
            .await?;

        if ch_resp.status().is_success() {
            if let Ok(ch_json) = ch_resp.json::<Value>().await {
                if let Some(gid) = ch_json["guild_id"].as_str() {
                    target_guild_id = gid.to_string();
                }
                let cname = ch_json["name"].as_str().unwrap_or("Voice Channel");
                target_channel_name = cname.to_string();
                println!("🔎 Channel obtained via REST API: Name='{}', GuildID='{}'", target_channel_name, target_guild_id);
            }
        }
    }

    if target_channel_id.is_empty() {
        let resp = client.get("https://discord.com/api/v9/users/@me/guilds")
            .header("Authorization", &token)
            .send()
            .await?;

        if resp.status().is_success() {
            if let Ok(guilds) = resp.json::<Value>().await {
                if let Some(guild_arr) = guilds.as_array() {
                    for g in guild_arr {
                        let gid = g["id"].as_str().unwrap_or("").to_string();
                        let gname = g["name"].as_str().unwrap_or("Server").to_string();

                        let chan_resp = client.get(format!("https://discord.com/api/v9/guilds/{}/channels", gid))
                            .header("Authorization", &token)
                            .send()
                            .await?;

                        if chan_resp.status().is_success() {
                            if let Ok(chans) = chan_resp.json::<Value>().await {
                                if let Some(ch_arr) = chans.as_array() {
                                    for ch in ch_arr {
                                        let ctype = ch["type"].as_u64().unwrap_or(0);
                                        if ctype == 2 {
                                            target_guild_id = gid.clone();
                                            target_channel_id = ch["id"].as_str().unwrap_or("").to_string();
                                            target_channel_name = format!("{} / {}", gname, ch["name"].as_str().unwrap_or("Voice"));
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        if !target_channel_id.is_empty() { break; }
                    }
                }
            }
        }
    }

    if target_channel_id.is_empty() {
        println!("❌ ERROR: No voice channel found.");
        return Ok(());
    }

    println!("📌 Target Channel: '{}' (ChannelID: {}, GuildID: '{}')", target_channel_name, target_channel_id, target_guild_id);
    println!("📡 Connecting to the Discord Main Gateway...");

    let (ws_stream, _) = connect_async("wss://gateway.discord.gg/?v=9&encoding=json").await?;
    let (mut write, mut read) = ws_stream.split();

    let session_id_arc = Arc::new(Mutex::new(None::<String>));
    let voice_token_arc = Arc::new(Mutex::new(None::<String>));
    let voice_ep_arc = Arc::new(Mutex::new(None::<String>));
    let my_uid_arc = Arc::new(Mutex::new(None::<String>));

    let session_id_w = Arc::clone(&session_id_arc);
    let voice_token_w = Arc::clone(&voice_token_arc);
    let voice_ep_w = Arc::clone(&voice_ep_arc);
    let my_uid_w = Arc::clone(&my_uid_arc);

    let t_gid = target_guild_id.clone();
    let t_cid = target_channel_id.clone();
    let token_clone = token.clone();

    tokio::spawn(async move {
        while let Some(Ok(msg)) = read.next().await {
            if let Message::Text(txt) = msg {
                if let Ok(val) = serde_json::from_str::<Value>(&txt) {
                    let op = val["op"].as_u64().unwrap_or(99);
                    match op {
                        10 => {
                            let identify = serde_json::json!({
                                "op": 2,
                                "d": {
                                    "token": token_clone,
                                    "capabilities": 16381,
                                    "properties": { "os": "Windows", "browser": "Chrome", "device": "" }
                                }
                            });
                            let _ = write.send(Message::Text(identify.to_string().into())).await;
                        }
                        0 => {
                            let t = val["t"].as_str().unwrap_or("");
                            if t == "READY" {
                                let uid = val["d"]["user"]["id"].as_str().unwrap_or("").to_string();
                                *my_uid_w.lock().unwrap() = Some(uid);

                                let op4_gid = if t_gid.is_empty() { serde_json::Value::Null } else { serde_json::json!(t_gid) };
                                let op4 = serde_json::json!({
                                    "op": 4,
                                    "d": {
                                        "guild_id": op4_gid,
                                        "channel_id": t_cid,
                                        "self_mute": false,
                                        "self_deaf": false
                                    }
                                });
                                let _ = write.send(Message::Text(op4.to_string().into())).await;
                            } else if t == "VOICE_STATE_UPDATE" {
                                if let Some(sid) = val["d"]["session_id"].as_str() {
                                    *session_id_w.lock().unwrap() = Some(sid.to_string());
                                }
                            } else if t == "VOICE_SERVER_UPDATE" {
                                let tok = val["d"]["token"].as_str().unwrap_or("").to_string();
                                let ep = val["d"]["endpoint"].as_str().unwrap_or("").to_string();
                                *voice_token_w.lock().unwrap() = Some(tok);
                                *voice_ep_w.lock().unwrap() = Some(ep);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    println!("⏳ Waiting for tokens and endpoint from the Voice Gateway...");
    for _ in 0..100 {
        sleep(Duration::from_millis(100)).await;
        if session_id_arc.lock().unwrap().is_some() && voice_token_arc.lock().unwrap().is_some() {
            break;
        }
    }

    let uid = my_uid_arc.lock().unwrap().clone().ok_or("User ID not received")?;
    let sid = session_id_arc.lock().unwrap().clone().ok_or("Voice session ID not received")?;
    let v_tok = voice_token_arc.lock().unwrap().clone().ok_or("Voice token not received")?;
    let raw_ep = voice_ep_arc.lock().unwrap().clone().ok_or("Voice endpoint not received")?;

    let clean_ep = raw_ep.trim();
    let v_url = format!("wss://{}/?v=4", clean_ep);

    println!("🌐 Connecting to the Discord Voice Gateway: {}", v_url);

    let (v_ws, _) = connect_async(&v_url).await?;
    let (mut v_write, mut v_read) = v_ws.split();

    let mut my_ssrc = 0u32;
    let mut secret_key = Vec::<u8>::new();
    let mut voice_ip = String::new();
    let mut voice_port = 0u16;

    while let Some(Ok(msg)) = v_read.next().await {
        if let Message::Text(txt) = msg {
            if let Ok(val) = serde_json::from_str::<Value>(&txt) {
                let op = val["op"].as_u64().unwrap_or(99);
                match op {
                    8 => {
                        let op4_gid = if target_guild_id.is_empty() { target_channel_id.clone() } else { target_guild_id.clone() };
                        let v_ident = serde_json::json!({
                            "op": 0,
                            "d": {
                                "server_id": op4_gid,
                                "user_id": uid,
                                "session_id": sid,
                                "token": v_tok,
                                "video": false,
                                "streams": [],
                                "max_dave_protocol_version": 1
                            }
                        });
                        let _ = v_write.send(Message::Text(v_ident.to_string().into())).await;
                    }
                    2 => {
                        my_ssrc = val["d"]["ssrc"].as_u64().unwrap_or(0) as u32;
                        voice_ip = val["d"]["ip"].as_str().unwrap_or("").to_string();
                        voice_port = val["d"]["port"].as_u64().unwrap_or(0) as u16;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    println!("✅ Opcode 2 Ready received! SSRC={}, Server IP={}:{}", my_ssrc, voice_ip, voice_port);

    // Open UDP socket
    let udp = UdpSocket::bind("0.0.0.0:0").await?;
    let local_port = udp.local_addr()?.port();
    let target_addr = format!("{}:{}", voice_ip, voice_port);

    let mut disc = [0u8; 70];
    disc[0] = 0x00; disc[1] = 0x01; disc[2] = 0x00; disc[3] = 0x46;
    disc[4..8].copy_from_slice(&my_ssrc.to_be_bytes());

    let mut my_pub_ip = String::new();
    let mut my_pub_port = 0u16;

    // Send UDP IP discovery to server address
    for attempt in 1..=3 {
        println!("📤 Sending UDP IP Discovery packet (attempt {})...", attempt);
        let send_res = udp.send_to(&disc, &target_addr).await;
        println!("   Send result: {:?}", send_res);

        let mut disc_resp = [0u8; 128];
        match tokio::time::timeout(Duration::from_secs(2), udp.recv_from(&mut disc_resp)).await {
            Ok(Ok((len, src_addr))) => {
                println!("📥 IP Discovery response received from {}: {} bytes", src_addr, len);
                if len >= 70 {
                    let ip_slice = &disc_resp[8..64];
                    let ip_end = ip_slice.iter().position(|&b| b == 0).unwrap_or(ip_slice.len());
                    my_pub_ip = String::from_utf8_lossy(&ip_slice[..ip_end]).trim().to_string();
                    my_pub_port = u16::from_be_bytes([disc_resp[68], disc_resp[69]]);
                    println!("🎉 Public IP discovered: {}:{}", my_pub_ip, my_pub_port);
                    break;
                }
            }
            Ok(Err(e)) => println!("   UDP receive error: {:?}", e),
            Err(_) => println!("   Timeout waiting for IP Discovery response."),
        }
    }

    if my_pub_ip.is_empty() {
        println!("⚠️ UDP IP Discovery failed 3 times! Using HTTP IP fallback...");
        if let Ok(resp) = client.get("https://api.ipify.org").send().await {
            if let Ok(ip_text) = resp.text().await {
                my_pub_ip = ip_text.trim().to_string();
                my_pub_port = local_port;
            }
        }
    }

    println!("🌐 IP Used in Select Protocol: {}:{}", my_pub_ip, my_pub_port);

    // Send OP 1 Select Protocol
    let select_p = serde_json::json!({
        "op": 1,
        "d": {
            "protocol": "udp",
            "data": {
                "address": my_pub_ip,
                "port": my_pub_port,
                "mode": "aead_aes256_gcm_rtpsize"
            }
        }
    });
    v_write.send(Message::Text(select_p.to_string().into())).await?;

    // Wait for OP 4 Session Description (Secret Key)
    while let Some(Ok(msg)) = v_read.next().await {
        if let Message::Text(txt) = msg {
            if let Ok(val) = serde_json::from_str::<Value>(&txt) {
                let op = val["op"].as_u64().unwrap_or(99);
                if op == 4 {
                    if let Some(arr) = val["d"]["secret_key"].as_array() {
                        secret_key = arr.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect();
                    }
                    break;
                }
            }
        }
    }

    if secret_key.len() != 32 {
        println!("❌ ERROR: 32-byte secret key not received.");
        return Ok(());
    }

    println!("🔑 OP 4 Session Description OK! 32-byte AES-256 key configured.");

    // Send OP 5 Speaking & Heartbeat loop task
    let speak = serde_json::json!({
        "op": 5,
        "d": { "speaking": 1, "delay": 0, "ssrc": my_ssrc }
    });
    v_write.send(Message::Text(speak.to_string().into())).await?;

    println!("🔊 Listening for UDP voice packets for 15 seconds...");

    let mut buf = vec![0u8; 4096];
    let cipher = Aes256Gcm::new_from_slice(&secret_key)?;
    let mut opus_decoder = OpusDecoder::new(48000, 1)?;

    let mut total_packets = 0u64;
    let mut decrypted_packets = 0u64;
    let mut opus_decoded_packets = 0u64;
    let mut max_rms = 0.0f32;
    let mut detected_ssrcs = std::collections::HashSet::new();

    let start_time = std::time::Instant::now();

    while start_time.elapsed() < Duration::from_secs(15) {
        if let Ok(Ok((len, src_addr))) = tokio::time::timeout(Duration::from_millis(100), udp.recv_from(&mut buf)).await {
            if len < 12 { continue; }
            let pkt = &buf[..len];

            let version = (pkt[0] >> 6) & 0x3;
            if version != 2 { continue; }
            let pt = pkt[1] & 0x7F;
            if pt >= 200 { continue; }

            let ssrc_recv = u32::from_be_bytes([pkt[8], pkt[9], pkt[10], pkt[11]]);
            if ssrc_recv == my_ssrc { continue; }

            total_packets += 1;
            if detected_ssrcs.insert(ssrc_recv) {
                println!("🎙️ UDP Voice packet detected from {}! SSRC={}", src_addr, ssrc_recv);
            }

            let cc = (pkt[0] & 0x0F) as usize;
            let has_ext = (pkt[0] & 0x10) != 0;
            let base_header_len = 12 + 4 * cc;
            if len < base_header_len { continue; }

            let (aad_len, ext_offset) = if has_ext {
                if len < base_header_len + 4 { continue; }
                let ext_words = u16::from_be_bytes([pkt[base_header_len + 2], pkt[base_header_len + 3]]) as usize;
                (base_header_len + 4, ext_words * 4)
            } else {
                (base_header_len, 0)
            };

            if len < aad_len + ext_offset + 4 { continue; }
            let nonce_suffix = &pkt[len-4..len];
            let mut nonce_bytes = [0u8; 12];
            nonce_bytes[0..4].copy_from_slice(nonce_suffix);

            let nonce = Nonce::from_slice(&nonce_bytes);
            let header = &pkt[..aad_len];
            let ciphertext = &pkt[aad_len..len-4];

            let payload = Payload { msg: ciphertext, aad: header };
            if let Ok(decrypted) = cipher.decrypt(nonce, payload) {
                decrypted_packets += 1;

                if decrypted.len() >= ext_offset {
                    let opus_bytes = &decrypted[ext_offset..];
                    let mut pcm_out = vec![0.0f32; 5760];
                    if let Ok(samples) = opus_decoder.decode(opus_bytes, 5760, &mut pcm_out) {
                        opus_decoded_packets += 1;

                        let mut sum_sq = 0.0f32;
                        for &s in &pcm_out[..samples] {
                            sum_sq += s * s;
                        }
                        let rms = (sum_sq / samples.max(1) as f32).sqrt();
                        if rms > max_rms {
                            max_rms = rms;
                        }
                    }
                }
            }
        }
    }

    println!("\n====================================================");
    println!("📊 AUDIO TEST REPORT");
    println!("====================================================");
    println!("📦 UDP Voice Packets Received from Other Users/Bots: {}", total_packets);
    println!("🔐 Packets Decrypted Successfully (AES-256-GCM): {}", decrypted_packets);
    println!("🎵 Opus Frames Decoded Successfully: {}", opus_decoded_packets);
    println!("👥 Number of Voice Sources (Remote SSRCs): {}", detected_ssrcs.len());
    println!("🔊 Peak Audio RMS Volume Detected: {:.2}%", max_rms * 100.0);

    if decrypted_packets > 0 && opus_decoded_packets > 0 {
        println!("\n✅ VERIFICATION COMPLETED SUCCESSFULLY! Audio reception and music are 100% WORKING!");
    } else if total_packets > 0 {
        println!("\n⚠️ UDP packets received ({}), but no audio in the 15 seconds.", total_packets);
    } else {
        println!("\n⚠️ No voice packet detected in the 15 seconds.");
    }
    println!("====================================================");

    Ok(())
}
