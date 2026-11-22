//! Right-hand member list of the open server: subscribes to the gateway's member list
//! and turns it into the rows the panel shows.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use slint::{ModelRc, VecModel};

use crate::{AppWindow, MemberRowItem};
use crate::gateway::{GatewayCommand, member_list::{self, ListRow, Presence}};
use crate::http::DiscordHttpClient;
use crate::utils::avatar_cache;

fn status_code(presence: Presence, group: Option<&str>) -> i32 {
    match presence {
        Presence::Online => 0,
        Presence::Idle => 1,
        Presence::Dnd => 2,
        // Only the offline group lists offline members; elsewhere a missing presence
        // just means the server did not send one.
        Presence::Offline if group != Some("offline") => 0,
        Presence::Offline => 3,
    }
}

/// Rows of the member list of a server, as kept by the gateway.
pub fn build_rows(guild_id: &str) -> Vec<MemberRowItem> {
    let avatars = avatar_cache::get_avatar_cache();
    member_list::with_roles(guild_id, |roles| {
        member_list::with_list(guild_id, |list| {
            list.rows()
                .into_iter()
                .map(|row| match row {
                    ListRow::Group { id, count } => MemberRowItem {
                        is_group: true,
                        text: format!("{} - {}", member_list::group_title(roles, id).to_uppercase(), count).into(),
                        ..Default::default()
                    },
                    ListRow::Member { group, member } => {
                        let avatar_url = member_list::avatar_url(member);
                        let color = member_list::member_color(roles, member);
                        MemberRowItem {
                            is_group: false,
                            text: member.name.as_str().into(),
                            avatar_text: member.name.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default().into(),
                            avatar_slot: avatars.slot_for(&avatar_url),
                            avatar_url: avatar_url.into(),
                            status: status_code(member.presence, group),
                            is_bot: member.bot,
                            has_color: color.is_some(),
                            name_color: color
                                .map(|c| slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8))
                                .unwrap_or_default(),
                        }
                    }
                })
                .collect()
        })
    })
}

/// Shows the current member list of `guild_id`, unless another server is open by now.
pub fn refresh(app_weak: slint::Weak<AppWindow>, guild_id: &str) {
    let rows = build_rows(guild_id);
    let guild_id = guild_id.to_string();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_weak.upgrade() {
            if ui.get_active_guild_id().as_str() == guild_id {
                ui.set_member_rows(ModelRc::new(VecModel::from(rows)));
            }
        }
    });
}

/// Starts the member list of a server for the text channel the user opened: asks the
/// gateway for it and loads the roles that give the names their colour.
pub fn request(
    cmd_tx_store: &Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
    app_weak: &slint::Weak<AppWindow>,
    http: &DiscordHttpClient,
    guild_id: &str,
    channel_id: &str,
) {
    if guild_id.is_empty() || guild_id == "@me" || channel_id.is_empty() {
        return;
    }
    refresh(app_weak.clone(), guild_id);

    if let Some(tx) = cmd_tx_store.lock().unwrap().as_ref() {
        let _ = tx.try_send(GatewayCommand::SubscribeMemberList {
            guild_id: guild_id.to_string(),
            channel_id: channel_id.to_string(),
        });
    }

    if !member_list::has_roles(guild_id) {
        let (http, app_weak, guild_id) = (http.clone(), app_weak.clone(), guild_id.to_string());
        tokio::spawn(async move {
            if let Ok(roles) = http.get_guild_roles(&guild_id).await {
                member_list::set_roles(&guild_id, member_list::parse_roles(&roles));
                refresh(app_weak, &guild_id);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_without_status_outside_the_offline_group_counts_as_online() {
        assert_eq!(status_code(Presence::Offline, Some("offline")), 3);
        assert_eq!(status_code(Presence::Offline, Some("online")), 0);
        assert_eq!(status_code(Presence::Offline, None), 0);
        assert_eq!(status_code(Presence::Dnd, Some("offline")), 2);
        assert_eq!(status_code(Presence::Idle, None), 1);
    }
}
