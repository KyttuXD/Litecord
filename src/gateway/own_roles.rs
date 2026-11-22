//! The user's own role ids per server, kept so role mentions can be recognised.
//!
//! Filled from READY (`merged_members`), GUILD_CREATE (`members` / `member`) and the
//! user's own GUILD_MEMBER_UPDATE events.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static ROLES: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();

fn store() -> std::sync::MutexGuard<'static, HashMap<String, Vec<String>>> {
    ROLES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The role ids the user holds in a server (empty when unknown).
pub fn get(guild_id: &str) -> Vec<String> {
    store().get(guild_id).cloned().unwrap_or_default()
}

fn role_ids(member: &Value) -> Vec<String> {
    member["roles"]
        .as_array()
        .map(|l| l.iter().filter_map(|r| r.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn is_me(member: &Value, my_id: &str) -> bool {
    member["user_id"].as_str() == Some(my_id) || member["user"]["id"].as_str() == Some(my_id)
}

/// Replaces everything with what a READY payload (`d`) holds.
pub fn apply_ready(d: &Value, my_id: &str) {
    let mut map = HashMap::new();
    if !my_id.is_empty() {
        if let (Some(guilds), Some(merged)) = (d["guilds"].as_array(), d["merged_members"].as_array()) {
            for (g, members) in guilds.iter().zip(merged) {
                let gid = g["id"].as_str().unwrap_or("");
                let mine = members.as_array().and_then(|l| l.iter().find(|m| is_me(m, my_id)));
                if let (false, Some(m)) = (gid.is_empty(), mine) {
                    map.insert(gid.to_string(), role_ids(m));
                }
            }
        }
    }
    *store() = map;
}

/// Records the user's roles from a GUILD_CREATE payload (`d`).
pub fn apply_guild_create(d: &Value, my_id: &str) {
    let gid = d["id"].as_str().unwrap_or("");
    if gid.is_empty() || my_id.is_empty() {
        return;
    }
    let mine = if d["member"].is_object() && (is_me(&d["member"], my_id) || d["member"]["user"].is_null()) {
        Some(&d["member"])
    } else {
        d["members"].as_array().and_then(|l| l.iter().find(|m| is_me(m, my_id)))
    };
    if let Some(m) = mine {
        store().insert(gid.to_string(), role_ids(m));
    }
}

/// Records the roles of a GUILD_MEMBER_UPDATE message (`text`) when it is about the user.
pub fn apply_member_update(text: &str, guild_id: &str, my_user_id: u64) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    if v["t"].as_str() != Some("GUILD_MEMBER_UPDATE") || v["d"]["user"]["id"].as_str().and_then(|s| s.parse().ok()) != Some(my_user_id) {
        return;
    }
    if v["d"]["roles"].is_array() {
        store().insert(guild_id.to_string(), role_ids(&v["d"]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_own_roles_from_each_source() {
        apply_ready(
            &json!({"guilds":[{"id":"g1"},{"id":"g2"}],"merged_members":[[{"user_id":"7","roles":["a","b"]}],[{"user_id":"8","roles":["z"]}]]}),
            "7",
        );
        assert_eq!(get("g1"), vec!["a", "b"]);
        assert!(get("g2").is_empty());
        apply_guild_create(&json!({"id":"g3","members":[{"user":{"id":"7"},"roles":["c"]}]}), "7");
        assert_eq!(get("g3"), vec!["c"]);
        let upd = r#"{"t":"GUILD_MEMBER_UPDATE","d":{"guild_id":"g1","roles":["d"],"user":{"id":"7"}}}"#;
        apply_member_update(upd, "g1", 7);
        assert_eq!(get("g1"), vec!["d"]);
    }
}
