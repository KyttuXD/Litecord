//! Stand-in for src/utils/i18n_ui.rs: the legacy edition is English only for now.
//! Only the strings the network code formats messages with.

pub struct UiStrings {
    pub open: &'static str,
    pub attachment_label: &'static str,
    pub time_in: &'static str,
    pub time_ago: &'static str,
    pub time_soon: &'static str,
    pub time_just_now: &'static str,
}

pub const EN_UI: UiStrings = UiStrings {
    open: "Open",
    attachment_label: "Attachment",
    time_in: "in {}",
    time_ago: "{} ago",
    time_soon: "in a moment",
    time_just_now: "just now",
};

pub fn ui_text() -> &'static UiStrings {
    &EN_UI
}
