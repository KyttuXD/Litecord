#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use tokio::sync::mpsc;
use log::{info, error};

use crate::{
    AppWindow, GuildItem, ChatMessage, LinkItem, ChannelItem, SearchResultItem,
};
use crate::gateway::{self, GatewayClient, GatewayEvent, GatewayCommand, GuildData, ChannelData, format_discord_author, format_discord_message_parts};
use crate::http::DiscordHttpClient;
use crate::ui::helpers::*;
use crate::ui::message_grouping::{apply_message_metadata, regroup_messages};
use crate::auth::vault::*;
use crate::utils::updater;
use crate::utils::{avatar_cache, emoji_cache};
use slint::{ComponentHandle, Image, Model};

#[derive(Clone, Debug, Default)]
pub struct RawGuildItem {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub icon_path: Option<String>,
}

pub async fn fetch_and_populate_guilds(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
    cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
) {
    info!("Fetching the user's servers via REST API...");
    let cache_dir = std::path::Path::new(".litecord_cache/icons");
    let _ = std::fs::create_dir_all(cache_dir);

    match http.get_user_guilds().await {
        Ok(guilds_json) => {
            info!("{} servers found via REST!", guilds_json.len());
            let mut raw_guilds: Vec<RawGuildItem> = Vec::new();
            let mut first_guild_id_opt: Option<String> = None;
            let mut pending_icon_downloads: Vec<(String, String)> = Vec::new();

            for g in guilds_json {
                let g_id = g["id"].as_str().unwrap_or("").to_string();
                let g_name = g["name"].as_str().unwrap_or("Server").to_string();
                let g_icon_hash = g["icon"].as_str().map(|s| s.to_string());

                if !g_id.is_empty() {
                    if first_guild_id_opt.is_none() {
                        first_guild_id_opt = Some(g_id.clone());
                    }

                    let icon_str = get_guild_initials(&g_name);
                    let local_icon_path = cache_dir.join(format!("{}.png", g_id));

                    let mut icon_path_opt = None;
                    if local_icon_path.exists() {
                        icon_path_opt = Some(local_icon_path.to_string_lossy().to_string());
                    } else if let Some(ref hash) = g_icon_hash {
                        let icon_url = format!("https://cdn.discordapp.com/icons/{}/{}.png?size=64", g_id, hash);
                        pending_icon_downloads.push((icon_url, local_icon_path.to_string_lossy().to_string()));
                    }

                    guilds_map.lock().unwrap().insert(g_id.clone(), GuildData {
                        id: g_id.clone(),
                        name: g_name.clone(),
                        channels: Vec::new(),
                    });

                    raw_guilds.push(RawGuildItem {
                        id: g_id,
                        name: g_name,
                        icon: icon_str,
                        icon_path: icon_path_opt,
                    });
                }
            }

            // 📌 Native Discord ordering / local persistence
            let mut ordered_ids: Vec<String> = Vec::new();
            if let Ok(data) = std::fs::read_to_string(".litecord_guild_order.json") {
                if let Ok(ids) = serde_json::from_str::<Vec<String>>(&data) {
                    ordered_ids = ids;
                }
            }
            if ordered_ids.is_empty() {
                if let Ok(settings) = http.get_user_settings().await {
                    if let Some(folders) = settings.get("guild_folders").and_then(|f| f.as_array()) {
                        for folder in folders {
                            if let Some(ids) = folder.get("guild_ids").and_then(|i| i.as_array()) {
                                for id in ids {
                                    if let Some(id_str) = id.as_str() {
                                        ordered_ids.push(id_str.to_string());
                                    }
                                }
                            }
                        }
                    } else if let Some(positions) = settings.get("guild_positions").and_then(|p| p.as_array()) {
                        for id in positions {
                            if let Some(id_str) = id.as_str() {
                                ordered_ids.push(id_str.to_string());
                            }
                        }
                    }
                }
            }

            if !ordered_ids.is_empty() {
                raw_guilds.sort_by_key(|g| {
                    ordered_ids.iter().position(|id| id == &g.id).unwrap_or(usize::MAX)
                });
                if let Some(first) = raw_guilds.first() {
                    first_guild_id_opt = Some(first.id.clone());
                }
            }

            let raw_guilds_clone = raw_guilds.clone();
            let app_w = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    let ui_guilds: Vec<GuildItem> = raw_guilds_clone.into_iter().map(|raw| {
                        let mut has_image = false;
                        let mut icon_image = Image::default();
                        if let Some(ref path_str) = raw.icon_path {
                            if let Some(img) = load_rounded_guild_icon(path_str, ui.window().scale_factor()) {
                                has_image = true;
                                icon_image = img;
                            }
                        }
                        let badge = crate::gateway::unread::guild_badge(&raw.id);
                        GuildItem {
                            id: raw.id.into(),
                            name: raw.name.into(),
                            icon: raw.icon.into(),
                            has_image,
                            icon_image,
                            has_unread: badge.unread > 0,
                            mention_count: badge.mentions,
                            is_muted: badge.muted,
                        }
                    }).collect();

                    let model = std::rc::Rc::new(slint::VecModel::from(ui_guilds));
                    ui.set_guilds(model.into());
                }
            });

            // Automatically select the first guild and fetch its channels & messages
            if let Some(first_gid) = first_guild_id_opt {
                fetch_and_populate_channels(
                    http,
                    app_weak.clone(),
                    guilds_map.clone(),
                    active_guild_id.clone(),
                    active_channel_id.clone(),
                    cmd_tx_store.clone(),
                    &first_gid,
                ).await;
            }

            // Download custom server icons asynchronously in the background
            if !pending_icon_downloads.is_empty() {
                let http_dl = http.clone();
                let app_w_dl = app_weak.clone();

                tokio::spawn(async move {
                    for (url, save_path) in pending_icon_downloads {
                        if let Ok(bytes) = http_dl.download_image(&url).await {
                            let _ = std::fs::write(&save_path, &bytes);
                        }
                    }

                    // Reload UI with newly cached icons
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w_dl.upgrade() {
                            let old_guilds = ui.get_guilds();
                            let mut updated = Vec::new();
                            for i in 0..old_guilds.row_count() {
                                if let Some(mut item) = old_guilds.row_data(i) {
                                    let gid = item.id.to_string();
                                    let icon_path = cache_dir.join(format!("{}.png", gid));
                                    if icon_path.exists() {
                                        if let Some(img) = load_rounded_guild_icon(&icon_path.to_string_lossy(), ui.window().scale_factor()) {
                                            item.has_image = true;
                                            item.icon_image = img;
                                        }
                                    }
                                    updated.push(item);
                                }
                            }
                            let model = std::rc::Rc::new(slint::VecModel::from(updated));
                            ui.set_guilds(model.into());
                        }
                    });
                });
            }

            // Preloads the DM channel cache (@me) in the background so direct messages are detected immediately
            let http_dms = http.clone();
            let gmap_dms = guilds_map.clone();
            tokio::spawn(async move {
                prefetch_dms_cache(&http_dms, gmap_dms).await;
            });
        }
        Err(e) => {
            error!("Error fetching servers via REST: {}", e);
        }
    }
}

