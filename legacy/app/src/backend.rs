//! What runs beside the window: QR login, the gateway connection, message loading and sending.
//! All of it is Litecord's own network code (see lib.rs); this file only moves its results
//! into the state the window draws.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use litecord_legacy::audio::engine::{start_mic_capture, MicCapture};
use litecord_legacy::auth::password_auth::{self, LoginStep};
pub use litecord_legacy::auth::password_auth::CodeKind;
pub use litecord_legacy::http_messaging::ReactionData;
use litecord_legacy::http_messaging::{apply_reaction, reactions_from_message};
use litecord_legacy::auth::{generate_qr_image, RemoteAuthEvent, RemoteAuthSession};
use litecord_legacy::gateway;
use litecord_legacy::gateway::{ChannelData, GatewayClient, GatewayCommand, GatewayEvent, GuildData};
use litecord_legacy::http::DiscordHttpClient;
use serde_json::Value;
use tokio::sync::mpsc;

pub struct Message {
    pub id: String,
    pub author: String,
    /// Written by the logged-in user (so it can be edited and deleted).
    pub mine: bool,
    pub time: String,
    pub text: String,
    /// "Name: start of the message" when this is a reply.
    pub reply_to: String,
    pub reactions: Vec<ReactionData>,
}

pub struct Shared {
    /// Bumped on every change, so the window knows when to redraw.
    pub version: u64,
    pub logged_in: bool,
    pub status: String,
    pub user: String,
    /// QR picture of the login screen: RGBA bytes, width, height.
    pub qr: Option<(Vec<u8>, u32, u32)>,
    /// Email and password login: what the form shows.
    pub login_stage: LoginStage,
    pub login_message: String,
    pub login_has_sms: bool,
    /// The second-step code asked for: an authenticator code unless this says backup or SMS.
    pub login_code_kind: CodeKind,
    pub guilds: Vec<GuildData>,
    pub channel_id: String,
    pub messages: Vec<Message>,
    /// Id of the logged-in user.
    pub my_id: String,
    /// Who is typing in the open channel, and since when.
    pub typing: Vec<(String, std::time::Instant)>,
    /// Voice channel joined (empty: none), its name, who is in it, and this user's mute switch.
    pub voice_channel_id: String,
    pub voice_channel_name: String,
    pub voice_connected: bool,
    pub voice_users: Vec<String>,
    pub muted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginStage {
    #[default]
    Form,
    Busy,
    Code,
}

pub type SharedRef = Arc<Mutex<Shared>>;

pub enum Action {
    SelectChannel { guild_id: String, channel_id: String },
    Send { channel_id: String, text: String, reply_to: Option<String> },
    Edit { channel_id: String, message_id: String, text: String },
    Delete { channel_id: String, message_id: String },
    /// Adds the reaction, or takes it back when the user already gave it.
    React { channel_id: String, message_id: String, key: String, label: String },
    Typing { channel_id: String },
    Upload { channel_id: String, path: String, reply_to: Option<String> },
    PasswordLogin { login: String, password: String },
    LoginCode { code: String },
    LoginSendSms,
    LoginUseKind(CodeKind),
    LoginCancel,
    JoinVoice { guild_id: String, channel_id: String, name: String },
    LeaveVoice,
    ToggleMute,
    LogOut,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            version: 0,
            logged_in: false,
            status: String::new(),
            user: String::new(),
            qr: None,
            login_stage: LoginStage::Form,
            login_message: String::new(),
            login_has_sms: false,
            login_code_kind: CodeKind::Totp,
            guilds: Vec::new(),
            channel_id: String::new(),
            messages: Vec::new(),
            my_id: String::new(),
            typing: Vec::new(),
            voice_channel_id: String::new(),
            voice_channel_name: String::new(),
            voice_connected: false,
            voice_users: Vec::new(),
            muted: false,
        }
    }
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

fn short(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() > max {
        format!("{}...", line.chars().take(max).collect::<String>())
    } else {
        line.to_string()
    }
}

