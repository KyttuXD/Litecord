//! Desktop notifications for direct messages and @mentions.
//!
//! No extra crate: every platform shells out to a tool it already has, best effort.
//! Linux uses `notify-send`, macOS `osascript`, Windows a PowerShell toast. A failure
//! to spawn is logged at debug level and never propagates.

use std::sync::atomic::AtomicBool;

/// True while the main window has keyboard focus. Updated from the winit event hook.
pub static WINDOW_FOCUSED: AtomicBool = AtomicBool::new(true);

const MAX_BODY_CHARS: usize = 200;

/// Decides whether a new message deserves a desktop notification.
///
/// Silent when the user turned notifications off, wrote the message, the window is
/// focused, the channel is muted, or the channel is open on a visible window. Otherwise
/// only direct messages, mentions (user, @everyone or a role the user holds) and replies
/// to the user's messages that ping them notify.
#[allow(clippy::too_many_arguments)]
pub fn should_notify(
    enabled: bool,
    is_self: bool,
    is_dm: bool,
    mentions_me: bool,
    reply_to_me: bool,
    window_focused: bool,
    window_visible: bool,
    channel_open: bool,
    channel_muted: bool,
) -> bool {
    if !enabled || is_self || window_focused || channel_muted {
        return false;
    }
    if channel_open && window_visible {
        return false;
    }
    is_dm || mentions_me || reply_to_me
}

/// True when the message pings the current user: listed in `mentions`, or @everyone.
#[cfg_attr(not(test), allow(dead_code))]
pub fn mentions_user(mention_ids: &[String], mention_everyone: bool, my_user_id: &str) -> bool {
    if mention_everyone {
        return true;
    }
    !my_user_id.is_empty() && mention_ids.iter().any(|id| id == my_user_id)
}

fn flatten_whitespace(input: &str) -> String {
    input
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Escapes text for an AppleScript double-quoted string literal. Line breaks and other
/// control characters become spaces so the literal stays on one line.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn escape_applescript(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    for c in flatten_whitespace(input).chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
}

/// Escapes text for a PowerShell single-quoted string literal. PowerShell treats the
/// typographic single quotes like `'`, so those are doubled as well. Nothing is
/// expanded inside single quotes, so backslashes and `$` stay as they are.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn escape_powershell(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    for c in flatten_whitespace(input).chars() {
        match c {
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => {
                out.push(c);
                out.push(c);
            }
            other => out.push(other),
        }
    }
    out
}

/// Escapes the characters notification daemons read as markup (`&`, `<`, `>`).
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub fn escape_markup(input: &str) -> String {
    input.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn truncate_chars(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max).collect();
    out.push('\u{2026}');
    out
}

/// Shows a notification without blocking the caller. Never panics.
pub fn show(title: &str, body: &str) {
    let title = truncate_chars(title.trim(), 80);
    let body = truncate_chars(body.trim(), MAX_BODY_CHARS);
    let spawned = std::thread::Builder::new()
        .name("desktop-notification".into())
        .spawn(move || {
            if let Err(e) = send(&title, &body) {
                log::debug!("desktop notification not shown: {}", e);
            }
        });
    if let Err(e) = spawned {
        log::debug!("desktop notification thread not started: {}", e);
    }
}

#[cfg(target_os = "linux")]
fn send(title: &str, body: &str) -> std::io::Result<()> {
    std::process::Command::new("notify-send")
        .args(["--app-name", "Litecord", "--"])
        .arg(escape_markup(&flatten_whitespace(title)))
        .arg(escape_markup(&flatten_whitespace(body)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn send(title: &str, body: &str) -> std::io::Result<()> {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_applescript(body),
        escape_applescript(title)
    );
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| ())
}

#[cfg(windows)]
fn send(title: &str, body: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = format!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
         $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
         $n = $t.GetElementsByTagName('text'); \
         $n.Item(0).AppendChild($t.CreateTextNode('{}')) | Out-Null; \
         $n.Item(1).AppendChild($t.CreateTextNode('{}')) | Out-Null; \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show([Windows.UI.Notifications.ToastNotification]::new($t))",
        escape_powershell(title),
        escape_powershell(body)
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command"])
        .arg(script)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| ())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn send(_title: &str, _body: &str) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type Case = (bool, bool, bool, bool, bool, bool, bool, bool, bool);

    // enabled, is_self, is_dm, mentions_me, reply_to_me, focused, visible, open, muted
    fn base() -> Case {
        (true, false, true, false, false, false, false, false, false)
    }

    fn run(t: Case) -> bool {
        should_notify(t.0, t.1, t.2, t.3, t.4, t.5, t.6, t.7, t.8)
    }

    #[test]
    fn reply_to_me_in_server_notifies() {
        let mut t = base();
        t.2 = false;
        assert!(!run(t));
        t.4 = true;
        assert!(run(t));
        t.8 = true;
        assert!(!run(t));
    }

    #[test]
    fn dm_while_unfocused_notifies() {
        assert!(run(base()));
    }

    #[test]
    fn mention_in_server_notifies() {
        let mut t = base();
        t.2 = false;
        t.3 = true;
        assert!(run(t));
    }

    #[test]
    fn plain_server_message_is_silent() {
        let mut t = base();
        t.2 = false;
        assert!(!run(t));
    }

    #[test]
    fn disabled_own_focused_and_muted_are_silent() {
        let mut t = base();
        t.0 = false;
        assert!(!run(t));
        let mut t = base();
        t.1 = true;
        assert!(!run(t));
        let mut t = base();
        t.5 = true;
        assert!(!run(t));
        let mut t = base();
        t.8 = true;
        assert!(!run(t));
    }

    #[test]
    fn open_channel_is_silent_only_while_window_visible() {
        let mut t = base();
        t.7 = true;
        t.6 = true;
        assert!(!run(t));
        t.6 = false;
        assert!(run(t));
    }

    #[test]
    fn markup_is_escaped() {
        assert_eq!(escape_markup("a < b & <@1>"), "a &lt; b &amp; &lt;@1&gt;");
    }

    #[test]
    fn mention_detection() {
        let ids = vec!["10".to_string(), "42".to_string()];
        assert!(mentions_user(&ids, false, "42"));
        assert!(!mentions_user(&ids, false, "43"));
        assert!(!mentions_user(&ids, false, ""));
        assert!(mentions_user(&[], true, "43"));
    }

    #[test]
    fn applescript_escapes_quotes_backslashes_newlines() {
        assert_eq!(escape_applescript(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(escape_applescript(r"C:\dir\file"), r"C:\\dir\\file");
        assert_eq!(escape_applescript("a\nb\r\nc"), "a b  c");
        assert_eq!(escape_applescript(r#"\" end"#), r#"\\\" end"#);
    }

    #[test]
    fn powershell_escapes_single_quotes_and_newlines() {
        assert_eq!(escape_powershell("it's"), "it''s");
        assert_eq!(escape_powershell("\u{2019}x\u{2018}"), "\u{2019}\u{2019}x\u{2018}\u{2018}");
        assert_eq!(escape_powershell("a\nb"), "a b");
        assert_eq!(escape_powershell(r#"C:\x "q" $env:A"#), r#"C:\x "q" $env:A"#);
    }

    #[test]
    fn truncation_counts_chars_not_bytes() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("ééééé", 3), "ééé\u{2026}");
    }
}