pub async fn fetch_and_populate_channels(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
    cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
    guild_id: &str,
) {
    info!("Fetching channels of server {} via REST API...", guild_id);
    *active_guild_id.lock().unwrap() = guild_id.to_string();

    let g_name = guilds_map.lock().unwrap()
        .get(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Server".to_string());

    let app_w_top = app_weak.clone();
    let g_name_top = g_name.clone();
    let g_id_top = guild_id.to_string();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_w_top.upgrade() {
            ui.set_connection_status(format!("Server: {} | Gateway v9 (Online)", g_name_top).into());
            ui.set_active_guild_name(g_name_top.into());
            ui.set_active_guild_id(g_id_top.into());
        }
    });

    // A server visited before shows its channels and opens its chat at once; the REST
    // answers below only correct the list.
    let cached: Vec<ChannelData> = guilds_map.lock().unwrap().get(guild_id).map(|g| g.channels.clone()).unwrap_or_default();
    if !cached.is_empty() {
        show_guild_channels(&app_weak, &active_guild_id, guild_id, &cached);
    }

    // Channels and permissions are fetched together, while the chat of the cached list loads.
    let (opened, (chans_res, perms_res)) = tokio::join!(
        async {
            if cached.is_empty() {
                None
            } else {
                open_first_readable_channel(http, &app_weak, &active_guild_id, &active_channel_id, guild_id, &cached).await
            }
        },
        async { tokio::join!(http.get_guild_channels(guild_id), http.get_channel_permissions(guild_id)) }
    );

    let chans_json = match chans_res {
        Ok(chans) => {
            info!("{} channels found in the server!", chans.len());
            chans
        }
        Err(e) => {
            error!("Error fetching server channels via REST: {}", e);
            return;
        }
    };
    let permissions = match perms_res {
        Ok(permissions) => permissions,
        Err(err) => {
            if !cached.is_empty() {
                // The cached list stays on screen.
                error!("Could not verify the server permissions: {}", err);
                return;
            }
            error!("Could not verify the server permissions: {}", err);
            *active_channel_id.lock().unwrap() = String::new();
            if let Some(guild) = guilds_map.lock().unwrap().get_mut(guild_id) {
                guild.channels.clear();
            }
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
                    ui.set_channels(slint::ModelRc::default());
                    ui.set_messages(slint::ModelRc::default());
                    ui.set_active_channel_id("".into());
                    ui.set_active_channel_name(crate::utils::i18n_ui::ui_text().perms_unavailable.into());
                    ui.set_active_channel_topic("".into());
                    ui.set_titlebar_error(crate::utils::i18n_ui::ui_text().perms_failed.into());
                }
            });
            return;
        }
    };
    let app_w_permissions = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_w_permissions.upgrade() {
            ui.set_titlebar_error("".into());
        }
    });

    pub struct RawChan {
        id: String,
        name: String,
        ch_type: u64,
        parent_id: Option<String>,
        position: i64,
        topic: Option<String>,
    }

    let mut raw_cats: Vec<RawChan> = Vec::new();
    let mut raw_chans: Vec<RawChan> = Vec::new();

    for ch in chans_json {
        let ch_id = ch["id"].as_str().unwrap_or("").to_string();
        let ch_name = ch["name"].as_str().unwrap_or("channel").to_string();
        let ch_type = ch["type"].as_u64().unwrap_or(0);
        let parent_id = ch["parent_id"].as_str().map(|s| s.to_string());
        let position = ch["position"].as_i64().unwrap_or(0);
        let topic = ch["topic"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);

        if ch_id.is_empty() { continue; }

        if ch_type == 4 {
            raw_cats.push(RawChan {
                id: ch_id,
                name: ch_name.to_uppercase(),
                ch_type,
                parent_id: None,
                position,
                topic: None,
            });
        } else if ch_type == 0 || ch_type == 2 || ch_type == 5 || ch_type == 13 || ch_type == 15 {
            match permissions.can_view(&ch) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(err) => {
                    error!("Channel {} skipped: {}", ch_id, err);
                    continue;
                }
            }
            raw_chans.push(RawChan {
                id: ch_id,
                name: ch_name,
                ch_type,
                parent_id,
                position,
                topic,
            });
        }
    }

    raw_cats.sort_by_key(|c| c.position);

    let cat_ids: std::collections::HashSet<String> = raw_cats.iter().map(|c| c.id.clone()).collect();
    let mut cat_map: HashMap<String, Vec<RawChan>> = HashMap::new();
    let mut uncategorized: Vec<RawChan> = Vec::new();

    for ch in raw_chans {
        if let Some(ref pid) = ch.parent_id {
            if cat_ids.contains(pid) {
                cat_map.entry(pid.clone()).or_default().push(ch);
                continue;
            }
        }
        uncategorized.push(ch);
    }

    let custom_chan_order: Option<Vec<String>> = std::fs::read_to_string(format!(".litecord_channel_order_{}.json", guild_id))
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok());

    let sort_channel_list = |list: &mut Vec<RawChan>| {
        if let Some(ref order) = custom_chan_order {
            list.sort_by_key(|c| order.iter().position(|id| id == &c.id).unwrap_or(usize::MAX));
        } else {
            list.sort_by(|a, b| {
                let a_is_voice = a.ch_type == 2 || a.ch_type == 13;
                let b_is_voice = b.ch_type == 2 || b.ch_type == 13;
                if a_is_voice == b_is_voice {
                    a.position.cmp(&b.position)
                } else if !a_is_voice {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            });
        }
    };

    sort_channel_list(&mut uncategorized);
    for children in cat_map.values_mut() {
        sort_channel_list(children);
    }

    let mut channels_data: Vec<ChannelData> = Vec::new();

    for ch in uncategorized {
        let is_voice = ch.ch_type == 2 || ch.ch_type == 13;
        channels_data.push(ChannelData {
            id: ch.id,
            name: ch.name,
            is_voice,
            is_category: false,
            parent_id: None,
            position: ch.position,
            topic: ch.topic,
        });
    }

    for cat in raw_cats {
        let cat_id = cat.id.clone();
        let children = cat_map.remove(&cat_id).unwrap_or_default();
        if children.is_empty() { continue; }

        channels_data.push(ChannelData {
            id: cat.id,
            name: cat.name,
            is_voice: false,
            is_category: true,
            parent_id: None,
            position: cat.position,
            topic: None,
        });

        for ch in children {
            let is_voice = ch.ch_type == 2 || ch.ch_type == 13;
            channels_data.push(ChannelData {
                id: ch.id,
                name: ch.name,
                is_voice,
                is_category: false,
                parent_id: Some(cat_id.clone()),
                position: ch.position,
                topic: ch.topic,
            });
        }
    }

    // Subscribe via Gateway Opcode 14 for all voice channels in this guild to get live voice_states
    let voice_cids: Vec<String> = channels_data.iter().filter(|c| c.is_voice).map(|c| c.id.clone()).collect();
    if let Some(tx) = cmd_tx_store.lock().unwrap().as_ref() {
        let _ = tx.try_send(GatewayCommand::SubscribeGuild {
            guild_id: guild_id.to_string(),
            channel_ids: voice_cids,
        });
    }

    // Update channels in guilds_map
    if let Some(g_data) = guilds_map.lock().unwrap().get_mut(guild_id) {
        g_data.channels = channels_data.clone();
        gateway::channel_names::remember(guild_id, channels_data.iter().map(|c| (c.id.as_str(), c.name.as_str())));
    }

    show_guild_channels(&app_weak, &active_guild_id, guild_id, &channels_data);

    // Members and bot commands are not needed to read the chat: they load in the background.
    {
        let (http, app_w, guild_id) = (http.clone(), app_weak.clone(), guild_id.to_string());
        tokio::spawn(async move {
            preload_guild_members(&http, &guild_id).await;
            load_guild_command_index(&http, app_w, &guild_id).await;
        });
    }

    // The chat opened from the cached list stays, unless that channel is no longer listed.
    if opened.as_ref().is_some_and(|id| channels_data.iter().any(|c| &c.id == id)) {
        return;
    }
    let opened_now = open_first_readable_channel(http, &app_weak, &active_guild_id, &active_channel_id, guild_id, &channels_data).await.is_some();
    if !opened_now && *active_guild_id.lock().unwrap() == guild_id {
        *active_channel_id.lock().unwrap() = String::new();
        let app_w_none = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = app_w_none.upgrade() {
                ui.set_active_channel_name(crate::utils::i18n_ui::ui_text().no_text_channel.into());
                ui.set_active_channel_topic("".into());
                ui.set_active_channel_id("".into());
                let empty_msgs = vec![ChatMessage {
                    id: "".into(),
                    author: "Litecord System".into(),
                    content: "This server has no text channels accessible to your account.".into(),
                    commands: slint::ModelRc::default(),
                    content_lines: slint::ModelRc::default(),
                    embed_content: "".into(),
                    embed_lines: slint::ModelRc::default(),
                    embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                    embed_footer: "".into(),
                    code_block: "".into(),
                    reply_author: "".into(),
                    reply_content: "".into(),
                    reply_command: "".into(),
                    links: slint::ModelRc::default(),
                    buttons: slint::ModelRc::default(),
                    attachments: slint::ModelRc::default(),
                    timestamp: crate::utils::i18n_ui::ui_text().now_label.into(),
                    show_header: true,
                    ..Default::default()
                }];
                let model = std::rc::Rc::new(slint::VecModel::from(empty_msgs));
                ui.set_messages(model.into());
                ui.set_has_more_older_messages(false);
            }
        });
    }
}

