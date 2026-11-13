//! What runs beside the window: QR login, the gateway connection, message loading and sending.
//! All of it is Litecord's own network code (see lib.rs); this file only moves its results
//! into the state the window draws.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use litecord_legacy::auth::{generate_qr_image, RemoteAuthEvent, RemoteAuthSession};
use litecord_legacy::gateway::{ChannelData, GatewayClient, GatewayCommand, GatewayEvent, GuildData};
use litecord_legacy::http::DiscordHttpClient;
use serde_json::Value;
use tokio::sync::mpsc;

pub struct Message {
    pub author: String,
    pub time: String,
    pub text: String,
}

#[derive(Default)]
pub struct Shared {
    /// Bumped on every change, so the window knows when to redraw.
    pub version: u64,
    pub logged_in: bool,
    pub status: String,
    pub user: String,
    /// QR picture of the login screen: RGBA bytes, width, height.
    pub qr: Option<(Vec<u8>, u32, u32)>,
    pub guilds: Vec<GuildData>,
    pub channel_id: String,
    pub messages: Vec<Message>,
}

pub type SharedRef = Arc<Mutex<Shared>>;

pub enum Action {
    SelectChannel { guild_id: String, channel_id: String },
    Send { channel_id: String, text: String },
    LogOut,
}

fn update(shared: &SharedRef, f: impl FnOnce(&mut Shared)) {
    if let Ok(mut s) = shared.lock() {
        f(&mut s);
        s.version += 1;
    }
}

