//! Stand-ins for the pieces of src/ui the network code calls.
pub mod helpers {
    /// The legacy window redraws on its own timer: nothing to wake.
    pub fn wake_ui_loops() {}

    /// The main app pauses some work while its window is hidden; this window never hides.
    pub static APP_IS_VISIBLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
}
pub mod message_grouping;
#[path = "../../../../src/ui/message_jump.rs"]
pub mod message_jump;