fn message_from_rest(m: &Value, my_id: &str) -> Message {
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
    if m["edited_timestamp"].is_string() {
        text.push_str(" (edited)");
    }
    let replied = &m["referenced_message"];
    let reply_to = if replied.is_object() {
        format!("{}: {}", display_name(&replied["author"]), short(replied["content"].as_str().unwrap_or(""), 60))
    } else {
        String::new()
    };
    Message {
        id: m["id"].as_str().unwrap_or("").to_string(),
        author: display_name(&m["author"]),
        mine: !my_id.is_empty() && m["author"]["id"].as_str() == Some(my_id),
        time: local_time(m["timestamp"].as_str().unwrap_or("")),
        text,
        reply_to,
        reactions: reactions_from_message(m),
    }
}

/// Shows the login screen until a login succeeds: the QR code, and the email and password
/// form (src/auth/password_auth.rs). Returns the session token.
async fn log_in(shared: &SharedRef, actions: &mut mpsc::UnboundedReceiver<Action>) -> String {
    let mut ticket = String::new();
    loop {
        update(shared, |s| {
            s.logged_in = false;
            s.qr = None;
            s.status = "Asking Discord for a login code...".to_string();
        });
        let (tx, mut rx) = mpsc::channel(16);
        let session = RemoteAuthSession::start(tx);
        let mut qr_failed = false;
        while !qr_failed {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Some(RemoteAuthEvent::QrCodeUrl(url)) => match generate_qr_image(&url) {
                            Ok(img) => update(shared, |s| {
                                s.qr = Some((img.rgba, img.width, img.height));
                                s.status = "Scan the code with the Discord app on your phone (Settings > Scan QR Code), or use email and password.".to_string();
                            }),
                            Err(e) => update(shared, |s| s.status = format!("Could not draw the login code: {e}")),
                        },
                        Some(RemoteAuthEvent::UserScanned { username }) => {
                            update(shared, |s| s.status = format!("{username}: confirm the login on your phone."))
                        }
                        Some(RemoteAuthEvent::TokenReceived(token)) => {
                            save_token(&token);
                            return token;
                        }
                        Some(RemoteAuthEvent::Error(e)) => {
                            update(shared, |s| s.status = format!("QR login failed: {e}. Trying again..."));
                            qr_failed = true;
                        }
                        Some(RemoteAuthEvent::Cancelled) | None => qr_failed = true,
                    }
                }
                action = actions.recv() => {
                    let Some(action) = action else { return std::future::pending::<String>().await };
                    match action {
                        Action::PasswordLogin { login, password } => {
                            update(shared, |s| {
                                s.login_stage = LoginStage::Busy;
                                s.login_message = "Logging in...".to_string();
                            });
                            match password_auth::login(&login, &password).await {
                                Ok(LoginStep::Token(token)) => {
                                    session.cancel();
                                    update(shared, |s| {
                                        s.login_stage = LoginStage::Form;
                                        s.login_message.clear();
                                    });
                                    save_token(&token);
                                    return token;
                                }
                                Ok(LoginStep::SecondStep { ticket: new_ticket, totp, sms, backup: _ }) => {
                                    ticket = new_ticket;
                                    update(shared, |s| {
                                        s.login_stage = LoginStage::Code;
                                        s.login_has_sms = sms;
                                        s.login_code_kind = if totp { CodeKind::Totp } else if sms { CodeKind::Sms } else { CodeKind::Backup };
                                        s.login_message = if totp {
                                            "Enter the 6-digit code from your authenticator app.".to_string()
                                        } else if sms {
                                            "Choose \"Text me a code\", then enter the code Discord sends you.".to_string()
                                        } else {
                                            "Enter one of your backup codes.".to_string()
                                        };
                                    });
                                }
                                Err(message) => update(shared, |s| {
                                    s.login_stage = LoginStage::Form;
                                    s.login_message = message;
                                }),
                            }
                        }
                        Action::LoginCode { code } => {
                            let kind = shared.lock().map(|s| s.login_code_kind).unwrap_or(CodeKind::Totp);
                            match password_auth::finish(kind, &ticket, &code).await {
                                Ok(token) => {
                                    session.cancel();
                                    update(shared, |s| {
                                        s.login_stage = LoginStage::Form;
                                        s.login_message.clear();
                                    });
                                    save_token(&token);
                                    return token;
                                }
                                Err(message) => update(shared, |s| s.login_message = message),
                            }
                        }
                        Action::LoginSendSms => match password_auth::send_sms(&ticket).await {
                            Ok(phone) => update(shared, |s| {
                                s.login_code_kind = CodeKind::Sms;
                                s.login_message = format!("Enter the code sent by SMS to {}.", if phone.is_empty() { "your phone" } else { phone.as_str() });
                            }),
                            Err(message) => update(shared, |s| s.login_message = message),
                        },
                        Action::LoginUseKind(kind) => update(shared, |s| {
                            s.login_code_kind = kind;
                            s.login_message = match kind {
                                CodeKind::Backup => "Enter one of your backup codes.".to_string(),
                                _ => "Enter the 6-digit code from your authenticator app.".to_string(),
                            };
                        }),
                        Action::LoginCancel => {
                            ticket.clear();
                            update(shared, |s| {
                                s.login_stage = LoginStage::Form;
                                s.login_message.clear();
                            });
                        }
                        // Everything else belongs to a logged-in session.
                        _ => {}
                    }
                }
            }
        }
        session.cancel();
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
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

