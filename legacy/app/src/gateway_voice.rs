//! Stand-in for src/gateway/voice.rs: the legacy edition has no voice calls yet.
use std::sync::Arc;
use tokio::sync::mpsc;

use super::types::GatewayEvent;

#[allow(clippy::too_many_arguments)]
pub async fn connect_voice_gateway(
    _raw_endpoint: &str,
    _guild_id: &str,
    _user_id: &str,
    _session_id: &str,
    _token: &str,
    channel_id: &str,
    _self_mute_state: Arc<std::sync::Mutex<bool>>,
    _event_tx: mpsc::Sender<GatewayEvent>,
) {
    log::warn!("voice channel {channel_id}: voice calls are not part of the legacy edition yet");
}
