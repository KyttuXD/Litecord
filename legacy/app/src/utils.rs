//! The helpers of src/utils the network code needs, from the main source tree, plus stand-ins.
#[path = "../../../src/utils/client_identity.rs"]
pub mod client_identity;
#[path = "../../../src/utils/network_settings.rs"]
pub mod network_settings;
#[path = "../../../src/utils/styled_text.rs"]
pub mod styled_text;
pub use network_settings::*;

#[path = "../../../src/utils/video_settings.rs"]
pub mod video_settings;
#[path = "../../../src/utils/timer_resolution.rs"]
pub mod timer_resolution;
#[path = "../../../src/utils/test_hooks.rs"]
pub mod test_hooks;

pub mod i18n;
pub mod i18n_ui;
