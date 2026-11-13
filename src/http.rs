use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::json;
use log::{info, warn, error};

#[derive(Clone)]
pub struct DiscordHttpClient {
    client: reqwest::Client,
    #[allow(dead_code)]
    token: String,
}

impl DiscordHttpClient {
    pub fn new(token: String) -> Self {
        let clean_token = token.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&clean_token).unwrap_or(HeaderValue::from_static("")));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        // Same client properties the Gateway IDENTIFY sends (see utils::client_identity).
        if let Ok(props) = HeaderValue::from_str(&crate::utils::client_identity::super_properties_header()) {
            headers.insert("x-super-properties", props);
        }

        let builder = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(crate::utils::client_identity::user_agent());
        let client = crate::utils::apply_proxy_to_builder(builder)
            .build()
            .unwrap_or_default();

        Self { client, token: clean_token }
    }

    pub async fn get_current_user(&self) -> Result<serde_json::Value, String> {
        let url = "https://discord.com/api/v10/users/@me";
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        return Ok(json);
                    }
                    Err("Failed to read JSON from the response".to_string())
                } else if status.as_u16() == 401 {
                    Err("HTTP 401 Unauthorized: Token rejected by Discord!".to_string())
                } else {
                    Err(format!("HTTP status {}", status))
                }
            }
            Err(e) => Err(format!("HTTP network error: {:?}", e)),
        }
    }

    /// The server list is fetched once at startup and nothing fetches it again, so a
    /// passing Discord error (a 5xx, a dropped connection) used to leave the sidebar
    /// empty for the whole session. Those are retried with a growing pause; any other
    /// status, such as a refused token, is returned at once.
    pub async fn get_user_guilds(&self) -> Result<Vec<serde_json::Value>, String> {
        const RETRY_DELAYS_MS: [u64; 3] = [1_000, 3_000, 8_000];
        let mut attempt = 0;
        loop {
            let (result, transient) = self.get_user_guilds_once().await;
            match result {
                Ok(guilds) => return Ok(guilds),
                Err(e) if transient && attempt < RETRY_DELAYS_MS.len() => {
                    warn!("Server list failed ({}), retrying in {} ms", e, RETRY_DELAYS_MS[attempt]);
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAYS_MS[attempt])).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn get_user_guilds_once(&self) -> (Result<Vec<serde_json::Value>, String>, bool) {
        let url = "https://discord.com/api/v10/users/@me/guilds";
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(guilds) = resp.json::<Vec<serde_json::Value>>().await {
                        return (Ok(guilds), false);
                    }
                }
                let transient = status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
                (Err(format!("HTTP status {}", status)), transient)
            }
            Err(e) => (Err(format!("HTTP error: {:?}", e)), true),
        }
    }

    pub async fn get_user_settings(&self) -> Result<serde_json::Value, String> {
        let url = "https://discord.com/api/v10/users/@me/settings";
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(settings) = resp.json::<serde_json::Value>().await {
                        return Ok(settings);
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error: {:?}", e)),
        }
    }

    pub async fn get_user_dms(&self) -> Result<Vec<serde_json::Value>, String> {
        let url = "https://discord.com/api/v10/users/@me/channels";
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(dms) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(dms);
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error: {:?}", e)),
        }
    }

    pub async fn get_guild_channels(&self, guild_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/guilds/{}/channels", guild_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(channels) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(channels);
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error: {:?}", e)),
        }
    }

    pub async fn get_channel_permissions(&self, guild_id: &str) -> Result<crate::channel_permissions::ChannelPermissions, String> {
        if let Some(permissions) = crate::channel_permissions::cached(guild_id) {
            return Ok(permissions);
        }
        // The gateway already said who the user is; asking again cost a round trip.
        let user_id = match crate::gateway::get_my_user_id() {
            0 => {
                let user = self.get_current_user().await?;
                user["id"].as_str().ok_or("Missing current user id")?.to_string()
            }
            id => id.to_string(),
        };
        let guild_url = format!("https://discord.com/api/v10/guilds/{}", guild_id);
        let member_url = format!("https://discord.com/api/v10/guilds/{}/members/{}", guild_id, user_id);
        let (guild, member) = tokio::try_join!(
            self.get_permission_data(&guild_url), self.get_permission_data(&member_url)
        )?;
        let permissions = crate::channel_permissions::ChannelPermissions::new(&guild, &member, &user_id)?;
        crate::channel_permissions::remember(&permissions);
        Ok(permissions)
    }

    async fn get_permission_data(&self, url: &str) -> Result<serde_json::Value, String> {
        let response = self.client.get(url).send().await.map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Permission lookup HTTP {}", response.status()));
        }
        response.json().await.map_err(|e| e.to_string())
    }

    pub async fn get_channel_messages(&self, channel_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/channels/{}/messages?limit=30", channel_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(msgs) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(msgs);
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error: {:?}", e)),
        }
    }

    pub async fn get_channel_messages_before(&self, channel_id: &str, before_msg_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/channels/{}/messages?limit=30&before={}", channel_id, before_msg_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(msgs) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(msgs);
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error: {:?}", e)),
        }
    }

    pub async fn download_image(&self, url: &str) -> Result<Vec<u8>, String> {
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        return Ok(bytes.to_vec());
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error: {:?}", e)),
        }
    }

    pub async fn send_message(&self, channel_id: &str, content: &str) -> Result<(), String> {
        let url = format!("https://discord.com/api/v10/channels/{}/messages", channel_id);
        let payload = json!({ "content": content });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("Message sent successfully to channel {}", channel_id);
                    Ok(())
                } else {
                    let err = format!("HTTP error status: {}", resp.status());
                    error!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Failed to send message via HTTP: {:?}", e);
                error!("{}", err);
                Err(err)
            }
        }
    }

    #[allow(dead_code)]
    pub async fn update_my_voice_state(&self, guild_id: &str, channel_id: &str) -> Result<(), String> {
        let url = format!("https://discord.com/api/v10/guilds/{}/voice-states/@me", guild_id);
        let payload = json!({
            "channel_id": channel_id,
            "suppress": false
        });

        match self.client.patch(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("Voice State REST @me updated SUCCESSFULLY on server {}!", guild_id);
                    Ok(())
                } else {
                    let err = format!("HTTP error status for Voice State: {}", resp.status());
                    error!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Failed to update Voice State via HTTP: {:?}", e);
                error!("{}", err);
                Err(err)
            }
        }
    }

    /// A scheduled event of a server the user is in, with its interested count.
    pub async fn get_scheduled_event(&self, guild_id: &str, event_id: &str) -> Result<serde_json::Value, String> {
        let url = format!("https://discord.com/api/v10/guilds/{guild_id}/scheduled-events/{event_id}?with_user_count=true");
        let resp = self.client.get(&url).send().await.map_err(|e| format!("HTTP error: {:?}", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("HTTP status {}", status));
        }
        resp.json::<serde_json::Value>().await.map_err(|e| format!("JSON: {:?}", e))
    }

    pub async fn get_user_profile(&self, user_id: &str) -> Result<serde_json::Value, String> {
        let url = format!("https://discord.com/api/v10/users/{}", user_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(user) = resp.json::<serde_json::Value>().await {
                        return Ok(user);
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error while fetching user: {:?}", e)),
        }
    }

    pub async fn get_guild_member(&self, guild_id: &str, user_id: &str) -> Result<serde_json::Value, String> {
        let url = format!("https://discord.com/api/v10/guilds/{}/members/{}", guild_id, user_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(member) = resp.json::<serde_json::Value>().await {
                        return Ok(member);
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error while fetching server member: {:?}", e)),
        }
    }

    pub async fn get_guild_members(&self, guild_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/guilds/{}/members?limit=1000", guild_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(members) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(members);
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error while fetching server members: {:?}", e)),
        }
    }

    pub async fn get_guild_application_command_index(&self, guild_id: &str) -> Result<serde_json::Value, String> {
        let url = format!("https://discord.com/api/v9/guilds/{}/application-command-index", guild_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(data) = resp.json::<serde_json::Value>().await {
                        return Ok(data);
                    }
                }
                Err(format!("HTTP status {}", status))
            }
            Err(e) => Err(format!("HTTP error while fetching the server command index: {:?}", e)),
        }
    }

    pub async fn send_slash_command_interaction(
        &self,
        guild_id: &str,
        channel_id: &str,
        app_id: &str,
        cmd_id: &str,
        cmd_name: &str,
        version: &str,
        options: Vec<serde_json::Value>,
        session_id: &str,
    ) -> Result<(), String> {
        let url = "https://discord.com/api/v9/interactions";
        let nonce = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis());
        
        let payload = json!({
            "type": 2,
            "application_id": app_id,
            "guild_id": guild_id,
            "channel_id": channel_id,
            "session_id": if session_id.is_empty() { "a0000000000000000000000000000000" } else { session_id },
            "data": {
                "version": if version.is_empty() { cmd_id } else { version },
                "id": cmd_id,
                "name": cmd_name,
                "type": 1,
                "options": options
            },
            "nonce": nonce
        });

        match self.client.post(url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() || resp.status().as_u16() == 204 {
                    info!("Slash command interaction '/{}' sent successfully!", cmd_name);
                    Ok(())
                } else {
                    let err = format!("HTTP error status on interaction: {}", resp.status());
                    error!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Network failure while sending slash command interaction: {:?}", e);
                error!("{}", err);
                Err(err)
            }
        }
    }

    pub async fn ring_dm_call(&self, channel_id: &str) -> Result<(), String> {
        let url = format!("https://discord.com/api/v9/channels/{}/call/ring", channel_id);
        let payload = json!({ "recipients": serde_json::Value::Null });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() || resp.status().as_u16() == 204 {
                    info!("🔔 DM call ringing in channel {}...", channel_id);
                    Ok(())
                } else {
                    let err = format!("HTTP error status when starting the DM ring: {}", resp.status());
                    warn!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Network failure while starting the DM ring: {:?}", e);
                warn!("{}", err);
                Err(err)
            }
        }
    }

    pub async fn stop_ringing_dm_call(&self, channel_id: &str) -> Result<(), String> {
        let url = format!("https://discord.com/api/v9/channels/{}/call/stop-ringing", channel_id);
        let payload = json!({ "recipients": serde_json::Value::Null });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() || resp.status().as_u16() == 204 {
                    info!("🔕 DM call ring stopped in channel {}.", channel_id);
                    Ok(())
                } else {
                    let err = format!("HTTP error status when stopping the DM ring: {}", resp.status());
                    warn!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Network failure while stopping the DM ring: {:?}", e);
                warn!("{}", err);
                Err(err)
            }
        }
    }
}
