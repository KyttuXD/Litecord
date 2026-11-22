//! Threads and forum posts.
//!
//! A thread is a channel (type 10 announcement thread, 11 public, 12 private) whose
//! messages use the normal message endpoints. The gateway tells which threads are
//! active (`GUILD_CREATE`, `THREAD_LIST_SYNC`, `THREAD_CREATE`, `THREAD_UPDATE`,
//! `THREAD_DELETE`); they are kept here per server and merged into the channel list
//! under their parent channel. The posts of a forum channel (type 15) are threads
//! too, but they are listed in the forum view instead of the sidebar.
//!
//! Everything but the small store at the bottom is pure and has tests.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use super::types::ChannelData;

/// Channel type of a forum.
pub const FORUM_CHANNEL_TYPE: u64 = 15;

/// Longest thread name Discord accepts.
const MAX_THREAD_NAME_CHARS: usize = 100;
/// Length the name proposed for a new thread is cut to.
const DEFAULT_THREAD_NAME_CHARS: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Thread {
    pub id: String,
    pub guild_id: String,
    /// The channel (or forum) the thread belongs to.
    pub parent_id: String,
    pub name: String,
    pub archived: bool,
    pub message_count: u32,
}

/// One change of the thread list that the gateway announced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadChange {
    /// A thread was created or updated.
    Upsert(Thread),
    /// A thread was deleted (or the user lost access to it).
    Remove(String),
    /// The active threads of `parents` (all channels when `None`) are exactly `threads`.
    Sync { parents: Option<Vec<String>>, threads: Vec<Thread> },
}

pub fn is_thread_type(channel_type: u64) -> bool {
    matches!(channel_type, 10 | 11 | 12)
}

/// A thread from a gateway or REST channel object. `fallback_guild` is used when the
/// object has no `guild_id` (the threads of `GUILD_CREATE` do not carry one).
pub fn parse_thread(v: &Value, fallback_guild: &str) -> Option<Thread> {
    if !is_thread_type(v["type"].as_u64()?) {
        return None;
    }
    let id = v["id"].as_str().filter(|s| !s.is_empty())?;
    let parent_id = v["parent_id"].as_str().filter(|s| !s.is_empty())?;
    let guild_id = v["guild_id"].as_str().filter(|s| !s.is_empty()).unwrap_or(fallback_guild);
    Some(Thread {
        id: id.to_string(),
        guild_id: guild_id.to_string(),
        parent_id: parent_id.to_string(),
        name: v["name"].as_str().unwrap_or("thread").to_string(),
        archived: v["thread_metadata"]["archived"].as_bool().unwrap_or(false),
        message_count: v["message_count"].as_u64().unwrap_or(0).min(u64::from(u32::MAX)) as u32,
    })
}

/// Every thread of a JSON array (`GUILD_CREATE.threads`, `THREAD_LIST_SYNC.threads`...).
pub fn parse_thread_list(list: &Value, fallback_guild: &str) -> Vec<Thread> {
    list.as_array()
        .map(|items| items.iter().filter_map(|t| parse_thread(t, fallback_guild)).collect())
        .unwrap_or_default()
}

/// The threads in the `GUILD_CREATE` payload, as a change that replaces the active ones.
pub fn guild_create_change(d: &Value, guild_id: &str) -> ThreadChange {
    ThreadChange::Sync { parents: None, threads: parse_thread_list(&d["threads"], guild_id) }
}

/// The server and change a gateway dispatch (`t`, payload `d`) announces, if it is a
/// thread event.
pub fn parse_event(t: &str, d: &Value) -> Option<(String, ThreadChange)> {
    match t {
        "THREAD_CREATE" | "THREAD_UPDATE" => {
            let thread = parse_thread(d, "")?;
            if thread.guild_id.is_empty() {
                return None;
            }
            Some((thread.guild_id.clone(), ThreadChange::Upsert(thread)))
        }
        "THREAD_DELETE" => {
            let id = d["id"].as_str().filter(|s| !s.is_empty())?;
            let guild_id = d["guild_id"].as_str().filter(|s| !s.is_empty())?;
            Some((guild_id.to_string(), ThreadChange::Remove(id.to_string())))
        }
        "THREAD_LIST_SYNC" => {
            let guild_id = d["guild_id"].as_str().filter(|s| !s.is_empty())?;
            let parents = d["channel_ids"].as_array().map(|ids| {
                ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect::<Vec<_>>()
            });
            let threads = parse_thread_list(&d["threads"], guild_id);
            Some((guild_id.to_string(), ThreadChange::Sync { parents, threads }))
        }
        _ => None,
    }
}

