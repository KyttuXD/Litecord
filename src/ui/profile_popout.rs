//! Wires the user profile popout (card opened from a message author's avatar or name).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use log::{info, warn};
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer, SharedString};

use crate::gateway::GuildData;
use crate::http::DiscordHttpClient;
use crate::ui::data_loader::fetch_and_populate_dms;
use crate::ui::profile::{
    account_created_label, avatar_url, fallback_banner_color, mutual_servers_label, parse_profile,
    ParsedProfile,
};
use crate::{AppWindow, ProfileRole};

/// Roles shown in the card; the rest collapse into a "+N more" row.
const MAX_ROLES_SHOWN: usize = 8;
const MAX_BIO_CHARS: usize = 400;

fn color_from_rgb(rgb: u32) -> slint::Color {
    slint::Color::from_rgb_u8(((rgb >> 16) & 0xFF) as u8, ((rgb >> 8) & 0xFF) as u8, (rgb & 0xFF) as u8)
}

fn set_status(app_weak: &slint::Weak<AppWindow>, uid: &str, message: String) {
    let app_weak = app_weak.clone();
    let uid = uid.to_string();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_weak.upgrade() {
            if ui.get_show_profile_popout() && ui.get_profile_user_id().as_str() == uid {
                ui.set_profile_status(message.into());
            }
        }
    });
}

/// Role id -> (name, colour, position) for a server.
async fn load_role_table(http: &DiscordHttpClient, guild_id: &str) -> HashMap<String, (String, u32, i64)> {
    let mut table = HashMap::new();
    if let Ok(roles) = http.get_guild_roles(guild_id).await {
        for role in roles {
            let id = role["id"].as_str().unwrap_or("").to_string();
            if id.is_empty() {
                continue;
            }
            let name = role["name"].as_str().unwrap_or("").to_string();
            let color = role["color"].as_u64().unwrap_or(0) as u32;
            let position = role["position"].as_i64().unwrap_or(0);
            table.insert(id, (name, color, position));
        }
    }
    table
}

fn build_role_rows(parsed: &ParsedProfile, table: &HashMap<String, (String, u32, i64)>) -> Vec<(String, u32)> {
    let mut known: Vec<&(String, u32, i64)> = parsed
        .role_ids
        .iter()
        .filter_map(|id| table.get(id))
        .filter(|(name, _, _)| !name.is_empty())
        .collect();
    known.sort_by(|a, b| b.2.cmp(&a.2));
    let mut rows: Vec<(String, u32)> = known
        .iter()
        .take(MAX_ROLES_SHOWN)
        .map(|(name, color, _)| (name.clone(), *color))
        .collect();
    if known.len() > MAX_ROLES_SHOWN {
        rows.push((format!("+{} more", known.len() - MAX_ROLES_SHOWN), 0));
    }
    rows
}

