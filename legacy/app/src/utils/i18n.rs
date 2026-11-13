//! Stand-in for src/utils/i18n.rs: the language list and the system-language guess.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Auto,
    English,
    Portuguese,
    Spanish,
    German,
    French,
    Russian,
    Japanese,
}

pub fn detect_os_language() -> Language {
    let lang = std::env::var("LANG").or_else(|_| std::env::var("LC_ALL")).unwrap_or_default().to_lowercase();
    match lang.get(..2) {
        Some("pt") => Language::Portuguese,
        Some("es") => Language::Spanish,
        Some("de") => Language::German,
        Some("fr") => Language::French,
        Some("ru") => Language::Russian,
        Some("ja") => Language::Japanese,
        _ => Language::English,
    }
}
