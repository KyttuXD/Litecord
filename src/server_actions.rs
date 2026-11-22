//! Pure request builders for server and profile actions (leave, invite, join,
//! custom status, mark as read, mute). Nothing here touches the network, so the
//! shapes of the requests can be unit tested.

use serde_json::{json, Value};

const API_BASE: &str = "https://discord.com/api/v9";

/// HTTP verbs used by the server actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiMethod {
    Post,
    Patch,
    Delete,
}

/// A fully described API call: verb, URL and optional JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiRequest {
    pub method: ApiMethod,
    pub url: String,
    pub body: Option<Value>,
}

fn is_snowflake(id: &str) -> bool {
    !id.is_empty() && id.len() <= 24 && id.bytes().all(|b| b.is_ascii_digit())
}

fn check_id(kind: &str, id: &str) -> Result<(), String> {
    if is_snowflake(id) {
        Ok(())
    } else {
        Err(format!("Invalid {} id: {:?}", kind, id))
    }
}

/// DELETE /users/@me/guilds/{id}
pub fn leave_guild(guild_id: &str) -> Result<ApiRequest, String> {
    check_id("server", guild_id)?;
    Ok(ApiRequest {
        method: ApiMethod::Delete,
        url: format!("{}/users/@me/guilds/{}", API_BASE, guild_id),
        body: None,
    })
}

/// POST /channels/{id}/invites with a 24 hour lifetime.
pub fn create_invite(channel_id: &str) -> Result<ApiRequest, String> {
    check_id("channel", channel_id)?;
    Ok(ApiRequest {
        method: ApiMethod::Post,
        url: format!("{}/channels/{}/invites", API_BASE, channel_id),
        body: Some(json!({ "max_age": 86400 })),
    })
}

/// The shareable link for an invite code.
pub fn invite_url(code: &str) -> String {
    format!("https://discord.gg/{}", code)
}

/// Extracts the invite code from a bare code or from a discord.gg,
/// discord.com/invite or discordapp.com/invite link.
pub fn parse_invite_code(input: &str) -> Option<String> {
    let mut s = input.trim();
    if let Some(i) = s.find("://") {
        s = &s[i + 3..];
    }
    let s_lower = s.to_ascii_lowercase();
    for prefix in [
        "discord.gg/",
        "www.discord.gg/",
        "discord.com/invite/",
        "www.discord.com/invite/",
        "discordapp.com/invite/",
        "www.discordapp.com/invite/",
    ] {
        if s_lower.starts_with(prefix) {
            s = &s[prefix.len()..];
            break;
        }
    }
    let code = s.split(|c| c == '/' || c == '?' || c == '#').next().unwrap_or("");
    let valid = code.len() >= 2
        && code.len() <= 32
        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Some(code.to_string())
    } else {
        None
    }
}

/// POST /invites/{code}
pub fn join_invite(input: &str) -> Result<ApiRequest, String> {
    let code = parse_invite_code(input).ok_or_else(|| "Invalid invite code or link".to_string())?;
    Ok(ApiRequest {
        method: ApiMethod::Post,
        url: format!("{}/invites/{}", API_BASE, code),
        body: Some(json!({})),
    })
}

/// PATCH /users/@me/settings with a custom status. Empty text clears the status.
pub fn set_custom_status(
    text: &str,
    emoji_name: Option<&str>,
    expires_at: Option<&str>,
) -> ApiRequest {
    let text = text.trim();
    let custom_status = if text.is_empty() && emoji_name.is_none() {
        Value::Null
    } else {
        json!({
            "text": if text.is_empty() { Value::Null } else { Value::String(text.chars().take(128).collect()) },
            "emoji_name": emoji_name,
            "expires_at": expires_at,
        })
    };
    ApiRequest {
        method: ApiMethod::Patch,
        url: format!("{}/users/@me/settings", API_BASE),
        body: Some(json!({ "custom_status": custom_status })),
    }
}

/// POST /guilds/{id}/ack (mark every channel of the server as read)
pub fn mark_guild_read(guild_id: &str) -> Result<ApiRequest, String> {
    check_id("server", guild_id)?;
    Ok(ApiRequest {
        method: ApiMethod::Post,
        url: format!("{}/guilds/{}/ack", API_BASE, guild_id),
        body: Some(json!({})),
    })
}

/// PATCH /users/@me/guilds/{id}/settings muting or unmuting the whole server.
pub fn set_guild_muted(guild_id: &str, muted: bool) -> Result<ApiRequest, String> {
    check_id("server", guild_id)?;
    Ok(ApiRequest {
        method: ApiMethod::Patch,
        url: format!("{}/users/@me/guilds/{}/settings", API_BASE, guild_id),
        body: Some(json!({ "muted": muted })),
    })
}

