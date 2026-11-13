//! Unread and mention state per channel, plus the user's mute settings.
//!
//! Fed by READY (`read_state`, `user_guild_settings`), MESSAGE_CREATE, MESSAGE_ACK and
//! USER_GUILD_SETTINGS_UPDATE; cleared when a channel is opened (see `mark_read`). The
//! state itself is plain data with pure methods so it can be tested; the free functions
//! at the bottom wrap one process-wide instance.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

/// Guild id used for direct messages.
pub const DM_GUILD: &str = "@me";

#[derive(Debug, Default, Clone, PartialEq)]
struct ChannelRead {
    guild_id: String,
    unread: i32,
    mentions: i32,
    last_message_id: String,
    last_read_id: String,
}

/// What to show for a channel or a server icon.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    /// Unread messages (always 0 for muted channels and servers).
    pub unread: i32,
    /// Messages that mention the user (shown even when muted).
    pub mentions: i32,
    pub muted: bool,
}

#[derive(Debug, Default)]
pub struct UnreadState {
    channels: HashMap<String, ChannelRead>,
    muted_channels: HashSet<String>,
    muted_guilds: HashSet<String>,
}

/// True when snowflake `a` is newer than `b`. Compared as numbers; ids that do not parse
/// fall back to length, then text order.
pub fn snowflake_gt(a: &str, b: &str) -> bool {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x > y,
        _ => (a.len(), a) > (b.len(), b),
    }
}

/// The items of a READY list: either a plain array or `{"entries": [...]}`.
fn entries(v: &Value) -> Vec<&Value> {
    if let Some(arr) = v.as_array() {
        arr.iter().collect()
    } else if let Some(arr) = v["entries"].as_array() {
        arr.iter().collect()
    } else {
        Vec::new()
    }
}

fn id_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn count_of(v: &Value) -> i32 {
    v.as_i64().unwrap_or(0).clamp(0, i32::MAX as i64) as i32
}

impl UnreadState {
    pub fn clear(&mut self) {
        self.channels.clear();
        self.muted_channels.clear();
        self.muted_guilds.clear();
    }

    pub fn register_channels(&mut self, guild_id: &str, channel_ids: &[&str]) {
        for id in channel_ids {
            let ch = self.channels.entry(id.to_string()).or_default();
            ch.guild_id = guild_id.to_string();
        }
    }

    /// Replaces the state with what a READY payload (`d`) holds.
    pub fn apply_ready(&mut self, d: &Value) {
        self.clear();
        let mut known: Vec<(&str, &Value)> = Vec::new();
        if let Some(guilds) = d["guilds"].as_array() {
            for g in guilds {
                let gid = g["id"].as_str().unwrap_or("");
                if let Some(chans) = g["channels"].as_array() {
                    for c in chans {
                        known.push((gid, c));
                    }
                }
            }
        }
        if let Some(dms) = d["private_channels"].as_array() {
            for c in dms {
                known.push((DM_GUILD, c));
            }
        }
        for (gid, c) in known {
            let cid = id_of(&c["id"]);
            if cid.is_empty() {
                continue;
            }
            let ch = self.channels.entry(cid).or_default();
            ch.guild_id = gid.to_string();
            ch.last_message_id = id_of(&c["last_message_id"]);
        }
        for e in entries(&d["read_state"]) {
            let cid = id_of(&e["id"]);
            if cid.is_empty() {
                continue;
            }
            let ch = self.channels.entry(cid).or_default();
            ch.last_read_id = id_of(&e["last_message_id"]);
            ch.mentions = count_of(&e["mention_count"]);
            let newer = !ch.last_message_id.is_empty()
                && !ch.last_read_id.is_empty()
                && snowflake_gt(&ch.last_message_id, &ch.last_read_id);
            ch.unread = if ch.mentions > 0 || newer { ch.mentions.max(1) } else { 0 };
        }
        for e in entries(&d["user_guild_settings"]) {
            self.apply_settings_entry(e);
        }
    }

