//! What runs beside the window: QR login, the gateway connection, message loading and sending.
//! All of it is Litecord's own network code (see lib.rs); this file only moves its results
//! into the state the window draws.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use litecord_legacy::audio::engine::{start_mic_capture, MicCapture};
use crate::accounts::Store;
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

/// A decoded picture (avatar, server icon, emoji): RGBA, row by row.
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// A piece of a line of text.
#[derive(Clone)]
pub enum Span {
    Text(String),
    Link(String),
    /// Picture address, and the text to fall back on.
    Emoji(String, String),
}

/// Where the picture of an emoji id from the message parser is: `u_<code points>` is a
/// Unicode emoji (Twemoji 14, as the main app uses), anything else a Discord custom emoji.
pub fn emoji_url(id: &str) -> String {
    match id.strip_prefix("u_") {
        Some(hex) => format!("https://cdnjs.cloudflare.com/ajax/libs/twemoji/14.0.2/72x72/{hex}.png"),
        None => format!("https://cdn.discordapp.com/emojis/{id}.png?size=48&quality=lossless"),
    }
}

/// Plain text (a name, a status) split into text and Unicode emoji.
pub fn spans_from_text(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some((start, len, emoji, id)) = gateway::find_next_unicode_emoji(rest) {
        if start > 0 {
            spans.push(Span::Text(rest[..start].to_string()));
        }
        spans.push(Span::Emoji(emoji_url(&id), emoji));
        rest = &rest[start + len..];
    }
    if !rest.is_empty() {
        spans.push(Span::Text(rest.to_string()));
    }
    spans
}

/// The parser's lines (text, links, custom and Unicode emoji) as spans.
fn spans_from_lines(lines: &[gateway::MessageLineData]) -> Vec<Vec<Span>> {
    lines
        .iter()
        .map(|line| {
            let mut spans = Vec::new();
            for block in &line.blocks {
                if block.is_emoji && !block.emoji_id.is_empty() {
                    spans.push(Span::Emoji(emoji_url(&block.emoji_id), block.text.clone()));
                } else if block.is_link {
                    spans.push(Span::Link(block.text.clone()));
                } else {
                    spans.extend(spans_from_text(&block.text));
                }
            }
            spans
        })
        .collect()
}

/// Discord serves avatars and icons as WebP by default; this app reads PNG.
fn as_png(url: &str) -> String {
    url.replace(".webp", ".png")
}

fn decode_png(bytes: &[u8]) -> Option<Picture> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    let data = &buffer[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 0xFF]).collect(),
        png::ColorType::GrayscaleAlpha => data.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|g| [*g, *g, *g, 0xFF]).collect(),
        png::ColorType::Indexed => return None, // EXPAND turns palettes into RGB(A)
    };
    if rgba.len() != (info.width * info.height * 4) as usize || info.width > 1024 || info.height > 1024 {
        return None;
    }
    Some(Picture { w: info.width, h: info.height, rgba })
}

/// Downloads and decodes one picture; `None` when that fails.
pub async fn fetch_picture(client: &reqwest::Client, url: &str) -> Option<Arc<Picture>> {
    let reply = client.get(url).timeout(std::time::Duration::from_secs(15)).send().await.ok()?;
    if !reply.status().is_success() {
        return None;
    }
    let bytes = reply.bytes().await.ok()?;
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    decode_png(&bytes).map(Arc::new)
}

pub struct Message {
    pub id: String,
    pub author: String,
    /// Address of the author's picture (empty: none, a coloured circle is drawn).
    pub avatar_url: String,
    /// The text as lines of text, links and emoji; then what the embeds say.
    pub lines: Vec<Vec<Span>>,
    pub embed_lines: Vec<Vec<Span>>,
    /// Written by the logged-in user (so it can be edited and deleted).
    pub mine: bool,
    pub time: String,
    pub text: String,
    /// "Name: start of the message" when this is a reply.
    pub reply_to: String,
    pub reactions: Vec<ReactionData>,
}

/// One saved account as the window shows it. Tokens never leave the backend.
#[derive(Clone, Debug)]
pub struct AccountView {
    /// Identifies the account in the actions (see `accounts::Account::key`).
    pub key: String,
    pub name: String,
    pub avatar: String,
    pub active: bool,
}