/// Closes the microphone, tells Discord the channel was left and empties the audio queues,
/// as the main app does when leaving a call.
async fn leave_voice(shared: &SharedRef, cmd_tx: &mpsc::Sender<GatewayCommand>, mic: &mut Option<MicCapture>, guild_id: &str) {
    let was_in_voice = shared.lock().map(|s| !s.voice_channel_id.is_empty()).unwrap_or(false);
    *mic = None;
    if !was_in_voice {
        return;
    }
    if let Ok(mut queues) = gateway::get_speaker_pcm_queues().lock() {
        queues.clear();
    }
    if let Ok(mut queue) = gateway::get_mic_pcm_queue().lock() {
        queue.clear();
    }
    gateway::clear_voice_participants();
    let _ = cmd_tx.send(GatewayCommand::UpdateVoiceState {
        guild_id: guild_id.to_string(),
        channel_id: None,
        self_mute: false,
        self_deaf: false,
    }).await;
    update(shared, |s| {
        s.voice_channel_id.clear();
        s.voice_channel_name.clear();
        s.voice_connected = false;
        s.voice_users.clear();
    });
}

/// Runs one logged-in session. Returns when the user logs out or the token is refused.
async fn run_session(token: String, shared: &SharedRef, actions: &mut mpsc::UnboundedReceiver<Action>) {
    let http = Arc::new(DiscordHttpClient::new(token.clone()));
    match http.get_current_user().await {
        Ok(user) => update(shared, |s| {
            s.logged_in = true;
            s.my_id = user["id"].as_str().unwrap_or("").to_string();
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

    // Voice: the open microphone and the guild of the joined channel. The voice connection
    // itself (signalling, DAVE encryption, Opus, playback) is Litecord's src/gateway/voice.rs.
    let mut mic: Option<MicCapture> = None;
    let mut voice_guild = String::new();
    let mut voice_refresh = tokio::time::interval(std::time::Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = voice_refresh.tick() => {
                // Who is in the channel and whether the call is up change without an event here.
                let (channel_id, was_connected, known) = match shared.lock() {
                    Ok(s) => (s.voice_channel_id.clone(), s.voice_connected, s.voice_users.clone()),
                    Err(_) => continue,
                };
                // Typing notices last about ten seconds unless repeated.
                let stale = shared.lock().map(|s| s.typing.iter().any(|(_, at)| at.elapsed().as_secs() >= 10)).unwrap_or(false);
                if stale {
                    update(shared, |s| s.typing.retain(|(_, at)| at.elapsed().as_secs() < 10));
                }
                if channel_id.is_empty() {
                    continue;
                }
                let connected = gateway::is_connected_to_voice();
                let users: Vec<String> = gateway::get_voice_channel_participants(&channel_id).into_iter().map(|(_, name)| name).collect();
                if connected != was_connected || users != known {
                    update(shared, |s| {
                        s.voice_connected = connected;
                        s.voice_users = users;
                    });
                }
            }
            event = event_rx.recv() => {
                let Some(event) = event else { break };
                match event {
                    GatewayEvent::Connected { user_tag } => update(shared, |s| s.status = format!("Connected as {user_tag}")),
                    GatewayEvent::Disconnected { reason } => update(shared, |s| s.status = format!("Disconnected: {reason}")),
                    GatewayEvent::VoiceStatesUpdated => update(shared, |_| {}),
                    GatewayEvent::VoiceDisconnected => update(shared, |s| s.voice_connected = false),
                    GatewayEvent::GuildLoaded { guild } => update(shared, |s| {
                        match s.guilds.iter_mut().find(|g| g.id == guild.id) {
                            Some(existing) => *existing = guild,
                            None => s.guilds.push(guild),
                        }
                    }),
                    GatewayEvent::MessageCreated { id, channel_id, author, author_id, created_at, content, attachments, reply_author, reply_content, is_self, .. } => {
                        let open = shared.lock().map(|s| s.channel_id == channel_id).unwrap_or(false);
                        if !open {
                            continue;
                        }
                        update(shared, |s| {
                            let mut text = content;
                            if !attachments.is_empty() {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(&format!("[{} attachment(s)]", attachments.len()));
                            }
                            let reply_to = if reply_author.is_empty() { String::new() } else { format!("{}: {}", reply_author, short(&reply_content, 60)) };
                            let mine = is_self || (!s.my_id.is_empty() && author_id == s.my_id);
                            // Whoever sent a message is no longer typing.
                            s.typing.retain(|(name, _)| *name != author);
                            s.messages.push(Message { id: id.clone(), author, mine, time: created_at, text, reply_to, reactions: Vec::new() });
                        });
                        // The channel is on screen: tell Discord it was read.
                        let (http_ack, channel_ack) = (Arc::clone(&http), channel_id.clone());
                        tokio::spawn(async move {
                            let _ = http_ack.ack_message(&channel_ack, &id).await;
                        });
                    }
                    GatewayEvent::MessageUpdated { id, channel_id, content, .. } => update(shared, |s| {
                        if s.channel_id == channel_id {
                            if let Some(message) = s.messages.iter_mut().find(|m| m.id == id) {
                                // Embed-only updates come with an empty text: keep what is shown.
                                if !content.is_empty() {
                                    message.text = format!("{content} (edited)");
                                }
                            }
                        }
                    }),
                    GatewayEvent::MessageDeleted { id, channel_id } => update(shared, |s| {
                        if s.channel_id == channel_id {
                            s.messages.retain(|m| m.id != id);
                        }
                    }),
                    GatewayEvent::ReactionChanged { channel_id, message_id, user_id, emoji_key, emoji_label, added } => update(shared, |s| {
                        if s.channel_id == channel_id {
                            let by_me = !s.my_id.is_empty() && user_id == s.my_id;
                            if let Some(message) = s.messages.iter_mut().find(|m| m.id == message_id) {
                                apply_reaction(&mut message.reactions, &emoji_key, &emoji_label, added, by_me);
                            }
                        }
                    }),
                    GatewayEvent::ReactionsCleared { channel_id, message_id } => update(shared, |s| {
                        if s.channel_id == channel_id {
                            if let Some(message) = s.messages.iter_mut().find(|m| m.id == message_id) {
                                message.reactions.clear();
                            }
                        }
                    }),
                    GatewayEvent::TypingStarted { channel_id, user_id, name } => update(shared, |s| {
                        if s.channel_id == channel_id && user_id != s.my_id && !name.is_empty() {
                            s.typing.retain(|(n, _)| *n != name);
                            s.typing.push((name, std::time::Instant::now()));
                        }
                    }),
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
                            s.typing.clear();
                            s.status = "Loading messages...".to_string();
                        });
                        if guild_id != "@me" {
                            let _ = cmd_tx.send(GatewayCommand::SubscribeGuild {
                                guild_id,
                                channel_ids: vec![channel_id.clone()],
                            }).await;
                        }
                        let result = http.get_channel_messages(&channel_id).await;
                        let mut newest = String::new();
                        update(shared, |s| {
                            if s.channel_id != channel_id {
                                return;
                            }
                            match result {
                                Ok(list) => {
                                    // Discord answers newest first.
                                    let my_id = s.my_id.clone();
                                    s.messages = list.iter().rev().map(|m| message_from_rest(m, &my_id)).collect();
                                    s.status = String::new();
                                    newest = s.messages.last().map(|m| m.id.clone()).unwrap_or_default();
                                }
                                Err(e) => s.status = format!("Could not load this channel: {e}"),
                            }
                        });
                        // Opening a channel marks it as read.
                        if !newest.is_empty() {
                            let _ = http.ack_message(&channel_id, &newest).await;
                        }
                    }
                    Action::Send { channel_id, text, reply_to } => {
                        if let Err(e) = http.send_message_with(&channel_id, &text, reply_to.as_deref()).await {
                            update(shared, |s| s.status = e);
                        }
                    }
                    Action::Edit { channel_id, message_id, text } => {
                        if let Err(e) = http.edit_message(&channel_id, &message_id, &text).await {
                            update(shared, |s| s.status = e);
                        }
                    }
                    Action::Delete { channel_id, message_id } => {
                        if let Err(e) = http.delete_message(&channel_id, &message_id).await {
                            update(shared, |s| s.status = e);
                        }
                    }
                    Action::React { channel_id, message_id, key, label: _ } => {
                        let mine = shared.lock().ok().and_then(|s| {
                            s.messages.iter().find(|m| m.id == message_id).map(|m| m.reactions.iter().any(|r| r.key == key && r.me))
                        }).unwrap_or(false);
                        let result = if mine {
                            http.remove_reaction(&channel_id, &message_id, &key).await
                        } else {
                            http.add_reaction(&channel_id, &message_id, &key).await
                        };
                        if let Err(e) = result {
                            update(shared, |s| s.status = e);
                        }
                    }
                    Action::Typing { channel_id } => {
                        let http_typing = Arc::clone(&http);
                        tokio::spawn(async move {
                            let _ = http_typing.trigger_typing(&channel_id).await;
                        });
                    }
                    Action::Upload { channel_id, path, reply_to } => {
                        update(shared, |s| s.status = format!("Sending {path}..."));
                        let result = match tokio::fs::read(&path).await {
                            Ok(bytes) => http.send_file(&channel_id, "", reply_to.as_deref(), &path, bytes).await,
                            Err(e) => Err(format!("File not sent: cannot read {path} ({e})")),
                        };
                        update(shared, |s| s.status = result.err().unwrap_or_default());
                    }
                    Action::JoinVoice { guild_id, channel_id, name } => {
                        if mic.is_none() {
                            // The level meter is not shown here; the receiver just has to exist.
                            let (level_tx, mut level_rx) = mpsc::channel::<f32>(8);
                            tokio::spawn(async move { while level_rx.recv().await.is_some() {} });
                            mic = Some(start_mic_capture(String::new(), level_tx));
                        }
                        voice_guild = guild_id.clone();
                        let muted = shared.lock().map(|s| s.muted).unwrap_or(false);
                        let _ = cmd_tx.send(GatewayCommand::UpdateVoiceState {
                            guild_id,
                            channel_id: Some(channel_id.clone()),
                            self_mute: muted,
                            self_deaf: false,
                        }).await;
                        update(shared, |s| {
                            s.voice_channel_id = channel_id;
                            s.voice_channel_name = name;
                            s.voice_connected = false;
                            s.voice_users.clear();
                        });
                    }
                    Action::LeaveVoice => {
                        leave_voice(shared, &cmd_tx, &mut mic, &voice_guild).await;
                    }
                    Action::ToggleMute => {
                        let (muted, channel_id) = match shared.lock() {
                            Ok(mut s) => {
                                s.muted = !s.muted;
                                s.version += 1;
                                (s.muted, s.voice_channel_id.clone())
                            }
                            Err(_) => continue,
                        };
                        if !channel_id.is_empty() {
                            let _ = cmd_tx.send(GatewayCommand::UpdateVoiceState {
                                guild_id: voice_guild.clone(),
                                channel_id: Some(channel_id),
                                self_mute: muted,
                                self_deaf: false,
                            }).await;
                        }
                    }
                    Action::PasswordLogin { .. } | Action::LoginCode { .. } | Action::LoginSendSms | Action::LoginUseKind(_) | Action::LoginCancel => {}
                    Action::LogOut => {
                        leave_voice(shared, &cmd_tx, &mut mic, &voice_guild).await;
                        let _ = std::fs::remove_file(token_path());
                        break;
                    }
                }
            }
        }
    }
    gateway_task.abort();
    drop(mic);
    update(shared, |s| {
        s.voice_channel_id.clear();
        s.voice_channel_name.clear();
        s.voice_connected = false;
        s.voice_users.clear();
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
            None => log_in(&shared, &mut actions).await,
        };
        run_session(token, &shared, &mut actions).await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
