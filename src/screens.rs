//! Headless screenshot mode: `litecord --screens <dir>`.
//!
//! Builds the real `AppWindow` on a software renderer at a fixed size (1280x800, scale 1,
//! dark theme, English), fills it with deterministic sample data through the same setters
//! the application uses, and writes one PNG per screen, menu, popup and dialog together
//! with an `index.json`. Nothing here touches the network, the gateway, audio, the tray or
//! saved settings: the process switches into `<dir>/.data` first, so every file the
//! application would write with a relative path lands there instead.
//!
//! The window runs on a tiny custom Slint platform (no display needed, a fake clock so
//! animations finish at once), which makes the output the same on Linux and macOS apart
//! from what the operating system itself contributes (system fonts, for example).
//! `scripts/ui_diff.py` compares two such directories. See `docs/UI_PARITY.md`.

use std::cell::Cell;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Image, LogicalPosition, ModelRc, PhysicalSize, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::{
    AccountItem, AppWindow, AudioDeviceItem, CameraSourceItem, ChannelItem, ChatMessage,
    FriendItem, GuildItem, InviteCard, LanguageItem, LinkItem, MemberRowItem, MessageAttachment,
    MessageBlock, MessageButton, MessageLine, PopoutStreamWindow, ProfileRole, ScreenSourceItem,
    SettingOptionItem, VoiceParticipant, WindowSourceItem,
};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;

thread_local! {
    /// Milliseconds on the fake clock the Slint platform reports.
    static CLOCK_MS: Cell<u64> = Cell::new(0);
}

struct ScreensPlatform;

impl Platform for ScreensPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        Ok(window)
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(CLOCK_MS.with(|c| c.get()))
    }
}

/// `--screens <dir>` or `--screens=<dir>`, when present on the command line.
pub fn requested_dir() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--screens" {
            return Some(args.next().unwrap_or_else(|| "screens".to_string()));
        }
        if let Some(rest) = arg.strip_prefix("--screens=") {
            return Some(rest.to_string());
        }
    }
    None
}

/// Renders every screen into `dir` and returns once `index.json` is written.
pub fn run(dir: &str) -> Result<(), Box<dyn Error>> {
    let out = absolute(Path::new(dir))?;
    std::fs::create_dir_all(&out)?;
    // Old pictures must not survive into the comparison.
    for entry in std::fs::read_dir(&out)? {
        let path = entry?.path();
        if path.extension().map(|e| e == "png").unwrap_or(false) {
            let _ = std::fs::remove_file(path);
        }
    }
    let data_dir = out.join(".data");
    std::fs::create_dir_all(&data_dir)?;
    std::env::set_current_dir(&data_dir)?;

    slint::platform::set_platform(Box::new(ScreensPlatform))
        .map_err(|e| format!("could not install the screenshot platform: {e}"))?;

    let ui = AppWindow::new()?;
    crate::apply_i18n_translations(&ui, crate::utils::i18n::Language::English);
    ui.set_use_light_theme(false);
    #[cfg(target_os = "linux")]
    ui.set_is_linux(true);
    ui.set_app_version("v1.3.1-beta".into());
    ui.show()?;
    ui.window().set_size(PhysicalSize::new(WIDTH, HEIGHT));

    let mut shots = Shots { ui, out, entries: Vec::new() };
    shots.capture_all()?;
    shots.write_index()?;
    println!("screens: wrote {} screenshots to {}", shots.entries.len(), shots.out.display());
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

struct Entry {
    file: String,
    name: String,
    title: String,
    width: u32,
    height: u32,
}

struct Shots {
    ui: AppWindow,
    out: PathBuf,
    entries: Vec<Entry>,
}

fn pump(ms: u64) {
    let mut left = ms;
    while left > 0 {
        let step = left.min(50);
        CLOCK_MS.with(|c| c.set(c.get() + step));
        slint::platform::update_timers_and_animations();
        left -= step;
    }
}

fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(rows))
}

fn text_model(rows: &[&str]) -> ModelRc<SharedString> {
    model(rows.iter().map(|s| SharedString::from(*s)).collect())
}

impl Shots {
    /// Lets layout, change handlers and animations run to their end.
    fn settle(&self, window: &slint::Window) -> Result<(), Box<dyn Error>> {
        // A render first: bindings are evaluated (and animations started) while rendering.
        window.take_snapshot()?;
        pump(1000);
        window.take_snapshot()?;
        pump(1000);
        Ok(())
    }

    fn save(&mut self, window: &slint::Window, name: &str, title: &str) -> Result<(), Box<dyn Error>> {
        self.settle(window)?;
        let buffer = window.take_snapshot()?;
        let file = format!("{:02}-{}.png", self.entries.len() + 1, name);
        image::save_buffer(
            self.out.join(&file),
            buffer.as_bytes(),
            buffer.width(),
            buffer.height(),
            image::ColorType::Rgba8,
        )?;
        println!("screens: {file}  {title}");
        self.entries.push(Entry {
            file,
            name: name.to_string(),
            title: title.to_string(),
            width: buffer.width(),
            height: buffer.height(),
        });
        Ok(())
    }

    /// Screenshot of the main window; the chat is scrolled to its newest message first.
    fn shot(&mut self, name: &str, title: &str) -> Result<(), Box<dyn Error>> {
        let ui = self.ui.clone_strong();
        ui.window().set_size(PhysicalSize::new(WIDTH, HEIGHT));
        // No hover highlight from wherever an earlier click left the pointer.
        ui.window().dispatch_event(WindowEvent::PointerExited);
        ui.window().take_snapshot()?;
        pump(100);
        ui.invoke_scroll_chat_to_bottom();
        self.save(ui.window(), name, title)
    }

    /// Clicks with the left button at a window position (logical pixels).
    fn click(&self, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        let window = self.ui.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        window.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        let _ = window.take_snapshot();
        pump(300);
    }