pub struct Shared {
    /// Bumped on every change, so the window knows when to redraw.
    pub version: u64,
    /// The saved accounts, the active one marked.
    pub accounts: Vec<AccountView>,
    /// The login screen is open to add one more account (it has a Back button).
    pub adding: bool,
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
    /// Id and picture address of the logged-in user.
    pub my_id: String,
    pub my_avatar: String,
    /// Server id to icon address.
    pub guild_icons: HashMap<String, String>,
    /// Pictures by address: `None` while loading or when it could not be loaded.
    pub images: HashMap<String, Option<Arc<Picture>>>,
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
    /// The window needs this picture (asked once per address).
    FetchImage(String),
    PasswordLogin { login: String, password: String },
    LoginCode { code: String },
    LoginSendSms,
    LoginUseKind(CodeKind),
    LoginCancel,
    JoinVoice { guild_id: String, channel_id: String, name: String },
    LeaveVoice,
    ToggleMute,
    LogOut,
    /// Signs the current account out and moves on to another saved one (or the login screen).
    SwitchAccount(String),
    /// Opens the login screen to add one more account, keeping the current one.
    AddAccount,
    /// Back from adding an account, to the one that was active.
    LoginBack,
    /// Opens the system's file dialog and sends the chosen file to the open channel.
    PickFile { channel_id: String, reply_to: Option<String> },
}

/// How the login screen ended.
pub enum LoginOutcome {
    /// A token to check and use.
    Token(String),
    /// The user picked a saved account instead.
    Switch(String),
    /// Adding an account was cancelled.
    Back,
}

/// Why a logged-in session ended.
pub enum SessionEnd {
    /// Go on with whichever account is active now (switched, signed out, or failed).
    Next,
    /// Show the login screen to add an account.
    Add,
}