/// Puts `channels` in the sidebar, unless the user has moved to another server meanwhile.
fn show_guild_channels(app_weak: &slint::Weak<AppWindow>, active_guild_id: &Arc<Mutex<String>>, guild_id: &str, channels: &[ChannelData]) {
    let ui_channels = {
        let cats_guard = get_collapsed_categories().lock().unwrap().clone();
        build_ui_channels(channels, &cats_guard)
    };
    let (app_w, active, gid) = (app_weak.clone(), Arc::clone(active_guild_id), guild_id.to_string());
    let _ = slint::invoke_from_event_loop(move || {
        if *active.lock().unwrap() != gid {
            return;
        }
        if let Some(ui) = app_w.upgrade() {
            ui.set_channels(std::rc::Rc::new(slint::VecModel::from(ui_channels)).into());
        }
    });
}

/// A channel change received from the gateway.
pub enum ChannelChange {
    Created(ChannelData, serde_json::Value),
    Updated(ChannelData, serde_json::Value),
    Deleted(String),
}

/// Applies a live channel change to the cached list of `guild_id` and, when that server is
/// open, to the sidebar and chat header. A server whose list is not loaded yet is left alone:
/// opening it fetches the channels fresh.
pub async fn apply_live_channel_change(
    http: Option<DiscordHttpClient>,
    app_weak: slint::Weak<AppWindow>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
    guild_id: String,
    change: ChannelChange,
) {
    use crate::gateway::channel_events::{apply_channel_create, apply_channel_delete, apply_channel_update};
    // None while the server's permissions are not cached: the channel is then not judged (and shown).
    let visible = |ch: &ChannelData, raw: &serde_json::Value| -> Option<bool> {
        if ch.is_category {
            return None;
        }
        crate::channel_permissions::cached(&guild_id).and_then(|p| p.can_view(raw).ok())
    };
    let (channels, changed_id) = {
        let Ok(mut map) = guilds_map.lock() else { return };
        let Some(guild) = map.get_mut(&guild_id) else { return };
        if guild.channels.is_empty() {
            return;
        }
        let (changed, id) = match change {
            ChannelChange::Created(ch, raw) => {
                let id = ch.id.clone();
                let seen = visible(&ch, &raw) != Some(false);
                (seen && apply_channel_create(&mut guild.channels, ch), id)
            }
            ChannelChange::Updated(ch, raw) => {
                let id = ch.id.clone();
                let changed = match visible(&ch, &raw) {
                    Some(false) => apply_channel_delete(&mut guild.channels, &id),
                    Some(true) => {
                        let again = ch.clone();
                        apply_channel_update(&mut guild.channels, ch) || apply_channel_create(&mut guild.channels, again)
                    }
                    None => apply_channel_update(&mut guild.channels, ch),
                };
                (changed, id)
            }
            ChannelChange::Deleted(id) => (apply_channel_delete(&mut guild.channels, &id), id),
        };
        if !changed {
            return;
        }
        (guild.channels.clone(), id)
    };
    gateway::channel_names::remember(&guild_id, channels.iter().map(|c| (c.id.as_str(), c.name.as_str())));

    if *active_guild_id.lock().unwrap() != guild_id {
        return;
    }
    show_guild_channels(&app_weak, &active_guild_id, &guild_id, &channels);

    if *active_channel_id.lock().unwrap() != changed_id {
        return;
    }
    match channels.iter().find(|c| c.id == changed_id) {
        Some(ch) if !ch.is_voice && !ch.is_category => {
            let (app_w, name, topic) = (app_weak.clone(), ch.name.clone(), ch.topic.clone().unwrap_or_default());
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    ui.set_active_channel_name(crate::utils::styled_text::plain_display_name(name.trim_start_matches('#').trim()).as_ref().into());
                    ui.set_active_channel_topic(topic.into());
                }
            });
        }
        Some(_) => {}
        None => {
            // The open channel was deleted or is no longer visible: move to the first readable one.
            let opened = match http {
                Some(ref http) => open_first_readable_channel(http, &app_weak, &active_guild_id, &active_channel_id, &guild_id, &channels).await,
                None => None,
            };
            if opened.is_none() && *active_guild_id.lock().unwrap() == guild_id {
                *active_channel_id.lock().unwrap() = String::new();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_weak.upgrade() {
                        ui.set_active_channel_name(crate::utils::i18n_ui::ui_text().no_text_channel.into());
                        ui.set_active_channel_topic("".into());
                        ui.set_active_channel_id("".into());
                        ui.set_messages(slint::ModelRc::default());
                    }
                });
            }
        }
    }
}