/// Applies `change` to the threads known of one server. Archived threads stay in the
/// list (a forum lists them, and they can still be opened) but are not shown in the
/// sidebar; a sync only replaces the active ones.
pub fn apply(list: &mut Vec<Thread>, change: ThreadChange) {
    match change {
        ThreadChange::Upsert(thread) => upsert(list, thread),
        ThreadChange::Remove(id) => list.retain(|t| t.id != id),
        ThreadChange::Sync { parents, threads } => {
            list.retain(|t| {
                let replaced = match &parents {
                    Some(parents) => parents.contains(&t.parent_id),
                    None => true,
                };
                t.archived || !replaced
            });
            for thread in threads {
                upsert(list, thread);
            }
        }
    }
}

fn upsert(list: &mut Vec<Thread>, thread: Thread) {
    match list.iter_mut().find(|t| t.id == thread.id) {
        Some(slot) => *slot = thread,
        None => list.push(thread),
    }
}

fn snowflake(id: &str) -> u64 {
    id.parse().unwrap_or(0)
}

/// `channels` without its old threads, plus the active `threads` whose parent is a
/// text channel of the list (forum posts are not listed in the sidebar), newest first.
pub fn with_threads(channels: Vec<ChannelData>, threads: &[Thread]) -> Vec<ChannelData> {
    let mut out: Vec<ChannelData> = channels.into_iter().filter(|c| !c.is_thread).collect();
    let mut shown: Vec<&Thread> = threads
        .iter()
        .filter(|t| !t.archived)
        .filter(|t| out.iter().any(|c| c.id == t.parent_id && !c.is_category && !c.is_forum))
        .collect();
    shown.sort_by(|a, b| snowflake(&b.id).cmp(&snowflake(&a.id)));
    for t in shown {
        out.push(ChannelData {
            id: t.id.clone(),
            name: t.name.clone(),
            is_voice: false,
            is_category: false,
            parent_id: Some(t.parent_id.clone()),
            position: 0,
            topic: None,
            is_thread: true,
            is_forum: false,
        });
    }
    out
}

/// The posts of a forum among `threads`, newest first.
pub fn forum_posts(threads: &[Thread], forum_id: &str) -> Vec<Thread> {
    let mut posts: Vec<Thread> = threads.iter().filter(|t| t.parent_id == forum_id).cloned().collect();
    posts.sort_by(|a, b| snowflake(&b.id).cmp(&snowflake(&a.id)));
    posts
}

/// Posts from the REST search answer (`{"threads": [...]}`).
pub fn parse_search_result(v: &Value, guild_id: &str) -> Vec<Thread> {
    parse_thread_list(&v["threads"], guild_id)
}

/// One line about a post: "12 messages", with " · archived" when it is.
/// `messages_label` holds a `{}` for the count.
pub fn post_summary(post: &Thread, messages_label: &str, archived_label: &str) -> String {
    let mut text = messages_label.replace("{}", &post.message_count.to_string());
    if post.archived {
        text.push_str(" · ");
        text.push_str(archived_label);
    }
    text
}

/// The name proposed for a thread started from a message: its first line of text,
/// cut to a readable length; `fallback` when the message has no text.
pub fn default_thread_name(content: &str, fallback: &str) -> String {
    let first_line = content.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let collapsed = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = collapsed.chars().take(DEFAULT_THREAD_NAME_CHARS).collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        fallback.chars().take(MAX_THREAD_NAME_CHARS).collect()
    } else {
        name
    }
}

fn store() -> &'static Mutex<HashMap<String, Vec<Thread>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Vec<Thread>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Applies a gateway (or REST) change to the threads of a server.
pub fn apply_to_guild(guild_id: &str, change: ThreadChange) {
    if let Ok(mut map) = store().lock() {
        apply(map.entry(guild_id.to_string()).or_default(), change);
    }
}

/// Every thread known of a server, archived ones included.
pub fn guild_threads(guild_id: &str) -> Vec<Thread> {
    store().lock().ok().and_then(|map| map.get(guild_id).cloned()).unwrap_or_default()
}

