//! Phase 0 capture harness, as a library so integration tests can drive the same code
//! path the binary does rather than a reimplementation of it.

#[cfg(target_os = "linux")]
pub mod linux;
