//! The Lua table form: what `photo:getDevelopSettings()` returns and
//! `photo:applyDevelopSettings()` takes, as the plugin's `JSON.lua` encodes
//! it (the `develop_settings` blob of a training example, and the table the
//! backend sends to be applied).
//!
//! - [`read`]: JSON.lua output → model + warnings.
//! - [`write`](mod@write): model → the table to apply to one photo (or to
//!   store as a plugin preset for it). **Provisional** while experiments E1,
//!   E2, E4 and E11 leave fields open; every form they measure is a
//!   [`LuaOptions`] switch, the defaults are [`LuaOptions::PROVISIONAL`] and
//!   how far the evidence carries each is [`LuaOptions::EVIDENCE`].

pub mod read;
pub mod write;

pub use read::{from_lua_str, from_lua_value, MAX_LUA_JSON_BYTES};
pub use write::{
    check_wire, lua_never_written, to_lua_value, ApplyCall, EnumAs, EvidenceStatus, FlagAs,
    LocalForm, LookForm, LuaMode, LuaOptions, LuaWritten, MaskForm, OptionEvidence, PanelSwitches,
    PhotoContext, WireError, ADAPTIVE_PRESET_LOCALS, FIRST_RUN, MASK_SWITCH, PANEL_SWITCHES,
};