    /// Turns the mouse wheel by `dy` logical pixels (negative scrolls the content up).
    fn scroll(&self, x: f32, y: f32, dy: f32) {
        let position = LogicalPosition::new(x, y);
        let window = self.ui.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerScrolled { position, delta_x: 0.0, delta_y: dy });
        let _ = window.take_snapshot();
        pump(1500);
    }

    /// Opens the settings window on `tab` (a fresh instance, so no popup is left open).
    fn open_settings(&self, tab: i32) {
        self.ui.set_show_settings_modal(false);
        let _ = self.ui.window().take_snapshot();
        pump(100);
        self.ui.set_settings_active_tab(tab);
        self.ui.set_show_settings_modal(true);
        let _ = self.ui.window().take_snapshot();
        pump(100);
    }

    fn write_index(&self) -> Result<(), Box<dyn Error>> {
        let screens: Vec<serde_json::Value> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                serde_json::json!({
                    "index": i + 1,
                    "file": e.file,
                    "name": e.name,
                    "title": e.title,
                    "width": e.width,
                    "height": e.height,
                })
            })
            .collect();
        let index = serde_json::json!({
            "app_version": env!("CARGO_PKG_VERSION"),
            "renderer": "software",
            "width": WIDTH,
            "height": HEIGHT,
            "scale": 1,
            "language": "en",
            "screens": screens,
        });
        std::fs::write(self.out.join("index.json"), serde_json::to_string_pretty(&index)?)?;
        Ok(())
    }

    fn capture_all(&mut self) -> Result<(), Box<dyn Error>> {
        self.login_screens()?;
        self.chat_screens()?;
        self.direct_message_screens()?;
        self.social_screens()?;
        self.voice_screens()?;
        self.dialog_screens()?;
        self.stream_modal_screens()?;
        self.settings_screens()?;
        self.light_theme_screens()?;
        self.popout_screen()?;
        Ok(())
    }

    // ---------------------------------------------------------------- login

    fn login_screens(&mut self) -> Result<(), Box<dyn Error>> {
        let ui = self.ui.clone_strong();
        ui.set_is_logged_in(false);
        ui.set_has_qr_code(false);
        self.shot("login-qr-not-generated", "Login: token and QR panel before a QR code exists")?;

        ui.set_qr_code_image(qr_image("https://discord.com/ra/litecord-sample-fingerprint"));
        ui.set_has_qr_code(true);
        self.shot("login-qr", "Login: token field next to the QR code")?;

        ui.set_qr_scanned_user("Mira".into());
        self.shot("login-qr-scanned", "Login: QR code scanned, waiting for approval on the phone")?;
        ui.set_qr_scanned_user("".into());

        ui.set_login_alert_message("The token was rejected. Check it and try again.".into());
        ui.set_login_alert_is_error(true);
        self.shot("login-error-banner", "Login: error banner above the card")?;

        ui.set_login_alert_message("Signed out. Sign in again to continue.".into());
        ui.set_login_alert_is_error(false);
        self.shot("login-info-banner", "Login: information banner above the card")?;

        ui.set_login_alert_message("".into());
        Ok(())
    }

    // ----------------------------------------------------------------- chat

    /// Logged in with the sample server, text channel `#general` open.
    fn enter_chat(&self) {
        let ui = &self.ui;
        ui.set_is_logged_in(true);
        ui.set_user_tag("sample#0001".into());
        ui.set_user_display_name("Sample User".into());
        ui.set_user_handle("sample".into());
        ui.set_user_avatar_initials("S".into());
        ui.set_user_avatar_has_image(true);
        ui.set_user_avatar_img(avatar_image(0, 64));
        ui.set_connection_status("Connected".into());
        ui.set_guilds(model(sample_guilds()));
        ui.set_active_guild_id("g1".into());
        ui.set_active_guild_name("Litecord HQ".into());
        ui.set_channels(model(sample_channels()));
        ui.set_channel_emoji_pool(model(vec![emoji_image(0)]));
        ui.set_active_channel_id("c-general".into());
        ui.set_active_channel_name("general".into());
        ui.set_messages(model(sample_messages().into_iter().take(5).collect()));
        ui.set_has_more_older_messages(false);
        ui.set_is_loading_older_messages(false);
        ui.set_is_voice_focused(false);
        ui.set_is_in_voice(false);
        ui.set_is_voice_connecting(false);
        ui.set_show_command_suggestions(false);
        ui.set_active_command_name("".into());
        ui.set_active_param_name("".into());
        ui.set_chat_input_text("".into());
        ui.set_titlebar_error("".into());
        ui.set_hardware_cpu_text("CPU 7%".into());
        ui.set_hardware_ram_text("RAM 142 MB".into());
        ui.set_hardware_gpu_text("GPU 3%".into());
        ui.set_saved_accounts(model(sample_accounts()));
        ui.set_member_rows(model(sample_members()));
        ui.set_show_member_list(true);
        ui.set_show_friends(false);
        ui.set_show_profile_popout(false);
        ui.set_show_server_menu(false);
    }

    fn chat_screens(&mut self) -> Result<(), Box<dyn Error>> {
        self.enter_chat();
        let ui = self.ui.clone_strong();
        let all = sample_messages();
        ui.set_messages(model(all[0..5].to_vec()));
        self.shot("chat-server", "Chat: server, channels, replies, links, commands and emoji")?;
        ui.set_messages(model(all[4..7].to_vec()));
        self.shot("chat-embed-buttons", "Chat: command reply, embed with buttons, code block")?;
        ui.set_messages(model(all[7..8].to_vec()));
        self.shot("chat-attachments", "Chat: image, not yet downloaded image and file attachments")?;
        ui.set_messages(model(all[8..9].to_vec()));
        self.shot("chat-invites", "Chat: server invite and event cards")?;
        ui.set_messages(model(all[0..5].to_vec()));

        ui.set_has_more_older_messages(true);
        self.shot("chat-load-older", "Chat: Load older messages button above the history")?;
        ui.set_is_loading_older_messages(true);
        self.shot("chat-loading-older", "Chat: older messages are loading")?;
        ui.set_has_more_older_messages(false);
        ui.set_is_loading_older_messages(false);

        ui.set_show_command_suggestions(true);
        ui.set_chat_input_text("/".into());
        ui.set_active_suggestion_index(1);
        self.shot("chat-command-suggestions", "Chat: slash command suggestion list")?;
        ui.set_show_command_suggestions(false);
        ui.set_chat_input_text("".into());

        ui.set_active_command_name("/play".into());
        ui.set_active_param_name("query".into());
        ui.set_active_param_desc("Song name or link".into());
        ui.set_command_arg_text("lofi hip hop".into());
        self.shot("chat-command-parameter", "Chat: command pill with its parameter box")?;
        ui.set_active_command_name("".into());
        ui.set_active_param_name("".into());
        ui.set_command_arg_text("".into());

        ui.set_chat_input_text("Hello everyone, the build is green.".into());
        self.shot("chat-composing", "Chat: text typed into the message box")?;
        ui.set_chat_input_text("".into());

        ui.set_titlebar_error("Gateway reconnecting...".into());
        self.shot("chat-titlebar-error", "Chat: error text in the title bar")?;
        ui.set_titlebar_error("".into());

        ui.set_show_hardware_monitor(false);
        self.shot("chat-hardware-monitor-off", "Chat: title bar without the hardware monitor")?;
        ui.set_show_hardware_monitor(true);

        ui.set_is_in_voice(true);
        ui.set_is_voice_connecting(true);
        ui.set_current_voice_channel("Lounge".into());
        self.shot("chat-voice-connecting-banner", "Chat: voice banner while connecting")?;
        ui.set_is_voice_connecting(false);
        self.shot("chat-voice-connected-banner", "Chat: voice banner when connected")?;
        ui.set_is_muted(true);
        self.shot("chat-voice-muted", "Chat: voice banner with the microphone muted")?;
        ui.set_is_deafened(true);
        self.shot("chat-voice-deafened", "Chat: voice banner muted and deafened")?;
        ui.set_is_muted(false);
        ui.set_is_deafened(false);
        ui.set_is_in_voice(false);

        ui.set_web_share_error("Could not start the web share: no stream is active.".into());
        self.shot("chat-web-share-error", "Chat: web share error toast")?;
        ui.set_web_share_error("".into());

        ui.set_channel_sidebar_width(300.0);
        self.shot("chat-wide-sidebar", "Chat: channel sidebar dragged wider")?;
        ui.set_channel_sidebar_width(230.0);
        Ok(())
    }

    // -------------------------------------------- members, profile, friends

    fn social_screens(&mut self) -> Result<(), Box<dyn Error>> {
        self.enter_chat();
        let ui = self.ui.clone_strong();
        let all = sample_messages();
        ui.set_messages(model(all[0..5].to_vec()));

        ui.set_show_member_list(false);
        self.shot("chat-members-hidden", "Chat: member list collapsed from the channel header")?;
        ui.set_show_member_list(true);
        ui.set_member_rows(model(Vec::new()));
        self.shot("chat-members-loading", "Chat: member list before the server sent it")?;
        ui.set_member_rows(model(sample_members()));

        // Threads under their channel and a forum channel.
        let mut rows = sample_channels();
        let at = rows.iter().position(|c| c.id == "c-general").map(|i| i + 1).unwrap_or(rows.len());
        let mut thread = channel("t-plan", "release plan");
        thread.is_thread = true;
        thread.has_parent = true;
        thread.parent_channel_id = "c-general".into();
        let mut thread_two = channel("t-bugs", "bug triage");
        thread_two.is_thread = true;
        thread_two.has_parent = true;
        thread_two.parent_channel_id = "c-general".into();
        let mut forum = channel("c-forum", "ideas");
        forum.is_forum = true;
        forum.has_parent = true;
        forum.parent_channel_id = "cat-text".into();
        for (offset, row) in [thread, thread_two, forum].into_iter().enumerate() {
            rows.insert(at + offset, row);
        }
        ui.set_channels(model(rows));
        self.shot("chat-threads-sidebar", "Chat: threads listed under their channel and a forum channel")?;
        ui.set_channels(model(sample_channels()));

        // A forum channel has no composer, only a hint.
        ui.set_active_channel_name("ideas".into());
        ui.set_active_channel_is_forum(true);
        self.shot("chat-forum-channel", "Chat: forum channel with the composer hidden and a thread hint")?;
        ui.set_active_channel_is_forum(false);
        ui.set_active_channel_name("general".into());

        // Profile card opened from a message author.
        ui.set_profile_user_id("u-mira".into());
        ui.set_profile_loading(true);
        ui.set_profile_name("Mira".into());
        ui.set_profile_avatar_initial("M".into());
        ui.set_show_profile_popout(true);
        self.shot("profile-popout-loading", "Profile card while the profile loads")?;
        ui.set_profile_loading(false);
        ui.set_profile_username("@mira".into());
        ui.set_profile_bio("Designer, coffee drinker, voice chat regular.".into());
        ui.set_profile_created("Account created Mar 4, 2019".into());
        ui.set_profile_mutual("3 mutual servers".into());
        ui.set_profile_has_avatar(true);
        ui.set_profile_avatar_img(avatar_image(1, 72));
        ui.set_profile_roles(model(vec![
            ProfileRole { name: "Moderator".into(), color: slint::Color::from_rgb_u8(46, 204, 113) },
            ProfileRole { name: "Designer".into(), color: slint::Color::from_rgb_u8(155, 89, 182) },
        ]));
        self.shot("profile-popout", "Profile card: bio, roles, mutual servers and actions")?;
        ui.set_profile_status("Click Block again to confirm".into());
        self.shot("profile-popout-status", "Profile card with a status line under the actions")?;
        ui.set_profile_status("".into());
        ui.set_profile_is_self(true);
        ui.set_profile_mutual("".into());
        self.shot("profile-popout-self", "Profile card of the signed in user")?;
        ui.set_profile_is_self(false);
        ui.set_show_profile_popout(false);

        // Server menu.
        ui.set_server_menu_guild_id("g1".into());
        ui.set_server_menu_guild_name("Litecord HQ".into());
        ui.set_show_server_menu(true);
        self.shot("server-menu", "Server menu: mark as read, invite, mute, join and leave")?;
        ui.set_server_menu_notice("Invite link copied".into());
        self.shot("server-menu-notice", "Server menu with a notice")?;
        ui.set_server_menu_notice("".into());
        ui.set_server_menu_confirm_leave(true);
        self.shot("server-menu-confirm-leave", "Server menu asking to confirm leaving the server")?;
        ui.set_server_menu_confirm_leave(false);
        ui.set_show_server_menu(false);

        // Account menu with the custom status notice.
        ui.set_show_user_menu(true);
        ui.set_status_notice("Status updated".into());
        self.shot("user-menu-status-notice", "Account menu: custom status with a notice")?;
        ui.set_status_notice("".into());
        ui.set_show_user_menu(false);

        // Friends view in the direct messages area.
        ui.set_active_guild_id("@me".into());
        ui.set_active_guild_name("Direct Messages".into());
        ui.set_channels(model(sample_dm_channels()));
        ui.set_avatar_pool(model(vec![avatar_image(1, 48), avatar_image(2, 48), avatar_image(3, 48)]));
        ui.set_avatar_pool_urls(text_model(&["sample://avatar/1", "sample://avatar/2", "sample://avatar/3"]));
        ui.set_friends_pending_count(2);
        ui.set_show_friends(true);
        ui.set_friends_tab(0);
        ui.set_friends_items(model(sample_friends(0)));
        self.shot("friends-online", "Friends: online tab")?;
        ui.set_friends_tab(1);
        ui.set_friends_items(model(sample_friends(1)));
        self.shot("friends-all", "Friends: all tab")?;
        ui.set_friends_tab(2);
        ui.set_friends_items(model(sample_friends(2)));
        self.shot("friends-pending", "Friends: pending requests with accept and ignore")?;
        ui.set_friends_tab(3);
        ui.set_friends_items(model(sample_friends(3)));
        self.shot("friends-blocked", "Friends: blocked users")?;
        ui.set_friends_tab(1);
        ui.set_friends_items(model(Vec::new()));
        self.shot("friends-empty", "Friends: empty list")?;
        ui.set_friends_items(model(sample_friends(1)));
        ui.set_friends_add_text("someone#1234".into());
        ui.set_friends_status("Friend request sent".into());
        ui.set_friends_status_is_error(false);
        self.shot("friends-add-ok", "Friends: add by username, request sent")?;
        ui.set_friends_status("No user with that name was found".into());
        ui.set_friends_status_is_error(true);
        self.shot("friends-add-error", "Friends: add by username, error under the box")?;
        ui.set_friends_status("".into());
        ui.set_friends_status_is_error(false);
        ui.set_friends_add_text("".into());
        ui.set_friends_pending_count(0);
        ui.set_show_friends(false);
        Ok(())
    }

    // ------------------------------------------------------------------ DMs

    fn direct_message_screens(&mut self) -> Result<(), Box<dyn Error>> {
        self.enter_chat();
        let ui = self.ui.clone_strong();
        ui.set_avatar_pool(model(vec![avatar_image(1, 48), avatar_image(2, 48), avatar_image(3, 48)]));
        ui.set_avatar_pool_urls(text_model(&["sample://avatar/1", "sample://avatar/2", "sample://avatar/3"]));
        ui.set_active_guild_id("@me".into());
        ui.set_active_guild_name("Direct Messages".into());
        ui.set_channels(model(sample_dm_channels()));
        ui.set_active_channel_id("dm-mira".into());
        ui.set_active_channel_name("Mira".into());
        ui.set_total_dm_unread_count(5);
        ui.set_messages(model(sample_dm_messages()));
        self.shot("dm-list", "Direct messages: conversation list with unread badges")?;

        ui.set_total_dm_unread_count(120);
        self.shot("dm-unread-overflow", "Direct messages: unread badge above 99")?;
        ui.set_total_dm_unread_count(5);

        ui.set_has_incoming_call(true);
        ui.set_incoming_call_caller_name("Mira".into());
        ui.set_incoming_call_channel_id("dm-mira".into());
        self.shot("dm-incoming-call", "Direct messages: incoming call panel")?;
        ui.set_has_incoming_call(false);

        ui.set_is_in_voice(true);
        ui.set_is_in_dm_call(true);
        ui.set_current_voice_channel("Mira".into());
        self.shot("dm-call-active", "Direct messages: call in progress")?;
        ui.set_is_in_dm_call(false);
        ui.set_is_in_voice(false);
        ui.set_total_dm_unread_count(0);
        Ok(())
    }

    // ---------------------------------------------------------- voice room

    fn voice_screens(&mut self) -> Result<(), Box<dyn Error>> {
        self.enter_chat();
        let ui = self.ui.clone_strong();
        ui.set_is_in_voice(true);
        ui.set_current_voice_channel("Lounge".into());
        ui.set_is_voice_focused(true);
        ui.set_is_voice_connecting(true);
        ui.set_voice_participants(model(Vec::new()));
        self.shot("voice-room-connecting", "Voice room: connecting")?;

        ui.set_is_voice_connecting(false);
        ui.set_voice_participants(model(sample_participants()));
        self.shot("voice-room", "Voice room: participants, one speaking, one muted")?;

        ui.set_is_muted(true);
        self.shot("voice-room-self-muted", "Voice room: own microphone muted")?;
        ui.set_is_muted(false);

        // Someone else streams and the viewer watches.
        ui.set_has_active_stream(true);
        ui.set_active_stream_user("Jonas".into());
        ui.set_active_stream_user_id("u-jonas".into());
        ui.set_remote_stream_frame(desktop_image(1280, 720, 0));
        ui.set_remote_stream_fps("60 FPS".into());
        ui.set_tr_stream_quality("1080p 60fps".into());
        self.shot("voice-room-watching-stream", "Voice room: watching another user's stream")?;
        ui.set_focused_stream_uid("u-jonas".into());
        self.shot("voice-room-stream-focused", "Voice room: the stream card focused")?;
        ui.set_focused_stream_uid("".into());
        ui.set_has_active_stream(false);
        ui.set_active_stream_user("".into());
        ui.set_active_stream_user_id("".into());

        // The viewer streams.
        ui.set_is_screen_sharing(true);
        ui.set_active_stream_frame(desktop_image(1280, 720, 1));
        ui.set_local_preview_fps("30 FPS".into());
        ui.set_tr_stream_quality("1080p 30fps".into());
        self.shot("voice-room-own-stream", "Voice room: own screen share preview")?;
        ui.set_show_self_preview_notice(true);
        self.shot("voice-room-own-stream-notice", "Voice room: own stream with the reduced preview notice")?;
        ui.set_show_self_preview_notice(false);

        ui.set_web_share_active(true);
        ui.set_web_share_url("https://litecord.example/watch/ab12cd34".into());
        ui.set_web_share_viewers(2);
        self.shot("voice-room-web-share-active", "Voice room: own stream with web sharing on")?;
        ui.set_web_share_pending(true);
        self.shot("voice-room-web-share-pending", "Voice room: web sharing connecting")?;
        ui.set_web_share_pending(false);
        ui.set_show_web_share_modal(true);
        self.shot("web-share-modal", "Web share dialog with the link to send")?;
        ui.set_web_share_copied(true);
        self.shot("web-share-modal-copied", "Web share dialog after the link was copied")?;
        ui.set_web_share_copied(false);
        ui.set_show_web_share_modal(false);
        ui.set_web_share_active(false);
        ui.set_web_share_viewers(0);

        ui.set_is_screen_sharing(false);
        ui.set_is_voice_focused(false);
        ui.set_is_in_voice(false);
        ui.set_voice_participants(model(Vec::new()));
        Ok(())
    }

    // ------------------------------------------------------------- dialogs

    fn dialog_screens(&mut self) -> Result<(), Box<dyn Error>> {
        self.enter_chat();
        let ui = self.ui.clone_strong();

        ui.set_show_user_menu(true);
        self.shot("user-menu", "Account menu: saved accounts, add account and sign out")?;
        ui.set_saved_accounts(model(Vec::new()));
        self.shot("user-menu-no-accounts", "Account menu without saved accounts")?;
        ui.set_saved_accounts(model(sample_accounts()));
        ui.set_show_user_menu(false);

        ui.set_update_version("1.4.0-beta".into());
        ui.set_update_release_name("Litecord v1.4.0-beta".into());
        ui.set_is_updating(false);
        ui.set_show_update_dialog(true);
        self.shot("update-dialog", "Update available dialog")?;
        ui.set_is_updating(true);
        ui.set_update_progress(0.42);
        ui.set_update_status_text("Downloading update... 42%".into());
        self.shot("update-dialog-downloading", "Update dialog while downloading")?;
        ui.set_is_updating(false);
        ui.set_show_update_dialog(false);
        Ok(())
    }

    fn stream_modal_screens(&mut self) -> Result<(), Box<dyn Error>> {
        self.enter_chat();
        let ui = self.ui.clone_strong();
        ui.set_is_in_voice(true);
        ui.set_current_voice_channel("Lounge".into());
        ui.set_is_voice_focused(true);
        ui.set_voice_participants(model(sample_participants()));
        ui.set_available_screens(model(vec![
            ScreenSourceItem { id: 0, name: "Display 1".into(), resolution: "2560x1440".into(), is_primary: true, ..Default::default() },
            ScreenSourceItem { id: 1, name: "Display 2".into(), resolution: "1920x1080".into(), is_primary: false, ..Default::default() },
        ]));
        ui.set_available_windows(model(vec![
            WindowSourceItem { id: "w1".into(), title: "Sample editor".into(), app_name: "Editor".into(), has_icon: true, icon_image: avatar_image(2, 32) },
            WindowSourceItem { id: "w2".into(), title: "Sample browser".into(), app_name: "Browser".into(), has_icon: false, ..Default::default() },
            WindowSourceItem { id: "w3".into(), title: "Litecord".into(), app_name: "Litecord".into(), has_icon: false, ..Default::default() },
        ]));
        ui.set_available_cameras(model(vec![
            CameraSourceItem { id: "cam0".into(), name: "Integrated Camera".into(), ..Default::default() },
            CameraSourceItem { id: "cam1".into(), name: "USB Webcam".into(), ..Default::default() },
        ]));
        ui.set_stream_selected_res("1080p".into());
        ui.set_stream_selected_fps(60);
        ui.set_stream_include_audio(true);
        ui.set_show_stream_modal(true);

        ui.set_stream_source_tab("screens".into());
        ui.set_stream_selected_source_index(0);
        self.shot("stream-modal-screens", "Share screen dialog: screens tab")?;

        ui.set_stream_selected_source_index(1);
        ui.set_stream_selected_res("720p".into());
        ui.set_stream_selected_fps(30);
        ui.set_stream_include_audio(false);
        self.shot("stream-modal-screens-720p", "Share screen dialog: second screen, 720p 30fps, audio off")?;
        ui.set_stream_selected_res("480p".into());
        ui.set_stream_selected_fps(15);
        self.shot("stream-modal-screens-480p", "Share screen dialog: 480p 15fps")?;
        ui.set_stream_selected_res("1080p".into());
        ui.set_stream_selected_fps(60);
        ui.set_stream_include_audio(true);

        ui.set_stream_source_tab("windows".into());
        ui.set_stream_selected_source_index(0);
        ui.set_stream_selected_window_id("w1".into());
        self.shot("stream-modal-windows", "Share screen dialog: windows tab")?;

        ui.set_stream_source_tab("cameras".into());
        ui.set_stream_selected_source_index(0);
        ui.set_stream_selected_camera_id("cam0".into());
        ui.set_stream_include_audio(false);
        self.shot("stream-modal-cameras", "Share screen dialog: cameras tab")?;
        ui.set_available_cameras(model(Vec::new()));
        self.shot("stream-modal-no-cameras", "Share screen dialog: cameras tab with no camera")?;

        ui.set_show_stream_modal(false);
        ui.set_stream_source_tab("screens".into());
        ui.set_stream_include_audio(true);
        ui.set_is_voice_focused(false);
        ui.set_is_in_voice(false);
        Ok(())
    }

    // ------------------------------------------------------------ settings

    fn settings_screens(&mut self) -> Result<(), Box<dyn Error>> {
        self.enter_chat();
        let ui = self.ui.clone_strong();
        fill_settings(&ui);

        // Tab 0: voice and audio. Click positions come from the 1280x800 layout.
        self.open_settings(0);
        self.shot("settings-voice", "Settings: voice and audio")?;
        self.click(750.0, 249.0);
        self.shot("settings-voice-output-menu", "Settings: output device dropdown open")?;
        self.open_settings(0);
        self.click(750.0, 398.0);
        self.shot("settings-voice-input-menu", "Settings: input device dropdown open")?;
        self.open_settings(0);
        self.scroll(750.0, 450.0, -700.0);
        self.shot("settings-voice-lower", "Settings: voice and audio, scrolled to the lower options")?;
        ui.set_is_testing_mic(true);
        ui.set_mic_level(0.6);
        self.shot("settings-voice-mic-test", "Settings: microphone test running with a level")?;
        ui.set_is_testing_mic(false);
        ui.set_mic_level(0.0);
        self.click(750.0, 472.0);
        self.shot("settings-voice-noise-menu", "Settings: noise suppression dropdown open")?;

        // Tab 5: video and interface.
        self.open_settings(5);
        self.shot("settings-video", "Settings: video and interface")?;
        self.click(750.0, 433.0);
        self.shot("settings-video-encoder-menu", "Settings: video encoder dropdown open")?;
        self.open_settings(5);
        self.click(750.0, 534.0);
        self.shot("settings-video-capture-menu", "Settings: capture method dropdown open")?;
        ui.set_enable_self_preview_notice(false);
        self.open_settings(5);
        self.shot("settings-video-notice-off", "Settings: reduced preview notice switched off")?;
        ui.set_enable_self_preview_notice(true);
        ui.set_gpu_selection_available(true);
        ui.set_stream_capture_status("Capture: GPU path, 1920x1080 at 60 fps".into());
        self.open_settings(5);
        self.shot("settings-video-capture-status", "Settings: video tab with GPU selection and capture status text")?;
        ui.set_gpu_selection_available(false);
        ui.set_stream_capture_status("".into());

        // Tab 2: keybinds.
        self.open_settings(2);
        self.shot("settings-keybinds", "Settings: keybinds")?;
        ui.set_recording_keybind_for(1);
        self.shot("settings-keybinds-recording", "Settings: recording a keybind")?;
        ui.set_recording_keybind_for(0);

        // Tab 1: language.
        self.open_settings(1);
        self.shot("settings-language", "Settings: appearance and language")?;
        self.scroll(750.0, 450.0, -700.0);
        self.shot("settings-language-lower", "Settings: appearance and language, scrolled to the language list")?;
        ui.set_use_gpu_renderer(true);
        ui.set_show_hardware_monitor(false);
        self.open_settings(1);
        self.shot("settings-appearance-toggles", "Settings: appearance with GPU renderer on and hardware monitor off")?;
        ui.set_use_gpu_renderer(false);
        ui.set_show_hardware_monitor(true);

        // Tab 6: network and proxy.
        self.open_settings(6);
        self.shot("settings-network-off", "Settings: network and proxy, proxy off")?;
        ui.set_selected_proxy_mode("http".into());
        ui.set_proxy_mode_options(model(proxy_modes("http")));
        self.shot("settings-network-http", "Settings: HTTP proxy fields")?;
        ui.set_selected_proxy_mode("socks5".into());
        ui.set_proxy_mode_options(model(proxy_modes("socks5")));
        ui.set_proxy_username_input("sample".into());
        ui.set_proxy_password_input("secret".into());
        self.shot("settings-network-socks5", "Settings: SOCKS5 proxy fields with credentials")?;
        ui.set_proxy_username_input("".into());
        ui.set_proxy_password_input("".into());
        ui.set_proxy_is_testing(true);
        self.shot("settings-network-testing", "Settings: proxy test running")?;
        ui.set_proxy_is_testing(false);
        ui.set_proxy_test_status_text("Proxy reachable (84 ms)".into());
        ui.set_proxy_test_is_success(true);
        self.shot("settings-network-test-ok", "Settings: proxy test succeeded")?;
        ui.set_proxy_test_status_text("Connection refused by 127.0.0.1:8080".into());
        ui.set_proxy_test_is_success(false);
        self.shot("settings-network-test-failed", "Settings: proxy test failed")?;
        ui.set_proxy_test_status_text("".into());
        ui.set_proxy_test_is_success(true);
        ui.set_selected_proxy_mode("off".into());
        ui.set_net_diag_running(true);
        self.shot("settings-network-diagnostics-running", "Settings: network diagnostics running")?;
        ui.set_net_diag_running(false);
        ui.set_net_diag_verdict(0);
        self.shot("settings-network-diagnostics-direct", "Settings: network diagnostics, direct connection")?;
        ui.set_net_diag_verdict(1);
        ui.set_net_diag_copied(true);
        self.shot("settings-network-diagnostics-symmetric", "Settings: network diagnostics, symmetric NAT, report copied")?;
        ui.set_net_diag_verdict(3);
        ui.set_net_diag_signaling_ok(false);
        ui.set_net_diag_copied(false);
        self.shot("settings-network-diagnostics-blocked", "Settings: network diagnostics, UDP blocked and signaling down")?;
        ui.set_net_diag_verdict(-1);
        ui.set_net_diag_signaling_ok(true);
        ui.set_proxy_mode_options(model(proxy_modes("off")));
        self.open_settings(6);
        self.click(750.0, 249.0);
        self.shot("settings-network-mode-menu", "Settings: proxy mode dropdown open")?;

        // Tab 4: updates and about.
        self.open_settings(4);
        self.shot("settings-about", "Settings: updates and about")?;
        ui.set_is_checking_update(true);
        self.shot("settings-about-checking", "Settings: checking for updates")?;
        ui.set_is_checking_update(false);
        ui.set_update_check_feedback("You are on the latest version.".into());
        self.shot("settings-about-up-to-date", "Settings: up to date message")?;
        ui.set_update_check_feedback("".into());

        ui.set_show_settings_modal(false);
        Ok(())
    }

    // ---------------------------------------------------------- light theme

    fn light_theme_screens(&mut self) -> Result<(), Box<dyn Error>> {
        let ui = self.ui.clone_strong();
        ui.set_use_light_theme(true);

        ui.set_is_logged_in(false);
        ui.set_has_qr_code(true);
        ui.set_qr_code_image(qr_image("https://discord.com/ra/litecord-sample-fingerprint"));
        self.shot("light-login", "Light theme: login")?;

        self.enter_chat();
        self.shot("light-chat", "Light theme: chat")?;

        ui.set_show_user_menu(true);
        self.shot("light-user-menu", "Light theme: account menu")?;
        ui.set_show_user_menu(false);

        fill_settings(&ui);
        self.open_settings(0);
        self.shot("light-settings-voice", "Light theme: settings, voice and audio")?;
        self.open_settings(5);
        self.shot("light-settings-video", "Light theme: settings, video and interface")?;
        ui.set_show_settings_modal(false);

        ui.set_use_light_theme(false);
        Ok(())
    }

    // --------------------------------------------------------------- popout

    fn popout_screen(&mut self) -> Result<(), Box<dyn Error>> {
        let popout = PopoutStreamWindow::new()?;
        popout.set_username("Jonas".into());
        popout.set_user_id("u-jonas".into());
        popout.set_stream_frame(desktop_image(640, 360, 0));
        popout.set_stream_fps("60 FPS".into());
        popout.show()?;
        popout.window().set_size(PhysicalSize::new(640, 360));
        self.save(popout.window(), "popout-stream", "Detached stream window with its control bar")?;
        popout.set_is_self(true);
        popout.set_show_self_preview_notice(true);
        popout.set_is_pinned(true);
        self.save(popout.window(), "popout-stream-own", "Detached own stream: pinned, with the reduced preview notice")?;
        popout.hide()?;
        Ok(())
    }
}