pub fn install(
    app: &AppWindow,
    http_client: Arc<Mutex<Option<DiscordHttpClient>>>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
) {
    // User id of a Block click waiting for its confirming second click.
    let pending_block: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    // Open the card for a user and load their profile.
    let app_weak = app.as_weak();
    let http_open = Arc::clone(&http_client);
    let active_guild_open = Arc::clone(&active_guild_id);
    let pending_block_open = Arc::clone(&pending_block);
    app.on_open_user_profile(move |user_id: SharedString| {
        let uid = user_id.to_string();
        if uid.is_empty() {
            return;
        }
        pending_block_open.lock().unwrap().clear();

        if let Some(ui) = app_weak.upgrade() {
            let is_self = uid == crate::gateway::get_my_user_id().to_string();
            let known_name = uid
                .parse::<u64>()
                .map(crate::gateway::get_user_name)
                .ok()
                .filter(|n| !n.starts_with("Participante #"))
                .unwrap_or_default();
            ui.set_profile_user_id(uid.clone().into());
            ui.set_profile_loading(true);
            ui.set_profile_is_self(is_self);
            ui.set_profile_is_bot(false);
            ui.set_profile_avatar_initial(known_name.chars().next().unwrap_or('?').to_uppercase().to_string().into());
            ui.set_profile_name(known_name.into());
            ui.set_profile_username("".into());
            ui.set_profile_bio("".into());
            ui.set_profile_created(account_created_label(&uid).into());
            ui.set_profile_mutual("".into());
            ui.set_profile_status("".into());
            ui.set_profile_has_avatar(false);
            ui.set_profile_banner(color_from_rgb(fallback_banner_color(&uid)));
            ui.set_profile_roles(std::rc::Rc::new(slint::VecModel::from(Vec::<ProfileRole>::new())).into());
            ui.set_show_profile_popout(true);
        }

        let http = match http_open.lock().unwrap().as_ref().cloned() {
            Some(http) => http,
            None => return,
        };
        let gid = active_guild_open.lock().unwrap().clone();
        let guild = if gid.is_empty() || gid == "@me" { None } else { Some(gid) };
        let app_weak = app_weak.clone();

        tokio::spawn(async move {
            let raw = match http.get_user_full_profile(&uid, guild.as_deref()).await {
                Ok(v) => Ok(v),
                Err(e) => {
                    warn!("Profile request failed ({}), falling back to the basic user object", e);
                    http.get_user_profile(&uid).await.map(|user| serde_json::json!({ "user": user }))
                }
            };
            let parsed = match raw.ok().and_then(|v| parse_profile(&v)) {
                Some(p) => p,
                None => {
                    let app_weak = app_weak.clone();
                    let uid = uid.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_weak.upgrade() {
                            if ui.get_profile_user_id().as_str() == uid {
                                ui.set_profile_loading(false);
                                ui.set_profile_status("Could not load this profile.".into());
                            }
                        }
                    });
                    return;
                }
            };

            let role_rows = match (&guild, parsed.role_ids.is_empty()) {
                (Some(gid), false) => build_role_rows(&parsed, &load_role_table(&http, gid).await),
                _ => Vec::new(),
            };

            let avatar_link = avatar_url(&parsed.id, &parsed.avatar_hash);
            let name = parsed.display_name().to_string();
            let username = if parsed.username.is_empty() { String::new() } else { format!("@{}", parsed.username) };
            let bio: String = parsed.bio.chars().take(MAX_BIO_CHARS).collect();
            let created = account_created_label(&uid);
            let mutual = if parsed.id == crate::gateway::get_my_user_id().to_string() { String::new() } else { mutual_servers_label(parsed.mutual_guilds) };
            let banner = parsed.banner_color.unwrap_or_else(|| fallback_banner_color(&uid));
            let is_bot = parsed.is_bot;

            {
                let app_weak = app_weak.clone();
                let uid = uid.clone();
                let name = name.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_weak.upgrade() {
                        if !ui.get_show_profile_popout() || ui.get_profile_user_id().as_str() != uid {
                            return;
                        }
                        ui.set_profile_loading(false);
                        ui.set_profile_avatar_initial(name.chars().next().unwrap_or('?').to_uppercase().to_string().into());
                        ui.set_profile_name(name.into());
                        ui.set_profile_username(username.into());
                        ui.set_profile_bio(bio.into());
                        ui.set_profile_created(created.into());
                        ui.set_profile_mutual(mutual.into());
                        ui.set_profile_banner(color_from_rgb(banner));
                        ui.set_profile_is_bot(is_bot);
                        let rows: Vec<ProfileRole> = role_rows
                            .into_iter()
                            .map(|(name, color)| ProfileRole {
                                name: name.into(),
                                color: if color == 0 { slint::Color::from_rgb_u8(148, 155, 164) } else { color_from_rgb(color) },
                            })
                            .collect();
                        ui.set_profile_roles(std::rc::Rc::new(slint::VecModel::from(rows)).into());
                    }
                });
            }

            if avatar_link.is_empty() {
                return;
            }
            let bytes = match http.download_image(&avatar_link).await {
                Ok(b) => b,
                Err(_) => return,
            };
            let decoded = tokio::task::spawn_blocking(move || {
                let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
                let (w, h) = (img.width(), img.height());
                Some(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h))
            })
            .await
            .ok()
            .flatten();
            if let Some(buffer) = decoded {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_weak.upgrade() {
                        if ui.get_show_profile_popout() && ui.get_profile_user_id().as_str() == uid {
                            ui.set_profile_avatar_img(slint::Image::from_rgba8(buffer));
                            ui.set_profile_has_avatar(true);
                        }
                    }
                });
            }
        });
    });

    let app_weak_close = app.as_weak();
    app.on_close_user_profile(move || {
        if let Some(ui) = app_weak_close.upgrade() {
            ui.set_show_profile_popout(false);
        }
    });

    // Copy ID
    let app_weak_copy = app.as_weak();
    app.on_profile_copy_id(move || {
        if let Some(ui) = app_weak_copy.upgrade() {
            let id = ui.get_profile_user_id().to_string();
            let copied = !id.is_empty()
                && copypasta::ClipboardContext::new()
                    .and_then(|mut ctx| copypasta::ClipboardProvider::set_contents(&mut ctx, id))
                    .is_ok();
            ui.set_profile_status(if copied { "User ID copied." } else { "Could not copy the ID." }.into());
        }
    });

    // Message: open (or create) the DM and show it.
    let app_weak_msg = app.as_weak();
    let http_msg = Arc::clone(&http_client);
    let guilds_map_msg = Arc::clone(&guilds_map);
    let active_guild_msg = Arc::clone(&active_guild_id);
    let active_channel_msg = Arc::clone(&active_channel_id);
    app.on_profile_message(move || {
        let ui = match app_weak_msg.upgrade() {
            Some(ui) => ui,
            None => return,
        };
        let uid = ui.get_profile_user_id().to_string();
        if uid.is_empty() || ui.get_profile_is_self() {
            return;
        }
        let http = match http_msg.lock().unwrap().as_ref().cloned() {
            Some(http) => http,
            None => return,
        };
        ui.set_profile_status("Opening conversation…".into());

        let app_weak = app_weak_msg.clone();
        let guilds_map = Arc::clone(&guilds_map_msg);
        let active_guild = Arc::clone(&active_guild_msg);
        let active_channel = Arc::clone(&active_channel_msg);
        tokio::spawn(async move {
            match http.create_dm(&uid).await {
                Ok(channel_id) => {
                    info!("DM channel {} ready for user {}", channel_id, uid);
                    // The DM list picks the active channel when it is in the list.
                    *active_channel.lock().unwrap() = channel_id.clone();
                    fetch_and_populate_dms(&http, app_weak.clone(), Arc::clone(&guilds_map), active_guild, active_channel).await;
                    let listed = guilds_map
                        .lock()
                        .unwrap()
                        .get("@me")
                        .map(|g| g.channels.iter().any(|c| c.id == channel_id))
                        .unwrap_or(false);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_weak.upgrade() {
                            ui.set_show_profile_popout(false);
                            if !listed {
                                ui.invoke_select_channel(channel_id.into());
                            }
                        }
                    });
                }
                Err(e) => {
                    warn!("Could not open DM with {}: {}", uid, e);
                    set_status(&app_weak, &uid, "Could not open the conversation.".to_string());
                }
            }
        });
    });

    // Block: asks for a second click before changing the relationship.
    let app_weak_block = app.as_weak();
    let http_block = Arc::clone(&http_client);
    app.on_profile_block(move || {
        let ui = match app_weak_block.upgrade() {
            Some(ui) => ui,
            None => return,
        };
        let uid = ui.get_profile_user_id().to_string();
        if uid.is_empty() || ui.get_profile_is_self() {
            return;
        }
        {
            let mut pending = pending_block.lock().unwrap();
            if *pending != uid {
                *pending = uid;
                ui.set_profile_status("Click Block again to confirm.".into());
                return;
            }
            pending.clear();
        }
        let http = match http_block.lock().unwrap().as_ref().cloned() {
            Some(http) => http,
            None => return,
        };
        let name = ui.get_profile_name().to_string();
        let app_weak = app_weak_block.clone();
        tokio::spawn(async move {
            match http.block_user(&uid).await {
                Ok(()) => set_status(&app_weak, &uid, format!("Blocked {}.", name)),
                Err(e) => {
                    warn!("Could not block {}: {}", uid, e);
                    set_status(&app_weak, &uid, "Could not block this user.".to_string());
                }
            }
        });
    });
}
