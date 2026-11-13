//! Jump-to-reply and message search helpers.
//!
//! Everything here is pure (no Slint types) so it can be unit tested on its own.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const API_BASE: &str = "https://discord.com/api/v10";
const MAX_REMEMBERED_REPLIES: usize = 20000;

/// Where a search runs: a whole guild or a single channel (DMs).
#[derive(Clone, Debug, PartialEq)]
pub enum SearchScope {
    Guild(String),
    Channel(String),
}

/// One row of the search results popup.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub id: String,
    pub channel_id: String,
    pub author: String,
    pub content: String,
}

/// Percent-encodes a string for use as a URL query value.
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.as_bytes() {
        let c = *b;
        if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.' || c == b'~' {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{:02X}", c));
        }
    }
    out
}

/// `GET /channels/{id}/messages?around=<id>&limit=<n>`.
pub fn messages_around_url(channel_id: &str, around_id: &str, limit: u32) -> String {
    format!(
        "{}/channels/{}/messages?around={}&limit={}",
        API_BASE,
        channel_id,
        percent_encode(around_id),
        limit
    )
}

/// `GET /guilds/{id}/messages/search?content=...` or the channel variant.
pub fn search_url(scope: &SearchScope, content: &str) -> String {
    let query = percent_encode(content.trim());
    match scope {
        SearchScope::Guild(id) => format!("{}/guilds/{}/messages/search?content={}", API_BASE, id, query),
        SearchScope::Channel(id) => format!("{}/channels/{}/messages/search?content={}", API_BASE, id, query),
    }
}

/// Position of `target` in the list of loaded message ids, if present.
pub fn find_message_index(ids: &[String], target: &str) -> Option<usize> {
    if target.is_empty() {
        return None;
    }
    ids.iter().position(|id| id == target)
}

fn reply_refs() -> &'static Mutex<HashMap<String, String>> {
    static REFS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    REFS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Extracts the id of the message a reply points to (type 19 messages).
pub fn reply_target_id(m: &Value) -> Option<String> {
    let id = m["message_reference"]["message_id"].as_str()?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Remembers `message id -> referenced message id` so a click on the reply
/// preview can find its target without extending the UI row struct.
pub fn remember_reply_ref(m: &Value) {
    let msg_id = match m["id"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return,
    };
    if let Some(target) = reply_target_id(m) {
        if let Ok(mut map) = reply_refs().lock() {
            if map.len() >= MAX_REMEMBERED_REPLIES {
                map.clear();
            }
            map.insert(msg_id.to_string(), target);
        }
    }
}

/// Looks up the message that `message_id` replies to.
pub fn lookup_reply_ref(message_id: &str) -> Option<String> {
    reply_refs().lock().ok().and_then(|m| m.get(message_id).cloned())
}

/// Parses a search response (`{"messages": [[msg, ...context], ...]}`) into hits.
/// Each inner array holds the hit (flagged `"hit": true`) plus surrounding context.
pub fn parse_search_results(body: &Value) -> Vec<SearchHit> {
    let groups = match body["messages"].as_array() {
        Some(g) => g,
        None => return Vec::new(),
    };
    let mut hits = Vec::new();
    for group in groups {
        let items = match group.as_array() {
            Some(i) => i,
            None => continue,
        };
        let chosen = items
            .iter()
            .find(|m| m["hit"].as_bool().unwrap_or(false))
            .or_else(|| items.first());
        if let Some(m) = chosen {
            let id = m["id"].as_str().unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let author = m["author"]["global_name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| m["author"]["username"].as_str())
                .unwrap_or("")
                .to_string();
            hits.push(SearchHit {
                id: id.to_string(),
                channel_id: m["channel_id"].as_str().unwrap_or("").to_string(),
                author,
                content: truncate_preview(m["content"].as_str().unwrap_or(""), 120),
            });
        }
    }
    hits
}

/// Collapses whitespace and limits to `max` characters.
pub fn truncate_preview(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<&str>>().join(" ");
    if flat.chars().count() > max {
        let cut: String = flat.chars().take(max.saturating_sub(3)).collect();
        format!("{}...", cut)
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn around_url() {
        assert_eq!(
            messages_around_url("10", "20", 50),
            "https://discord.com/api/v10/channels/10/messages?around=20&limit=50"
        );
    }

    #[test]
    fn search_urls_encode_query() {
        let g = search_url(&SearchScope::Guild("1".into()), " ola mundo&x=1 ");
        assert_eq!(g, "https://discord.com/api/v10/guilds/1/messages/search?content=ola%20mundo%26x%3D1");
        let c = search_url(&SearchScope::Channel("2".into()), "a");
        assert_eq!(c, "https://discord.com/api/v10/channels/2/messages/search?content=a");
        assert_eq!(percent_encode("\u{e9}"), "%C3%A9");
    }

    #[test]
    fn index_lookup() {
        let ids = vec!["1".to_string(), "2".to_string()];
        assert_eq!(find_message_index(&ids, "2"), Some(1));
        assert_eq!(find_message_index(&ids, "9"), None);
        assert_eq!(find_message_index(&ids, ""), None);
    }

    #[test]
    fn reply_refs_roundtrip() {
        let m = json!({"id": "m-test-1", "message_reference": {"message_id": "t-1"}});
        remember_reply_ref(&m);
        assert_eq!(lookup_reply_ref("m-test-1"), Some("t-1".to_string()));
        assert_eq!(lookup_reply_ref("nope"), None);
        assert_eq!(reply_target_id(&json!({"id": "x"})), None);
    }

    #[test]
    fn parses_hits() {
        let body = json!({"messages": [[
            {"id": "5", "content": "ctx"},
            {"id": "6", "hit": true, "channel_id": "c", "content": "a  b\nc",
             "author": {"username": "u", "global_name": "G"}}
        ]]});
        let hits = parse_search_results(&body);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "6");
        assert_eq!(hits[0].author, "G");
        assert_eq!(hits[0].content, "a b c");
        assert!(parse_search_results(&json!({})).is_empty());
    }

    #[test]
    fn truncates() {
        assert_eq!(truncate_preview("abcdef", 5), "ab...");
        assert_eq!(truncate_preview("abc", 5), "abc");
    }
}