/// The account left or lost a server: forgets it and, when it was open, empties the view.
pub fn remove_guild(
    app_weak: slint::Weak<AppWindow>,
    guilds_map: &Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: &Arc<Mutex<String>>,
    active_channel_id: &Arc<Mutex<String>>,
    guild_id: &str,
) {
    if let Ok(mut map) = guilds_map.lock() {
        map.remove(guild_id);
    }
    crate::channel_permissions::forget(guild_id);
    let was_open = {
        let mut active = active_guild_id.lock().unwrap();
        let open = *active == guild_id;
        if open {
            *active = String::new();
        }
        open
    };
    if was_open {
        *active_channel_id.lock().unwrap() = String::new();
    }
    let gid = guild_id.to_string();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_weak.upgrade() {
            let remaining: Vec<GuildItem> = ui.get_guilds().iter().filter(|g| g.id != gid.as_str()).collect();
            ui.set_guilds(std::rc::Rc::new(slint::VecModel::from(remaining)).into());
            if was_open {
                ui.set_channels(slint::ModelRc::default());
                ui.set_messages(slint::ModelRc::default());
                ui.set_active_channel_id("".into());
                ui.set_active_channel_name(crate::utils::i18n_ui::ui_text().no_text_channel.into());
                ui.set_active_channel_topic("".into());
                ui.set_active_guild_id("".into());
                ui.set_active_guild_name("".into());
            }
        }
    });
}

/// Opens the first text channel of `channels` the account can read. Stops when the user
/// moves to another server or opens a channel meanwhile. Returns the channel now open.
async fn open_first_readable_channel(
    http: &DiscordHttpClient,
    app_weak: &slint::Weak<AppWindow>,
    active_guild_id: &Arc<Mutex<String>>,
    active_channel_id: &Arc<Mutex<String>>,
    guild_id: &str,
    channels: &[ChannelData],
) -> Option<String> {
    for text_ch in channels.iter().filter(|c| !c.is_voice && !c.is_category) {
        if *active_guild_id.lock().unwrap() != guild_id {
            return None;
        }
        let ch_id = text_ch.id.clone();
        *active_channel_id.lock().unwrap() = ch_id.clone();
        let readable = load_messages_for_channel(http, app_weak.clone(), &ch_id).await;
        let current = active_channel_id.lock().unwrap().clone();
        if current != ch_id {
            // The user opened another channel while this one loaded.
            return (!current.is_empty()).then_some(current);
        }
        if readable {
            let (app_w, name, topic) = (app_weak.clone(), text_ch.name.clone(), text_ch.topic.clone().unwrap_or_default());
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    ui.set_active_channel_name(crate::utils::styled_text::plain_display_name(name.trim_start_matches('#').trim()).as_ref().into());
                    ui.set_active_channel_topic(topic.into());
                    ui.set_active_channel_id(ch_id.into());
                }
            });
            request_chat_scroll_to_bottom(app_weak.clone());
            return Some(current);
        }
    }
    *active_channel_id.lock().unwrap() = String::new();
    None
}

/// Names of the server's members and bots, for mentions and voice rows.
async fn preload_guild_members(http: &DiscordHttpClient, guild_id: &str) {
    if let Ok(members) = http.get_guild_members(guild_id).await {
        for m in members {
            let uid_str = m["user"]["id"].as_str().unwrap_or("");
            if let Ok(uid) = uid_str.parse::<u64>() {
                let mut display_name = String::new();
                if let Some(nick) = m["nick"].as_str() {
                    if !nick.is_empty() { display_name = nick.to_string(); }
                }
                if display_name.is_empty() {
                    if let Some(gname) = m["user"]["global_name"].as_str() {
                        if !gname.is_empty() { display_name = gname.to_string(); }
                    }
                }
                if display_name.is_empty() {
                    if let Some(uname) = m["user"]["username"].as_str() {
                        if !uname.is_empty() { display_name = uname.to_string(); }
                    }
                }
                if !display_name.is_empty() {
                    gateway::register_user_name(uid, display_name);
                }
            }
        }
        info!("✅ Members and bots of server {} preloaded via REST API successfully!", guild_id);
    }
}

