//! Glue between the friends store (`relationships`), the REST calls and the Friends view.

use log::info;

use crate::http::DiscordHttpClient;
use crate::ui::helpers::get_guild_initials;
use crate::ui::relationships::{self as rel, FriendsTab};
use crate::{AppWindow, FriendItem};

/// Refills the list of the selected tab and the pending badge from the store.
pub fn refresh_friends_ui(ui: &AppWindow) {
    let (rels, online) = rel::snapshot();
    let tab = FriendsTab::from_index(ui.get_friends_tab());
    let items: Vec<FriendItem> = rel::tab_items(&rels, &online, tab)
        .into_iter()
        .map(|r| {
            let is_online = online.contains(&r.id);
            FriendItem {
                id: r.id.to_string().into(),
                initials: get_guild_initials(&r.name).into(),
                status: rel::status_label(r.kind, is_online).into(),
                kind: i32::from(r.kind),
                online: is_online,
                name: r.name.into(),
            }
        })
        .collect();
    ui.set_friends_items(slint::ModelRc::new(slint::VecModel::from(items)));
    ui.set_friends_pending_count(rel::incoming_count(&rels) as i32);
}

fn show_status(app_weak: &slint::Weak<AppWindow>, text: String, is_error: bool, clear_input: bool) {
    let app_w = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_w.upgrade() {
            ui.set_friends_status(text.into());
            ui.set_friends_status_is_error(is_error);
            if clear_input {
                ui.set_friends_add_text("".into());
            }
            refresh_friends_ui(&ui);
        }
    });
}

/// Applies the answer of a successful request to the store without waiting for the
/// RELATIONSHIP_* event, which does the same later (both are idempotent).
fn apply_local(user_id: u64, action: &str) {
    if action == "accept" {
        let (rels, _) = rel::snapshot();
        if let Some(r) = rels.into_iter().find(|r| r.id == user_id) {
            rel::upsert(rel::Relationship { kind: rel::REL_FRIEND, ..r });
        }
    } else {
        rel::remove(user_id);
    }
}

/// Accept / ignore / cancel / remove / unblock for one row of the Friends view.
pub async fn run_friend_action(http: &DiscordHttpClient, app_weak: slint::Weak<AppWindow>, user_id: &str, action: &str) {
    let Ok(id) = user_id.parse::<u64>() else { return };
    let result = match action {
        "accept" => http.put_relationship(user_id).await,
        "ignore" | "cancel" | "remove" | "unblock" => http.delete_relationship(user_id).await,
        _ => return,
    };
    match result {
        Ok(()) => {
            info!("Friends: {} {}", action, user_id);
            apply_local(id, action);
            show_status(&app_weak, String::new(), false, false);
        }
        Err(e) => show_status(&app_weak, e, true, false),
    }
}

/// Sends a friend request to the username the user typed.
pub async fn run_add_friend(http: &DiscordHttpClient, app_weak: slint::Weak<AppWindow>, input: &str) {
    let Some((name, discriminator)) = rel::parse_friend_input(input) else { return };
    match http.add_friend(&name, discriminator.as_deref()).await {
        Ok(()) => {
            info!("Friends: request sent to {}", name);
            show_status(&app_weak, format!("Success! Your friend request to {} was sent.", name), false, true);
        }
        Err(e) => show_status(&app_weak, e, true, false),
    }
}
