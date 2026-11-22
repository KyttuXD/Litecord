//! Friends list data: relationships from READY and RELATIONSHIP_* events, grouped and
//! sorted for the Friends view. Pure logic plus a small shared store; no UI types here.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

pub const REL_FRIEND: u8 = 1;
pub const REL_BLOCKED: u8 = 2;
pub const REL_INCOMING: u8 = 3;
pub const REL_OUTGOING: u8 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relationship {
    pub id: u64,
    /// One of the REL_* constants.
    pub kind: u8,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FriendsTab {
    Online,
    All,
    Pending,
    Blocked,
}

impl FriendsTab {
    pub fn from_index(i: i32) -> Self {
        match i {
            0 => FriendsTab::Online,
            2 => FriendsTab::Pending,
            3 => FriendsTab::Blocked,
            _ => FriendsTab::All,
        }
    }
}

/// Reads the id of a relationship object (`id`, or `user_id` in the newer READY shape).
fn read_id(v: &Value) -> Option<u64> {
    for key in ["id", "user_id"] {
        match &v[key] {
            Value::String(s) => {
                if let Ok(n) = s.parse::<u64>() {
                    if n > 0 {
                        return Some(n);
                    }
                }
            }
            Value::Number(n) => {
                if let Some(n) = n.as_u64() {
                    if n > 0 {
                        return Some(n);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn non_empty_str(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}

/// Builds a relationship from a READY `relationships` entry or a RELATIONSHIP_ADD
/// payload. `lookup` names users that carry no `user` object (newer READY shape,
/// where the users are listed apart).
pub fn parse_relationship(v: &Value, lookup: &dyn Fn(u64) -> Option<String>) -> Option<Relationship> {
    let id = read_id(v)?;
    let kind = u8::try_from(v["type"].as_u64()?).ok()?;
    if !(REL_FRIEND..=REL_OUTGOING).contains(&kind) {
        return None;
    }
    let user = &v["user"];
    let name = non_empty_str(&v["nickname"])
        .or_else(|| non_empty_str(&user["global_name"]))
        .or_else(|| non_empty_str(&user["username"]))
        .map(str::to_string)
        .or_else(|| lookup(id))
        .unwrap_or_else(|| format!("User {}", id));
    Some(Relationship { id, kind, name })
}

/// Ids of the users READY lists as present (anything but offline/invisible).
pub fn parse_online_ids(presences: &Value) -> HashSet<u64> {
    let mut out = HashSet::new();
    if let Some(arr) = presences.as_array() {
        for p in arr {
            let status = p["status"].as_str().unwrap_or("offline");
            if status == "offline" || status == "invisible" {
                continue;
            }
            let id = read_id(&p["user"]).or_else(|| read_id(p));
            if let Some(id) = id {
                out.insert(id);
            }
        }
    }
    out
}

fn sort_key(r: &Relationship) -> (String, u64) {
    (r.name.to_lowercase(), r.id)
}

/// The rows of one tab. Friends and blocked users go by name; pending puts incoming
/// requests (which wait for an answer) before outgoing ones.
pub fn tab_items(rels: &[Relationship], online: &HashSet<u64>, tab: FriendsTab) -> Vec<Relationship> {
    let mut out: Vec<Relationship> = rels
        .iter()
        .filter(|r| match tab {
            FriendsTab::Online => r.kind == REL_FRIEND && online.contains(&r.id),
            FriendsTab::All => r.kind == REL_FRIEND,
            FriendsTab::Pending => r.kind == REL_INCOMING || r.kind == REL_OUTGOING,
            FriendsTab::Blocked => r.kind == REL_BLOCKED,
        })
        .cloned()
        .collect();
    if tab == FriendsTab::Pending {
        out.sort_by(|a, b| {
            let rank = |r: &Relationship| if r.kind == REL_INCOMING { 0 } else { 1 };
            rank(a).cmp(&rank(b)).then_with(|| sort_key(a).cmp(&sort_key(b)))
        });
    } else {
        out.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    }
    out
}

#[allow(dead_code)]
pub fn online_count(rels: &[Relationship], online: &HashSet<u64>) -> usize {
    rels.iter().filter(|r| r.kind == REL_FRIEND && online.contains(&r.id)).count()
}

/// Incoming requests only: the ones that need the user's answer.
pub fn incoming_count(rels: &[Relationship]) -> usize {
    rels.iter().filter(|r| r.kind == REL_INCOMING).count()
}

pub fn status_label(kind: u8, online: bool) -> &'static str {
    match kind {
        REL_FRIEND => {
            if online {
                "Online"
            } else {
                "Offline"
            }
        }
        REL_BLOCKED => "Blocked",
        REL_INCOMING => "Incoming Friend Request",
        REL_OUTGOING => "Outgoing Friend Request",
        _ => "",
    }
}

/// Splits what the user typed into the username and the optional legacy discriminator
/// (`name#1234`). Returns None for empty input.
pub fn parse_friend_input(input: &str) -> Option<(String, Option<String>)> {
    let s = input.trim().trim_start_matches('@').trim();
    if s.is_empty() {
        return None;
    }
    if let Some((name, tag)) = s.rsplit_once('#') {
        let name = name.trim();
        if !name.is_empty() && tag.len() == 4 && tag.chars().all(|c| c.is_ascii_digit()) {
            return Some((name.to_string(), Some(tag.to_string())));
        }
    }
    Some((s.to_string(), None))
}

#[derive(Default)]
struct Store {
    rels: HashMap<u64, Relationship>,
    online: HashSet<u64>,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let m = STORE.get_or_init(|| Mutex::new(Store::default()));
    let mut guard = match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

/// Replaces everything, as READY does (also after a new session).
pub fn replace_all(rels: Vec<Relationship>, online: HashSet<u64>) {
    with_store(|s| {
        s.rels = rels.into_iter().map(|r| (r.id, r)).collect();
        s.online = online;
    });
}

pub fn upsert(rel: Relationship) {
    with_store(|s| {
        s.rels.insert(rel.id, rel);
    });
}

pub fn set_online(online: HashSet<u64>) {
    with_store(|s| s.online = online);
}

pub fn remove(id: u64) {
    with_store(|s| {
        s.rels.remove(&id);
    });
}

pub fn snapshot() -> (Vec<Relationship>, HashSet<u64>) {
    with_store(|s| (s.rels.values().cloned().collect(), s.online.clone()))
}

#[allow(dead_code)]
pub fn clear() {
    with_store(|s| {
        s.rels.clear();
        s.online.clear();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rel(id: u64, kind: u8, name: &str) -> Relationship {
        Relationship { id, kind, name: name.to_string() }
    }

    fn no_lookup(_: u64) -> Option<String> {
        None
    }

    #[test]
    fn parses_ready_entries_in_both_shapes() {
        let a = json!({"id":"5","type":1,"nickname":null,"user":{"id":"5","username":"bob","global_name":"Bobby"}});
        assert_eq!(parse_relationship(&a, &no_lookup), Some(rel(5, 1, "Bobby")));
        let b = json!({"id":"6","type":3,"user":{"id":"6","username":"amy","global_name":null}});
        assert_eq!(parse_relationship(&b, &no_lookup), Some(rel(6, 3, "amy")));
        let c = json!({"user_id":"7","type":2,"nickname":"Zed"});
        assert_eq!(parse_relationship(&c, &no_lookup), Some(rel(7, 2, "Zed")));
        let d = json!({"user_id":"8","type":4});
        let lookup = |id: u64| if id == 8 { Some("Looked".to_string()) } else { None };
        assert_eq!(parse_relationship(&d, &lookup), Some(rel(8, 4, "Looked")));
        assert_eq!(parse_relationship(&d, &no_lookup), Some(rel(8, 4, "User 8")));
        // Unknown types and missing ids are skipped.
        assert_eq!(parse_relationship(&json!({"id":"9","type":0}), &no_lookup), None);
        assert_eq!(parse_relationship(&json!({"id":"9","type":5}), &no_lookup), None);
        assert_eq!(parse_relationship(&json!({"type":1}), &no_lookup), None);
    }

    #[test]
    fn presences_list_the_users_that_are_not_offline() {
        let p = json!([
            {"user":{"id":"1"},"status":"online"},
            {"user":{"id":"2"},"status":"idle"},
            {"user_id":"3","status":"dnd"},
            {"user":{"id":"4"},"status":"offline"},
            {"user":{"id":"5"},"status":"invisible"},
            {"status":"online"}
        ]);
        let ids = parse_online_ids(&p);
        let mut v: Vec<u64> = ids.into_iter().collect();
        v.sort();
        assert_eq!(v, vec![1, 2, 3]);
        assert!(parse_online_ids(&Value::Null).is_empty());
    }

    #[test]
    fn tabs_group_and_sort_the_relationships() {
        let rels = vec![
            rel(1, REL_FRIEND, "zoe"),
            rel(2, REL_FRIEND, "Adam"),
            rel(3, REL_FRIEND, "bea"),
            rel(4, REL_OUTGOING, "carl"),
            rel(5, REL_INCOMING, "dora"),
            rel(6, REL_INCOMING, "Anna"),
            rel(7, REL_BLOCKED, "troll"),
        ];
        let online: HashSet<u64> = [1u64, 3, 4, 99].into_iter().collect();
        let names = |tab| tab_items(&rels, &online, tab).into_iter().map(|r| r.name).collect::<Vec<_>>();
        assert_eq!(names(FriendsTab::All), vec!["Adam", "bea", "zoe"]);
        // Only friends count as online, even if a pending user has a presence.
        assert_eq!(names(FriendsTab::Online), vec!["bea", "zoe"]);
        assert_eq!(names(FriendsTab::Pending), vec!["Anna", "dora", "carl"]);
        assert_eq!(names(FriendsTab::Blocked), vec!["troll"]);
        assert_eq!(online_count(&rels, &online), 2);
        assert_eq!(incoming_count(&rels), 2);
    }

    #[test]
    fn equal_names_keep_a_stable_order() {
        let rels = vec![rel(9, REL_FRIEND, "sam"), rel(3, REL_FRIEND, "Sam")];
        let items = tab_items(&rels, &HashSet::new(), FriendsTab::All);
        assert_eq!(items.iter().map(|r| r.id).collect::<Vec<_>>(), vec![3, 9]);
    }

    #[test]
    fn tab_index_maps_to_the_tab() {
        assert_eq!(FriendsTab::from_index(0), FriendsTab::Online);
        assert_eq!(FriendsTab::from_index(1), FriendsTab::All);
        assert_eq!(FriendsTab::from_index(2), FriendsTab::Pending);
        assert_eq!(FriendsTab::from_index(3), FriendsTab::Blocked);
        assert_eq!(FriendsTab::from_index(42), FriendsTab::All);
    }

    #[test]
    fn friend_input_splits_the_legacy_discriminator() {
        assert_eq!(parse_friend_input("  "), None);
        assert_eq!(parse_friend_input("@"), None);
        assert_eq!(parse_friend_input("bob"), Some(("bob".to_string(), None)));
        assert_eq!(parse_friend_input(" @bob "), Some(("bob".to_string(), None)));
        assert_eq!(parse_friend_input("bob#0420"), Some(("bob".to_string(), Some("0420".to_string()))));
        assert_eq!(parse_friend_input("bob#42"), Some(("bob#42".to_string(), None)));
        assert_eq!(parse_friend_input("#1234"), Some(("#1234".to_string(), None)));
    }

    #[test]
    fn labels_follow_the_relationship() {
        assert_eq!(status_label(REL_FRIEND, true), "Online");
        assert_eq!(status_label(REL_FRIEND, false), "Offline");
        assert_eq!(status_label(REL_INCOMING, false), "Incoming Friend Request");
        assert_eq!(status_label(REL_OUTGOING, false), "Outgoing Friend Request");
        assert_eq!(status_label(REL_BLOCKED, false), "Blocked");
    }

    #[test]
    fn store_applies_add_and_remove_events() {
        replace_all(vec![rel(1, REL_FRIEND, "a"), rel(2, REL_INCOMING, "b")], [1u64].into_iter().collect());
        upsert(rel(2, REL_FRIEND, "b"));
        upsert(rel(3, REL_BLOCKED, "c"));
        remove(1);
        let (mut rels, online) = snapshot();
        rels.sort_by_key(|r| r.id);
        assert_eq!(rels, vec![rel(2, REL_FRIEND, "b"), rel(3, REL_BLOCKED, "c")]);
        assert!(online.contains(&1));
        clear();
        assert!(snapshot().0.is_empty());
    }
}
