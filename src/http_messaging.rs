//! Message actions beyond plain sending: replies, editing and deleting, reactions, the
//! typing notice, marking a channel as read, and file uploads. Used by the main app and by
//! the legacy edition (legacy/app).

use serde_json::{json, Value};

use crate::http::DiscordHttpClient;

const API: &str = "https://discord.com/api/v10";

/// Discord's limit for an upload without Nitro.
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

/// One reaction under a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionData {
    /// What the reaction endpoints take: the emoji itself, or `name:id` for a custom one.
    pub key: String,
    /// What to show: the emoji itself, or `:name:` for a custom one.
    pub label: String,
    pub count: u32,
    /// Whether the logged-in user is one of those who reacted.
    pub me: bool,
}

/// Key and label (see [`ReactionData`]) of the `emoji` object Discord sends.
pub fn reaction_key_and_label(emoji: &Value) -> (String, String) {
    let name = emoji["name"].as_str().unwrap_or("");
    match emoji["id"].as_str() {
        Some(id) if !id.is_empty() => (format!("{name}:{id}"), format!(":{name}:")),
        _ => (name.to_string(), name.to_string()),
    }
}

/// The reactions of a message object from the REST API or a gateway event.
pub fn reactions_from_message(message: &Value) -> Vec<ReactionData> {
    message["reactions"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|r| {
                    let (key, label) = reaction_key_and_label(&r["emoji"]);
                    ReactionData { key, label, count: r["count"].as_u64().unwrap_or(1) as u32, me: r["me"].as_bool().unwrap_or(false) }
                })
                .filter(|r| !r.key.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Applies one reaction event to a message's list. `by_me`: the event is about the logged-in user.
pub fn apply_reaction(list: &mut Vec<ReactionData>, key: &str, label: &str, added: bool, by_me: bool) {
    match list.iter().position(|r| r.key == key) {
        Some(i) if added => {
            list[i].count += 1;
            list[i].me |= by_me;
        }
        Some(i) => {
            list[i].count = list[i].count.saturating_sub(1);
            if by_me {
                list[i].me = false;
            }
            if list[i].count == 0 {
                list.remove(i);
            }
        }
        None if added => list.push(ReactionData { key: key.to_string(), label: label.to_string(), count: 1, me: by_me }),
        None => {}
    }
}

/// A reaction key as a URL path segment (every byte outside `A-Z a-z 0-9 - . _ ~` escaped).
pub fn reaction_path(key: &str) -> String {
    let mut out = String::new();
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A file name that is safe inside a multipart header: no path, quotes or line breaks.
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(|c| c == '/' || c == '\\').next().unwrap_or(name);
    let clean: String = base.chars().map(|c| if c == '"' || c.is_control() { '_' } else { c }).collect();
    if clean.trim().is_empty() {
        "file".to_string()
    } else {
        clean
    }
}

/// The body of a `multipart/form-data` request with the message as `payload_json` and one file.
pub fn multipart_body(boundary: &str, payload_json: &str, file_name: &str, file: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(file.len() + payload_json.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n").as_bytes());
    body.extend_from_slice(payload_json.as_bytes());
    body.extend_from_slice(
        format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files[0]\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n",
            safe_file_name(file_name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// A boundary that does not occur in the file.
fn pick_boundary(file: &[u8]) -> String {
    let mut n: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9);
    loop {
        let boundary = format!("----litecord{n:016x}");
        let needle = boundary.as_bytes();
        if !file.windows(needle.len()).any(|w| w == needle) {
            return boundary;
        }
        n = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    }
}

/// What a message body with an optional reply looks like.
fn message_payload(content: &str, reply_to: Option<&str>) -> Value {
    let mut payload = json!({ "content": content });
    if let Some(id) = reply_to.filter(|id| !id.is_empty()) {
        payload["message_reference"] = json!({ "message_id": id });
        // Do not ping the author of the message replied to unless they are mentioned.
        payload["allowed_mentions"] = json!({ "parse": ["users", "roles", "everyone"], "replied_user": false });
    }
    payload
}

/// Turns a reply into `Ok(())` or a sentence with Discord's own reason.
async fn finish(action: &str, result: Result<reqwest::Response, reqwest::Error>) -> Result<(), String> {
    let response = result.map_err(|e| format!("{action}: could not reach Discord ({e})"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let reason = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|body| body["message"].as_str().map(|m| m.to_string()))
        .unwrap_or_default();
    Err(if reason.is_empty() { format!("{action}: Discord answered {status}") } else { format!("{action}: {reason} ({status})") })
}

impl DiscordHttpClient {
    /// Sends a message, as a reply to `reply_to` when that is given.
    pub async fn send_message_with(&self, channel_id: &str, content: &str, reply_to: Option<&str>) -> Result<(), String> {
        let url = format!("{API}/channels/{channel_id}/messages");
        finish("Message not sent", self.client.post(&url).json(&message_payload(content, reply_to)).send().await).await
    }

    pub async fn edit_message(&self, channel_id: &str, message_id: &str, content: &str) -> Result<(), String> {
        let url = format!("{API}/channels/{channel_id}/messages/{message_id}");
        finish("Message not edited", self.client.patch(&url).json(&json!({ "content": content })).send().await).await
    }

    pub async fn delete_message(&self, channel_id: &str, message_id: &str) -> Result<(), String> {
        let url = format!("{API}/channels/{channel_id}/messages/{message_id}");
        finish("Message not deleted", self.client.delete(&url).send().await).await
    }

    /// `key` as in [`ReactionData::key`].
    pub async fn add_reaction(&self, channel_id: &str, message_id: &str, key: &str) -> Result<(), String> {
        let url = format!("{API}/channels/{channel_id}/messages/{message_id}/reactions/{}/@me", reaction_path(key));
        finish("Reaction not added", self.client.put(&url).header("Content-Length", "0").send().await).await
    }

    pub async fn remove_reaction(&self, channel_id: &str, message_id: &str, key: &str) -> Result<(), String> {
        let url = format!("{API}/channels/{channel_id}/messages/{message_id}/reactions/{}/@me", reaction_path(key));
        finish("Reaction not removed", self.client.delete(&url).send().await).await
    }

    /// Shows "… is typing" to the others for about ten seconds.
    pub async fn trigger_typing(&self, channel_id: &str) -> Result<(), String> {
        let url = format!("{API}/channels/{channel_id}/typing");
        finish("Typing notice not sent", self.client.post(&url).header("Content-Length", "0").send().await).await
    }

    /// Marks the channel as read up to `message_id`.
    pub async fn ack_message(&self, channel_id: &str, message_id: &str) -> Result<(), String> {
        let url = format!("{API}/channels/{channel_id}/messages/{message_id}/ack");
        finish("Channel not marked as read", self.client.post(&url).json(&json!({ "token": null })).send().await).await
    }

    /// Sends the file at `path` (read here, checked against the size limit first).
    pub async fn send_path(&self, channel_id: &str, reply_to: Option<&str>, path: &std::path::Path) -> Result<(), String> {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".to_string());
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("File not sent: cannot read {} ({e})", path.display()))?;
        if !meta.is_file() {
            return Err(format!("File not sent: {} is not a regular file.", path.display()));
        }
        let size = meta.len();
        if size > MAX_UPLOAD_BYTES as u64 {
            return Err(format!("File not sent: it is {:.1} MB and Discord takes at most {} MB.", size as f64 / (1024.0 * 1024.0), MAX_UPLOAD_BYTES / (1024 * 1024)));
        }
        let bytes = tokio::fs::read(path).await.map_err(|e| format!("File not sent: cannot read {} ({e})", path.display()))?;
        self.send_file(channel_id, "", reply_to, &name, bytes).await
    }

    /// Sends one file, with an optional text and reply.
    pub async fn send_file(&self, channel_id: &str, content: &str, reply_to: Option<&str>, file_name: &str, file: Vec<u8>) -> Result<(), String> {
        if file.len() > MAX_UPLOAD_BYTES {
            return Err(format!(
                "File not sent: it is {:.1} MB and Discord takes at most {} MB.",
                file.len() as f64 / (1024.0 * 1024.0),
                MAX_UPLOAD_BYTES / (1024 * 1024)
            ));
        }
        let mut payload = message_payload(content, reply_to);
        payload["attachments"] = json!([{ "id": 0, "filename": safe_file_name(file_name) }]);
        let boundary = pick_boundary(&file);
        let body = multipart_body(&boundary, &payload.to_string(), file_name, &file);
        let url = format!("{API}/channels/{channel_id}/messages");
        let request = self
            .client
            .post(&url)
            .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
            .timeout(std::time::Duration::from_secs(120))
            .body(body);
        finish("File not sent", request.send().await).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_and_custom_emoji_become_keys_labels_and_paths() {
        assert_eq!(reaction_key_and_label(&json!({ "id": null, "name": "👍" })), ("👍".to_string(), "👍".to_string()));
        assert_eq!(reaction_key_and_label(&json!({ "id": "123", "name": "party" })), ("party:123".to_string(), ":party:".to_string()));
        assert_eq!(reaction_path("👍"), "%F0%9F%91%8D");
        assert_eq!(reaction_path("party:123"), "party%3A123");
    }

    #[test]
    fn reactions_are_read_from_a_message() {
        let message = json!({ "reactions": [
            { "count": 3, "me": true, "emoji": { "id": null, "name": "❤️" } },
            { "count": 1, "me": false, "emoji": { "id": "9", "name": "blob" } }
        ]});
        let list = reactions_from_message(&message);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].count, list[0].me, list[0].label.as_str()), (3, true, "❤️"));
        assert_eq!((list[1].key.as_str(), list[1].label.as_str()), ("blob:9", ":blob:"));
        assert!(reactions_from_message(&json!({})).is_empty());
    }

    #[test]
    fn reaction_events_add_count_and_remove() {
        let mut list = Vec::new();
        apply_reaction(&mut list, "👍", "👍", true, false);
        apply_reaction(&mut list, "👍", "👍", true, true);
        assert_eq!((list[0].count, list[0].me), (2, true));
        apply_reaction(&mut list, "👍", "👍", false, true);
        assert_eq!((list[0].count, list[0].me), (1, false));
        apply_reaction(&mut list, "👍", "👍", false, false);
        assert!(list.is_empty());
        apply_reaction(&mut list, "x", "x", false, false);
        assert!(list.is_empty());
    }

    #[test]
    fn a_reply_names_the_message_and_does_not_ping() {
        let payload = message_payload("hi", Some("42"));
        assert_eq!(payload["message_reference"]["message_id"], "42");
        assert_eq!(payload["allowed_mentions"]["replied_user"], false);
        assert!(message_payload("hi", None).get("message_reference").is_none());
        assert!(message_payload("hi", Some("")).get("message_reference").is_none());
    }

    #[test]
    fn the_upload_body_holds_the_message_and_the_file_between_boundaries() {
        let body = multipart_body("XYZ", r#"{"content":"a"}"#, "/tmp/pic \"1\".png", b"\x00\x01DATA");
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with("--XYZ\r\nContent-Disposition: form-data; name=\"payload_json\""));
        assert!(text.contains("{\"content\":\"a\"}\r\n--XYZ\r\n"));
        assert!(text.contains("filename=\"pic _1_.png\""));
        assert!(body.ends_with(b"\x00\x01DATA\r\n--XYZ--\r\n"));
    }

    #[test]
    fn file_names_lose_their_path_and_unsafe_characters() {
        assert_eq!(safe_file_name("C:\\Users\\me\\a.txt"), "a.txt");
        assert_eq!(safe_file_name("/home/me/b\n.txt"), "b_.txt");
        assert_eq!(safe_file_name(""), "file");
    }

    #[test]
    fn the_boundary_is_not_in_the_file() {
        let boundary = pick_boundary(b"some file content");
        assert!(boundary.starts_with("----litecord"));
        assert!(!b"some file content".windows(boundary.len()).any(|w| w == boundary.as_bytes()));
    }
}