/// Set while the system's file dialog is open: one at a time.
static PICKER_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Default for Shared {
    fn default() -> Self {
        Shared {
            version: 0,
            accounts: Vec::new(),
            adding: false,
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
            my_avatar: String::new(),
            guild_icons: HashMap::new(),
            images: HashMap::new(),
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

/// The saved accounts (see accounts.rs). The single session file of earlier versions is
/// read once, for migration.
fn accounts_path() -> PathBuf {
    data_dir().join("accounts.json")
}

fn session_path() -> PathBuf {
    data_dir().join("session")
}

/// Copies the store's accounts into the window's list, without the tokens.
fn publish_accounts(shared: &SharedRef, store: &Store) {
    let views: Vec<AccountView> = store
        .accounts
        .iter()
        .map(|a| AccountView {
            key: a.key(),
            name: if a.name.is_empty() { "Account".to_string() } else { a.name.clone() },
            avatar: a.avatar.clone(),
            active: a.key() == store.active,
        })
        .collect();
    update(shared, |s| s.accounts = views);
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

/// Lines for the attachments of a message: this window shows their names only.
fn attachment_lines(attachments: &[gateway::MessageAttachmentData]) -> Vec<Vec<Span>> {
    attachments.iter().map(|a| vec![Span::Text(format!("[file] {} ({})", a.filename, a.size_str))]).collect()
}

fn message_from_rest(m: &Value, my_id: &str) -> Message {
    let parts = gateway::format_discord_message_parts(m);
    let (content_lines, embed_lines, attachments) = (&parts.2, &parts.4, &parts.13);
    let mut lines = spans_from_lines(content_lines);
    lines.extend(attachment_lines(attachments));
    if m["edited_timestamp"].is_string() {
        match lines.last_mut() {
            Some(last) => last.push(Span::Text(" (edited)".to_string())),
            None => lines.push(vec![Span::Text("(edited)".to_string())]),
        }
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
        avatar_url: as_png(&litecord_legacy::ui::message_grouping::avatar_url_for_message(m)),
        lines,
        embed_lines: spans_from_lines(embed_lines),
        mine: !my_id.is_empty() && m["author"]["id"].as_str() == Some(my_id),
        time: local_time(m["timestamp"].as_str().unwrap_or("")),
        text: m["content"].as_str().unwrap_or("").to_string(),
        reply_to,
        reactions: reactions_from_message(m),
    }
}

/// Shows the login screen until a login succeeds: the QR code, and the email and password
/// form (src/auth/password_auth.rs). Returns the session token.
async fn log_in(shared: &SharedRef, actions: &mut mpsc::UnboundedReceiver<Action>, adding: bool) -> LoginOutcome {
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
                            return LoginOutcome::Token(token);
                        }
                        Some(RemoteAuthEvent::Error(e)) => {
                            update(shared, |s| s.status = format!("QR login failed: {e}. Trying again..."));
                            qr_failed = true;
                        }
                        Some(RemoteAuthEvent::Cancelled) | None => qr_failed = true,
                    }
                }
                action = actions.recv() => {
                    let Some(action) = action else { return std::future::pending::<LoginOutcome>().await };
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
                                    return LoginOutcome::Token(token);
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
                                    return LoginOutcome::Token(token);
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
                        Action::SwitchAccount(key) => return LoginOutcome::Switch(key),
                        Action::LoginBack if adding => return LoginOutcome::Back,
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
            Some((last, ChannelData { id, name, is_voice: false, is_category: false, parent_id: None, position: 0, topic: None }))
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
async fn run_session(token: String, shared: &SharedRef, actions: &mut mpsc::UnboundedReceiver<Action>, store: &mut Store) -> SessionEnd {
    let http = Arc::new(DiscordHttpClient::new(token.clone()));
    // Until the check answers, the account is the one this token is known as (or no one).
    let known_id = store.accounts.iter().find(|a| a.token == token).map(|a| a.id.clone()).unwrap_or_default();
    update(shared, |s| s.my_id = known_id.clone());
    // A check that never answers (a stalled connection) must not hold up switching accounts.
    let checked = match tokio::time::timeout(std::time::Duration::from_secs(20), http.get_current_user()).await {
        Ok(result) => result,
        Err(_) => Err("the check timed out".to_string()),
    };
    match checked {
        Ok(user) => {
            let id = user["id"].as_str().unwrap_or("").to_string();
            let avatar = match user["avatar"].as_str() {
                Some(hash) if !id.is_empty() && litecord_legacy::ui::message_grouping::valid_avatar_hash(hash) => {
                    format!("https://cdn.discordapp.com/avatars/{id}/{hash}.png?size=64")
                }
                _ => String::new(),
            };
            let name = display_name(&user);
            store.upsert(&token, &id, &name, &avatar);
            publish_accounts(shared, store);
            update(shared, |s| {
                s.logged_in = true;
                s.my_id = id.clone();
                s.my_avatar = avatar.clone();
                s.user = name.clone();
                s.status = "Connecting...".to_string();
            });
        }
        Err(e) if e.contains("401") => {
            // This account's login is refused; the others stay.
            let name = store.accounts.iter().find(|a| a.token == token).map(|a| a.name.clone()).unwrap_or_default();
            if let Some(key) = store.accounts.iter().find(|a| a.token == token).map(|a| a.key()) {
                store.remove(&key);
            }
            publish_accounts(shared, store);
            let who = if name.is_empty() { "This account".to_string() } else { name };
            update(shared, |s| s.status = format!("The saved login for {who} is no longer valid. Choose another account or sign in again."));
            return SessionEnd::Next;
        }
        Err(e) => {
            // Could not be checked (offline, say): the account is kept, and the session goes on.
            store.upsert(&token, "", "", "");
            publish_accounts(shared, store);
            update(shared, |s| {
                s.logged_in = true;
                s.status = format!("Could not read the account ({e}). Connecting anyway...");
            });
        }
    }
    let mut end = SessionEnd::Next;

    let (event_tx, mut event_rx) = mpsc::channel::<GatewayEvent>(512);
    let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(32);
    let gateway = Arc::new(GatewayClient::new(token, event_tx));
    let gateway_task = tokio::spawn(async move { gateway.start(cmd_rx).await });
    // Direct messages and server icons load in the background: until they are in, the
    // session already answers switching, adding and logging out.
    {
        let (http_bg, shared_bg) = (Arc::clone(&http), Arc::clone(shared));
        tokio::spawn(async move {
            load_direct_messages(&http_bg, &shared_bg).await;
            if let Ok(guilds) = http_bg.get_user_guilds().await {
                update(&shared_bg, |s| {
                    for guild in &guilds {
                        if let (Some(id), Some(icon)) = (guild["id"].as_str(), guild["icon"].as_str()) {
                            if id.bytes().all(|b| b.is_ascii_digit()) && litecord_legacy::ui::message_grouping::valid_avatar_hash(icon) {
                                s.guild_icons.insert(id.to_string(), format!("https://cdn.discordapp.com/icons/{id}/{icon}.png?size=64"));
                            }
                        }
                    }
                });
            }
        });
    }
    // Pictures are fetched without the login: they come from public image servers.
    let picture_client = litecord_legacy::utils::build_generic_http_client();
    let picture_slots = Arc::new(tokio::sync::Semaphore::new(6));

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
                    GatewayEvent::MessageCreated { id, channel_id, author, author_id, created_at, avatar_url, content, content_lines, embed_lines, attachments, reply_author, reply_content, is_self, .. } => {
                        let open = shared.lock().map(|s| s.channel_id == channel_id).unwrap_or(false);
                        if !open {
                            continue;
                        }
                        update(shared, |s| {
                            let text = content;
                            let mut lines = spans_from_lines(&content_lines);
                            lines.extend(attachment_lines(&attachments));
                            let reply_to = if reply_author.is_empty() { String::new() } else { format!("{}: {}", reply_author, short(&reply_content, 60)) };
                            let mine = is_self || (!s.my_id.is_empty() && author_id == s.my_id);
                            // Whoever sent a message is no longer typing.
                            s.typing.retain(|(name, _)| *name != author);
                            s.messages.push(Message {
                                id: id.clone(),
                                author,
                                avatar_url: as_png(&avatar_url),
                                lines,
                                embed_lines: spans_from_lines(&embed_lines),
                                mine,
                                time: created_at,
                                text,
                                reply_to,
                                reactions: Vec::new(),
                            });
                        });
                        // The channel is on screen: tell Discord it was read.
                        let (http_ack, channel_ack) = (Arc::clone(&http), channel_id.clone());
                        tokio::spawn(async move {
                            let _ = http_ack.ack_message(&channel_ack, &id).await;
                        });
                    }
                    GatewayEvent::MessageUpdated { id, channel_id, content, content_lines, embed_lines, .. } => update(shared, |s| {
                        if s.channel_id == channel_id {
                            if let Some(message) = s.messages.iter_mut().find(|m| m.id == id) {
                                // Embed-only updates come with an empty text: keep what is shown.
                                if !content.is_empty() {
                                    message.text = content;
                                    message.lines = spans_from_lines(&content_lines);
                                    match message.lines.last_mut() {
                                        Some(last) => last.push(Span::Text(" (edited)".to_string())),
                                        None => {}
                                    }
                                }
                                if !embed_lines.is_empty() {
                                    message.embed_lines = spans_from_lines(&embed_lines);
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
                    Action::FetchImage(url) => {
                        // Only the image servers this app builds addresses for.
                        if !(url.starts_with("https://cdn.discordapp.com/") || url.starts_with("https://cdnjs.cloudflare.com/ajax/libs/twemoji/")) {
                            continue;
                        }
                        let (client, slots, shared_pictures) = (picture_client.clone(), Arc::clone(&picture_slots), Arc::clone(shared));
                        tokio::spawn(async move {
                            let _slot = slots.acquire().await;
                            let picture = fetch_picture(&client, &url).await;
                            // A failed picture stays `None`: it is not asked for again.
                            update(&shared_pictures, |s| {
                                s.images.insert(url, picture);
                            });
                        });
                    }
                    Action::Typing { channel_id } => {
                        let http_typing = Arc::clone(&http);
                        tokio::spawn(async move {
                            let _ = http_typing.trigger_typing(&channel_id).await;
                        });
                    }
                    Action::Upload { channel_id, path, reply_to } => {
                        update(shared, |s| s.status = format!("Sending {path}..."));
                        let result = http.send_path(&channel_id, reply_to.as_deref(), std::path::Path::new(&path)).await;
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
                    Action::PasswordLogin { .. } | Action::LoginCode { .. } | Action::LoginSendSms | Action::LoginUseKind(_) | Action::LoginCancel | Action::LoginBack => {}
                    Action::LogOut => {
                        leave_voice(shared, &cmd_tx, &mut mic, &voice_guild).await;
                        if let Some(key) = store.active_account().map(|a| a.key()) {
                            store.remove(&key);
                        }
                        publish_accounts(shared, store);
                        break;
                    }
                    Action::SwitchAccount(key) => {
                        leave_voice(shared, &cmd_tx, &mut mic, &voice_guild).await;
                        store.set_active(&key);
                        publish_accounts(shared, store);
                        break;
                    }
                    Action::AddAccount => {
                        leave_voice(shared, &cmd_tx, &mut mic, &voice_guild).await;
                        end = SessionEnd::Add;
                        break;
                    }
                    Action::PickFile { channel_id, reply_to } => {
                        if PICKER_OPEN.swap(true, std::sync::atomic::Ordering::SeqCst) {
                            continue;
                        }
                        update(shared, |s| s.status = "Choose a file in the dialog...".to_string());
                        let account_at_pick = shared.lock().map(|s| s.my_id.clone()).unwrap_or_default();
                        let (http_pick, shared_pick) = (Arc::clone(&http), Arc::clone(shared));
                        tokio::spawn(async move {
                            let picked = tokio::task::spawn_blocking(|| litecord_legacy::file_picker::pick_file("Choose a file to send")).await;
                            PICKER_OPEN.store(false, std::sync::atomic::Ordering::SeqCst);
                            let result = match picked {
                                Ok(Ok(Some(path))) => {
                                    // Logged out or switched while the dialog was open: nothing is sent.
                                    if shared_pick.lock().map(|s| s.my_id.clone()).unwrap_or_default() != account_at_pick {
                                        update(&shared_pick, |s| s.status = "Upload cancelled: the account changed.".to_string());
                                        return;
                                    }
                                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                                    update(&shared_pick, |s| s.status = format!("Sending {name}..."));
                                    http_pick.send_path(&channel_id, reply_to.as_deref(), &path).await
                                }
                                Ok(Ok(None)) => Ok(()),
                                Ok(Err(e)) => Err(e),
                                Err(e) => Err(format!("The file dialog failed: {e}")),
                            };
                            update(&shared_pick, |s| s.status = result.err().unwrap_or_default());
                        });
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
        s.guild_icons.clear();
        s.my_avatar.clear();
        s.guilds.clear();
        s.messages.clear();
        s.channel_id.clear();
        s.user.clear();
    });
    end
}

pub async fn run(shared: SharedRef, mut actions: mpsc::UnboundedReceiver<Action>) {
    let mut store = Store::load(&accounts_path(), &session_path());
    // A token from the login screen to use next (after adding an account).
    let mut next: Option<String> = None;
    loop {
        publish_accounts(&shared, &store);
        let token = match next.take() {
            Some(token) => token,
            None => match store.active_account().map(|a| a.token.clone()) {
                Some(token) => token,
                None => {
                    update(&shared, |s| s.adding = false);
                    match log_in(&shared, &mut actions, false).await {
                        LoginOutcome::Token(token) => token,
                        LoginOutcome::Switch(key) => {
                            store.set_active(&key);
                            continue;
                        }
                        LoginOutcome::Back => continue,
                    }
                }
            },
        };
        match run_session(token, &shared, &mut actions, &mut store).await {
            SessionEnd::Next => {}
            SessionEnd::Add => {
                update(&shared, |s| s.adding = true);
                match log_in(&shared, &mut actions, true).await {
                    LoginOutcome::Token(token) => next = Some(token),
                    LoginOutcome::Switch(key) => store.set_active(&key),
                    // Back: the account that was active is used again.
                    LoginOutcome::Back => {}
                }
                update(&shared, |s| s.adding = false);
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Made-up content for `--render-demo`: a few servers, channels and messages that use
/// every part of the interface (replies, links, emoji, reactions, an embed, voice).
pub fn demo_state() -> Shared {
    let channel = |id: &str, name: &str, voice: bool, category: bool, position: i64| ChannelData {
        id: id.to_string(),
        name: name.to_string(),
        is_voice: voice,
        is_category: category,
        parent_id: None,
        position,
        topic: None,
    };
    let guild = |id: &str, name: &str, channels: Vec<ChannelData>| GuildData { id: id.to_string(), name: name.to_string(), channels };
    let text = |t: &str| vec![spans_from_text(t)];
    let message = |id: &str, author: &str, time: &str, lines: Vec<Vec<Span>>, reply: &str, mine: bool, reactions: Vec<ReactionData>| Message {
        id: id.to_string(),
        author: author.to_string(),
        avatar_url: String::new(),
        lines,
        embed_lines: Vec::new(),
        mine,
        time: time.to_string(),
        text: String::new(),
        reply_to: reply.to_string(),
        reactions,
    };
    let reaction = |emoji: &str, count: u32, me: bool| ReactionData { key: emoji.to_string(), label: emoji.to_string(), count, me };
    let mut embed = message("5", "News Bot", "12/11 20:41", text("A new build is out."), "", false, Vec::new());
    embed.embed_lines = vec![
        vec![Span::Text("Litecord nightly".to_string())],
        vec![Span::Text("Text chat, voice, reactions and more. ".to_string()), Span::Link("github.com/KyttuXD/Litecord".to_string())],
    ];
    Shared {
        logged_in: true,
        user: "Kyttu".to_string(),
        status: "Connected as kyttu".to_string(),
        guilds: vec![
            guild("@me", "Direct Messages", vec![channel("d1", "Ana Souza", false, false, 0)]),
            guild(
                "1",
                "Old Macs Club",
                vec![
                    channel("c0", "Text channels", false, true, 0),
                    channel("c1", "general", false, false, 1),
                    channel("c2", "snow-leopard", false, false, 2),
                    channel("c3", "Voice channels", false, true, 3),
                    channel("c4", "Lounge", true, false, 4),
                ],
            ),
            guild("2", "Rust Brasil", vec![channel("c5", "geral", false, false, 0)]),
            guild("3", "Pixel Art", vec![channel("c6", "showcase", false, false, 0)]),
        ],
        channel_id: "c1".to_string(),
        messages: vec![
            message("1", "Ana Souza", "12/11 20:30", text("Did the Mac Pro boot with the new build?"), "", false, Vec::new()),
            message("2", "Ana Souza", "12/11 20:30", text("I can test on the MacBook Air later today."), "", false, Vec::new()),
            message(
                "3",
                "Kyttu",
                "12/11 20:34",
                text("It did! Window, sound and the login code all work on 10.6.8 \u{1F389}"),
                "Ana Souza: Did the Mac Pro boot with the new build?",
                true,
                vec![reaction("\u{1F44D}", 3, true), reaction("\u{2764}\u{FE0F}", 1, false)],
            ),
            message(
                "4",
                "Bo Lindqvist",
                "12/11 20:36",
                vec![
                    spans_from_text("Nice \u{1F602} here is the long line that has to wrap around the edge of the window so that the wrapping of words can be seen in the picture, with a link in it:"),
                    vec![Span::Link("https://github.com/KyttuXD/Litecord/releases/tag/nightly".to_string())],
                ],
                "",
                false,
                vec![reaction("\u{1F525}", 2, false)],
            ),
            embed,
        ],
        typing: vec![("Ana Souza".to_string(), std::time::Instant::now())],
        accounts: vec![
            AccountView { key: "1".to_string(), name: "Kyttu".to_string(), avatar: String::new(), active: true },
            AccountView { key: "2".to_string(), name: "Ana Souza".to_string(), avatar: String::new(), active: false },
        ],
        voice_channel_id: "c4".to_string(),
        voice_channel_name: "Lounge".to_string(),
        voice_connected: true,
        voice_users: vec!["Kyttu".to_string(), "Bo Lindqvist".to_string()],
        ..Shared::default()
    }
}