/// PATCH /users/@me/guilds/{id}/settings muting or unmuting one channel.
pub fn set_channel_muted(guild_id: &str, channel_id: &str, muted: bool) -> Result<ApiRequest, String> {
    check_id("server", guild_id)?;
    check_id("channel", channel_id)?;
    let mut overrides = serde_json::Map::new();
    overrides.insert(channel_id.to_string(), json!({ "muted": muted }));
    Ok(ApiRequest {
        method: ApiMethod::Patch,
        url: format!("{}/users/@me/guilds/{}/settings", API_BASE, guild_id),
        body: Some(json!({ "channel_overrides": Value::Object(overrides) })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leave_is_delete_without_body() {
        let r = leave_guild("123").unwrap();
        assert_eq!(r.method, ApiMethod::Delete);
        assert_eq!(r.url, "https://discord.com/api/v9/users/@me/guilds/123");
        assert!(r.body.is_none());
    }

    #[test]
    fn rejects_bad_ids() {
        assert!(leave_guild("").is_err());
        assert!(leave_guild("1/../2").is_err());
        assert!(create_invite("abc").is_err());
        assert!(set_channel_muted("1", "x", true).is_err());
    }

    #[test]
    fn invite_has_one_day_lifetime() {
        let r = create_invite("42").unwrap();
        assert_eq!(r.method, ApiMethod::Post);
        assert_eq!(r.url, "https://discord.com/api/v9/channels/42/invites");
        assert_eq!(r.body, Some(json!({ "max_age": 86400 })));
        assert_eq!(invite_url("abcDEF"), "https://discord.gg/abcDEF");
    }

    #[test]
    fn parses_invite_forms() {
        assert_eq!(parse_invite_code("abcDEF1"), Some("abcDEF1".to_string()));
        assert_eq!(parse_invite_code(" https://discord.gg/abc-123 "), Some("abc-123".to_string()));
        assert_eq!(parse_invite_code("discord.gg/abc123?event=1"), Some("abc123".to_string()));
        assert_eq!(parse_invite_code("https://discord.com/invite/xyz789/"), Some("xyz789".to_string()));
        assert_eq!(parse_invite_code("http://www.discordapp.com/invite/Qq1"), Some("Qq1".to_string()));
        assert_eq!(parse_invite_code(""), None);
        assert_eq!(parse_invite_code("a"), None);
        assert_eq!(parse_invite_code("bad code"), None);
        assert_eq!(parse_invite_code("https://example.com/"), None);
    }

    #[test]
    fn join_builds_url_from_link() {
        let r = join_invite("https://discord.gg/hello").unwrap();
        assert_eq!(r.method, ApiMethod::Post);
        assert_eq!(r.url, "https://discord.com/api/v9/invites/hello");
        assert!(join_invite("???").is_err());
    }

    #[test]
    fn custom_status_set_and_clear() {
        let r = set_custom_status("  Playing  ", None, None);
        assert_eq!(r.method, ApiMethod::Patch);
        assert_eq!(r.url, "https://discord.com/api/v9/users/@me/settings");
        assert_eq!(
            r.body,
            Some(json!({ "custom_status": { "text": "Playing", "emoji_name": null, "expires_at": null } }))
        );
        let c = set_custom_status("   ", None, None);
        assert_eq!(c.body, Some(json!({ "custom_status": null })));
        let e = set_custom_status("brb", Some("x"), Some("2030-01-01T00:00:00.000Z"));
        assert_eq!(e.body.unwrap()["custom_status"]["expires_at"], "2030-01-01T00:00:00.000Z");
    }

    #[test]
    fn custom_status_text_is_capped() {
        let long = "a".repeat(300);
        let r = set_custom_status(&long, None, None);
        let t = r.body.unwrap()["custom_status"]["text"].as_str().unwrap().len();
        assert_eq!(t, 128);
    }

    #[test]
    fn ack_and_mute() {
        let a = mark_guild_read("9").unwrap();
        assert_eq!(a.method, ApiMethod::Post);
        assert_eq!(a.url, "https://discord.com/api/v9/guilds/9/ack");
        let m = set_guild_muted("9", true).unwrap();
        assert_eq!(m.method, ApiMethod::Patch);
        assert_eq!(m.url, "https://discord.com/api/v9/users/@me/guilds/9/settings");
        assert_eq!(m.body, Some(json!({ "muted": true })));
        let c = set_channel_muted("9", "5", false).unwrap();
        assert_eq!(c.body, Some(json!({ "channel_overrides": { "5": { "muted": false } } })));
    }
}
