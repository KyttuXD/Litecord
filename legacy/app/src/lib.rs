//! Litecord's own network code, compiled unchanged from the main source tree, plus small
//! stand-ins for the parts of the app the legacy edition does not have.
#![allow(dead_code, unused_imports, ambiguous_glob_reexports)]

pub mod audio;
pub mod auth;
pub mod gateway;
pub mod utils;

#[path = "../../../src/http.rs"]
pub mod http;
#[path = "../../../src/file_picker.rs"]
pub mod file_picker;
#[path = "../../../src/http_messaging.rs"]
pub mod http_messaging;
#[path = "../../../src/channel_permissions.rs"]
pub mod channel_permissions;

pub mod screen_capture;
pub mod ui;

pub use gateway::*;
pub use http::*;
