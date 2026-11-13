//! Live channel changes (CHANNEL_CREATE / CHANNEL_UPDATE / CHANNEL_DELETE) applied to the
//! channel list of a server. The list keeps the sidebar layout: channels without a
//! category first, then each category followed by its channels (text before voice).

use super::types::ChannelData;
use serde_json::Value;

/// A server channel as the sidebar shows it, or None for types it does not list (DMs, threads...).
pub fn parse_channel(v: &Value) -> Option<ChannelData> {
    let id = v["id"].as_str().filter(|s| !s.is_empty())?;
    let ch_type = v["type"].as_u64()?;
    if !matches!(ch_type, 0 | 2 | 4 | 5 | 13 | 15) {
        return None;
    }
    let name = v["name"].as_str().unwrap_or("canal");
    let topic = v["topic"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    Some(ChannelData {
        id: id.to_string(),
        name: if ch_type == 4 { name.to_uppercase() } else { name.to_string() },
        is_voice: ch_type == 2 || ch_type == 13,
        is_category: ch_type == 4,
        parent_id: if ch_type == 4 { None } else { v["parent_id"].as_str().map(str::to_string) },
        position: v["position"].as_i64().unwrap_or(0),
        topic,
    })
}

/// Index range where the children of `parent` live: after the category, up to the next one;
/// without a parent, from the start up to the first category.
fn region(channels: &[ChannelData], parent: Option<&str>) -> (usize, usize) {
    match parent {
        Some(pid) => {
            let start = channels.iter().position(|c| c.is_category && c.id == pid).map(|i| i + 1).unwrap_or(0);
            let end = channels[start..].iter().position(|c| c.is_category).map(|i| start + i).unwrap_or(channels.len());
            (start, end)
        }
        None => (0, channels.iter().position(|c| c.is_category).unwrap_or(channels.len())),
    }
}

fn insert_child(channels: &mut Vec<ChannelData>, mut ch: ChannelData) {
    let parent = ch.parent_id.clone().filter(|pid| channels.iter().any(|c| c.is_category && &c.id == pid));
    // A channel whose category is not listed (empty categories are not) shows uncategorized.
    ch.parent_id = parent.clone();
    let (start, end) = region(channels, parent.as_deref());
    let key = (ch.is_voice, ch.position);
    let at = channels[start..end]
        .iter()
        .position(|c| (c.is_voice, c.position) > key)
        .map(|i| start + i)
        .unwrap_or(end);
    channels.insert(at, ch);
}

/// Adds a created channel. Returns false when the list is unchanged (already known, or a
/// category: categories only show once a channel needs them, as in the server load).
pub fn apply_channel_create(channels: &mut Vec<ChannelData>, ch: ChannelData) -> bool {
    if ch.is_category || channels.iter().any(|c| c.id == ch.id) {
        return false;
    }
    insert_child(channels, ch);
    true
}

/// Applies a changed channel (name, topic, position, category). Returns false when it is not listed.
pub fn apply_channel_update(channels: &mut Vec<ChannelData>, ch: ChannelData) -> bool {
    let Some(idx) = channels.iter().position(|c| c.id == ch.id) else {
        return false;
    };
    if ch.is_category {
        let old = &mut channels[idx];
        old.name = ch.name;
        old.position = ch.position;
        return true;
    }
    let moved = {
        let old = &channels[idx];
        old.position != ch.position || old.parent_id != ch.parent_id || old.is_voice != ch.is_voice
    };
    if moved {
        channels.remove(idx);
        insert_child(channels, ch);
    } else {
        let old = &mut channels[idx];
        old.name = ch.name;
        old.topic = ch.topic;
    }
    true
}

/// Removes a deleted channel. The channels of a deleted category become uncategorized.
/// Returns false when it is not listed.
pub fn apply_channel_delete(channels: &mut Vec<ChannelData>, channel_id: &str) -> bool {
    let Some(idx) = channels.iter().position(|c| c.id == channel_id) else {
        return false;
    };
    let removed = channels.remove(idx);
    if removed.is_category {
        let orphans: Vec<ChannelData> = channels
            .iter()
            .filter(|c| c.parent_id.as_deref() == Some(channel_id))
            .cloned()
            .collect();
        channels.retain(|c| c.parent_id.as_deref() != Some(channel_id));
        for mut orphan in orphans {
            orphan.parent_id = None;
            insert_child(channels, orphan);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ch(id: &str, voice: bool, parent: Option<&str>, position: i64) -> ChannelData {
        ChannelData {
            id: id.into(),
            name: id.into(),
            is_voice: voice,
            is_category: false,
            parent_id: parent.map(str::to_string),
            position,
            topic: None,
        }
    }

    fn cat(id: &str) -> ChannelData {
        ChannelData { is_category: true, ..ch(id, false, None, 0) }
    }

    fn ids(list: &[ChannelData]) -> Vec<&str> {
        list.iter().map(|c| c.id.as_str()).collect()
    }

    fn sample() -> Vec<ChannelData> {
        vec![ch("u1", false, None, 0), cat("c1"), ch("a", false, Some("c1"), 0), ch("v", true, Some("c1"), 1), cat("c2"), ch("b", false, Some("c2"), 0)]
    }

    #[test]
    fn parses_listed_types_only() {
        let text = parse_channel(&json!({"id":"1","type":0,"name":"general","topic":"  hello ","parent_id":"9","position":3})).unwrap();
        assert_eq!((text.name.as_str(), text.topic.as_deref(), text.parent_id.as_deref(), text.position), ("general", Some("hello"), Some("9"), 3));
        let category = parse_channel(&json!({"id":"2","type":4,"name":"Info","parent_id":"9"})).unwrap();
        assert!(category.is_category && category.name == "INFO" && category.parent_id.is_none());
        assert!(parse_channel(&json!({"id":"3","type":2,"name":"v"})).unwrap().is_voice);
        assert!(parse_channel(&json!({"id":"4","type":1})).is_none());
        assert!(parse_channel(&json!({"id":"5","type":11,"name":"thread"})).is_none());
        assert!(parse_channel(&json!({"type":0})).is_none());
        assert_eq!(parse_channel(&json!({"id":"6","type":0,"topic":""})).unwrap().topic, None);
    }

    #[test]
    fn create_goes_to_its_category_in_order() {
        let mut list = sample();
        assert!(apply_channel_create(&mut list, ch("a2", false, Some("c1"), 5)));
        assert_eq!(ids(&list), ["u1", "c1", "a", "a2", "v", "c2", "b"]);
        assert!(apply_channel_create(&mut list, ch("v2", true, Some("c2"), 0)));
        assert_eq!(ids(&list), ["u1", "c1", "a", "a2", "v", "c2", "b", "v2"]);
        assert!(apply_channel_create(&mut list, ch("u0", false, None, 0)));
        assert!(apply_channel_create(&mut list, ch("uv", true, None, 0)));
        assert_eq!(ids(&list), ["u1", "u0", "uv", "c1", "a", "a2", "v", "c2", "b", "v2"]);
    }

    #[test]
    fn create_ignores_known_channels_and_categories_and_unlisted_parents() {
        let mut list = sample();
        assert!(!apply_channel_create(&mut list, ch("a", false, Some("c1"), 0)));
        assert!(!apply_channel_create(&mut list, cat("c3")));
        assert_eq!(list.len(), 6);
        assert!(apply_channel_create(&mut list, ch("x", false, Some("gone"), 0)));
        assert_eq!(list[1].id, "x");
        assert_eq!(list[1].parent_id, None);
    }

    #[test]
    fn update_changes_name_and_topic_in_place() {
        let mut list = sample();
        let mut changed = ch("a", false, Some("c1"), 0);
        changed.name = "renamed".into();
        changed.topic = Some("new topic".into());
        assert!(apply_channel_update(&mut list, changed));
        assert_eq!(ids(&list), ["u1", "c1", "a", "v", "c2", "b"]);
        assert_eq!((list[2].name.as_str(), list[2].topic.as_deref()), ("renamed", Some("new topic")));
        assert!(!apply_channel_update(&mut list, ch("nope", false, None, 0)));
    }

    #[test]
    fn update_moves_between_categories_and_positions() {
        let mut list = sample();
        assert!(apply_channel_update(&mut list, ch("a", false, Some("c2"), 7)));
        assert_eq!(ids(&list), ["u1", "c1", "v", "c2", "b", "a"]);
        assert!(apply_channel_update(&mut list, ch("b", false, Some("c2"), 9)));
        assert_eq!(ids(&list), ["u1", "c1", "v", "c2", "a", "b"]);
        assert!(apply_channel_update(&mut list, ch("a", false, None, 0)));
        assert_eq!(ids(&list), ["u1", "a", "c1", "v", "c2", "b"]);
    }

    #[test]
    fn update_renames_a_category() {
        let mut list = sample();
        let mut renamed = cat("c1");
        renamed.name = "NEW".into();
        assert!(apply_channel_update(&mut list, renamed));
        assert_eq!(list[1].name, "NEW");
        assert_eq!(ids(&list), ["u1", "c1", "a", "v", "c2", "b"]);
    }

    #[test]
    fn delete_removes_and_frees_children_of_a_category() {
        let mut list = sample();
        assert!(apply_channel_delete(&mut list, "v"));
        assert_eq!(ids(&list), ["u1", "c1", "a", "c2", "b"]);
        assert!(!apply_channel_delete(&mut list, "v"));
        assert!(apply_channel_delete(&mut list, "c1"));
        assert_eq!(ids(&list), ["u1", "a", "c2", "b"]);
        assert_eq!(list[1].parent_id, None);
    }
}
