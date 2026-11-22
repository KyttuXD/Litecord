//! Pure helpers for the user profile popout (no UI types, easy to unit test).

use serde_json::Value;

/// Milliseconds between the Unix epoch and the Discord epoch (2015-01-01).
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// Unix timestamp in milliseconds embedded in a snowflake id.
pub fn snowflake_to_timestamp(id: &str) -> Option<u64> {
    let raw: u64 = id.trim().parse().ok()?;
    Some((raw >> 22) + DISCORD_EPOCH_MS)
}

/// Formats Unix milliseconds as "Mon D, YYYY" (UTC), e.g. "Jan 1, 2015".
pub fn format_date(unix_ms: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // Civil-from-days algorithm (Howard Hinnant), valid for dates after 1970.
    let days = (unix_ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{} {}, {}", MONTHS[(month - 1) as usize], day, year)
}

/// Account creation date computed from the user id, or an empty string.
pub fn account_created_label(id: &str) -> String {
    snowflake_to_timestamp(id).map(format_date).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedProfile {
    pub id: String,
    pub username: String,
    pub global_name: String,
    pub nick: String,
    pub bio: String,
    pub avatar_hash: String,
    /// 0xRRGGBB banner / accent colour, when the account has one.
    pub banner_color: Option<u32>,
    pub role_ids: Vec<String>,
    pub mutual_guilds: usize,
    pub is_bot: bool,
}

impl ParsedProfile {
    /// Best name to show: server nickname, then global name, then username.
    pub fn display_name(&self) -> &str {
        [&self.nick, &self.global_name, &self.username]
            .into_iter()
            .map(|s| s.as_str())
            .find(|s| !s.is_empty())
            .unwrap_or("Unknown user")
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn parse_hex_color(s: &str) -> Option<u32> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// Parses the body of `GET /users/{id}/profile`. Returns `None` without a user id.
pub fn parse_profile(json: &Value) -> Option<ParsedProfile> {
    let user = &json["user"];
    let id = text(&user["id"]);
    if id.is_empty() {
        return None;
    }

    let bio = {
        let from_profile = text(&json["user_profile"]["bio"]);
        if from_profile.is_empty() {
            text(&user["bio"])
        } else {
            from_profile
        }
    };

    let banner_color = [
        &json["user_profile"]["accent_color"],
        &user["accent_color"],
    ]
    .into_iter()
    .find_map(|v| v.as_u64())
    .map(|c| (c & 0xFF_FFFF) as u32)
    .or_else(|| user["banner_color"].as_str().and_then(parse_hex_color));

    let role_ids = json["guild_member"]["roles"]
        .as_array()
        .map(|roles| {
            roles
                .iter()
                .filter_map(|r| r.as_str())
                .map(|r| r.to_string())
                .collect()
        })
        .unwrap_or_default();

    Some(ParsedProfile {
        id,
        username: text(&user["username"]),
        global_name: text(&user["global_name"]),
        nick: text(&json["guild_member"]["nick"]),
        bio,
        avatar_hash: text(&user["avatar"]),
        banner_color,
        role_ids,
        mutual_guilds: json["mutual_guilds"].as_array().map(|a| a.len()).unwrap_or(0),
        is_bot: user["bot"].as_bool().unwrap_or(false),
    })
}

/// CDN URL for a profile avatar, or an empty string when the user has none.
pub fn avatar_url(user_id: &str, hash: &str) -> String {
    let hex = hash.strip_prefix("a_").unwrap_or(hash);
    if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return String::new();
    }
    format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=128", user_id, hash)
}

/// "1 mutual server" / "N mutual servers".
pub fn mutual_servers_label(count: usize) -> String {
    if count == 1 {
        "1 mutual server".to_string()
    } else {
        format!("{} mutual servers", count)
    }
}

/// Default banner when the account has no colour: a stable pick from the id.
pub fn fallback_banner_color(user_id: &str) -> u32 {
    const PALETTE: [u32; 5] = [0x5865F2, 0x3BA55D, 0xFAA81A, 0xEB459E, 0xED4245];
    let n: u64 = user_id.trim().parse().unwrap_or(0);
    PALETTE[(n % PALETTE.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snowflake_timestamp_known_value() {
        // 175928847299117063 -> 2016-04-30 11:18:25.796 UTC (Discord docs example).
        assert_eq!(snowflake_to_timestamp("175928847299117063"), Some(1_462_015_105_796));
        assert_eq!(snowflake_to_timestamp("0"), Some(DISCORD_EPOCH_MS));
        assert_eq!(snowflake_to_timestamp("abc"), None);
        assert_eq!(snowflake_to_timestamp(""), None);
    }

    #[test]
    fn date_formatting() {
        assert_eq!(format_date(0), "Jan 1, 1970");
        assert_eq!(format_date(DISCORD_EPOCH_MS), "Jan 1, 2015");
        assert_eq!(format_date(1_462_015_105_796), "Apr 30, 2016");
        // Leap day.
        assert_eq!(format_date(1_709_164_800_000), "Feb 29, 2024");
        assert_eq!(account_created_label("175928847299117063"), "Apr 30, 2016");
        assert_eq!(account_created_label("x"), "");
    }

    #[test]
    fn parses_full_profile() {
        let v = json!({
            "user": {"id": "42", "username": "kyttu", "global_name": "Kyttu", "avatar": "ab12", "bot": false},
            "user_profile": {"bio": "hello", "accent_color": 16711680},
            "guild_member": {"nick": "Nicky", "roles": ["1", "2"]},
            "mutual_guilds": [{"id": "9"}, {"id": "10"}]
        });
        let p = parse_profile(&v).unwrap();
        assert_eq!(p.id, "42");
        assert_eq!(p.display_name(), "Nicky");
        assert_eq!(p.bio, "hello");
        assert_eq!(p.banner_color, Some(0xFF0000));
        assert_eq!(p.role_ids, vec!["1", "2"]);
        assert_eq!(p.mutual_guilds, 2);
        assert_eq!(p.avatar_hash, "ab12");
    }

    #[test]
    fn parses_minimal_profile_and_fallbacks() {
        let v = json!({"user": {"id": "7", "username": "bob", "bio": "old bio", "banner_color": "#00ff10"}});
        let p = parse_profile(&v).unwrap();
        assert_eq!(p.display_name(), "bob");
        assert_eq!(p.bio, "old bio");
        assert_eq!(p.banner_color, Some(0x00FF10));
        assert!(p.role_ids.is_empty());
        assert_eq!(p.mutual_guilds, 0);
        assert!(parse_profile(&json!({"user": {}})).is_none());
        assert!(parse_profile(&json!({})).is_none());
    }

    #[test]
    fn labels_and_urls() {
        assert_eq!(mutual_servers_label(0), "0 mutual servers");
        assert_eq!(mutual_servers_label(1), "1 mutual server");
        assert_eq!(mutual_servers_label(3), "3 mutual servers");
        assert_eq!(avatar_url("1", "a_ff"), "https://cdn.discordapp.com/avatars/1/a_ff.png?size=128");
        assert_eq!(avatar_url("1", ""), "");
        assert_eq!(avatar_url("1", "../x"), "");
        assert!(fallback_banner_color("5") <= 0xFFFFFF);
    }
}
