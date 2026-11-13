//! Direct messages and relationships (friends / blocks): pure path and payload builders,
//! READY relationship parsing and a channel -> recipient registry.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const API_BASE: &str = "https://discord.com/api/v9";

pub const REL_FRIEND: u8 = 1;
pub const REL_BLOCKED: u8 = 2;
pub const REL_INCOMING: u8 = 3;
pub const REL_OUTGOING: u8 = 4;

/// POST target that opens (or returns) a DM with a user.
pub fn create_dm_path() -> String {
    format!("{}/users/@me/channels", API_BASE)
}

pub fn create_dm_payload(recipient_id: &str) -> Value {
    json!({ "recipient_id": recipient_id })
}

/// DELETE target that closes a DM channel.
pub fn close_dm_path(channel_id: &str) -> String {
    format!("{}/channels/{}", API_BASE, channel_id)
}

/// PUT/DELETE target for a relationship with a user.
pub fn relationship_path(user_id: &str) -> String {
    format!("{}/users/@me/relationships/{}", API_BASE, user_id)
}

pub fn block_payload() -> Value {
    json!({ "type": REL_BLOCKED })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipEntry {
    pub user_id: String,
    pub name: String,
    pub kind: u8,
}

/// Parses the `relationships` array of the READY payload. Entries without an id are skipped.
pub fn parse_relationships(ready: &Value) -> Vec<RelationshipEntry> {
    let mut out = Vec::new();
    if let Some(arr) = ready["relationships"].as_array() {
        for r in arr {
            let user_id = r["id"].as_str().or_else(|| r["user"]["id"].as_str()).unwrap_or("");
            if user_id.is_empty() {
                continue;
            }
            let user = &r["user"];
            let name = user["global_name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| user["username"].as_str())
                .unwrap_or("Unknown")
                .to_string();
            let kind = r["type"].as_u64().unwrap_or(0) as u8;
            out.push(RelationshipEntry { user_id: user_id.to_string(), name, kind });
        }
    }
    out
}

/// Friends (type 1) sorted by name, case-insensitive.
pub fn friends_sorted(entries: &[RelationshipEntry]) -> Vec<RelationshipEntry> {
    let mut v: Vec<RelationshipEntry> = entries.iter().filter(|e| e.kind == REL_FRIEND).cloned().collect();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

static DM_RECIPIENTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn dm_map() -> &'static Mutex<HashMap<String, String>> {
    DM_RECIPIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remembers who the other side of a 1:1 DM channel is.
pub fn register_dm_recipient(channel_id: &str, user_id: &str) {
    if channel_id.is_empty() || user_id.is_empty() {
        return;
    }
    if let Ok(mut m) = dm_map().lock() {
        m.insert(channel_id.to_string(), user_id.to_string());
    }
}

/// Forgets a closed DM.
pub fn unregister_dm_recipient(channel_id: &str) {
    if let Ok(mut m) = dm_map().lock() {
        m.remove(channel_id);
    }
}

pub fn dm_recipient(channel_id: &str) -> Option<String> {
    dm_map().lock().ok().and_then(|m| m.get(channel_id).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_paths_and_payloads() {
        assert_eq!(create_dm_path(), "https://discord.com/api/v9/users/@me/channels");
        assert_eq!(create_dm_payload("42"), json!({ "recipient_id": "42" }));
        assert_eq!(close_dm_path("7"), "https://discord.com/api/v9/channels/7");
        assert_eq!(relationship_path("9"), "https://discord.com/api/v9/users/@me/relationships/9");
        assert_eq!(block_payload(), json!({ "type": 2 }));
    }

    #[test]
    fn parses_and_sorts_relationships() {
        let ready = json!({ "relationships": [
            { "id": "1", "type": 1, "user": { "id": "1", "username": "zed" } },
            { "id": "2", "type": 2, "user": { "id": "2", "username": "bad" } },
            { "id": "3", "type": 1, "user": { "id": "3", "username": "x", "global_name": "Alice" } },
            { "type": 1, "user": {} }
        ]});
        let all = parse_relationships(&ready);
        assert_eq!(all.len(), 3);
        let f = friends_sorted(&all);
        assert_eq!(f.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["Alice", "zed"]);
        assert!(parse_relationships(&json!({})).is_empty());
    }

    #[test]
    fn registry_roundtrip() {
        register_dm_recipient("c1", "u1");
        assert_eq!(dm_recipient("c1"), Some("u1".to_string()));
        assert_eq!(dm_recipient("nope"), None);
    }
}
