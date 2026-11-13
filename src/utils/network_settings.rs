use std::sync::{Mutex, OnceLock};
use serde::{Deserialize, Serialize};
use log::{info, error};

pub const NETWORK_SETTINGS_FILE: &str = ".litecord_network_settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSettings {
    /// "off" | "http" | "socks5" | "system"
    pub proxy_mode: String,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_username: String,
    pub proxy_password: String,
    /// If enabled, tries to route media calls; false by default (keeps UDP direct for minimum latency)
    pub route_media: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            proxy_mode: "off".to_string(),
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 8080,
            proxy_username: String::new(),
            proxy_password: String::new(),
            route_media: false,
        }
    }
}

static NETWORK_SETTINGS: OnceLock<Mutex<NetworkSettings>> = OnceLock::new();

pub fn get_network_settings_store() -> &'static Mutex<NetworkSettings> {
    NETWORK_SETTINGS.get_or_init(|| {
        let settings = if let Ok(data) = std::fs::read_to_string(NETWORK_SETTINGS_FILE) {
            serde_json::from_str::<NetworkSettings>(&data).unwrap_or_default()
        } else {
            NetworkSettings::default()
        };
        info!("🌐 Network and proxy settings loaded: mode={}", settings.proxy_mode);
        Mutex::new(settings)
    })
}

pub fn get_network_settings() -> NetworkSettings {
    get_network_settings_store().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn save_network_settings(settings: &NetworkSettings) {
    {
        let mut store = get_network_settings_store().lock().unwrap_or_else(|e| e.into_inner());
        *store = settings.clone();
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(NETWORK_SETTINGS_FILE, json);
    }
}

pub fn update_network_settings(
    mode: String,
    host: String,
    port: u16,
    user: String,
    pass: String,
    route_media: bool,
) {
    let settings = NetworkSettings {
        proxy_mode: mode,
        proxy_host: host,
        proxy_port: port,
        proxy_username: user,
        proxy_password: pass,
        route_media,
    };
    save_network_settings(&settings);
    info!("🌐 Proxy settings saved: mode={}, host={}:{}", settings.proxy_mode, settings.proxy_host, settings.proxy_port);
}

/// Applies the active proxy configuration to any `reqwest::ClientBuilder`
pub fn apply_proxy_to_builder(mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let settings = get_network_settings();
    match settings.proxy_mode.as_str() {
        "http" | "https" => {
            let url = format!("http://{}:{}", settings.proxy_host.trim(), settings.proxy_port);
            match reqwest::Proxy::all(&url) {
                Ok(mut p) => {
                    if !settings.proxy_username.is_empty() {
                        p = p.basic_auth(&settings.proxy_username, &settings.proxy_password);
                    }
                    info!("🌐 Aplicando Proxy HTTP/HTTPS: {}", url);
                    builder = builder.proxy(p);
                }
                Err(e) => {
                    error!("❌ Failed to configure HTTP proxy ({}): {:?}", url, e);
                }
            }
        }
        "socks5" => {
            let url = format!("socks5://{}:{}", settings.proxy_host.trim(), settings.proxy_port);
            match reqwest::Proxy::all(&url) {
                Ok(mut p) => {
                    if !settings.proxy_username.is_empty() {
                        p = p.basic_auth(&settings.proxy_username, &settings.proxy_password);
                    }
                    info!("🌐 Aplicando Proxy SOCKS5: {}", url);
                    builder = builder.proxy(p);
                }
                Err(e) => {
                    error!("❌ Failed to configure SOCKS5 proxy ({}): {:?}", url, e);
                }
            }
        }
        "system" => {
            info!("🌐 Using the operating system proxy.");
            // reqwest reads the system environment variables (HTTP_PROXY / HTTPS_PROXY / ALL_PROXY)
        }
        _ => {
            // "off" (Direct) -> Disables any environment proxy
            builder = builder.no_proxy();
        }
    }
    builder
}

/// Builds a generic `reqwest::Client` with the configured proxy and user-agent
pub fn build_generic_http_client() -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .user_agent(crate::utils::client_identity::user_agent());
    apply_proxy_to_builder(builder).build().unwrap_or_default()
}

/// Tests the proxy connection by hitting the public Discord API
pub async fn test_proxy_connection() -> Result<String, String> {
    let client = build_generic_http_client();
    let url = "https://discord.com/api/v10/gateway";
    let start = std::time::Instant::now();
    match client.get(url).timeout(std::time::Duration::from_secs(6)).send().await {
        Ok(resp) => {
            let latency_ms = start.elapsed().as_millis();
            if resp.status().is_success() {
                Ok(format!("Connection successful! Latency: {}ms (Status: {})", latency_ms, resp.status()))
            } else {
                Ok(format!("Proxy responded in {}ms, but with HTTP status {}", latency_ms, resp.status()))
            }
        }
        Err(e) => {
            Err(format!("Failed to connect via proxy: {}", e))
        }
    }
}

/// Checks whether the error shows signs of being caused by an unreachable proxy or a failed connection to the proxy
pub fn is_proxy_error(err_str: &str) -> bool {
    let settings = get_network_settings();
    if settings.proxy_mode == "off" {
        return false;
    }
    let lower = err_str.to_lowercase();
    // If it is 401 Unauthorized and does NOT indicate a TCP/HTTP connection failure, Discord answered directly, rejecting the token
    if lower.contains("401") && !lower.contains("connect") && !lower.contains("10061") && !lower.contains("refused") {
        return false;
    }
    lower.contains("connect")
        || lower.contains("10061")
        || lower.contains("refused")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("proxy")
        || lower.contains("socks")
        || lower.contains("network error")
        || lower.contains("reset by peer")
        || lower.contains("failed to lookup")
        || lower.contains("websocket")
        || lower.contains("ws")
        || lower.contains("handshake")
        || !lower.contains("401")
}

/// Formats the friendly, instructive alert message for the login screen
pub fn format_proxy_alert_message() -> String {
    let settings = get_network_settings();
    let mode = settings.proxy_mode.to_uppercase();
    if settings.proxy_mode == "system" {
        "Connection failed via the system proxy! Click here or the gear at the top to open Settings and disable it.".to_string()
    } else {
        format!(
            "The proxy ({}: {}:{}) is unreachable or refusing connections! Click here or the gear at the top to open Settings and disable it.",
            mode, settings.proxy_host, settings.proxy_port
        )
    }
}