pub async fn fetch_and_populate_dms(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
) {
    info!("Fetching DM conversations via REST API (/users/@me/channels)...");
    *active_guild_id.lock().unwrap() = "@me".to_string();

    let app_w_top = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_w_top.upgrade() {
            ui.set_connection_status("Direct Messages | Gateway v9 (Online)".into());
            ui.set_active_guild_name("Direct Messages".into());
            ui.set_active_guild_id("@me".into());
        }
    });

    match http.get_user_dms().await {
        Ok(dms_json) => {
            info!("{} DM conversations found via REST!", dms_json.len());

            let mut dms_sorted = dms_json;
            // Sorts by last_message_id descending (larger Snowflake = more recent message)
            dms_sorted.sort_by(|a, b| {
                let a_id: u64 = a["last_message_id"].as_str()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
                let b_id: u64 = b["last_message_id"].as_str()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
                b_id.cmp(&a_id)
            });

            let mut channels_data: Vec<ChannelData> = Vec::new();
            let mut ui_channels: Vec<ChannelItem> = Vec::new();
            let mut first_ch_id_opt: Option<String> = None;

            for dm in &dms_sorted {
                let ch_id = dm["id"].as_str().unwrap_or("").to_string();
                if ch_id.is_empty() { continue; }

                let ch_type = dm["type"].as_u64().unwrap_or(1);

                let (ch_name, avatar_text) = if ch_type == 3 {
                    // Group
                    let name = if let Some(n) = dm["name"].as_str() {
                        if !n.is_empty() { n.to_string() } else { "Group".to_string() }
                    } else if let Some(recipients) = dm["recipients"].as_array() {
                        let names: Vec<String> = recipients.iter()
                            .filter_map(|r| r["global_name"].as_str().or_else(|| r["username"].as_str()))
                            .map(|s| s.to_string())
                            .collect();
                        if names.is_empty() { "Group".to_string() } else { names.join(", ") }
                    } else {
                        "Group".to_string()
                    };
                    let initials = get_guild_initials(&name);
                    (name, initials)
                } else {
                    // Direct 1:1 DM
                    let (name, initials) = if let Some(recipients) = dm["recipients"].as_array() {
                        if let Some(first_user) = recipients.first() {
                            let disp_name = first_user["global_name"].as_str()
                                .unwrap_or_else(|| first_user["username"].as_str().unwrap_or("Friend"));
                            let inits = get_guild_initials(disp_name);
                            (disp_name.to_string(), inits)
                        } else {
                            ("Friend".to_string(), "DM".to_string())
                        }
                    } else {
                        ("Friend".to_string(), "DM".to_string())
                    };
                    (name, initials)
                };

                let avatar_url = if ch_type == 3 {
                    crate::ui::message_grouping::group_icon_url(&ch_id, dm["icon"].as_str().unwrap_or(""))
                } else {
                    dm["recipients"]
                        .as_array()
                        .and_then(|recipients| recipients.first())
                        .map(|user| {
                            let uid = user["id"].as_str().unwrap_or("").parse::<u64>().unwrap_or(0);
                            gateway::register_user_avatar(uid, user["avatar"].as_str().unwrap_or(""));
                            gateway::get_user_avatar_url(uid)
                        })
                        .unwrap_or_default()
                };

                if ch_type == 1 {
                    if let Some(uid) = dm["recipients"].as_array().and_then(|r| r.first()).and_then(|u| u["id"].as_str()) {
                        crate::relationships::register_dm_recipient(&ch_id, uid);
                    }
                }

                if first_ch_id_opt.is_none() {
                    first_ch_id_opt = Some(ch_id.clone());
                }

                channels_data.push(ChannelData {
                    id: ch_id.clone(),
                    name: ch_name.clone(),
                    is_voice: false,
                    is_category: false,
                    parent_id: None,
                    position: 0,
                    topic: None,
                });

                let unread_dm_count = crate::ui::helpers::get_dm_unread(&ch_id);

                ui_channels.push(ChannelItem {
                    id: ch_id.into(),
                    name: crate::utils::styled_text::plain_display_name(&ch_name).as_ref().into(),
                    is_voice: false,
                    voice_count: 0,
                    is_voice_member: false,
                    parent_channel_id: "".into(),
                    avatar_text: avatar_text.into(),
                    avatar_slot: avatar_cache::get_avatar_cache().slot_for(&avatar_url),
                    avatar_url: avatar_url.into(),
                    is_category: false,
                    has_parent: false,
                    is_collapsed: false,
                    has_separator: false,
                    unread_count: unread_dm_count,
                    ..Default::default()
                });
            }

            // Registers @me in the map so on_select_channel works transparently
            guilds_map.lock().unwrap().insert("@me".to_string(), GuildData {
                id: "@me".to_string(),
                name: "Direct Messages".to_string(),
                channels: channels_data,
            });

            let app_w_ui = app_weak.clone();
            let ui_channels_clone = ui_channels.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w_ui.upgrade() {
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_channels_clone));
                    ui.set_channels(model.into());
                    ui.set_total_dm_unread_count(crate::ui::helpers::get_total_dm_unreads());
                }
            });

            // If no channel is selected or the current one is not in the DM list, selects the first one
            let current_c = active_channel_id.lock().unwrap().clone();
            let target_ch_id = if current_c.is_empty() || !ui_channels.iter().any(|c| c.id == current_c.as_str()) {
                first_ch_id_opt
            } else {
                Some(current_c)
            };

            if let Some(cid) = target_ch_id {
                let remaining = crate::ui::helpers::clear_dm_unread(&cid);
                for c in ui_channels.iter_mut() {
                    if c.id == cid.as_str() {
                        c.unread_count = 0;
                    }
                }

                let target_name = ui_channels.iter()
                    .find(|c| c.id == cid.as_str())
                    .map(|c| c.name.to_string())
                    .unwrap_or_else(|| "conversation".to_string());
                *active_channel_id.lock().unwrap() = cid.clone();
                let app_w_name = app_weak.clone();
                let cid_top = cid.clone();
                let ui_channels_updated = ui_channels.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w_name.upgrade() {
                        ui.set_active_channel_id(cid_top.into());
                        ui.set_active_channel_name(crate::utils::styled_text::plain_display_name(&target_name).as_ref().into());
                        ui.set_active_channel_topic("".into());
                        let model = std::rc::Rc::new(slint::VecModel::from(ui_channels_updated));
                        ui.set_channels(model.into());
                        ui.set_total_dm_unread_count(remaining);
                    }
                });

                load_messages_for_channel(http, app_weak.clone(), &cid).await;
            }
        }
        Err(e) => {
            error!("Error fetching DM conversations via REST: {}", e);
        }
    }
}

