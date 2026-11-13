//! Litecord's QR-code and password logins, compiled from the main source tree.
#[path = "../../../src/auth/remote_auth.rs"]
pub mod remote_auth;
#[path = "../../../src/auth/password_auth.rs"]
pub mod password_auth;
pub use remote_auth::*;
