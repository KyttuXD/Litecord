//! Litecord's audio engine (microphone capture, devices) and signal processing, compiled
//! from the main source tree.
#[path = "../../../src/audio/dsp.rs"]
pub mod dsp;
#[path = "../../../src/audio/engine.rs"]
pub mod engine;

pub use engine::*;
