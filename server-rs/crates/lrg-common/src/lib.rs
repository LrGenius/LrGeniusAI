//! Shared foundation for the LrGeniusAI Rust backend: configuration,
//! logging (with the startup-rotation scheme the plugin expects),
//! version info/compatibility checks, PID/OK lifecycle files, and the API
//! access token.

pub mod api_token;
pub mod config;
pub mod jobs;
pub mod lifecycle;
pub mod logging;
pub mod version;
