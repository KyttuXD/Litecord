//! Stand-ins for the pieces of src/ui the network code calls.
pub mod helpers {
    /// The legacy window redraws on its own timer: nothing to wake.
    pub fn wake_ui_loops() {}
}
pub mod message_grouping;