// ------------------------------------------------------------ sample data

fn fill_settings(ui: &AppWindow) {
    ui.set_languages(model(vec![
        LanguageItem { code: "auto".into(), name: "Automatic".into(), native_name: "Auto".into(), is_selected: false, ..Default::default() },
        LanguageItem { code: "en".into(), name: "English".into(), native_name: "English".into(), is_selected: true, ..Default::default() },
        LanguageItem { code: "pt".into(), name: "Portuguese".into(), native_name: "Português".into(), is_selected: false, ..Default::default() },
        LanguageItem { code: "es".into(), name: "Spanish".into(), native_name: "Español".into(), is_selected: false, ..Default::default() },
        LanguageItem { code: "de".into(), name: "German".into(), native_name: "Deutsch".into(), is_selected: false, ..Default::default() },
        LanguageItem { code: "fr".into(), name: "French".into(), native_name: "Français".into(), is_selected: false, ..Default::default() },
        LanguageItem { code: "ru".into(), name: "Russian".into(), native_name: "Русский".into(), is_selected: false, ..Default::default() },
        LanguageItem { code: "ja".into(), name: "Japanese".into(), native_name: "日本語".into(), is_selected: false, ..Default::default() },
    ]));
    ui.set_input_devices(model(vec![
        AudioDeviceItem { id: "in0".into(), name: "Built-in Microphone".into(), is_selected: true, ..Default::default() },
        AudioDeviceItem { id: "in1".into(), name: "USB Headset Microphone".into(), is_selected: false, ..Default::default() },
    ]));
    ui.set_output_devices(model(vec![
        AudioDeviceItem { id: "out0".into(), name: "Built-in Speakers".into(), is_selected: true, ..Default::default() },
        AudioDeviceItem { id: "out1".into(), name: "USB Headset".into(), is_selected: false, ..Default::default() },
    ]));
    ui.set_selected_input_device("Built-in Microphone".into());
    ui.set_selected_output_device("Built-in Speakers".into());
    ui.set_vad_threshold(0.12);
    ui.set_mic_volume(1.0);
    ui.set_mic_level(0.0);
    ui.set_is_testing_mic(false);
    ui.set_noise_suppression_options(model(vec![
        option("advanced", "Advanced Suppression", "Deep neural network background noise removal and transient filtering.", false),
        option("standard", "Standard (RNNoise)", "Classic recurrent neural network noise reduction.", true),
        option("none", "Disabled (Default)", "No noise suppression applied to microphone.", false),
    ]));
    ui.set_echo_cancellation(true);
    ui.set_automatic_gain_control(false);
    ui.set_video_encoder_options(model(vec![
        option("auto", "Automatic (recommended)", "Uses hardware acceleration when it is available, CPU otherwise.", true),
        option("nvenc", "NVIDIA NVENC", "Hardware encoding on NVIDIA GPUs.", false),
        option("openh264", "CPU (OpenH264)", "Compatibility option: encodes in software and uses more CPU.", false),
    ]));
    ui.set_video_capture_options(model(vec![
        option("auto", "Automatic", "Picks the best capture method for the screen.", true),
        option("portal", "System portal", "Capture through the desktop portal.", false),
    ]));
    ui.set_audio_loopback_options(model(vec![
        option("auto", "Automatic", "Captures the audio of the shared screen or window.", true),
        option("off", "Off", "Streams are sent without sound.", false),
    ]));
    ui.set_selected_video_encoder("auto".into());
    ui.set_selected_video_capture_backend("auto".into());
    ui.set_selected_audio_loopback_backend("auto".into());
    ui.set_gpu_selection_available(false);
    ui.set_stream_capture_status("".into());
    ui.set_enable_self_preview_notice(true);
    ui.set_show_hardware_monitor(true);
    ui.set_use_gpu_renderer(false);
    ui.set_net_diag_running(false);
    ui.set_net_diag_verdict(-1);
    ui.set_net_diag_signaling_ok(true);
    ui.set_net_diag_copied(false);
    ui.set_keybind_mute_str("Ctrl+Shift+M".into());
    ui.set_keybind_deafen_str("Ctrl+Shift+D".into());
    ui.set_recording_keybind_for(0);
    ui.set_proxy_mode_options(model(proxy_modes("off")));
    ui.set_selected_proxy_mode("off".into());
    ui.set_proxy_host_input("127.0.0.1".into());
    ui.set_proxy_port_input("8080".into());
    ui.set_proxy_username_input("".into());
    ui.set_proxy_password_input("".into());
    ui.set_proxy_route_media_enabled(false);
    ui.set_proxy_is_testing(false);
    ui.set_proxy_test_status_text("".into());
    ui.set_proxy_test_is_success(true);
    ui.set_update_check_feedback("".into());
    ui.set_is_checking_update(false);
}

