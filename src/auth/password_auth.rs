//! Login with email (or phone number) and password, as on discord.com, with the second
//! step an account may ask for: a code from an authenticator app, a code sent by SMS, or a
//! backup code.
//!
//! The password goes to `https://discord.com/api/v9/auth/login` over TLS and nowhere else:
//! it is not kept, logged or written to disk. What comes back and is kept is the same
//! session token the QR and token logins produce.
//!
//! Discord may answer a login from a new device with a captcha, or with a "check your
//! e-mail to verify this login" notice. A captcha cannot be shown in this app, and is not
//! worked around: the caller gets an error that says so, and the QR login remains.

use serde_json::{json, Value};

const API: &str = "https://discord.com/api/v9";

/// What Discord wants after the password was accepted.
#[derive(Debug, Clone)]
pub enum LoginStep {
    /// Logged in: the session token.
    Token(String),
    /// A second step is needed; `ticket` goes back with the code.
    SecondStep { ticket: String, totp: bool, sms: bool, backup: bool },
}

/// Which kind of code is being sent for the second step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    /// Six digits from an authenticator app.
    Totp,
    /// The code Discord sent by SMS (ask for it with [`send_sms`] first).
    Sms,
    /// One of the eight-character backup codes.
    Backup,
}

impl CodeKind {
    fn path(self) -> &'static str {
        match self {
            CodeKind::Totp => "totp",
            CodeKind::Sms => "sms",
            CodeKind::Backup => "backup",
        }
    }
}

async fn post(path: &str, body: Value) -> Result<(u16, Value), String> {
    let client = crate::utils::build_generic_http_client();
    let response = client
        .post(format!("{API}{path}"))
        .header("Content-Type", "application/json")
        .header("Origin", "https://discord.com")
        .header("Referer", "https://discord.com/login")
        .header("X-Super-Properties", crate::utils::client_identity::super_properties_header())
        .json(&body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("Could not reach Discord: {e}"))?;
    let status = response.status().as_u16();
    // Error replies are JSON too; an empty or odd body becomes Null.
    let json = response.json::<Value>().await.unwrap_or(Value::Null);
    Ok((status, json))
}

/// Collects the `message` texts of Discord's nested `errors` object.
fn error_messages(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(list) = map.get("_errors").and_then(|e| e.as_array()) {
                for item in list {
                    if let Some(message) = item["message"].as_str() {
                        out.push(message.to_string());
                    }
                }
            }
            for (key, child) in map {
                if key != "_errors" {
                    error_messages(child, out);
                }
            }
        }
        Value::Array(list) => list.iter().for_each(|child| error_messages(child, out)),
        _ => {}
    }
}

/// A sentence for the person at the keyboard, from a failed reply.
fn describe_failure(status: u16, json: &Value) -> String {
    if json.get("captcha_key").is_some() || json.get("captcha_sitekey").is_some() {
        return "Discord asks for a captcha for this login, which this app cannot show. Use the QR code login instead (or log in once on discord.com from this network and try again).".to_string();
    }
    let mut messages = Vec::new();
    error_messages(&json["errors"], &mut messages);
    if !messages.is_empty() {
        // The same sentence often comes once per field.
        messages.dedup();
        return messages.join(" ");
    }
    if let Some(message) = json["message"].as_str() {
        if status == 429 {
            let wait = json["retry_after"].as_f64().unwrap_or(0.0).ceil() as u64;
            return format!("Too many attempts. Try again in {wait} seconds.");
        }
        return message.to_string();
    }
    format!("Discord refused the login (HTTP {status}).")
}

fn token_from(json: &Value) -> Option<String> {
    json["token"].as_str().filter(|t| !t.is_empty()).map(|t| t.to_string())
}

/// First step: email or phone number, and password.
pub async fn login(login: &str, password: &str) -> Result<LoginStep, String> {
    let (status, json) = post(
        "/auth/login",
        json!({
            "login": login.trim(),
            "password": password,
            "undelete": false,
            "login_source": null,
            "gift_code_sku_id": null,
        }),
    )
    .await?;
    if let Some(token) = token_from(&json) {
        return Ok(LoginStep::Token(token));
    }
    if json["mfa"].as_bool() == Some(true) {
        if let Some(ticket) = json["ticket"].as_str() {
            return Ok(LoginStep::SecondStep {
                ticket: ticket.to_string(),
                totp: json["totp"].as_bool().unwrap_or(false),
                sms: json["sms"].as_bool().unwrap_or(false),
                backup: json["backup"].as_bool().unwrap_or(false),
            });
        }
    }
    Err(describe_failure(status, &json))
}

/// Asks Discord to send the SMS code. Returns the hidden phone number it was sent to
/// (for example `+*******12`), to show to the user.
pub async fn send_sms(ticket: &str) -> Result<String, String> {
    let (status, json) = post("/auth/mfa/sms/send", json!({ "ticket": ticket })).await?;
    match json["phone"].as_str() {
        Some(phone) => Ok(phone.to_string()),
        None if (200..300).contains(&status) => Ok(String::new()),
        None => Err(describe_failure(status, &json)),
    }
}

/// Second step: the code. Returns the session token.
pub async fn finish(kind: CodeKind, ticket: &str, code: &str) -> Result<String, String> {
    // Codes are often typed with spaces or a dash (backup codes are shown as "abcd-efgh").
    let code: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if code.is_empty() {
        return Err("Enter the code first.".to_string());
    }
    let (status, json) = post(
        &format!("/auth/mfa/{}", kind.path()),
        json!({
            "code": code,
            "ticket": ticket,
            "login_source": null,
            "gift_code_sku_id": null,
        }),
    )
    .await?;
    token_from(&json).ok_or_else(|| describe_failure(status, &json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_captcha_reply_says_so_and_points_to_the_qr_login() {
        let reply = json!({ "captcha_key": ["captcha-required"], "captcha_sitekey": "x", "captcha_service": "hcaptcha" });
        let text = describe_failure(400, &reply);
        assert!(text.contains("captcha") && text.contains("QR"), "{text}");
    }

    #[test]
    fn field_errors_are_read_from_the_nested_object() {
        let reply = json!({
            "message": "Invalid Form Body",
            "code": 50035,
            "errors": {
                "login": { "_errors": [{ "code": "INVALID_LOGIN", "message": "Login or password is invalid." }] },
                "password": { "_errors": [{ "code": "INVALID_LOGIN", "message": "Login or password is invalid." }] }
            }
        });
        assert_eq!(describe_failure(400, &reply), "Login or password is invalid.");
    }

    #[test]
    fn a_rate_limit_gives_the_wait_in_whole_seconds() {
        let reply = json!({ "message": "You are being rate limited.", "retry_after": 12.3 });
        assert_eq!(describe_failure(429, &reply), "Too many attempts. Try again in 13 seconds.");
    }

    #[test]
    fn a_plain_message_is_passed_on() {
        assert_eq!(describe_failure(400, &json!({ "message": "Invalid two-factor code" })), "Invalid two-factor code");
        assert_eq!(describe_failure(500, &Value::Null), "Discord refused the login (HTTP 500).");
    }
}