pub async fn prefetch_dms_cache(
    http: &DiscordHttpClient,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
) {
    if let Ok(dms_json) = http.get_user_dms().await {
        let mut channels_data: Vec<ChannelData> = Vec::new();
        for dm in &dms_json {
            let ch_id = dm["id"].as_str().unwrap_or("").to_string();
            if ch_id.is_empty() { continue; }
            channels_data.push(ChannelData {
                id: ch_id,
                name: "DM".to_string(),
                is_voice: false,
                is_category: false,
                parent_id: None,
                position: 0,
                topic: None,
            });
        }
        if let Ok(mut gmap) = guilds_map.lock() {
            gmap.insert("@me".to_string(), GuildData {
                id: "@me".to_string(),
                name: "Direct Messages".to_string(),
                channels: channels_data,
            });
        }
    }
}

pub async fn load_messages_for_channel(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    channel_id: &str,
) -> bool {
    info!("Loading messages of channel {}...", channel_id);
    emoji_cache::get_emoji_cache().set_active_channel(channel_id);
    let ch_id_for_emojis = channel_id.to_string();
    let app_weak_load = app_weak.clone();
    match http.get_channel_messages(channel_id).await {
        Ok(msgs_val) => {
            // The channel is being opened: tell Discord it was read up to its newest message.
            if let Some(newest) = msgs_val.first().and_then(|m| m["id"].as_str()) {
                let (http_ack, channel_ack, newest) = (http.clone(), channel_id.to_string(), newest.to_string());
                tokio::spawn(async move {
                    let _ = http_ack.ack_message(&channel_ack, &newest).await;
                });
            }
            let has_more = msgs_val.len() >= 30;
            if let Some(oldest) = msgs_val.last() {
                if let Some(id_str) = oldest["id"].as_str() {
                    get_oldest_message_map().lock().unwrap().insert(channel_id.to_string(), id_str.to_string());
                }
            }

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
                    ui.set_has_more_older_messages(has_more);
                    ui.set_is_loading_older_messages(false);

                    let mut ui_msgs: Vec<ChatMessage> = if msgs_val.is_empty() {
                        vec![ChatMessage {
                            id: "".into(),
                            author: "Litecord System".into(),
                            content: "This channel is empty or has no recent messages.".into(),
                            commands: slint::ModelRc::default(),
                            content_lines: slint::ModelRc::default(),
                            embed_content: "".into(),
                            embed_lines: slint::ModelRc::default(),
                            embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                            embed_footer: "".into(),
                            code_block: "".into(),
                            reply_author: "".into(),
                            reply_content: "".into(),
                            reply_command: "".into(),
                            links: slint::ModelRc::default(),
                            buttons: slint::ModelRc::default(),
                            attachments: slint::ModelRc::default(),
                            timestamp: crate::utils::i18n_ui::ui_text().now_label.into(),
                            show_header: true,
                            ..Default::default()
                        }]
                    } else {
                        msgs_val.iter().rev().map(|m| {
                            let msg_id = m["id"].as_str().unwrap_or("");
                            let author = format_discord_author(m);
                            let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(m);
                            
                            let slint_cmds: Vec<slint::SharedString> = commands.into_iter().map(|c| c.into()).collect();
                            let commands_model = std::rc::Rc::new(slint::VecModel::from(slint_cmds));

                            let slint_links: Vec<LinkItem> = links.iter().map(|l| LinkItem {
                                label: l.label.clone().into(),
                                url: l.url.clone().into(),
                            }).collect();
                            let links_model = std::rc::Rc::new(slint::VecModel::from(slint_links));

                            let mut message = ChatMessage {
                                id: msg_id.into(),
                                author: author.into(),
                                content: content.into(),
                                commands: slint::ModelRc::from(commands_model),
                                content_lines: map_message_lines(&content_lines, &ch_id_for_emojis, &app_weak_load),
                                embed_content: embed_content.into(),
                                embed_lines: map_message_lines(&embed_lines, &ch_id_for_emojis, &app_weak_load),
                                embed_color: parse_hex_color(&embed_color),
                                embed_footer: embed_footer.into(),
                                code_block: code_block.into(),
                                reply_author: reply_author.into(),
                                reply_content: reply_content.into(),
                                reply_command: reply_command.into(),
                                links: slint::ModelRc::from(links_model),
                                buttons: map_message_buttons(&buttons, &ch_id_for_emojis, &app_weak_load),
                                attachments: map_message_attachments(&attachments, &app_weak_load),
                                timestamp: crate::utils::i18n_ui::ui_text().now_label.into(),
                                ..Default::default()
                            };
                            apply_message_metadata(&mut message, m);
                            message
                        }).collect()
                    };
                    regroup_messages(&mut ui_msgs);
                    avatar_cache::get_avatar_cache().hydrate(&mut ui_msgs, &app_weak);
                    crate::ui::invite_cards::hydrate(&mut ui_msgs, &app_weak);
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_msgs));
                    ui.set_messages(model.into());

                    request_chat_scroll_to_bottom(app_weak.clone());
                }
            });
            true
        }
        Err(err) => {
            error!("Error loading messages of channel {}: {}", channel_id, err);
            let friendly_msg = if err.contains("403") || err.contains("Forbidden") {
                "Private Channel\nThis channel is restricted and requires specific roles on the server to view its messages.".to_string()
            } else {
                format!("Could not load the messages ({})", err)
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
                    ui.set_has_more_older_messages(false);
                    let ui_msgs = vec![ChatMessage {
                        id: "".into(),
                        author: "Litecord System".into(),
                        content: friendly_msg.into(),
                        commands: slint::ModelRc::default(),
                        content_lines: slint::ModelRc::default(),
                        embed_content: "".into(),
                        embed_lines: slint::ModelRc::default(),
                        embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                        embed_footer: "".into(),
                        code_block: "".into(),
                        reply_author: "".into(),
                        reply_content: "".into(),
                        reply_command: "".into(),
                        links: slint::ModelRc::default(),
                        buttons: slint::ModelRc::default(),
                        attachments: slint::ModelRc::default(),
                        timestamp: crate::utils::i18n_ui::ui_text().now_label.into(),
                        show_header: true,
                        ..Default::default()
                    }];
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_msgs));
                    ui.set_messages(model.into());
                }
            });
            false
        }
    }
}