fn option(id: &str, name: &str, desc: &str, selected: bool) -> SettingOptionItem {
    SettingOptionItem { id: id.into(), name: name.into(), desc: desc.into(), is_selected: selected, ..Default::default() }
}

fn proxy_modes(selected: &str) -> Vec<SettingOptionItem> {
    vec![
        option("off", "Disabled (direct connection)", "Connects straight to the Discord servers (default).", selected == "off"),
        option("http", "HTTP / HTTPS proxy", "HTTP CONNECT tunnel for restricted networks and proxies.", selected == "http"),
        option("socks5", "SOCKS5 proxy", "SOCKS5 tunnel compatible with Tor, Shadowsocks and SSH.", selected == "socks5"),
    ]
}

fn sample_accounts() -> Vec<AccountItem> {
    vec![
        AccountItem { id: "u-sample".into(), username: "Sample User".into(), tag: "sample#0001".into(), avatar_initials: "S".into(), is_active: true, ..Default::default() },
        AccountItem { id: "u-alt".into(), username: "Second Account".into(), tag: "second#0002".into(), avatar_initials: "S".into(), is_active: false, ..Default::default() },
        AccountItem { id: "u-work".into(), username: "Work Account".into(), tag: "work#0003".into(), avatar_initials: "W".into(), is_active: false, ..Default::default() },
    ]
}