/// Folder for the session file and the log; also made the working directory, because the
/// shared network code keeps its settings files relative to it.
pub fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Application Support/Litecord Legacy")
    } else {
        PathBuf::from(home).join(".config/litecord-legacy")
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn token_path() -> PathBuf {
    data_dir().join("session")
}

fn load_token() -> Option<String> {
    let token = std::fs::read_to_string(token_path()).ok()?.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// The session token, readable by this user only. It is not encrypted.
fn save_token(token: &str) {
    let path = token_path();
    if std::fs::write(&path, token).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

fn display_name(user: &Value) -> String {
    user["global_name"]
        .as_str()
        .filter(|n| !n.is_empty())
        .or_else(|| user["username"].as_str())
        .unwrap_or("?")
        .to_string()
}

fn local_time(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|t| t.with_timezone(&chrono::Local).format("%d/%m %H:%M").to_string())
        .unwrap_or_default()
}

fn message_from_rest(m: &Value) -> Message {
    let mut text = m["content"].as_str().unwrap_or("").to_string();
    let attachments = m["attachments"].as_array().map(|a| a.len()).unwrap_or(0);
    let embeds = m["embeds"].as_array().map(|a| a.len()).unwrap_or(0);
    if attachments > 0 {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("[{attachments} attachment(s)]"));
    }
    if text.is_empty() && embeds > 0 {
        text.push_str("[embed]");
    }
    Message {
        author: display_name(&m["author"]),
        time: local_time(m["timestamp"].as_str().unwrap_or("")),
        text,
    }
}

async fn log_in(shared: &SharedRef) -> String {
    loop {
        update(shared, |s| {
            s.logged_in = false;
            s.qr = None;
            s.status = "Asking Discord for a login code...".to_string();
        });
        let (tx, mut rx) = mpsc::channel(16);
        let session = RemoteAuthSession::start(tx);
        let mut failed = false;
        while let Some(event) = rx.recv().await {
            match event {
                RemoteAuthEvent::QrCodeUrl(url) => match generate_qr_image(&url) {
                    Ok(img) => update(shared, |s| {
                        s.qr = Some((img.rgba, img.width, img.height));
                        s.status = "Scan this code with the Discord app on your phone (Settings > Scan QR Code).".to_string();
                    }),
                    Err(e) => update(shared, |s| s.status = format!("Could not draw the login code: {e}")),
                },
                RemoteAuthEvent::UserScanned { username } => {
                    update(shared, |s| s.status = format!("{username}: confirm the login on your phone."))
                }
                RemoteAuthEvent::TokenReceived(token) => {
                    save_token(&token);
                    return token;
                }
                RemoteAuthEvent::Cancelled => {
                    failed = true;
                    break;
                }
                RemoteAuthEvent::Error(e) => {
                    update(shared, |s| s.status = format!("Login failed: {e}. Trying again..."));
                    failed = true;
                    break;
                }
            }
        }
        session.cancel();
        if failed {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    }
}

async fn load_direct_messages(http: &DiscordHttpClient, shared: &SharedRef) {
    let Ok(dms) = http.get_user_dms().await else { return };
    let mut channels: Vec<(u64, ChannelData)> = dms
        .iter()
        .filter_map(|dm| {
            let id = dm["id"].as_str()?.to_string();
            let mut name = dm["name"].as_str().unwrap_or("").to_string();
            if name.is_empty() {
                let names: Vec<String> = dm["recipients"].as_array()?.iter().map(display_name).collect();
                name = names.join(", ");
            }
            if name.is_empty() {
                return None;
            }
            // Most recently used first.
            let last: u64 = dm["last_message_id"].as_str().and_then(|v| v.parse().ok()).unwrap_or(0);
            Some((last, ChannelData { id, name, is_voice: false, is_category: false, parent_id: None, position: 0 }))
        })
        .collect();
    channels.sort_by(|a, b| b.0.cmp(&a.0));
    let guild = GuildData {
        id: "@me".to_string(),
        name: "Direct Messages".to_string(),
        channels: channels.into_iter().map(|(_, c)| c).collect(),
    };
    update(shared, |s| {
        s.guilds.retain(|g| g.id != "@me");
        s.guilds.insert(0, guild);
    });
}

/// Runs one logged-in session. Returns when the user logs out or the token is refused.
async fn run_session(token: String, shared: &SharedRef, actions: &mut mpsc::UnboundedReceiver<Action>) {
    let http = Arc::new(DiscordHttpClient::new(token.clone()));
    match http.get_current_user().await {
        Ok(user) => update(shared, |s| {
            s.logged_in = true;
            s.user = display_name(&user);
            s.status = "Connecting...".to_string();
        }),
        Err(e) if e.contains("401") => {
            let _ = std::fs::remove_file(token_path());
            update(shared, |s| s.status = "The saved login is no longer valid.".to_string());
            return;
        }
        Err(e) => update(shared, |s| {
            s.logged_in = true;
            s.status = format!("Could not read the account ({e}). Connecting anyway...");
        }),
    }

    let (event_tx, mut event_rx) = mpsc::channel::<GatewayEvent>(512);
    let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(32);
    let gateway = Arc::new(GatewayClient::new(token, event_tx));
    let gateway_task = tokio::spawn(async move { gateway.start(cmd_rx).await });
    load_direct_messages(&http, shared).await;

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                let Some(event) = event else { break };
                match event {
                    GatewayEvent::Connected { user_tag } => update(shared, |s| s.status = format!("Connected as {user_tag}")),
                    GatewayEvent::Disconnected { reason } => update(shared, |s| s.status = format!("Disconnected: {reason}")),
                    GatewayEvent::GuildLoaded { guild } => update(shared, |s| {
                        match s.guilds.iter_mut().find(|g| g.id == guild.id) {
                            Some(existing) => *existing = guild,
                            None => s.guilds.push(guild),
                        }
                    }),
                    GatewayEvent::MessageCreated { channel_id, author, created_at, content, attachments, .. } => {
                        update(shared, |s| {
                            if s.channel_id == channel_id {
                                let mut text = content;
                                if !attachments.is_empty() {
                                    if !text.is_empty() {
                                        text.push('\n');
                                    }
                                    text.push_str(&format!("[{} attachment(s)]", attachments.len()));
                                }
                                s.messages.push(Message { author, time: created_at, text });
                            }
                        });
                    }
                    _ => {}
                }
            }
            action = actions.recv() => {
                let Some(action) = action else { break };
                match action {
                    Action::SelectChannel { guild_id, channel_id } => {
                        update(shared, |s| {
                            s.channel_id = channel_id.clone();
                            s.messages.clear();
                            s.status = "Loading messages...".to_string();
                        });
                        if guild_id != "@me" {
                            let _ = cmd_tx.send(GatewayCommand::SubscribeGuild {
                                guild_id,
                                channel_ids: vec![channel_id.clone()],
                            }).await;
                        }
                        let result = http.get_channel_messages(&channel_id).await;
                        update(shared, |s| {
                            if s.channel_id != channel_id {
                                return;
                            }
                            match result {
                                Ok(list) => {
                                    // Discord answers newest first.
                                    s.messages = list.iter().rev().map(message_from_rest).collect();
                                    s.status = String::new();
                                }
                                Err(e) => s.status = format!("Could not load this channel: {e}"),
                            }
                        });
                    }
                    Action::Send { channel_id, text } => {
                        if let Err(e) = http.send_message(&channel_id, &text).await {
                            update(shared, |s| s.status = format!("Message not sent: {e}"));
                        }
                    }
                    Action::LogOut => {
                        let _ = std::fs::remove_file(token_path());
                        break;
                    }
                }
            }
        }
    }
    gateway_task.abort();
    update(shared, |s| {
        s.logged_in = false;
        s.guilds.clear();
        s.messages.clear();
        s.channel_id.clear();
        s.user.clear();
    });
}

pub async fn run(shared: SharedRef, mut actions: mpsc::UnboundedReceiver<Action>) {
    loop {
        let token = match load_token() {
            Some(token) => token,
            None => log_in(&shared).await,
        };
        run_session(token, &shared, &mut actions).await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