    /// One `user_guild_settings` entry (also the payload of USER_GUILD_SETTINGS_UPDATE).
    pub fn apply_settings_entry(&mut self, e: &Value) {
        let mut gid = id_of(&e["guild_id"]);
        if gid.is_empty() {
            gid = DM_GUILD.to_string();
        }
        if e["muted"].as_bool().unwrap_or(false) {
            self.muted_guilds.insert(gid);
        } else {
            self.muted_guilds.remove(&gid);
        }
        if let Some(overrides) = e["channel_overrides"].as_array() {
            for o in overrides {
                let cid = id_of(&o["channel_id"]);
                if cid.is_empty() {
                    continue;
                }
                if o["muted"].as_bool().unwrap_or(false) {
                    self.muted_channels.insert(cid);
                } else {
                    self.muted_channels.remove(&cid);
                }
            }
        }
    }

    /// Counts a new message. Returns false when it was already counted (a replayed event).
    /// Own messages count as read.
    pub fn on_message(&mut self, guild_id: &str, channel_id: &str, message_id: &str, mentions_me: bool, is_self: bool) -> bool {
        let ch = self.channels.entry(channel_id.to_string()).or_default();
        if !guild_id.is_empty() {
            ch.guild_id = guild_id.to_string();
        }
        if !message_id.is_empty() && !ch.last_message_id.is_empty() && !snowflake_gt(message_id, &ch.last_message_id) {
            return false;
        }
        if !message_id.is_empty() {
            ch.last_message_id = message_id.to_string();
        }
        if is_self {
            ch.unread = 0;
            ch.mentions = 0;
            ch.last_read_id = ch.last_message_id.clone();
        } else {
            ch.unread += 1;
            if mentions_me {
                ch.mentions += 1;
            }
        }
        true
    }

    /// Marks a channel as read. Returns the id of the message to acknowledge to Discord,
    /// or None when there was nothing new.
    pub fn mark_read(&mut self, channel_id: &str) -> Option<String> {
        let ch = self.channels.get_mut(channel_id)?;
        ch.unread = 0;
        ch.mentions = 0;
        if ch.last_message_id.is_empty() || ch.last_message_id == ch.last_read_id {
            return None;
        }
        ch.last_read_id = ch.last_message_id.clone();
        Some(ch.last_message_id.clone())
    }

    /// Another device read the channel up to `message_id` (MESSAGE_ACK).
    pub fn on_remote_ack(&mut self, channel_id: &str, message_id: &str, mention_count: i32) {
        let Some(ch) = self.channels.get_mut(channel_id) else { return };
        if !message_id.is_empty() {
            ch.last_read_id = message_id.to_string();
        }
        ch.mentions = mention_count.max(0);
        let newer = !ch.last_message_id.is_empty() && snowflake_gt(&ch.last_message_id, &ch.last_read_id);
        if !newer && ch.mentions == 0 {
            ch.unread = 0;
        } else {
            ch.unread = ch.unread.max(ch.mentions);
        }
    }

    pub fn is_channel_muted(&self, channel_id: &str) -> bool {
        if self.muted_channels.contains(channel_id) {
            return true;
        }
        self.channels
            .get(channel_id)
            .is_some_and(|c| !c.guild_id.is_empty() && self.muted_guilds.contains(&c.guild_id))
    }

    pub fn channel_badge(&self, channel_id: &str) -> Badge {
        let muted = self.is_channel_muted(channel_id);
        let Some(ch) = self.channels.get(channel_id) else {
            return Badge { unread: 0, mentions: 0, muted };
        };
        Badge { unread: if muted { 0 } else { ch.unread }, mentions: ch.mentions, muted }
    }