fn sample_members() -> Vec<MemberRowItem> {
    let group = |text: &str| MemberRowItem { is_group: true, text: text.into(), ..Default::default() };
    let member = |name: &str, status: i32, bot: bool, color: Option<(u8, u8, u8)>| MemberRowItem {
        is_group: false,
        text: name.into(),
        avatar_text: name.chars().next().map(|c| c.to_string()).unwrap_or_default().into(),
        status,
        is_bot: bot,
        has_color: color.is_some(),
        name_color: color.map(|(r, g, b)| slint::Color::from_rgb_u8(r, g, b)).unwrap_or_default(),
        ..Default::default()
    };
    vec![
        group("MODERATOR - 2"),
        member("Mira", 0, false, Some((46, 204, 113))),
        member("Jonas", 2, false, Some((46, 204, 113))),
        group("ONLINE - 3"),
        member("Aiko", 0, false, None),
        member("Litebot", 0, true, None),
        member("Sam", 1, false, None),
        group("OFFLINE - 2"),
        member("Noor", 3, false, None),
        member("Theo", 3, false, None),
    ]
}

/// Rows of the friends view for a tab: 0 online, 1 all, 2 pending, 3 blocked.
fn sample_friends(tab: i32) -> Vec<FriendItem> {
    let item = |id: &str, name: &str, status: &str, kind: i32, online: bool| FriendItem {
        id: id.into(),
        name: name.into(),
        initials: name.chars().next().map(|c| c.to_string()).unwrap_or_default().into(),
        status: status.into(),
        kind,
        online,
    };
    match tab {
        0 => vec![item("f1", "Mira", "Online", 1, true), item("f2", "Jonas", "Online", 1, true)],
        1 => vec![
            item("f1", "Mira", "Online", 1, true),
            item("f2", "Jonas", "Online", 1, true),
            item("f3", "Noor", "Offline", 1, false),
            item("f4", "Theo", "Offline", 1, false),
        ],
        2 => vec![
            item("p1", "Aiko", "Incoming friend request", 3, false),
            item("p2", "Sam", "Outgoing friend request", 4, false),
        ],
        _ => vec![item("b1", "Spammer", "Blocked", 2, false)],
    }
}