pub static COLLAPSED_CATEGORIES: std::sync::OnceLock<Arc<Mutex<std::collections::HashSet<String>>>> = std::sync::OnceLock::new();
pub fn get_collapsed_categories() -> Arc<Mutex<std::collections::HashSet<String>>> {
    COLLAPSED_CATEGORIES.get_or_init(|| Arc::new(Mutex::new(std::collections::HashSet::new()))).clone()
}


pub async fn try_login_with_candidates(
    candidates: Vec<String>,
    app_weak: slint::Weak<AppWindow>,
    http_client: Arc<Mutex<Option<DiscordHttpClient>>>,
    last_token: Arc<Mutex<String>>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
    cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
    event_tx_gw: mpsc::Sender<GatewayEvent>,
) -> bool {
    for token_str in candidates {
        info!("Testing token candidate via Discord REST API...");
        let http = DiscordHttpClient::new(token_str.clone());
        match http.get_current_user().await {
            Ok(user_info) => {
                crate::ui::invite_cards::set_session_client(http.clone());
                let username = user_info["username"].as_str().unwrap_or("User");
                let user_id = user_info["id"].as_str().unwrap_or("");
                let global_name = user_info["global_name"].as_str().unwrap_or(username);
                let tag = if let Some(discrim) = user_info["discriminator"].as_str() {
                    if discrim != "0" {
                        format!("{}#{}", username, discrim)
                    } else {
                        format!("@{}", username)
                    }
                } else {
                    format!("@{}", username)
                };
                let display_tag = if !global_name.is_empty() && global_name != username {
                    format!("{} ({})", global_name, tag)
                } else {
                    tag.clone()
                };
                info!("VALID token found! Connected as {} ({})", username, user_id);

                let vault = save_or_update_account(&token_str, user_id, global_name, &tag);
                sync_ui_saved_accounts(&app_weak, &vault);

                *last_token.lock().unwrap() = token_str.clone();
                *http_client.lock().unwrap() = Some(http.clone());

                let avatar_hash = user_info["avatar"].as_str().unwrap_or("");
                let avatar_url = if !avatar_hash.is_empty() && !user_id.is_empty() {
                    format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", user_id, avatar_hash)
                } else {
                    String::new()
                };
                let display_name_val = if !global_name.is_empty() {
                    global_name.to_string()
                } else {
                    username.to_string()
                };
                let handle_val = tag.clone();
                let initials_val = crate::ui::helpers::get_guild_initials(&display_name_val);

                let app_w = app_weak.clone();
                let display_tag_clone = display_tag.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w.upgrade() {
                        ui.set_is_logged_in(true);
                        ui.set_user_tag(display_tag_clone.into());
                        ui.set_user_display_name(display_name_val.into());
                        ui.set_user_handle(handle_val.into());
                        ui.set_user_avatar_initials(initials_val.into());
                        ui.set_user_avatar_url(avatar_url.clone().into());
                        if !avatar_url.is_empty() {
                            let img = avatar_cache::get_avatar_cache().image_for(&avatar_url);
                            if img.size().width > 0 {
                                ui.set_user_avatar_img(img);
                                ui.set_user_avatar_has_image(true);
                            }
                        }
                        ui.set_connection_status("Connected successfully!".into());
                        ui.set_login_alert_message("".into());
                        ui.set_titlebar_error("".into());
                    }
                });

                trigger_update_check(app_weak.clone());

                let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(100);
                *cmd_tx_store.lock().unwrap() = Some(cmd_tx);

                fetch_and_populate_guilds(
                    &http,
                    app_weak.clone(),
                    guilds_map,
                    active_guild_id,
                    active_channel_id,
                    cmd_tx_store,
                ).await;

                crate::gateway::start_gateway_task(Arc::new(GatewayClient::new(token_str, event_tx_gw)), cmd_rx);

                return true;
            }
            Err(err_msg) => {
                info!("Token candidate rejected by Discord ({}). Trying next candidate...", err_msg);
                if crate::utils::is_proxy_error(&err_msg) {
                    let app_w = app_weak.clone();
                    let proxy_msg = crate::utils::format_proxy_alert_message();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_login_alert_message(proxy_msg.into());
                            ui.set_login_alert_is_error(true);
                        }
                    });
                }
            }
        }
    }
    false
}


pub static PENDING_UPDATE_STORE: std::sync::OnceLock<Arc<Mutex<Option<updater::ReleaseInfo>>>> = std::sync::OnceLock::new();

pub fn get_pending_update_store() -> Arc<Mutex<Option<updater::ReleaseInfo>>> {
    PENDING_UPDATE_STORE.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}

pub static OLDEST_MESSAGE_MAP: std::sync::OnceLock<Arc<Mutex<HashMap<String, String>>>> = std::sync::OnceLock::new();

pub fn get_oldest_message_map() -> Arc<Mutex<HashMap<String, String>>> {
    OLDEST_MESSAGE_MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone()
}


pub fn trigger_update_check(app_weak: slint::Weak<AppWindow>) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        if let Some(rel) = updater::check_for_updates().await {
            info!("🔔 Update v{} is ready to be shown to the user!", rel.version);
            *get_pending_update_store().lock().unwrap() = Some(rel.clone());
            let app_w = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    ui.set_update_version(format!("v{}", rel.version).into());
                    ui.set_update_release_name(rel.release_name.into());
                    ui.set_show_update_dialog(true);
                }
            });
        }
    });
}

