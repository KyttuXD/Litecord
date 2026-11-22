//! Member list of a server, as the gateway sends it after a "lazy guild" subscription
//! (opcode 14) as GUILD_MEMBER_LIST_UPDATE events.
//!
//! The list is a flat sequence of group headers ("online", "offline" or a role id) and
//! members. The server only sends the ranges that were subscribed to and then patches them
//! with SYNC / UPDATE / INSERT / DELETE / INVALIDATE operations on indexes.
//! Everything here is pure (JSON in, state out) apart from the small stores at the bottom.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

/// The list is never kept longer than this many slots.
const MAX_ITEMS: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presence {
    Online,
    Idle,
    Dnd,
    #[default]
    Offline,
}

impl Presence {
    fn parse(status: &str) -> Presence {
        match status {
            "online" => Presence::Online,
            "idle" => Presence::Idle,
            "dnd" => Presence::Dnd,
            _ => Presence::Offline,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberEntry {
    pub user_id: String,
    pub name: String,
    pub avatar_hash: String,
    pub presence: Presence,
    pub bot: bool,
    pub role_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListItem {
    Group { id: String, count: u64 },
    Member(MemberEntry),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListOp {
    /// Replaces the slots `start..=end` with `items`.
    Sync { start: usize, end: usize, items: Vec<ListItem> },
    Update { index: usize, item: ListItem },
    Insert { index: usize, item: ListItem },
    Delete { index: usize },
    /// The slots `start..=end` are no longer kept up to date.
    Invalidate { start: usize, end: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberListUpdate {
    pub guild_id: String,
    /// "everyone" or a hash of the permissions that decide who is listed.
    pub list_id: String,
    pub member_count: u64,
    pub online_count: u64,
    pub ops: Vec<ListOp>,
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn count(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

fn index(v: &Value) -> Option<usize> {
    v.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n < MAX_ITEMS)
}

fn range(v: &Value) -> Option<(usize, usize)> {
    let pair = v.as_array()?;
    let start = index(pair.first()?)?;
    let end = index(pair.get(1)?)?;
    (start <= end).then_some((start, end))
}

/// One element of `items` / `item`: `{"group": {...}}` or `{"member": {...}}`.
fn parse_item(v: &Value) -> Option<ListItem> {
    if let Some(g) = v.get("group") {
        return Some(ListItem::Group { id: text(&g["id"])?, count: count(&g["count"]) });
    }
    let m = v.get("member")?;
    let user = &m["user"];
    let user_id = text(&user["id"])?;
    let name = [&m["nick"], &user["global_name"], &user["username"]]
        .into_iter()
        .filter_map(|v| v.as_str())
        .find(|s| !s.is_empty())
        .unwrap_or("Unknown")
        .to_string();
    let presence = m["presence"]["status"].as_str().map(Presence::parse).unwrap_or_default();
    let role_ids = m["roles"]
        .as_array()
        .map(|roles| roles.iter().filter_map(text).collect())
        .unwrap_or_default();
    Some(ListItem::Member(MemberEntry {
        user_id,
        name,
        avatar_hash: user["avatar"].as_str().unwrap_or("").to_string(),
        presence,
        bot: user["bot"].as_bool().unwrap_or(false),
        role_ids,
    }))
}

fn parse_op(v: &Value) -> Option<ListOp> {
    match v["op"].as_str()? {
        "SYNC" => {
            let (start, end) = range(&v["range"])?;
            let items = v["items"].as_array()?.iter().filter_map(parse_item).collect();
            Some(ListOp::Sync { start, end, items })
        }
        "UPDATE" => Some(ListOp::Update { index: index(&v["index"])?, item: parse_item(&v["item"])? }),
        "INSERT" => Some(ListOp::Insert { index: index(&v["index"])?, item: parse_item(&v["item"])? }),
        "DELETE" => Some(ListOp::Delete { index: index(&v["index"])? }),
        "INVALIDATE" => {
            let (start, end) = range(&v["range"])?;
            Some(ListOp::Invalidate { start, end })
        }
        _ => None,
    }
}

/// Parses the `d` of a GUILD_MEMBER_LIST_UPDATE event. Unknown operations are skipped.
pub fn parse_update(d: &Value) -> Option<MemberListUpdate> {
    let guild_id = text(&d["guild_id"])?;
    let ops = d["ops"].as_array()?.iter().filter_map(parse_op).collect();
    Some(MemberListUpdate {
        guild_id,
        list_id: text(&d["id"]).unwrap_or_default(),
        member_count: count(&d["member_count"]),
        online_count: count(&d["online_count"]),
        ops,
    })
}

/// The opcode 14 message that subscribes to the first `last_index + 1` rows of the list
/// seen from `channel_id`.
pub fn subscribe_payload(guild_id: &str, channel_id: &str, last_index: u32) -> Value {
    serde_json::json!({
        "op": 14,
        "d": {
            "guild_id": guild_id,
            "typing": true,
            "activities": true,
            "threads": true,
            "channels": { channel_id: [[0, last_index]] }
        }
    })
}

/// What a client has built up from the updates so far. Slots the server has not sent
/// (or has invalidated) are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberListState {
    pub list_id: String,
    pub member_count: u64,
    pub online_count: u64,
    items: Vec<Option<ListItem>>,
}

impl MemberListState {
    pub fn apply(&mut self, update: &MemberListUpdate) {
        if !update.list_id.is_empty() && update.list_id != self.list_id {
            // Another list (other permissions): its indexes mean nothing for the old one.
            self.items.clear();
            self.list_id = update.list_id.clone();
        }
        self.member_count = update.member_count;
        self.online_count = update.online_count;
        for op in &update.ops {
            self.apply_op(op);
        }
    }

    fn apply_op(&mut self, op: &ListOp) {
        match op {
            ListOp::Sync { start, end, items } => {
                if self.items.len() < *start {
                    self.items.resize(*start, None);
                }
                // A range cut short by the end of the list simply holds fewer items.
                let upto = (*end + 1).min(self.items.len());
                self.items.splice(*start..upto, items.iter().cloned().map(Some));
            }
            ListOp::Update { index, item } => {
                if *index >= self.items.len() {
                    self.items.resize(*index + 1, None);
                }
                self.items[*index] = Some(item.clone());
            }
            ListOp::Insert { index, item } => {
                if *index > self.items.len() {
                    self.items.resize(*index, None);
                }
                self.items.insert(*index, Some(item.clone()));
            }
            ListOp::Delete { index } => {
                if *index < self.items.len() {
                    self.items.remove(*index);
                }
            }
            ListOp::Invalidate { start, end } => {
                for slot in self.items.iter_mut().take(end + 1).skip(*start) {
                    *slot = None;
                }
            }
        }
        self.items.truncate(MAX_ITEMS);
    }

    /// The slots that are known, in order. A member comes with the id of its group.
    pub fn rows(&self) -> Vec<ListRow<'_>> {
        let mut group: Option<&str> = None;
        let mut rows = Vec::with_capacity(self.items.len());
        for item in self.items.iter().flatten() {
            match item {
                ListItem::Group { id, count } => {
                    group = Some(id);
                    rows.push(ListRow::Group { id, count: *count });
                }
                ListItem::Member(member) => rows.push(ListRow::Member { group, member }),
            }
        }
        rows
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListRow<'a> {
    Group { id: &'a str, count: u64 },
    Member { group: Option<&'a str>, member: &'a MemberEntry },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoleInfo {
    pub name: String,
    /// 0xRRGGBB; 0 means the role has no colour.
    pub color: u32,
    pub position: i64,
}

/// Parses the body of GET /guilds/{id}/roles.
pub fn parse_roles(roles: &[Value]) -> HashMap<String, RoleInfo> {
    roles
        .iter()
        .filter_map(|r| {
            Some((
                text(&r["id"])?,
                RoleInfo {
                    name: r["name"].as_str().unwrap_or("").to_string(),
                    color: r["color"].as_u64().and_then(|c| u32::try_from(c).ok()).unwrap_or(0) & 0x00FF_FFFF,
                    position: r["position"].as_i64().unwrap_or(0),
                },
            ))
        })
        .collect()
}

/// Colour of a member's name: that of its highest role that has one, as Discord shows it.
pub fn member_color(roles: &HashMap<String, RoleInfo>, member: &MemberEntry) -> Option<u32> {
    member
        .role_ids
        .iter()
        .filter_map(|id| roles.get(id))
        .filter(|r| r.color != 0)
        .max_by_key(|r| r.position)
        .map(|r| r.color)
}

/// Title of a group header: "Online", "Offline" or the name of the role.
pub fn group_title(roles: &HashMap<String, RoleInfo>, group_id: &str) -> String {
    match group_id {
        "online" => "Online".to_string(),
        "offline" => "Offline".to_string(),
        id => roles.get(id).map(|r| r.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Role".to_string()),
    }
}

/// `https://cdn.discordapp.com` URL of a member's avatar, or empty when it has none.
pub fn avatar_url(member: &MemberEntry) -> String {
    let hash = member.avatar_hash.as_str();
    let hex = hash.strip_prefix("a_").unwrap_or(hash);
    let id_ok = !member.user_id.is_empty() && member.user_id.bytes().all(|b| b.is_ascii_digit());
    if id_ok && !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("https://cdn.discordapp.com/avatars/{}/{}.webp?size=64", member.user_id, hash)
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------------------------
// Stores shared between the gateway task and the UI
// ---------------------------------------------------------------------------------

fn lists() -> &'static Mutex<HashMap<String, MemberListState>> {
    static LISTS: OnceLock<Mutex<HashMap<String, MemberListState>>> = OnceLock::new();
    LISTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn role_store() -> &'static Mutex<HashMap<String, HashMap<String, RoleInfo>>> {
    static ROLES: OnceLock<Mutex<HashMap<String, HashMap<String, RoleInfo>>>> = OnceLock::new();
    ROLES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Applies an update from the gateway to the list of its server.
pub fn apply_update(update: &MemberListUpdate) {
    if let Ok(mut all) = lists().lock() {
        all.entry(update.guild_id.clone()).or_default().apply(update);
    }
}

/// Forgets the list of a server, e.g. before subscribing from another channel.
pub fn reset(guild_id: &str) {
    if let Ok(mut all) = lists().lock() {
        all.remove(guild_id);
    }
}

/// Forgets every list, e.g. after a fresh gateway session.
pub fn reset_all() {
    if let Ok(mut all) = lists().lock() {
        all.clear();
    }
}

/// Runs `f` on the list of a server (an empty one when nothing has arrived yet).
pub fn with_list<R>(guild_id: &str, f: impl FnOnce(&MemberListState) -> R) -> R {
    match lists().lock() {
        Ok(all) => match all.get(guild_id) {
            Some(list) => f(list),
            None => f(&MemberListState::default()),
        },
        Err(_) => f(&MemberListState::default()),
    }
}

pub fn set_roles(guild_id: &str, roles: HashMap<String, RoleInfo>) {
    if let Ok(mut all) = role_store().lock() {
        all.insert(guild_id.to_string(), roles);
    }
}

pub fn has_roles(guild_id: &str) -> bool {
    role_store().lock().map(|all| all.contains_key(guild_id)).unwrap_or(false)
}

pub fn with_roles<R>(guild_id: &str, f: impl FnOnce(&HashMap<String, RoleInfo>) -> R) -> R {
    match role_store().lock() {
        Ok(all) => match all.get(guild_id) {
            Some(roles) => f(roles),
            None => f(&HashMap::new()),
        },
        Err(_) => f(&HashMap::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn member(id: &str, name: &str) -> Value {
        json!({"member": {"user": {"id": id, "username": name}, "roles": [], "presence": {"status": "online"}}})
    }

    fn group(id: &str, count: u64) -> Value {
        json!({"group": {"id": id, "count": count}})
    }

    fn update(ops: Value) -> MemberListUpdate {
        parse_update(&json!({"guild_id": "1", "id": "everyone", "member_count": 5, "online_count": 2, "ops": ops})).unwrap()
    }

    fn names(state: &MemberListState) -> Vec<String> {
        state
            .rows()
            .iter()
            .map(|r| match r {
                ListRow::Group { id, .. } => format!("#{id}"),
                ListRow::Member { member, .. } => member.name.clone(),
            })
            .collect()
    }

    fn synced() -> MemberListState {
        let mut s = MemberListState::default();
        s.apply(&update(json!([{"op": "SYNC", "range": [0, 99], "items": [
            group("online", 2), member("10", "ann"), member("11", "bob"), group("offline", 1), member("12", "cy")
        ]}])));
        s
    }

    #[test]
    fn parses_members_names_presence_and_roles() {
        let d = json!({"guild_id": "1", "id": "everyone", "member_count": 9, "online_count": 4, "ops": [
            {"op": "SYNC", "range": [0, 99], "items": [
                group("online", 1),
                {"member": {"user": {"id": "10", "username": "ann", "global_name": "Ann", "avatar": "ab12", "bot": true},
                            "nick": "Annie", "roles": ["5", "6"], "presence": {"status": "dnd"}}},
                {"member": {"user": {"id": 11, "username": "bob"}, "presence": {"status": "invisible"}}}
            ]},
            {"op": "BOGUS"}
        ]});
        let u = parse_update(&d).unwrap();
        assert_eq!((u.member_count, u.online_count, u.ops.len()), (9, 4, 1));
        let ListOp::Sync { start, end, items } = &u.ops[0] else { panic!("not a sync") };
        assert_eq!((*start, *end, items.len()), (0, 99, 3));
        let ListItem::Member(a) = &items[1] else { panic!() };
        assert_eq!(a.name, "Annie");
        assert_eq!(a.presence, Presence::Dnd);
        assert!(a.bot);
        assert_eq!(a.role_ids, vec!["5", "6"]);
        let ListItem::Member(b) = &items[2] else { panic!() };
        assert_eq!((b.user_id.as_str(), b.name.as_str(), b.presence), ("11", "bob", Presence::Offline));
    }

    #[test]
    fn name_falls_back_from_nick_to_global_name_to_username() {
        let item = |m: Value| match parse_item(&json!({ "member": m })).unwrap() {
            ListItem::Member(m) => m.name,
            _ => unreachable!(),
        };
        assert_eq!(item(json!({"user": {"id": "1", "username": "u", "global_name": "G"}, "nick": ""})), "G");
        assert_eq!(item(json!({"user": {"id": "1", "username": "u", "global_name": null}})), "u");
        assert_eq!(item(json!({"user": {"id": "1"}})), "Unknown");
    }

    #[test]
    fn rejects_malformed_updates() {
        assert!(parse_update(&json!({"ops": []})).is_none());
        assert!(parse_update(&json!({"guild_id": "1"})).is_none());
        let u = update(json!([
            {"op": "DELETE"},
            {"op": "INSERT", "index": 1},
            {"op": "SYNC", "range": [5, 2], "items": []},
            {"op": "DELETE", "index": 99999999}
        ]));
        assert!(u.ops.is_empty());
    }

    #[test]
    fn sync_fills_the_range_and_rows_carry_their_group() {
        let s = synced();
        assert_eq!(names(&s), ["#online", "ann", "bob", "#offline", "cy"]);
        assert_eq!(s.len(), 5);
        let rows = s.rows();
        let ListRow::Member { group, .. } = rows[4] else { panic!() };
        assert_eq!(group, Some("offline"));
        let ListRow::Member { group, .. } = rows[1] else { panic!() };
        assert_eq!(group, Some("online"));
        assert_eq!((s.member_count, s.online_count), (5, 2));
    }

    #[test]
    fn insert_update_and_delete_move_the_following_rows() {
        let mut s = synced();
        s.apply(&update(json!([{"op": "INSERT", "index": 2, "item": member("13", "dee")}])));
        assert_eq!(names(&s), ["#online", "ann", "dee", "bob", "#offline", "cy"]);
        s.apply(&update(json!([{"op": "UPDATE", "index": 2, "item": member("13", "dee2")}])));
        assert_eq!(names(&s)[2], "dee2");
        s.apply(&update(json!([{"op": "DELETE", "index": 1}, {"op": "DELETE", "index": 1}])));
        assert_eq!(names(&s), ["#online", "bob", "#offline", "cy"]);
    }

    #[test]
    fn operations_past_the_end_leave_unknown_slots_that_rows_skip() {
        let mut s = MemberListState::default();
        s.apply(&update(json!([{"op": "UPDATE", "index": 3, "item": member("1", "x")}])));
        assert_eq!(s.len(), 4);
        assert_eq!(names(&s), ["x"]);
        s.apply(&update(json!([{"op": "INSERT", "index": 6, "item": member("2", "y")}])));
        assert_eq!(s.len(), 7);
        s.apply(&update(json!([{"op": "DELETE", "index": 50}])));
        assert_eq!(s.len(), 7);
    }

    #[test]
    fn invalidate_forgets_a_range() {
        let mut s = synced();
        s.apply(&update(json!([{"op": "INVALIDATE", "range": [1, 2]}])));
        assert_eq!(names(&s), ["#online", "#offline", "cy"]);
        assert_eq!(s.len(), 5);
        s.apply(&update(json!([{"op": "INVALIDATE", "range": [3, 500]}])));
        assert_eq!(names(&s), ["#online"]);
    }

    #[test]
    fn a_shorter_sync_ends_the_list_and_a_second_range_replaces_only_itself() {
        let mut s = synced();
        s.apply(&update(json!([{"op": "SYNC", "range": [0, 99], "items": [group("online", 1), member("10", "ann")]}])));
        assert_eq!(names(&s), ["#online", "ann"]);

        let mut s = MemberListState::default();
        let many: Vec<Value> = (0..4).map(|i| member(&i.to_string(), &format!("m{i}"))).collect();
        s.apply(&update(json!([{"op": "SYNC", "range": [0, 3], "items": many}])));
        s.apply(&update(json!([{"op": "SYNC", "range": [4, 5], "items": [member("8", "m8")]}])));
        assert_eq!(names(&s), ["m0", "m1", "m2", "m3", "m8"]);
        s.apply(&update(json!([{"op": "SYNC", "range": [1, 2], "items": [member("20", "n1"), member("21", "n2")]}])));
        assert_eq!(names(&s), ["m0", "n1", "n2", "m3", "m8"]);
    }

    #[test]
    fn a_different_list_id_starts_over() {
        let mut s = synced();
        let other = parse_update(&json!({"guild_id": "1", "id": "abc", "member_count": 1, "online_count": 1, "ops": [
            {"op": "SYNC", "range": [0, 9], "items": [member("99", "zed")]}
        ]}))
        .unwrap();
        s.apply(&other);
        assert_eq!(names(&s), ["zed"]);
        assert_eq!(s.list_id, "abc");
    }

    #[test]
    fn the_list_is_capped() {
        let mut s = MemberListState::default();
        s.apply(&update(json!([{"op": "UPDATE", "index": 19_999, "item": member("1", "x")}])));
        assert_eq!(s.len(), 20_000);
        s.apply(&update(json!([{"op": "INSERT", "index": 19_999, "item": member("2", "y")}])));
        assert_eq!(s.len(), 20_000);
        assert!(parse_update(&json!({"guild_id": "1", "ops": [{"op": "DELETE", "index": 20_000}]})).unwrap().ops.is_empty());
    }

    #[test]
    fn subscribe_payload_asks_for_the_first_rows_of_the_channel() {
        let p = subscribe_payload("5", "7", 99);
        assert_eq!(p["op"], 14);
        assert_eq!(p["d"]["guild_id"], "5");
        assert_eq!(p["d"]["channels"]["7"], json!([[0, 99]]));
    }

    #[test]
    fn member_colour_is_the_one_of_the_highest_coloured_role() {
        let roles = parse_roles(&[
            json!({"id": "1", "name": "Mod", "color": 0xFF0000, "position": 5}),
            json!({"id": "2", "name": "Admin", "color": 0, "position": 9}),
            json!({"id": "3", "name": "Fun", "color": 0x00FF00, "position": 2}),
        ]);
        let mut m = MemberEntry {
            user_id: "10".into(),
            name: "ann".into(),
            avatar_hash: String::new(),
            presence: Presence::Online,
            bot: false,
            role_ids: vec!["3".into(), "2".into(), "1".into(), "404".into()],
        };
        assert_eq!(member_color(&roles, &m), Some(0xFF0000));
        m.role_ids = vec!["2".into()];
        assert_eq!(member_color(&roles, &m), None);
        assert_eq!(group_title(&roles, "online"), "Online");
        assert_eq!(group_title(&roles, "1"), "Mod");
        assert_eq!(group_title(&roles, "404"), "Role");
    }

    #[test]
    fn avatar_urls_are_only_built_from_valid_parts() {
        let mut m = MemberEntry {
            user_id: "10".into(),
            name: "a".into(),
            avatar_hash: "a_0f1e".into(),
            presence: Presence::Online,
            bot: false,
            role_ids: vec![],
        };
        assert_eq!(avatar_url(&m), "https://cdn.discordapp.com/avatars/10/a_0f1e.webp?size=64");
        m.avatar_hash = "../x".into();
        assert_eq!(avatar_url(&m), "");
        m.avatar_hash = String::new();
        assert_eq!(avatar_url(&m), "");
    }
}