fn sample_guilds() -> Vec<GuildItem> {
    vec![
        GuildItem { id: "g1".into(), name: "Litecord HQ".into(), icon: "L".into(), has_image: true, icon_image: icon_image(0, 48), mention_count: 3, ..Default::default() },
        GuildItem { id: "g2".into(), name: "Rust Lounge".into(), icon: "R".into(), has_image: false, ..Default::default() },
        GuildItem { id: "g3".into(), name: "Game Night".into(), icon: "G".into(), has_image: true, icon_image: icon_image(2, 48), has_unread: true, ..Default::default() },
        GuildItem { id: "g4".into(), name: "Design Club".into(), icon: "D".into(), has_image: false, ..Default::default() },
    ]
}

fn channel(id: &str, name: &str) -> ChannelItem {
    ChannelItem { id: id.into(), name: name.into(), label: name.into(), ..Default::default() }
}

fn sample_channels() -> Vec<ChannelItem> {
    let mut voice_lounge = channel("c-lounge", "Lounge");
    voice_lounge.is_voice = true;
    voice_lounge.voice_count = 2;
    voice_lounge.has_parent = true;
    voice_lounge.parent_channel_id = "cat-voice".into();
    let mut voice_member_a = channel("vm-1", "Mira");
    voice_member_a.is_voice_member = true;
    voice_member_a.has_parent = true;
    voice_member_a.avatar_text = "M".into();
    let mut voice_member_b = channel("vm-2", "Jonas");
    voice_member_b.is_voice_member = true;
    voice_member_b.has_parent = true;
    voice_member_b.avatar_text = "J".into();
    let mut voice_quiet = channel("c-quiet", "Quiet room");
    voice_quiet.is_voice = true;
    voice_quiet.has_parent = true;
    voice_quiet.parent_channel_id = "cat-voice".into();

    let mut general = channel("c-general", "general");
    general.has_parent = true;
    general.parent_channel_id = "cat-text".into();
    let mut announcements = channel("c-news", "announcements");
    announcements.has_parent = true;
    announcements.parent_channel_id = "cat-text".into();
    announcements.unread_count = 3;
    let mut emoji_channel = channel("c-emoji", "party-room");
    emoji_channel.has_parent = true;
    emoji_channel.parent_channel_id = "cat-text".into();
    emoji_channel.emoji_id = "e-party".into();
    emoji_channel.emoji_slot = 0;
    emoji_channel.label = "party-room".into();
    let mut dev = channel("c-dev", "development");
    dev.has_parent = true;
    dev.parent_channel_id = "cat-text".into();
    dev.unread_count = 12;

    let mut text_category = channel("cat-text", "TEXT CHANNELS");
    text_category.is_category = true;
    let mut voice_category = channel("cat-voice", "VOICE CHANNELS");
    voice_category.is_category = true;
    voice_category.has_separator = true;
    let mut archive_category = channel("cat-archive", "ARCHIVE");
    archive_category.is_category = true;
    archive_category.has_separator = true;
    archive_category.is_collapsed = true;

    vec![
        text_category,
        general,
        announcements,
        emoji_channel,
        dev,
        voice_category,
        voice_lounge,
        voice_member_a,
        voice_member_b,
        voice_quiet,
        archive_category,
    ]
}

