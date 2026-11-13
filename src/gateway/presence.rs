//! The user's own presence (online / idle / dnd / invisible): kept for the session so
//! IDENTIFY honours it, and turned into gateway op 3 payloads.

use serde_json::{json, Value};
use std::sync::Mutex;

static MY_STATUS: Mutex<String> = Mutex::new(String::new());

/// Maps anything the UI or Discord may send to one of the four statuses the gateway
/// accepts. Unknown values fall back to "online".
pub fn normalize_status(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "idle" | "away" => "idle",
        "dnd" | "do_not_disturb" | "busy" => "dnd",
        "invisible" | "offline" => "invisible",
        _ => "online",
    }
}

/// Op 3 (presence update) frame for the given status.
pub fn build_presence_payload(status: &str) -> Value {
    json!({
        "op": 3,
        "d": {
            "status": normalize_status(status),
            "since": 0,
            "activities": [],
            "afk": false
        }
    })
}

pub fn set_my_status(status: &str) {
    if let Ok(mut s) = MY_STATUS.lock() {
        *s = normalize_status(status).to_string();
    }
}

/// The status chosen by the user, "online" until one is set.
pub fn get_my_status() -> String {
    match MY_STATUS.lock() {
        Ok(s) => normalize_status(&s).to_string(),
        Err(_) => "online".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_status_accepts_known_values_and_defaults_to_online() {
        assert_eq!(normalize_status("idle"), "idle");
        assert_eq!(normalize_status(" DND "), "dnd");
        assert_eq!(normalize_status("invisible"), "invisible");
        assert_eq!(normalize_status("offline"), "invisible");
        assert_eq!(normalize_status("online"), "online");
        assert_eq!(normalize_status(""), "online");
        assert_eq!(normalize_status("whatever"), "online");
    }

    #[test]
    fn presence_payload_is_op_3_with_normalized_status() {
        let p = build_presence_payload("Idle");
        assert_eq!(p["op"], 3);
        assert_eq!(p["d"]["status"], "idle");
        assert_eq!(p["d"]["afk"], false);
        assert!(p["d"]["activities"].as_array().unwrap().is_empty());
        assert_eq!(build_presence_payload("bogus")["d"]["status"], "online");
    }
}
