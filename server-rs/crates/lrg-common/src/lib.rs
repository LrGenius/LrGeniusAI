//! Shared foundation for the LrGeniusAI Rust backend: configuration,
//! logging (with the startup-rotation scheme the plugin expects),
//! version info/compatibility checks, and PID/OK lifecycle files.

pub mod config;
pub mod jobs;
pub mod lifecycle;
pub mod logging;
pub mod version;