fn sample_dm_channels() -> Vec<ChannelItem> {
    let dm = |id: &str, name: &str, initial: &str, slot: i32, url: &str, unread: i32| ChannelItem {
        id: id.into(),
        name: name.into(),
        label: name.into(),
        avatar_text: initial.into(),
        avatar_slot: slot,
        avatar_url: url.into(),
        unread_count: unread,
        ..Default::default()
    };
    vec![
        dm("dm-mira", "Mira", "M", 0, "sample://avatar/1", 0),
        dm("dm-jonas", "Jonas", "J", 1, "sample://avatar/2", 4),
        dm("dm-sage", "Sage", "S", 2, "sample://avatar/3", 1),
        dm("dm-group", "Weekend crew", "W", 0, "", 0),
    ]
}

fn sample_participants() -> Vec<VoiceParticipant> {
    vec![
        VoiceParticipant {
            user_id: "u-sample".into(), username: "Sample User".into(), avatar_text: "S".into(),
            avatar_url: "sample://avatar/0".into(), avatar_img: avatar_image(0, 96),
            is_speaking: false, audio_level: 0.0, is_muted: false, volume: 1.0, priority: 0,
            is_self: true, is_streaming: false, is_stream_hidden: false,
            ..Default::default()
        },
        VoiceParticipant {
            user_id: "u-mira".into(), username: "Mira".into(), avatar_text: "M".into(),
            avatar_url: "sample://avatar/1".into(), avatar_img: avatar_image(1, 96),
            is_speaking: true, audio_level: 0.7, is_muted: false, volume: 1.0, priority: 1,
            is_self: false, is_streaming: false, is_stream_hidden: false,
            ..Default::default()
        },
        VoiceParticipant {
            user_id: "u-jonas".into(), username: "Jonas".into(), avatar_text: "J".into(),
            avatar_url: "sample://avatar/2".into(), avatar_img: avatar_image(2, 96),
            is_speaking: false, audio_level: 0.0, is_muted: true, volume: 0.5, priority: 0,
            is_self: false, is_streaming: true, is_stream_hidden: false,
            ..Default::default()
        },
        VoiceParticipant {
            user_id: "u-sage".into(), username: "Sage".into(), avatar_text: "S".into(),
            avatar_url: "".into(), avatar_img: Image::default(),
            is_speaking: false, audio_level: 0.0, is_muted: false, volume: 1.5, priority: 0,
            is_self: false, is_streaming: false, is_stream_hidden: false,
            ..Default::default()
        },
    ]
}

fn text_line(text: &str) -> MessageLine {
    MessageLine {
        blocks: model(vec![MessageBlock { text: text.into(), ..Default::default() }]),
    }
}

fn plain_message(id: &str, author: &str, author_id: &str, initial: &str, avatar: Option<Image>, time: &str, body: &str) -> ChatMessage {
    let has_avatar = avatar.is_some();
    ChatMessage {
        id: id.into(),
        author: author.into(),
        author_id: author_id.into(),
        created_at: "2026-03-14T12:00:00Z".into(),
        show_header: true,
        avatar_initial: initial.into(),
        avatar_url: if has_avatar { format!("sample://{author_id}").into() } else { "".into() },
        avatar_img: avatar.unwrap_or_default(),
        timestamp: time.into(),
        hover_timestamp: time.rsplit(' ').next().unwrap_or("").into(),
        content: body.into(),
        content_lines: model(body.split('\n').map(text_line).collect()),
        ..Default::default()
    }
}

fn continuation(mut message: ChatMessage, time: &str) -> ChatMessage {
    message.show_header = false;
    message.timestamp = time.into();
    message.hover_timestamp = time.rsplit(' ').next().unwrap_or("").into();
    message
}

fn sample_messages() -> Vec<ChatMessage> {
    let mut first = plain_message(
        "m1", "Mira", "u-mira", "M", Some(avatar_image(1, 72)), "14/03/2026 12:00",
        "Welcome to Litecord HQ! The nightly build is up.",
    );
    first.day_label = "March 14, 2026".into();
    let second = continuation(
        plain_message("m2", "Mira", "u-mira", "M", Some(avatar_image(1, 72)), "14/03/2026 12:01", "Tell me if anything looks off on your platform."),
        "14/03/2026 12:01",
    );

    let mut reply = plain_message(
        "m3", "Jonas", "u-jonas", "J", Some(avatar_image(2, 72)), "14/03/2026 12:04",
        "Yes, installed it a minute ago and it starts fast.",
    );
    reply.reply_author = "Mira".into();
    reply.reply_content = "Tell me if anything looks off on your platform.".into();

    let mut rich = plain_message("m4", "Sample User", "u-sample", "S", Some(avatar_image(0, 72)), "14/03/2026 12:06", "");
    rich.content = "Docs: see the link, run /play, and :party: everyone".into();
    rich.content_lines = model(vec![
        MessageLine {
            blocks: model(vec![
                MessageBlock { text: "Docs: see ".into(), ..Default::default() },
                MessageBlock { text: "https://litecord.example/docs".into(), is_link: true, url: "https://litecord.example/docs".into(), ..Default::default() },
                MessageBlock { text: " and run ".into(), ..Default::default() },
                MessageBlock { text: "/play".into(), is_command: true, command_name: "/play".into(), ..Default::default() },
                MessageBlock { text: " with ".into(), ..Default::default() },
                MessageBlock { text: ":party:".into(), is_emoji: true, emoji_id: "e-party".into(), emoji_img: emoji_image(0), ..Default::default() },
            ]),
        },
        text_line("Second line of the same message."),
    ]);
    rich.links = model(vec![LinkItem { label: "Litecord documentation".into(), url: "https://litecord.example/docs".into() }]);

    let mut command_reply = plain_message("m5", "Sage", "u-sage", "S", None, "14/03/2026 12:07", "Now playing: Lofi Beats");
    command_reply.is_bot = true;
    command_reply.reply_author = "Sample User".into();
    command_reply.reply_command = "/play".into();

    let mut embed = plain_message("m6", "Sage", "u-sage", "S", None, "14/03/2026 12:08", "");
    embed.is_bot = true;
    embed.show_header = false;
    embed.content = "".into();
    embed.content_lines = model(Vec::new());
    embed.embed_content = "Build 1342 passed on all platforms.\nDownload it from the releases page.".into();
    embed.embed_lines = model(vec![
        text_line("Build 1342 passed on all platforms."),
        MessageLine {
            blocks: model(vec![
                MessageBlock { text: "Download it from ".into(), ..Default::default() },
                MessageBlock { text: "the releases page".into(), is_link: true, url: "https://litecord.example/releases".into(), ..Default::default() },
            ]),
        },
    ]);
    embed.embed_color = slint::Color::from_rgb_u8(88, 101, 242);
    embed.embed_footer = "Nightly CI".into();
    embed.buttons = model(vec![
        MessageButton { label: "Primary".into(), style_type: 1, ..Default::default() },
        MessageButton { label: "Secondary".into(), style_type: 2, ..Default::default() },
        MessageButton { label: "Success".into(), style_type: 3, ..Default::default() },
        MessageButton { label: "Danger".into(), style_type: 4, ..Default::default() },
        MessageButton { label: "Open page".into(), style_type: 5, url: "https://litecord.example".into(), ..Default::default() },
        MessageButton { label: "Disabled".into(), style_type: 2, is_disabled: true, ..Default::default() },
    ]);

    let mut code = plain_message("m7", "Jonas", "u-jonas", "J", Some(avatar_image(2, 72)), "14/03/2026 12:10", "Here is the fix:");
    code.code_block = "fn main() {\n    println!(\"hello litecord\");\n}".into();

    let mut files = plain_message("m8", "Mira", "u-mira", "M", Some(avatar_image(1, 72)), "14/03/2026 12:12", "Screenshots and logs:");
    files.attachments = model(vec![
        MessageAttachment {
            id: "a1".into(), filename: "overview.png".into(), size_str: "1.2 MB".into(),
            width: 640, height: 400, is_image: true, is_downloaded: true,
            full_img: desktop_image(640, 400, 2), ..Default::default()
        },
        MessageAttachment {
            id: "a2".into(), filename: "large-capture.png".into(), size_str: "4.8 MB".into(),
            width: 1920, height: 1080, is_image: true, is_downloaded: false,
            preview_img: desktop_image(320, 180, 1), ..Default::default()
        },
        MessageAttachment {
            id: "a3".into(), filename: "build-log.txt".into(), size_str: "38 KB".into(),
            url: "https://litecord.example/build-log.txt".into(), ..Default::default()
        },
    ]);

    let mut invites = plain_message("m9", "Jonas", "u-jonas", "J", Some(avatar_image(2, 72)), "14/03/2026 12:14", "Join the testers:");
    invites.invites = model(vec![
        InviteCard {
            code: "litecord".into(), url: "https://discord.gg/litecord".into(), loaded: true,
            guild_name: "Litecord Testers".into(), description: "Nightly builds and bug reports".into(),
            counts: "1,204 online  -  5,310 members".into(), has_icon: true, icon_img: icon_image(1, 48),
            ..Default::default()
        },
        InviteCard {
            code: "1/2".into(), url: "https://discord.com/events/1/2".into(), loaded: true, is_event: true,
            guild_name: "Release party".into(), description: "Live demo of the new version".into(),
            counts: "86 interested".into(), event_when: "20/03/2026 18:00".into(), event_live: false,
            has_cover: true, cover_img: desktop_image(360, 120, 0), ..Default::default()
        },
    ]);

    vec![first, second, reply, rich, command_reply, embed, code, files, invites]
}