/// A known thread, whatever its server.
pub fn find(thread_id: &str) -> Option<Thread> {
    let map = store().lock().ok()?;
    map.values().flatten().find(|t| t.id == thread_id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn thread(id: &str, parent: &str) -> Thread {
        Thread {
            id: id.into(),
            guild_id: "g".into(),
            parent_id: parent.into(),
            name: format!("t{id}"),
            ..Default::default()
        }
    }

    fn chan(id: &str, category: bool, forum: bool) -> ChannelData {
        ChannelData {
            id: id.into(),
            name: id.into(),
            is_voice: false,
            is_category: category,
            parent_id: None,
            position: 0,
            topic: None,
            is_thread: false,
            is_forum: forum,
        }
    }

    #[test]
    fn a_thread_object_is_read_with_the_guild_of_its_payload() {
        let v = json!({"id":"5","type":11,"parent_id":"2","name":"plans","message_count":7,
            "thread_metadata":{"archived":true}});
        let t = parse_thread(&v, "g").unwrap();
        assert_eq!((t.id.as_str(), t.parent_id.as_str(), t.guild_id.as_str()), ("5", "2", "g"));
        assert_eq!((t.name.as_str(), t.archived, t.message_count), ("plans", true, 7));
        let with_guild = json!({"id":"5","type":12,"parent_id":"2","guild_id":"h"});
        assert_eq!(parse_thread(&with_guild, "g").unwrap().guild_id, "h");
        assert!(!parse_thread(&with_guild, "g").unwrap().archived);
        // Not a thread, or no parent: nothing to show.
        assert_eq!(parse_thread(&json!({"id":"5","type":0,"parent_id":"2"}), "g"), None);
        assert_eq!(parse_thread(&json!({"id":"5","type":11}), "g"), None);
        assert_eq!(parse_thread(&json!({"type":11,"parent_id":"2"}), "g"), None);
    }

    #[test]
    fn gateway_events_become_changes_of_a_server() {
        let create = json!({"id":"5","type":11,"parent_id":"2","guild_id":"g","name":"a"});
        let (guild, change) = parse_event("THREAD_CREATE", &create).unwrap();
        assert_eq!(guild, "g");
        assert!(matches!(change, ThreadChange::Upsert(t) if t.id == "5"));
        // No server: it cannot be placed.
        assert_eq!(parse_event("THREAD_UPDATE", &json!({"id":"5","type":11,"parent_id":"2"})), None);
        let delete = json!({"id":"5","guild_id":"g","parent_id":"2","type":11});
        assert_eq!(parse_event("THREAD_DELETE", &delete), Some(("g".into(), ThreadChange::Remove("5".into()))));
        let sync = json!({"guild_id":"g","channel_ids":["2"],"threads":[{"id":"6","type":11,"parent_id":"2"}]});
        let (guild, change) = parse_event("THREAD_LIST_SYNC", &sync).unwrap();
        assert_eq!(guild, "g");
        match change {
            ThreadChange::Sync { parents, threads } => {
                assert_eq!(parents, Some(vec!["2".to_string()]));
                assert_eq!(threads.len(), 1);
                assert_eq!(threads[0].guild_id, "g");
            }
            other => panic!("{other:?}"),
        }
        let sync_all = json!({"guild_id":"g","threads":[]});
        assert!(matches!(
            parse_event("THREAD_LIST_SYNC", &sync_all),
            Some((_, ThreadChange::Sync { parents: None, .. }))
        ));
        assert_eq!(parse_event("MESSAGE_CREATE", &create), None);
    }

    #[test]
    fn guild_create_threads_replace_the_active_ones() {
        let d = json!({"threads":[{"id":"5","type":11,"parent_id":"2"},{"id":"6","type":0,"parent_id":"2"}]});
        let mut list = vec![thread("1", "2")];
        apply(&mut list, guild_create_change(&d, "g"));
        assert_eq!(list.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["5"]);
        assert_eq!(list[0].guild_id, "g");
    }

    #[test]
    fn changes_add_update_archive_and_remove_threads() {
        let mut list = Vec::new();
        apply(&mut list, ThreadChange::Upsert(thread("5", "2")));
        apply(&mut list, ThreadChange::Upsert(thread("6", "2")));
        let mut renamed = thread("5", "2");
        renamed.name = "new".into();
        apply(&mut list, ThreadChange::Upsert(renamed));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "new");
        let mut archived = thread("6", "2");
        archived.archived = true;
        apply(&mut list, ThreadChange::Upsert(archived));
        assert!(list[1].archived, "an archived thread is kept for the forum list");
        apply(&mut list, ThreadChange::Remove("5".into()));
        assert_eq!(list.len(), 1);
        apply(&mut list, ThreadChange::Remove("nope".into()));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn a_sync_replaces_only_the_active_threads_of_the_named_parents() {
        let mut archived = thread("4", "2");
        archived.archived = true;
        let mut list = vec![thread("5", "2"), thread("6", "3"), archived];
        apply(&mut list, ThreadChange::Sync { parents: Some(vec!["2".into()]), threads: vec![thread("7", "2")] });
        let ids: Vec<&str> = list.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["6", "4", "7"]);
        apply(&mut list, ThreadChange::Sync { parents: None, threads: vec![] });
        assert_eq!(list.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["4"]);
    }

    #[test]
    fn active_threads_go_under_text_channels_newest_first() {
        let channels = vec![chan("cat", true, false), chan("2", false, false), chan("9", false, true)];
        let mut hidden = thread("30", "2");
        hidden.archived = true;
        let threads = vec![
            thread("10", "2"),
            thread("20", "2"),
            hidden,
            thread("21", "9"),    // post of a forum: not in the sidebar
            thread("22", "gone"), // parent not visible to the user
            thread("23", "cat"),  // a category has no threads
        ];
        let out = with_threads(channels, &threads);
        let ids: Vec<&str> = out.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["cat", "2", "9", "20", "10"]);
        assert!(out[3].is_thread && !out[3].is_forum);
        assert_eq!(out[3].parent_id.as_deref(), Some("2"));
        // Building again does not duplicate the threads.
        let again = with_threads(out, &threads);
        assert_eq!(again.len(), 5);
        // A thread whose parent vanished from the list goes away with it.
        let without_parent = with_threads(again.into_iter().filter(|c| c.id != "2").collect(), &threads);
        assert_eq!(without_parent.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["cat", "9"]);
    }

    #[test]
    fn forum_posts_are_the_threads_of_the_forum_newest_first() {
        let mut archived = thread("5", "9");
        archived.archived = true;
        let threads = vec![thread("7", "9"), thread("8", "2"), archived, thread("11", "9")];
        let ids: Vec<String> = forum_posts(&threads, "9").into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec!["11", "7", "5"]);
        assert!(forum_posts(&threads, "none").is_empty());
        let found = parse_search_result(&json!({"threads":[{"id":"5","type":11,"parent_id":"9"}]}), "g");
        assert_eq!(found.len(), 1);
        assert!(parse_search_result(&json!({}), "g").is_empty());
    }

    #[test]
    fn a_post_summary_counts_messages_and_flags_archived_posts() {
        let mut post = thread("5", "9");
        post.message_count = 12;
        assert_eq!(post_summary(&post, "{} messages", "archived"), "12 messages");
        post.archived = true;
        assert_eq!(post_summary(&post, "{} messages", "archived"), "12 messages · archived");
    }

    #[test]
    fn a_new_thread_is_named_after_the_first_line_of_its_message() {
        assert_eq!(default_thread_name("  Lunch   plans?\nsecond line", "Thread"), "Lunch plans?");
        assert_eq!(default_thread_name("\n\n  hello", "Thread"), "hello");
        assert_eq!(default_thread_name("", "Thread"), "Thread");
        assert_eq!(default_thread_name("   \n ", "Thread"), "Thread");
        let long = "é".repeat(80);
        assert_eq!(default_thread_name(&long, "Thread").chars().count(), 50);
    }

    #[test]
    fn the_store_keeps_threads_per_server_and_finds_them_by_id() {
        apply_to_guild("store-g1", ThreadChange::Upsert(thread("901", "2")));
        apply_to_guild("store-g2", ThreadChange::Upsert(thread("902", "2")));
        assert_eq!(guild_threads("store-g1").len(), 1);
        assert_eq!(find("902").unwrap().id, "902");
        apply_to_guild("store-g2", ThreadChange::Remove("902".into()));
        assert!(find("902").is_none());
        assert!(guild_threads("store-none").is_empty());
    }
}
