//! Litecord's gateway code, compiled from the main source tree .
#[path = "../../../src/gateway/types.rs"]
pub mod types;
#[path = "../../../src/gateway/formatting.rs"]
pub mod formatting;
#[path = "../../../src/gateway/channel_names.rs"]
pub mod channel_names;
#[path = "../../../src/gateway/presence.rs"]
pub mod presence;
#[path = "../../../src/gateway/channel_events.rs"]
pub mod channel_events;
#[path = "../../../src/gateway/unread.rs"]
pub mod unread;
#[path = "../../../src/gateway/client.rs"]
pub mod client;

#[path = "../../../src/gateway/voice.rs"]
pub mod voice;

pub use client::*;
pub use formatting::*;
pub use types::*;
pub use voice::*;