fn sample_dm_messages() -> Vec<ChatMessage> {
    let mut first = plain_message("d1", "Mira", "u-mira", "M", Some(avatar_image(1, 72)), "14/03/2026 09:30", "Are you joining the call later?");
    first.day_label = "March 14, 2026".into();
    let mut mine = plain_message("d2", "Sample User", "u-sample", "S", Some(avatar_image(0, 72)), "14/03/2026 09:32", "Yes, give me ten minutes.");
    mine.reply_author = "Mira".into();
    mine.reply_content = "Are you joining the call later?".into();
    vec![first, mine]
}

// ---------------------------------------------------------- sample images

const PALETTE: [([u8; 3], [u8; 3]); 4] = [
    ([88, 101, 242], [155, 89, 182]),
    ([237, 66, 69], [250, 168, 26]),
    ([35, 165, 90], [26, 188, 156]),
    ([52, 152, 219], [88, 101, 242]),
];

fn pixel_image(width: u32, height: u32, color_at: impl Fn(u32, u32) -> [u8; 4]) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    for (i, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let x = (i as u32) % width;
        let y = (i as u32) / width;
        let c = color_at(x, y);
        *pixel = Rgba8Pixel::new(c[0], c[1], c[2], c[3]);
    }
    Image::from_rgba8(buffer)
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// Round avatar: a diagonal gradient inside a disc.
fn avatar_image(seed: usize, size: u32) -> Image {
    let (a, b) = PALETTE[seed % PALETTE.len()];
    let radius = size as f32 / 2.0;
    pixel_image(size, size, move |x, y| {
        let dx = x as f32 + 0.5 - radius;
        let dy = y as f32 + 0.5 - radius;
        let edge = (radius - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
        let c = mix(a, b, (x + y) as f32 / (2.0 * size as f32));
        [c[0], c[1], c[2], (edge * 255.0) as u8]
    })
}

/// Server icon: a gradient inside a rounded square.
fn icon_image(seed: usize, size: u32) -> Image {
    let (a, b) = PALETTE[seed % PALETTE.len()];
    let corner = size as f32 * 0.3;
    let half = size as f32 / 2.0;
    pixel_image(size, size, move |x, y| {
        let px = (x as f32 + 0.5 - half).abs() - (half - corner);
        let py = (y as f32 + 0.5 - half).abs() - (half - corner);
        let dist = (px.max(0.0).powi(2) + py.max(0.0).powi(2)).sqrt() - corner;
        let edge = (0.5 - dist).clamp(0.0, 1.0);
        let c = mix(a, b, y as f32 / size as f32);
        [c[0], c[1], c[2], (edge * 255.0) as u8]
    })
}

fn emoji_image(_seed: usize) -> Image {
    let size = 32u32;
    let radius = size as f32 / 2.0;
    pixel_image(size, size, move |x, y| {
        let dx = x as f32 + 0.5 - radius;
        let dy = y as f32 + 0.5 - radius;
        let edge = (radius - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
        let eye = (dx.abs() - 5.0).abs() < 2.0 && (dy + 4.0).abs() < 2.5;
        let mouth = dy > 3.0 && dy < 6.0 && dx.abs() < 7.0;
        if eye || mouth {
            [60, 40, 0, (edge * 255.0) as u8]
        } else {
            [250, 190, 30, (edge * 255.0) as u8]
        }
    })
}

/// A stand-in for a shared screen: gradient desktop with a few window shapes.
fn desktop_image(width: u32, height: u32, seed: usize) -> Image {
    let (a, b) = PALETTE[seed % PALETTE.len()];
    pixel_image(width, height, move |x, y| {
        let fx = x as f32 / width as f32;
        let fy = y as f32 / height as f32;
        let in_window = |l: f32, t: f32, r: f32, bt: f32| fx > l && fx < r && fy > t && fy < bt;
        if in_window(0.08, 0.10, 0.60, 0.80) {
            if fy < 0.16 { [45, 47, 52, 255] } else { [240, 241, 243, 255] }
        } else if in_window(0.45, 0.30, 0.92, 0.88) {
            if fy < 0.36 { [30, 31, 34, 255] } else { [49, 51, 56, 255] }
        } else {
            let c = mix(a, b, (fx + fy) / 2.0);
            [c[0], c[1], c[2], 255]
        }
    })
}

/// A real QR code, drawn the way the login screen draws it.
fn qr_image(url: &str) -> Image {
    use qrcode::types::Color;
    use qrcode::QrCode;

    let code = QrCode::new(url.as_bytes()).expect("the sample QR payload is valid");
    let modules = code.width();
    let scale = 5usize;
    let quiet = 3usize;
    let total = (modules + quiet * 2) * scale;
    let colors = code.to_colors();
    let mut buffer = SharedPixelBuffer::<slint::Rgb8Pixel>::new(total as u32, total as u32);
    for (i, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let px = i % total;
        let py = i / total;
        let mx = (px / scale) as isize - quiet as isize;
        let my = (py / scale) as isize - quiet as isize;
        let dark = mx >= 0
            && my >= 0
            && (mx as usize) < modules
            && (my as usize) < modules
            && colors[my as usize * modules + mx as usize] == Color::Dark;
        *pixel = if dark { slint::Rgb8Pixel::new(17, 18, 20) } else { slint::Rgb8Pixel::new(255, 255, 255) };
    }
    Image::from_rgb8(buffer)
}
