//! EchoFlow platform-agnostic Rust Core.
//!
//! Strictly free of Tauri dependencies so Phase 2 mobile shells can reuse it via UniFFI (ADR-0001).

pub mod audio;
pub mod controller;
pub mod delivery;
pub mod domain;
pub mod gemini;
pub mod hotkey;
pub mod platform;
pub mod store;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;