    pub fn guild_badge(&self, guild_id: &str) -> Badge {
        let muted = self.muted_guilds.contains(guild_id);
        let mut badge = Badge { unread: 0, mentions: 0, muted };
        for (id, ch) in &self.channels {
            if ch.guild_id != guild_id {
                continue;
            }
            badge.mentions += ch.mentions;
            if !muted && !self.muted_channels.contains(id) {
                badge.unread += ch.unread;
            }
        }
        badge
    }
}

/// True when a MESSAGE_CREATE payload mentions the user: directly, through
/// @everyone/@here, or because it is a direct message (no `guild_id`).
pub fn message_mentions_user(d: &Value, my_user_id: &str) -> bool {
    if d.get("guild_id").map_or(true, |g| g.is_null()) {
        return true;
    }
    if d["mention_everyone"].as_bool().unwrap_or(false) {
        return true;
    }
    !my_user_id.is_empty()
        && d["mentions"]
            .as_array()
            .is_some_and(|m| m.iter().any(|u| u["id"].as_str() == Some(my_user_id)))
}

static STATE: OnceLock<Mutex<UnreadState>> = OnceLock::new();

fn with_state<R>(f: impl FnOnce(&mut UnreadState) -> R) -> R {
    let m = STATE.get_or_init(|| Mutex::new(UnreadState::default()));
    let mut guard = match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

pub fn reset() {
    with_state(|s| s.clear());
}

pub fn apply_ready(d: &Value) {
    with_state(|s| s.apply_ready(d));
}

pub fn apply_settings_entry(e: &Value) {
    with_state(|s| s.apply_settings_entry(e));
}

pub fn register_channels(guild_id: &str, channel_ids: &[&str]) {
    with_state(|s| s.register_channels(guild_id, channel_ids));
}

/// Records a MESSAGE_CREATE payload (`d`) for the user `my_user_id`.
pub fn record_message_event(d: &Value, my_user_id: &str) {
    let channel_id = d["channel_id"].as_str().unwrap_or("");
    if channel_id.is_empty() {
        return;
    }
    let guild_id = d["guild_id"].as_str().unwrap_or(DM_GUILD);
    let message_id = d["id"].as_str().unwrap_or("");
    let is_self = !my_user_id.is_empty() && d["author"]["id"].as_str() == Some(my_user_id);
    let mentions = message_mentions_user(d, my_user_id);
    with_state(|s| {
        s.on_message(guild_id, channel_id, message_id, mentions, is_self);
    });
}

/// Records a MESSAGE_ACK payload (`d`): the channel was read on another device.
pub fn record_remote_ack(d: &Value) {
    let channel_id = d["channel_id"].as_str().unwrap_or("");
    let message_id = d["message_id"].as_str().unwrap_or("");
    let mentions = count_of(&d["mention_count"]);
    if !channel_id.is_empty() {
        with_state(|s| s.on_remote_ack(channel_id, message_id, mentions));
    }
}

pub fn mark_read(channel_id: &str) -> Option<String> {
    with_state(|s| s.mark_read(channel_id))
}

pub fn channel_badge(channel_id: &str) -> Badge {
    with_state(|s| s.channel_badge(channel_id))
}

pub fn guild_badge(guild_id: &str) -> Badge {
    with_state(|s| s.guild_badge(guild_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ready() -> Value {
        json!({
            "guilds": [{"id": "1", "channels": [
                {"id": "10", "last_message_id": "205"},
                {"id": "11", "last_message_id": "300"},
                {"id": "12", "last_message_id": "400"}
            ]}],
            "read_state": {"entries": [
                {"id": "10", "last_message_id": "200", "mention_count": 0},
                {"id": "11", "last_message_id": "300", "mention_count": 0},
                {"id": "12", "last_message_id": "100", "mention_count": 3}
            ]},
            "user_guild_settings": {"entries": [
                {"guild_id": "1", "muted": false, "channel_overrides": [{"channel_id": "11", "muted": true}]},
                {"guild_id": null, "muted": false}
            ]}
        })
    }

    #[test]
    fn snowflakes_compare_numerically() {
        assert!(snowflake_gt("1000", "999"));
        assert!(!snowflake_gt("999", "1000"));
        assert!(!snowflake_gt("5", "5"));
    }

    #[test]
    fn ready_seeds_unread_mentions_and_mutes() {
        let mut s = UnreadState::default();
        s.apply_ready(&ready());
        assert_eq!(s.channel_badge("10"), Badge { unread: 1, mentions: 0, muted: false });
        assert_eq!(s.channel_badge("11"), Badge { unread: 0, mentions: 0, muted: true });
        assert_eq!(s.channel_badge("12"), Badge { unread: 3, mentions: 3, muted: false });
        let g = s.guild_badge("1");
        assert_eq!((g.unread, g.mentions, g.muted), (4, 3, false));
    }

    #[test]
    fn ready_accepts_plain_arrays() {
        let mut s = UnreadState::default();
        s.apply_ready(&json!({
            "read_state": [{"id": "7", "mention_count": 2}],
            "user_guild_settings": [{"guild_id": "9", "muted": true}]
        }));
        assert_eq!(s.channel_badge("7").mentions, 2);
        assert!(s.muted_guilds.contains("9"));
    }

    #[test]
    fn message_counts_once_and_own_message_reads() {
        let mut s = UnreadState::default();
        assert!(s.on_message("1", "10", "500", true, false));
        assert!(!s.on_message("1", "10", "500", true, false));
        assert!(!s.on_message("1", "10", "499", false, false));
        assert!(s.on_message("1", "10", "501", false, false));
        assert_eq!(s.channel_badge("10"), Badge { unread: 2, mentions: 1, muted: false });
        assert!(s.on_message("1", "10", "502", false, true));
        assert_eq!(s.channel_badge("10"), Badge::default());
    }

    #[test]
    fn mark_read_returns_the_message_to_ack_once() {
        let mut s = UnreadState::default();
        s.on_message("1", "10", "500", false, false);
        assert_eq!(s.mark_read("10"), Some("500".to_string()));
        assert_eq!(s.channel_badge("10"), Badge::default());
        assert_eq!(s.mark_read("10"), None);
        assert_eq!(s.mark_read("unknown"), None);
    }

    #[test]
    fn muted_hides_unread_but_not_mentions() {
        let mut s = UnreadState::default();
        s.apply_settings_entry(&json!({"guild_id": "1", "muted": true}));
        s.on_message("1", "10", "500", true, false);
        s.on_message("1", "11", "501", false, false);
        let c = s.channel_badge("11");
        assert!(c.muted);
        assert_eq!(c.unread, 0);
        let g = s.guild_badge("1");
        assert_eq!((g.unread, g.mentions, g.muted), (0, 1, true));
        s.apply_settings_entry(&json!({"guild_id": "1", "muted": false}));
        assert_eq!(s.guild_badge("1").unread, 2);
    }

    #[test]
    fn remote_ack_clears_what_was_read_elsewhere() {
        let mut s = UnreadState::default();
        s.on_message("1", "10", "500", true, false);
        s.on_remote_ack("10", "500", 0);
        assert_eq!(s.channel_badge("10"), Badge::default());
        s.on_message("1", "10", "501", false, false);
        s.on_remote_ack("10", "500", 0);
        assert_eq!(s.channel_badge("10").unread, 1);
    }

    #[test]
    fn mention_detection() {
        assert!(message_mentions_user(&json!({"id": "1"}), "5"));
        assert!(message_mentions_user(&json!({"guild_id": "2", "mention_everyone": true}), "5"));
        assert!(message_mentions_user(&json!({"guild_id": "2", "mentions": [{"id": "5"}]}), "5"));
        assert!(!message_mentions_user(&json!({"guild_id": "2", "mentions": [{"id": "6"}]}), "5"));
    }
}
