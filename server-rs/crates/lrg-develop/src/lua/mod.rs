//! The Lua table form: what `photo:getDevelopSettings()` returns, as the
//! plugin's `JSON.lua` encodes it (the `develop_settings` blob of a training
//! example). Reader now; the writer follows in PR 1g, after the experiments
//! settle the write-side types.

pub mod read;

pub use read::{from_lua_str, from_lua_value, MAX_LUA_JSON_BYTES};
