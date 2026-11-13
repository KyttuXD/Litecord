//! The two avatar helpers of src/ui/message_grouping.rs the network code calls. The rest
//! of that file works on Slint models, which the legacy edition does not have.
use serde_json::Value;

pub fn valid_avatar_hash(hash: &str) -> bool {
    let hash = hash.strip_prefix("a_").unwrap_or(hash);
    !hash.is_empty() && hash.len() <= 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn avatar_url_for_message(source: &Value) -> String {
    let user_id = source["author"]["id"].as_str().unwrap_or("");
    if user_id.is_empty() || !user_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return String::new();
    }
    let hash = source["author"]["avatar"].as_str().unwrap_or("");
    if valid_avatar_hash(hash) {
        format!("https://cdn.discordapp.com/avatars/{user_id}/{hash}.webp?size=64")
    } else {
        String::new()
    }
}
