//! The backend's native Lightroom develop model.
//!
//! A leaf crate (no image, network or async dependencies) holding facts and
//! shapes of Lightroom Classic's develop settings:
//!
//! - [`registry`]: every `crs:` develop key with its type, level, stored
//!   range, UI scaling, number format, defaults, policy class, frame scope,
//!   file kind, presence (Lua/XMP), minimum process version and recipe alias;
//! - [`model`]: the typed settings the readers and writers exchange
//!   ([`DevelopSettings`], corrections and masks, the profile, white
//!   balance) and the policy filter ([`model::policy`]);
//! - [`lua`]: the `getDevelopSettings()` table form as the plugin's JSON.lua
//!   encodes it (reader);
//! - [`xmp`]: sidecars, develop presets and profiles in XMP (reader). Both
//!   readers share their typing and classification rules, so the same
//!   settings land in the same model from either form;
//! - [`parse`]: the readers' errors and warnings.
//!
//! Learning and blending logic stays in `lrg-analysis::style_engine`; the LLM
//! recipe schema stays in `lrg-providers::edit_recipe`.

pub mod lua;
pub mod model;
pub mod parse;
mod reader;
pub mod registry;
pub mod xmp;

pub use model::value::Finite;
pub use model::{DevelopSettings, FileKind, FileKindHint};
pub use parse::{ParseError, ParseWarning, WarningKind};
