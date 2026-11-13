//! Stand-in for src/utils/test_hooks.rs (test-only switches of the main app).
use std::sync::atomic::AtomicBool;

pub static DROP_GATEWAY: AtomicBool = AtomicBool::new(false);

pub fn test_account() -> Option<String> {
    None
}