/// Server icon with its corners already rounded (radius = a third of the side, the 16 px
/// of the 48 px tile), so it looks the same on the software renderer.
/// It is resized here, once and with a smooth filter, to the tile's size in physical
/// pixels: the software renderer would otherwise shrink the full-size icon on every
/// frame without smoothing, which made the edges jagged. It also keeps less in memory.
fn load_rounded_guild_icon(path: &str, scale_factor: f32) -> Option<Image> {
    const TILE_LOGICAL_PX: f32 = 48.0;
    let side = (TILE_LOGICAL_PX * scale_factor.clamp(1.0, 4.0)).round() as u32;
    let decoded = image::open(path).ok()?;
    let mut rgba = if decoded.width() > side || decoded.height() > side {
        decoded.resize_to_fill(side, side, image::imageops::FilterType::Lanczos3).to_rgba8()
    } else {
        decoded.to_rgba8()
    };
    let (w, h) = rgba.dimensions();
    crate::utils::image_shape::round_corners(&mut rgba, w, h, w.min(h) as f32 / 3.0);
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba.as_raw(), w, h);
    Some(Image::from_rgba8(buffer))
}

/// Converts raw API messages (newest first) into chat rows (oldest first).
fn build_chat_rows(msgs_val: &[serde_json::Value], channel_id: &str, app_weak: &slint::Weak<AppWindow>) -> Vec<ChatMessage> {
    msgs_val.iter().rev().map(|m| {
        let msg_id = m["id"].as_str().unwrap_or("");
        let author = format_discord_author(m);
        let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(m);

        let slint_cmds: Vec<slint::SharedString> = commands.into_iter().map(|c| c.into()).collect();
        let commands_model = std::rc::Rc::new(slint::VecModel::from(slint_cmds));

        let slint_links: Vec<LinkItem> = links.iter().map(|l| LinkItem {
            label: l.label.clone().into(),
            url: l.url.clone().into(),
        }).collect();
        let links_model = std::rc::Rc::new(slint::VecModel::from(slint_links));

        let mut message = ChatMessage {
            id: msg_id.into(),
            author: author.into(),
            content: content.into(),
            commands: slint::ModelRc::from(commands_model),
            content_lines: map_message_lines(&content_lines, channel_id, app_weak),
            embed_content: embed_content.into(),
            embed_lines: map_message_lines(&embed_lines, channel_id, app_weak),
            embed_color: parse_hex_color(&embed_color),
            embed_footer: embed_footer.into(),
            code_block: code_block.into(),
            reply_author: reply_author.into(),
            reply_content: reply_content.into(),
            reply_command: reply_command.into(),
            links: slint::ModelRc::from(links_model),
            buttons: map_message_buttons(&buttons, channel_id, app_weak),
            attachments: map_message_attachments(&attachments, app_weak),
            timestamp: crate::utils::i18n_ui::ui_text().now_label.into(),
            ..Default::default()
        };
        apply_message_metadata(&mut message, m);
        message
    }).collect()
}

/// Asks the chat view to scroll to `target_id`. Delayed slightly so freshly
/// created rows exist before the target property changes.
fn schedule_jump_target(app_weak: slint::Weak<AppWindow>, target_id: String) {
    slint::Timer::single_shot(std::time::Duration::from_millis(80), move || {
        if let Some(ui) = app_weak.upgrade() {
            ui.set_jump_target_id(target_id.clone().into());
        }
    });
}

/// Scrolls to `target_id` in the active channel, loading a window of messages
/// around it first when it is not part of the loaded history.
pub async fn jump_to_message_in_channel(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    channel_id: &str,
    target_id: &str,
) {
    let target = target_id.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    let app_check = app_weak.clone();
    let target_check = target.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let mut found = false;
        if let Some(ui) = app_check.upgrade() {
            let ids: Vec<String> = ui.get_messages().iter().map(|m| m.id.to_string()).collect();
            found = crate::ui::message_jump::find_message_index(&ids, &target_check).is_some();
            if found {
                // Reset first so repeated clicks on the same target still fire a change.
                ui.set_jump_target_id("".into());
                schedule_jump_target(app_check.clone(), target_check.clone());
            }
        }
        let _ = tx.send(found);
    });
    if rx.await.unwrap_or(false) {
        return;
    }

    match http.get_channel_messages_around(channel_id, &target).await {
        Ok(msgs_val) => {
            if msgs_val.is_empty() || !msgs_val.iter().any(|m| m["id"].as_str() == Some(target.as_str())) {
                return;
            }
            if let Some(oldest) = msgs_val.last() {
                if let Some(id_str) = oldest["id"].as_str() {
                    get_oldest_message_map().lock().unwrap().insert(channel_id.to_string(), id_str.to_string());
                }
            }
            let ch_id = channel_id.to_string();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
                    // The user may have opened another channel while the window loaded.
                    if ui.get_active_channel_id().as_str() != ch_id {
                        return;
                    }
                    let mut ui_msgs = build_chat_rows(&msgs_val, &ch_id, &app_weak);
                    regroup_messages(&mut ui_msgs);
                    avatar_cache::get_avatar_cache().hydrate(&mut ui_msgs, &app_weak);
                    crate::ui::invite_cards::hydrate(&mut ui_msgs, &app_weak);
                    ui.set_has_more_older_messages(true);
                    ui.set_is_loading_older_messages(false);
                    ui.set_messages(std::rc::Rc::new(slint::VecModel::from(ui_msgs)).into());
                    ui.set_jump_target_id("".into());
                    schedule_jump_target(app_weak.clone(), target);
                }
            });
        }
        Err(err) => {
            error!("Failed to load messages around {}: {}", target, err);
        }
    }
}

/// Runs a message search and fills the results popup.
pub async fn search_messages_and_show(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    scope: crate::ui::message_jump::SearchScope,
    query: &str,
) {
    let result = http.search_messages(&scope, query).await;
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_weak.upgrade() {
            ui.set_is_searching(false);
            match result {
                Ok(hits) => {
                    ui.set_search_status(format!("{} result(s)", hits.len()).into());
                    let rows: Vec<SearchResultItem> = hits.into_iter().map(|h| SearchResultItem {
                        id: h.id.into(),
                        channel_id: h.channel_id.into(),
                        author: h.author.into(),
                        content: h.content.into(),
                    }).collect();
                    ui.set_search_results(std::rc::Rc::new(slint::VecModel::from(rows)).into());
                }
                Err(err) => {
                    ui.set_search_status(format!("Search failed ({})", err).into());
                    ui.set_search_results(std::rc::Rc::new(slint::VecModel::<SearchResultItem>::default()).into());
                }
            }
        }
    });
}
