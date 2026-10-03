//! [`DevelopSettings`] → the Lua table `photo:applyDevelopSettings()` takes,
//! as the JSON the plugin's `JSON.lua` decodes into that table.
//!
//! **Provisional.** Experiments E1, E2, E4 and E11 (the plugin task
//! `TaskDevelopExperiments.lua`) show which write-side forms Lightroom
//! accepts. Every alternative they measure that shapes the table is a
//! switch in [`LuaOptions`]; the writer decides none of them. The defaults
//! sit in one place, [`LuaOptions::PROVISIONAL`], and how far the evidence
//! carries each of them in [`LuaOptions::EVIDENCE`]. One run so far
//! (Lightroom Classic 15.6, October 2026, a single non-raw TIFF, run from the
//! Library module) settled the AI-mask forms for subject, sky and background
//! and the non-raw mode-only white-balance choices (`wb_mode_only`,
//! `flatten_auto_now`); `wb_custom_with_numbers` is only supported, because
//! `Custom` with the incremental numbers (the writer's non-raw form) was
//! never applied. People-part masks, the raw side, the `Look` (E11), the
//! flag encoding and `panel_switches` are still open, and the wire goldens
//! in `server-rs/testdata/develop/wire/` are not frozen.
//!
//! # Modes
//!
//! - [`LuaMode::Apply`]: what production writes, for one photo
//!   ([`PhotoContext`]). The settings go through [`DevelopSettings::filtered`]
//!   with [`Target::ApplyPhoto`] first, so PHOTO values pass (the settings
//!   are meant for this photo, as the policy table says) and every value
//!   that does not reach the photo is returned in [`LuaWritten::skipped`].
//! - [`LuaMode::Preset`]: the same table for a plugin preset applied to
//!   that one photo (the E4 route), plus the amount flags when
//!   [`LuaOptions::preset_amount_flags`] asks for them.
//! - `LuaMode::TestRoundTrip` (crate feature `test-roundtrip`, enabled only
//!   through this crate's own dev-dependency): everything the model holds,
//!   opaque content read from a Lua table included, so Lua → model → Lua →
//!   model compares equal (plan §7.3, round trip 2). Content read from XMP
//!   that has no Lua form is an error ([`WireError::NotLua`]).
//!
//! # Rules (plan §4.1, writer rules 1–10)
//!
//! 1. Keys without the `crs:` prefix; only what the policy filter lets
//!    through for the photo, then a never-write guard ([`lua_never_written`]):
//!    digests, `FullMaskSize`, `WholeImageArea`, `Origin`, `ModelVersion`,
//!    `CorrectionID`, `MaskID`, `CorrectionReferenceX/Y`, brush and retouch
//!    payloads, XMP-only keys. A value the guard drops is reported.
//! 2. Integer keys as JSON integers (a real on an integer key goes through
//!    [`KeySpec::coerce`], the one rounding place), real keys as JSON
//!    numbers; every number is a [`Finite`], so never `null`.
//! 3. Booleans as JSON booleans; 0/1 flag keys (`ValueKind::IntFlag`) as
//!    numbers or booleans ([`LuaOptions::int_flag_as`]).
//! 4. Numeric-looking strings stay strings: `ProcessVersion`,
//!    `ReferencePoint`, `LumRange`, `FocalRange`, `Look.UUID`, local curve
//!    points.
//! 5. Global curves as a flat number list (`[x, y, x, y, ...]`, at least two
//!    points); local curves as a list of `"x,y"` strings.
//! 6. `MaskGroupBasedCorrections` as a list in panel order, `Local*` values
//!    directly on the correction ([`LuaOptions::local_form`]: zero-filled
//!    as Adobe's adaptive presets, or the model's own only),
//!    `CorrectionMasks` never empty.
//! 7. Never an empty container: JSON.lua encodes `{}` as `[]`, and an empty
//!    `MaskGroupBasedCorrections` would delete every mask of the photo.
//!    Corrections that do not reach the photo leave the key out entirely.
//! 8. No mixed tables, no sparse arrays (no `null`, one JSON type per list).
//! 9. White balance as a family: the numbers of the photo's family only,
//!    never next to a mode other than `Custom`, never `Temp`.
//! 10. The forms the experiments decide are [`LuaOptions`].
//!
//! Rules 7, 8 and 9 are checked once more on the finished table
//! ([`check_wire`]); a failure there is a writer bug and an error, never a
//! table handed to Lightroom.
//!
//! Like the XMP preset writer, apply mode sets what describes the settings'
//! context rather than the settings: `ToneCurveName2012` next to a point
//! curve, `WhiteBalance = "Custom"` next to numbers, and the fixed form of a
//! correction (`What`, `CorrectionAmount` 1, `CorrectionActive`; the zero
//! `Local*` keys with [`LocalForm::AdaptivePreset`]) and of a component
//! (`MaskActive`; more with [`MaskForm::PresetForm`]). The settings' own
//! `ProcessVersion` is never written (no process-version stamp either:
//! the photo keeps its own).
//!
//! Number spelling is not part of the contract: JSON.lua under Lua 5.1 (the
//! Lightroom runtime) re-encodes numbers with `%.14g`, a local Lua 5.5 with
//! the shortest exact form. Tests compare **decoded** values, never encoded
//! text.

use serde_json::{Map, Number, Value as J};

use crate::model::correction::{Correction, MaskComponent, MaskTool, Semantic};
use crate::model::policy::{SkipReason, Skipped, Target, WB_NUMBERS};
use crate::model::value::{Fields, Finite, Opaque, OpaqueEntry, Struct, Value};
use crate::model::whitebalance::WbFamily;
use crate::model::{DevelopSettings, FileKind, Look};
use crate::registry::{
    self, CurveKind, Def, Gate, KeyId, KeySpec, Level, NumFmt, Policy, Presence, ProcessVersion,
    Scalar, StructKind, ValueKind,
};
use crate::xmp::format::{format_compound, hex32_upper, reformat_compound};
use crate::xmp::write::{look_struct, NEVER_WRITE};

/// The photo the settings are applied to: what the policy filter checks
/// gates, file-kind keys, process versions and camera-restricted profiles
/// against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PhotoContext {
    /// Raw or non-raw, from the photo's own white-balance keys (a DNG
    /// converted from a JPEG is non-raw). Unknown: no white-balance numbers
    /// and no raw-only or non-raw-only key are written.
    pub file_kind: Option<FileKind>,
    /// The photo's process version: keys newer than it are skipped. It is
    /// never written.
    pub process_version: Option<ProcessVersion>,
    /// The photo's camera model, for a camera-restricted `Look`.
    pub camera: Option<String>,
}

impl PhotoContext {
    /// The policy target for this photo.
    pub fn target(&self) -> Target {
        Target::ApplyPhoto {
            file_kind: self.file_kind,
            process_version: self.process_version,
            camera: self.camera.clone(),
        }
    }
}

/// How [`to_lua_value`] writes.
///
/// `#[non_exhaustive]` for the same reason as `xmp::WriteMode`:
/// `TestRoundTrip` exists only with the crate feature `test-roundtrip`, so a
/// match outside this crate needs a wildcard arm and no other crate may name
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LuaMode {
    /// `photo:applyDevelopSettings()` on this photo: policy-filtered,
    /// guarded, completed.
    Apply(PhotoContext),
    /// A plugin preset (`LrApplication.addDevelopPresetForPlugin`, the E4
    /// route) for this photo: the same table as [`LuaMode::Apply`], plus
    /// `SupportsAmount`/`SupportsAmount2` when
    /// [`LuaOptions::preset_amount_flags`] asks for them. The only mode that
    /// honours that field, so a preset-route answer cannot leak into the
    /// `applyDevelopSettings` route.
    Preset(PhotoContext),
    /// Tests only: everything the model holds, verbatim. Of the options only
    /// the encodings ([`LuaOptions::mask_enum_as`], [`LuaOptions::int_flag_as`])
    /// apply, and the reader reads every one of them back; nothing is added,
    /// completed or filtered.
    #[cfg(feature = "test-roundtrip")]
    TestRoundTrip,
}

/// How a mask enum (`MaskSubType`, `MaskSubCategoryID`, `MaskBlendMode`,
/// `ErrorReason`, `CorrectionRangeMask.Type`, ...; see
/// [`KeySpec::is_mask_enum`]) is written. The Lua reader accepts both forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnumAs {
    /// A JSON number (`"MaskSubType": 1`).
    Number,
    /// A numeric string (`"MaskSubType": "1"`).
    String,
}

/// How a 0/1 flag key (`LensProfileEnable`, `AutoLateralCA`,
/// `CropConstrainToWarp`, `HDREditMode`) is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlagAs {
    /// `0`/`1`.
    Number,
    /// `false`/`true`.
    Bool,
}

/// Which fields a written mask component carries beyond what the model
/// holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaskForm {
    /// The "compute me" form of Adobe's adaptive presets, the form
    /// `DevelopExperiments.aiMaskTool` (E2, E4) applies: an AI mask gets
    /// `MaskVersion` 1, `ReferencePoint` `"0.500000 0.500000"` and
    /// `ErrorReason` 0, a luminance range `CorrectionRangeMask.Version` 3 and
    /// `SampleType` 0, when the model does not have them. (No experiment
    /// applies a luminance range.)
    PresetForm,
    /// Only what the model holds (plus `What` and `MaskActive`, which every
    /// form carries). Applied by no experiment.
    Minimal,
}

/// Which `Local*` values a written correction carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LocalForm {
    /// Every key of [`ADAPTIVE_PRESET_LOCALS`], at 0 unless the model has a
    /// value for it: the correction form of Adobe's adaptive presets, which
    /// E2 and E4 apply (`DevelopExperiments.aiMaskCorrection`). The legacy
    /// PV2010 keys in it (`LocalExposure`, `LocalClarity`, ...) are written
    /// only as this 0; a value of theirs never passes the policy filter.
    AdaptivePreset,
    /// The model's own values only. Applied by no experiment.
    Sparse,
}

/// The `Local*` keys every correction of Adobe's adaptive presets carries,
/// in their order; [`LocalForm::AdaptivePreset`] writes each at 0 unless
/// the model has a value. The same list as `LOCAL_KEYS` in
/// `plugin/LrGeniusAI.lrdevplugin/DevelopExperiments.lua` (pinned by
/// `tests/lua_wire_goldens.rs`), so the writer can produce the correction
/// form E2 and E4 apply.
pub const ADAPTIVE_PRESET_LOCALS: &[&str] = &[
    "LocalExposure",
    "LocalHue",
    "LocalSaturation",
    "LocalContrast",
    "LocalClarity",
    "LocalSharpness",
    "LocalBrightness",
    "LocalToningHue",
    "LocalToningSaturation",
    "LocalExposure2012",
    "LocalContrast2012",
    "LocalHighlights2012",
    "LocalShadows2012",
    "LocalWhites2012",
    "LocalBlacks2012",
    "LocalClarity2012",
    "LocalDehaze",
    "LocalLuminanceNoise",
    "LocalMoire",
    "LocalDefringe",
    "LocalTemperature",
    "LocalTint",
    "LocalTexture",
];

/// Which `Enable*` panel switches a table carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelSwitches {
    /// None (plan §4.1 rule 10: `include_panel_switches = false`). Applied
    /// by no experiment.
    None,
    /// [`MASK_SWITCH`] (`EnableMaskGroupBasedCorrections = true`) next to
    /// corrections, nothing else: what E2 and E4 send with every mask, and
    /// the form E2 applied (a no-op on that photo: the switch was already
    /// `true` there, E13).
    MaskOnly,
    /// [`MASK_SWITCH`] next to corrections and the switch of every panel
    /// the table touches ([`PANEL_SWITCHES`], an unverified map).
    All,
}

/// How the `Look` (creative profile) is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LookForm {
    /// `Name`, `UUID`, `Amount` and `Stubbed = true`: the reference Adobe's
    /// own presets carry (E11a).
    Stub,
    /// `Name`, `UUID` and `Amount` only (E11c).
    BareStub,
    /// The whole profile record as the source has it, `Parameters`
    /// included when it was read from a Lua table and is itself a valid
    /// wire table (E11b). The one place the writer passes kept-verbatim
    /// content to Lightroom, and only on request.
    Full,
}

/// The write-side forms experiments E1, E2, E4 and E11 bear on.
///
/// Every alternative an experiment measures that shapes the table (or the
/// call that applies it) is one value of one field, so the model can
/// represent whichever answer comes back; some fields also hold forms no
/// experiment applies (see [`LuaOptions::PROVISIONAL`] for which). The
/// defaults are [`LuaOptions::PROVISIONAL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LuaOptions {
    /// Mask enums ([`KeySpec::is_mask_enum`]) as numbers or numeric
    /// strings. E2 and E4 apply numbers only, and they work.
    pub mask_enum_as: EnumAs,
    /// 0/1 flag keys as numbers or booleans. No experiment writes a flag
    /// key.
    pub int_flag_as: FlagAs,
    /// Which `Enable*` panel switches the table carries (the plan's
    /// `include_panel_switches`). E2 and E4 always send
    /// [`PanelSwitches::MaskOnly`], on a photo where the switch was already
    /// on, so no run has told it from [`PanelSwitches::None`].
    pub panel_switches: PanelSwitches,
    /// Complete AI and range masks in Adobe's preset form or write them
    /// minimal. E2 and E4 apply the preset form only.
    pub mask_form: MaskForm,
    /// Zero-fill a correction's `Local*` keys in Adobe's adaptive-preset
    /// form or write the model's own only. E2 and E4 apply the zero-filled
    /// form only.
    pub local_form: LocalForm,
    /// Write `WhiteBalance = "Custom"` next to white-balance numbers when the
    /// settings name no mode. Raw: E1c (with) against E1b (without);
    /// non-raw: only numbers alone (E1b) are applied, and they leave the
    /// mode at `"As Shot"`; `Custom` with the incremental numbers in one
    /// call is applied by no experiment.
    pub wb_custom_with_numbers: bool,
    /// Write a named white-balance mode (`"Auto"`, `"Daylight"`, ...) on
    /// its own, without numbers, for Lightroom to resolve (E1e, E1f, E1g
    /// apply `Daylight` and `Auto`; `"As Shot"` and the other named modes
    /// alone are applied by no experiment). Off: such a mode and the
    /// numbers next to it are skipped and reported. Honoured for raw (and
    /// unknown) photos only: on a non-raw photo a mode alone is always
    /// skipped and reported, because `"Auto"` alone left the old
    /// incremental numbers in place there (E1f/E1g). `"Custom"` without
    /// numbers is always skipped and reported: it would pin whatever the
    /// photo has.
    pub wb_mode_only: bool,
    /// Ask for `applyDevelopSettings(..., optFlattenAutoNow = true)` when the
    /// table writes `WhiteBalance = "Auto"` ([`ApplyCall::flatten_auto_now`])
    /// (E1f against E1g).
    pub flatten_auto_now: bool,
    /// Ask for `photo:updateAISettings()` after applying a table with AI
    /// content ([`ApplyCall::update_ai_settings`]) (E2b against E2c).
    pub ai_update: bool,
    /// How the `Look` is written (E11: E11a, E11b, E11c).
    pub look_form: LookForm,
    /// Write `SupportsAmount = true` and `SupportsAmount2 = true`, for a
    /// table handed to `LrApplication.addDevelopPresetForPlugin` so that the
    /// preset's amount applies (E4e, plain against flagged). Honoured only
    /// by [`LuaMode::Preset`]; [`LuaMode::Apply`] (the
    /// `applyDevelopSettings` route) never writes the flags, whatever this
    /// says. In Lightroom 15.6 the amount of a plugin preset changed nothing
    /// with or without the flags (E4e).
    pub preset_amount_flags: bool,
}

impl LuaOptions {
    /// The write-side defaults. **The one place every write-side decision is
    /// made**; [`LuaOptions::EVIDENCE`] says, per field, how far the
    /// experiments carry it. The type keeps its provisional name while
    /// fields are open (see the Status column).
    ///
    /// The first run (Lightroom Classic 15.6, 2026-10-03, **one non-raw
    /// TIFF** without develop edits, a virtual copy, run from the Library
    /// module; no raw file) answered E1 (non-raw), E2 and E4; E1e and E11
    /// need a raw file and did not run. Whether the photo shows a person the
    /// report does not say. "Settled" below means: applied in that run, did
    /// what the writer needs; it is not a claim about raw files, other
    /// Lightroom versions or the Develop module.
    ///
    /// | Field | Default | Status | Evidence | Still open |
    /// |---|---|---|---|---|
    /// | `mask_enum_as` | `Number` | settled (E2, 15.6, non-raw) | E2a–E2d apply mask enums as numbers: the masks are kept and computed (subject, sky, background; a hair mask ended `failed`, `ErrorReason` 1) | `String` is applied by no experiment, and is not needed |
    /// | `int_flag_as` | `Number` | open | no experiment writes a flag key; read side only: numbers in 1,294 of 1,294 training rows and in the E13 readback (`AutoLateralCA`, `LensProfileEnable`, `CropConstrainToWarp`, `HDREditMode` all `0`) | a write of a 0/1 flag key, numbers against booleans |
    /// | `panel_switches` | `MaskOnly` | supported (E2, 15.6, non-raw) | E2 sent the switch, but E13 shows it was already `true` on the source photo (E2's copies inherit it), so the run does not tell `None` from `MaskOnly`; the default is the form E2 applied | `None` against `MaskOnly` (the blocking wire-golden variant, with and without the switch); `All` and the `PANEL_SWITCHES` map are unverified |
    /// | `mask_form` | `PresetForm` | settled for subject, sky and background AI masks (E2, 15.6, non-raw) | E2a: a digest-less subject mask in this form is kept, pending until `updateAISettings()`, then computed; E2d: sky and background likewise | people parts: the one applied (E2d hair, `MaskSubType` 3 / `MaskSubCategoryID` 5, the writer's form) ended `failed`, `ErrorReason` 1; form or photo is not known. `Minimal`, and a luminance range's `Version`/`SampleType`, applied by no experiment |
    /// | `local_form` | `AdaptivePreset` | settled (E2, 15.6, non-raw) | E2 applies the 23 zero-filled `Local*` keys and the correction works | `Sparse` applied by no experiment |
    /// | `wb_custom_with_numbers` | `true` | supported on non-raw (E1b, E1d, 15.6); open on raw | E1b: incremental numbers alone are stored, and leave the mode at `"As Shot"` in the readback, so the writer keeps writing `Custom` with them (whether `"As Shot"` + numbers renders them is the open Basic-panel check); E1d: `"Custom"` is taken as a mode | non-raw: `Custom` + incremental numbers in one call; raw: E1b/E1c on a raw file |
    /// | `wb_mode_only` | `false` | settled on non-raw (E1f/E1g, 15.6); open on raw | E1f/E1g: `"Auto"` alone sets the mode, and the readback showed the previous incremental numbers unchanged within 15 s (Library module; the Basic-panel check is open). Never honoured for a non-raw photo, whatever this says. Stays together with `lrg_analysis::style_engine::EMIT_NAMED_WB_MODES` | raw: E1e (`Daylight`) and `Auto` on a raw file (a positive answer turns it on for raw targets only); `"As Shot"` and the other named modes alone are applied by no experiment |
    /// | `flatten_auto_now` | `false` | settled on non-raw (E1f against E1g, 15.6) | `optFlattenAutoNow = true` made no difference to the readback within 15 s (Library module) | raw (matters only with `wb_mode_only`) |
    /// | `ai_update` | `true` | settled for AI masks (E2b against E2c, 15.6, non-raw) | without the call the subject mask is still pending after 30 s (Library module); after `photo:updateAISettings()` it is computed (call 0.6 s) | an Adobe Adaptive `Look` (E11 rejects adaptive Looks); people parts (the subType-3 form, on a photo with a visible person) |
    /// | `look_form` | `Stub` | open | E11 needs a raw file and did not run; every Look in Adobe's bundled presets is a `Stubbed = true` stub (E11a's form) | E11a/E11b/E11c on a raw file; camera-restricted and adaptive Looks by no experiment |
    /// | `preset_amount_flags` | `false` | settled: no effect (E4e, 15.6, non-raw) | amount 50 and 200 leave the globals and the mask values at the preset's own, with and without `SupportsAmount`/`SupportsAmount2` (read back; the visual check is manual) | the preset route is not chosen (E4: preset masks replace the photo's, `updateAI = true` left the mask pending) |
    ///
    /// When a run answers more, in one change:
    ///
    /// 1. change the field here, its row in [`LuaOptions::EVIDENCE`] and
    ///    `the_provisional_choices_are_pinned`;
    /// 2. E1 (mode only): flip `wb_mode_only` together with
    ///    `lrg_analysis::style_engine::EMIT_NAMED_WB_MODES` and its const
    ///    assert in `a_named_mode_majority_is_not_sent_while_the_switch_is_off`.
    ///    A positive raw E1e turns mode-only on for raw targets only: both
    ///    refuse a mode alone for a non-raw photo before reading the switch
    ///    (E1f/E1g), and the tests `custom_without_numbers_is_never_written_alone`
    ///    and `a_non_raw_photo_gets_no_mode_alone_whatever_the_switch` pin it;
    /// 3. re-bless the wire goldens (`server-rs/testdata/develop/wire/README.md`)
    ///    and run `busted`;
    /// 4. set the field's Status on the wiki page Dev-Develop-Model
    ///    ("Options and what decides them"), and correct the field doc
    ///    above if it names the experiment differently.
    pub const PROVISIONAL: LuaOptions = LuaOptions {
        // Settled (E2, 15.6, non-raw): numbers work; strings never applied.
        mask_enum_as: EnumAs::Number,
        // Open: no experiment writes a flag key; read-back evidence only.
        int_flag_as: FlagAs::Number,
        // Supported (E2, 15.6): the form E2 applied, a no-op on that photo.
        panel_switches: PanelSwitches::MaskOnly,
        // Settled for subject/sky/background (E2, 15.6, non-raw); people
        // parts open; Minimal never applied.
        mask_form: MaskForm::PresetForm,
        // Settled (E2, 15.6, non-raw); Sparse never applied.
        local_form: LocalForm::AdaptivePreset,
        // Supported on non-raw (E1b/E1d, 15.6); raw E1b vs E1c open.
        wb_custom_with_numbers: true,
        // Settled on non-raw (E1f/E1g, 15.6; never honoured for non-raw);
        // raw E1e open. With EMIT_NAMED_WB_MODES.
        wb_mode_only: false,
        // Settled on non-raw (E1f vs E1g, 15.6): no effect.
        flatten_auto_now: false,
        // Settled for AI masks (E2b vs E2c, 15.6, non-raw).
        ai_update: true,
        // Open: E11 needs a raw file.
        look_form: LookForm::Stub,
        // Settled (E4e, 15.6): the amount changes nothing either way.
        preset_amount_flags: false,
    };

    /// How far the experiments carry each field of
    /// [`LuaOptions::PROVISIONAL`], in field order: the table above, typed,
    /// so a test can require one row per field.
    pub const EVIDENCE: &'static [OptionEvidence] = &[
        OptionEvidence {
            field: "mask_enum_as",
            status: EvidenceStatus::Settled,
            by: "E2a-E2d (numbers; a hair mask ended failed, ErrorReason 1)",
            scope: FIRST_RUN,
            open: "String is applied by no experiment (not needed)",
        },
        OptionEvidence {
            field: "int_flag_as",
            status: EvidenceStatus::Open,
            by: "",
            scope: "",
            open: "no experiment writes a 0/1 flag key; numbers only on the read side (training rows, E13)",
        },
        OptionEvidence {
            field: "panel_switches",
            status: EvidenceStatus::Supported,
            by: "E2a-E2d sent the switch, but E13 shows it was already true on the source photo (E2's copies inherit it)",
            scope: FIRST_RUN,
            open: "the run does not tell None from MaskOnly (the wire-golden variant with and without the switch); All and PANEL_SWITCHES unverified",
        },
        OptionEvidence {
            field: "mask_form",
            status: EvidenceStatus::Settled,
            by: "E2a-E2d (subject, sky, background)",
            scope: FIRST_RUN,
            open: "people parts: the one applied (E2d hair, MaskSubType 3 / MaskSubCategoryID 5, the writer's form) ended failed, ErrorReason 1; form or photo is not known. Minimal and a luminance range's preset form are applied by no experiment",
        },
        OptionEvidence {
            field: "local_form",
            status: EvidenceStatus::Settled,
            by: "E2a-E2d",
            scope: FIRST_RUN,
            open: "Sparse is applied by no experiment",
        },
        OptionEvidence {
            field: "wb_custom_with_numbers",
            status: EvidenceStatus::Supported,
            by: "E1b (numbers alone leave As Shot in the readback), E1d (Custom is taken)",
            scope: FIRST_RUN,
            open: "non-raw Custom + incremental numbers in one call (the writer's non-raw form); whether As Shot + numbers renders them (Basic panel); raw E1b against E1c",
        },
        OptionEvidence {
            field: "wb_mode_only",
            status: EvidenceStatus::Settled,
            by: "E1f/E1g (Auto alone: the readback showed the old numbers within 15 s)",
            scope: FIRST_RUN,
            open: "raw: E1e (Daylight) and Auto on a raw file; a positive answer enables raw targets only (non-raw is refused whatever the field says); the Basic-panel check",
        },
        OptionEvidence {
            field: "flatten_auto_now",
            status: EvidenceStatus::Settled,
            by: "E1f against E1g (no difference)",
            scope: FIRST_RUN,
            open: "raw",
        },
        OptionEvidence {
            field: "ai_update",
            status: EvidenceStatus::Settled,
            by: "E2b (still pending after 30 s) against E2c (computed after the call)",
            scope: FIRST_RUN,
            open: "an Adobe Adaptive Look; people parts (the subType-3 form, on a photo with a visible person)",
        },
        OptionEvidence {
            field: "look_form",
            status: EvidenceStatus::Open,
            by: "",
            scope: "",
            open: "E11 needs a raw file and did not run",
        },
        OptionEvidence {
            field: "preset_amount_flags",
            status: EvidenceStatus::Settled,
            by: "E4e (amount 50 and 200 change nothing, with and without the flags)",
            scope: FIRST_RUN,
            open: "visual check of the amount copies (manual)",
        },
    ];
}

/// Where the first run's answers hold: the scope of every
/// [`EvidenceStatus::Settled`] and [`EvidenceStatus::Supported`] row so far.
pub const FIRST_RUN: &str =
    "Lightroom Classic 15.6, one non-raw TIFF without develop edits, run from the Library module";

/// How far the experiments carry one [`LuaOptions`] field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EvidenceStatus {
    /// The default was applied by an experiment and did what the writer
    /// needs, within [`OptionEvidence::scope`]; alternatives that were never
    /// applied are named in [`OptionEvidence::open`].
    Settled,
    /// The evidence points to the default without settling it: the default
    /// was not applied as written, or was applied where it could make no
    /// difference.
    Supported,
    /// No experiment has answered it: the default is a provisional guess.
    Open,
}

/// One row of [`LuaOptions::EVIDENCE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OptionEvidence {
    /// The [`LuaOptions`] field.
    pub field: &'static str,
    /// How far the evidence goes.
    pub status: EvidenceStatus,
    /// Which experiments (empty when open).
    pub by: &'static str,
    /// Lightroom version and file kind they ran on (empty when open).
    pub scope: &'static str,
    /// What is still open about the field.
    pub open: &'static str,
}

impl Default for LuaOptions {
    /// [`LuaOptions::PROVISIONAL`].
    fn default() -> LuaOptions {
        LuaOptions::PROVISIONAL
    }
}

/// How the table must be applied: the call arguments that go with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApplyCall {
    /// Call `photo:updateAISettings()` after applying (for a preset: pass
    /// `updateAI = true`): the table adds an AI mask (`Mask/Image`),
    /// another key whose gate needs it (`LensBlur`) or an Adobe Adaptive
    /// `Look` (`isAdobeAdaptive`; its `AILook` state is never written), and
    /// [`LuaOptions::ai_update`] is on. The adaptive `Look` case is applied
    /// by no experiment (E11 rejects adaptive Looks).
    pub update_ai_settings: bool,
    /// Pass `optFlattenAutoNow = true`: the table sets
    /// `WhiteBalance = "Auto"` and [`LuaOptions::flatten_auto_now`] is on.
    pub flatten_auto_now: bool,
}

/// A written table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaWritten {
    /// The settings table, always a JSON object (empty when nothing
    /// reaches the photo; check [`LuaWritten::is_empty`] before applying).
    pub table: J,
    /// Every value that was not written although it differs from its
    /// default: the policy filter's reports, then the writer's own (the
    /// white-balance family, the `Look` form, the never-write guard).
    /// Empty in a test round trip.
    pub skipped: Vec<Skipped>,
    /// How to apply it.
    pub call: ApplyCall,
}

impl LuaWritten {
    /// True when the table holds nothing (applying it would change nothing).
    pub fn is_empty(&self) -> bool {
        self.table.as_object().is_none_or(Map::is_empty)
    }
}

/// The settings cannot be written as a Lua table.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// A value whose variant does not fit its registry key.
    #[error("{path}: expected {expected}, found {found}")]
    WrongValue {
        /// Where (`"MaskGroupBasedCorrections[0].LocalExposure2012"`).
        path: String,
        /// The registry's kind.
        expected: String,
        /// The model value's variant.
        found: &'static str,
    },
    /// An id that is not 32 hex digits, in apply mode.
    #[error("{path}: {value:?} is not 32 hex digits")]
    NotHex32 {
        /// Where.
        path: String,
        /// The value.
        value: String,
    },
    /// A compound number string (`ReferencePoint`, `LumRange`) that is not
    /// a list of numbers, in apply mode.
    #[error("{path}: {value:?} is not a list of numbers")]
    NotCompound {
        /// Where.
        path: String,
        /// The value.
        value: String,
    },
    /// A tone curve with fewer than two points, in apply mode.
    #[error("{path}: a tone curve needs at least two points, found {points}")]
    ShortCurve {
        /// Where.
        path: String,
        /// How many it has.
        points: usize,
    },
    /// Content kept from an XMP file (`Opaque::Xmp`) has no Lua form; a test
    /// round trip cannot write it (apply mode skips and reports it).
    #[error("{path}: content read from XMP has no Lua table form")]
    NotLua {
        /// Where.
        path: String,
    },
    /// The finished table breaks a wire rule (an empty container, `null`, a
    /// mixed list, a `Temp` key). A writer bug: the table is not handed out.
    #[error("{path}: {rule}")]
    NotWireSafe {
        /// Where in the table.
        path: String,
        /// The broken rule.
        rule: &'static str,
    },
}

/// Panel switches [`PanelSwitches::All`] writes: the switch, and the global
/// key-name prefixes of the panel it enables. A switch is written (`true`)
/// when the table carries a key with one of its prefixes.
///
/// **Provisional and unverified**, like [`LuaOptions::PROVISIONAL`]: only
/// [`MASK_SWITCH`] is ever applied by an experiment (E2, E4); check these
/// mappings in Lightroom before turning [`PanelSwitches::All`] on.
pub const PANEL_SWITCHES: &[(&str, &[&str])] = &[
    (
        "EnableToneCurve",
        &["ToneCurvePV2012", "Parametric", "CurveRefineSaturation"],
    ),
    (
        "EnableColorAdjustments",
        &[
            "HueAdjustment",
            "SaturationAdjustment",
            "LuminanceAdjustment",
        ],
    ),
    ("EnableGrayscaleMix", &["GrayMixer"]),
    ("EnableSplitToning", &["SplitToning", "ColorGrade"]),
    (
        "EnableDetail",
        &[
            "Sharpness",
            "Sharpen",
            "LuminanceSmoothing",
            "LuminanceNoiseReduction",
            "ColorNoiseReduction",
        ],
    ),
    ("EnableEffects", &["PostCropVignette", "Grain"]),
    (
        "EnableLensCorrections",
        &[
            "LensProfile",
            "LensManualDistortion",
            "AutoLateralCA",
            "Defringe",
            "VignetteAmount",
            "VignetteMidpoint",
        ],
    ),
    ("EnableTransform", &["Perspective", "Upright"]),
    (
        "EnableCalibration",
        &[
            "ShadowTint",
            "RedHue",
            "RedSaturation",
            "GreenHue",
            "GreenSaturation",
            "BlueHue",
            "BlueSaturation",
        ],
    ),
];

/// The switch of the masking panel, written next to corrections by
/// [`PanelSwitches::MaskOnly`] and [`PanelSwitches::All`].
pub const MASK_SWITCH: &str = "EnableMaskGroupBasedCorrections";

/// No Lightroom develop key: AI Edit once wrote it for white balance, and
/// experiment E1 asks what Lightroom does with it. [`check_wire`] refuses it
/// at any depth; white balance goes as a family (rule 9). The one place the
/// name is spelled (allowed by `tests/no_temp_key.rs`).
pub const RETIRED_TEMP_KEY: &str = "Temp";

/// Whether `spec` never reaches a Lua table Lightroom applies, whatever its
/// policy says (checked after the policy filter, as a last guard): the XMP
/// writer's never-write list ([`NEVER_WRITE`]: runtime ids and reference
/// points, mask payload bookkeeping, brush dabs, retouch and filter
/// payloads, ...), every key with `Digest` in its name, `MaskBrushTable*`,
/// the global `Enable*` panel switches (only [`LuaOptions::panel_switches`]
/// writes those), and keys Lightroom
/// only has in XMP or was never seen writing.
pub fn lua_never_written(spec: &KeySpec) -> bool {
    NEVER_WRITE.contains(&spec.name)
        || spec.name.contains("Digest")
        || spec.name.starts_with("MaskBrushTable")
        || (spec.level == Level::Global && spec.name.starts_with("Enable"))
        || matches!(spec.presence, Presence::XmpOnly | Presence::Unobserved)
}

/// Writes `settings` as a Lua settings table.
///
/// In [`LuaMode::Apply`] and [`LuaMode::Preset`], see the module docs for
/// the rules; the result's
/// [`LuaWritten::skipped`] names everything that did not reach the photo,
/// and [`LuaWritten::call`] how to apply the table.
pub fn to_lua_value(
    settings: &DevelopSettings,
    mode: &LuaMode,
    options: &LuaOptions,
) -> Result<LuaWritten, WireError> {
    match mode {
        LuaMode::Apply(photo) => write_apply(settings, photo, options, false),
        LuaMode::Preset(photo) => write_apply(settings, photo, options, true),
        #[cfg(feature = "test-roundtrip")]
        LuaMode::TestRoundTrip => write_round_trip(settings, options),
    }
}

fn id(level: Level, name: &str) -> KeyId {
    registry::lookup(level, name).unwrap_or_else(|| panic!("registry row {level}/{name}"))
}

fn child(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn variant(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "an integer",
        Value::Real(_) => "a real",
        Value::Bool(_) => "a boolean",
        Value::Str(_) => "text",
        Value::Curve(_) => "a curve",
        Value::StrList(_) => "a text list",
        Value::Alt(_) => "a language alternative",
        Value::Struct(_) => "a structure",
        Value::StructList(_) => "a structure list",
        Value::Tools(_) => "mask components",
        Value::Corrections(_) => "corrections",
        Value::Opaque(_) => "opaque content",
    }
}

fn real(x: Finite) -> J {
    // `Finite` is never NaN or infinite, the only values `from_f64` refuses.
    J::Number(Number::from_f64(x.get()).expect("finite"))
}

/// Integral and small enough to be exact as an `i64`.
fn as_integral(x: Finite) -> Option<i64> {
    let v = x.get();
    (v.fract() == 0.0 && v.abs() < 1e15).then_some(v as i64)
}

/// A global curve coordinate: an integer when it is one (the read-back
/// form, `[0, 0, 255, 255]`), a real otherwise.
fn coordinate(x: Finite) -> J {
    as_integral(x).map_or_else(|| real(x), J::from)
}

/// A coordinate in a local curve point string: the integer, or the
/// shortest text that reads back as the same number.
fn coordinate_text(x: Finite) -> String {
    as_integral(x).map_or_else(|| x.get().to_string(), |i| i.to_string())
}

fn write_apply(
    settings: &DevelopSettings,
    photo: &PhotoContext,
    opt: &LuaOptions,
    preset: bool,
) -> Result<LuaWritten, WireError> {
    let (filtered, skipped) = settings.filtered(&photo.target());
    let mut w = Writer {
        apply: true,
        opt,
        file_kind: photo.file_kind,
        skipped,
        ai: false,
    };
    let mut table = Map::new();

    // White balance first: it is written (or skipped) as a family.
    w.white_balance(&filtered, &mut table)?;
    for (kid, v) in filtered.values() {
        let spec = kid.spec();
        if spec.name == "WhiteBalance" || WB_NUMBERS.contains(&spec.name) {
            continue;
        }
        if let Some(j) = w.keyed(spec, v, spec.name)? {
            if spec.policy.gate() == Some(Gate::NeedsAiUpdate) {
                w.ai = true;
            }
            table.insert(spec.name.to_owned(), j);
        }
    }
    // Set from context, as the XMP preset writer does: the curve's name.
    if table.contains_key("ToneCurvePV2012") {
        let linear = filtered
            .get_by_name("ToneCurvePV2012")
            .is_some_and(|c| c.equals_lit(&registry::Lit::IntList(&[0, 0, 255, 255])));
        table.insert(
            "ToneCurveName2012".into(),
            J::from(if linear { "Linear" } else { "Custom" }),
        );
    }
    if let Some(look) = &filtered.look {
        if let Some(j) = w.look(look)? {
            table.insert("Look".into(), j);
        }
    }
    let mut corrections = Vec::new();
    for (i, c) in filtered.corrections.iter().enumerate() {
        if let Some(j) = w.correction(c, &format!("MaskGroupBasedCorrections[{i}]"))? {
            corrections.push(j);
        }
    }
    // Rule 7: no corrections, no key (an empty list deletes every mask).
    if !corrections.is_empty() {
        table.insert("MaskGroupBasedCorrections".into(), J::Array(corrections));
    }
    if opt.panel_switches == PanelSwitches::All {
        for (switch, prefixes) in PANEL_SWITCHES {
            if table
                .keys()
                .any(|k| prefixes.iter().any(|p| k.starts_with(p)))
            {
                table.insert((*switch).to_owned(), J::Bool(true));
            }
        }
    }
    if opt.panel_switches != PanelSwitches::None && table.contains_key("MaskGroupBasedCorrections")
    {
        table.insert(MASK_SWITCH.to_owned(), J::Bool(true));
    }
    // The preset route only (`LuaMode::Preset`): `applyDevelopSettings` has
    // no amount.
    if preset && opt.preset_amount_flags && !table.is_empty() {
        table.insert("SupportsAmount".into(), J::Bool(true));
        table.insert("SupportsAmount2".into(), J::Bool(true));
    }
    let call = ApplyCall {
        update_ai_settings: opt.ai_update && w.ai,
        flatten_auto_now: opt.flatten_auto_now
            && table.get("WhiteBalance").and_then(J::as_str) == Some("Auto"),
    };
    let table = J::Object(table);
    check_wire(&table)?;
    Ok(LuaWritten {
        table,
        skipped: w.skipped,
        call,
    })
}

#[cfg(feature = "test-roundtrip")]
fn write_round_trip(settings: &DevelopSettings, opt: &LuaOptions) -> Result<LuaWritten, WireError> {
    let mut w = Writer {
        apply: false,
        opt,
        file_kind: None,
        skipped: Vec::new(),
        ai: false,
    };
    let mut f = Fields {
        values: settings.values().map(|(k, v)| (k, v.clone())).collect(),
        opaque: settings.opaque.clone(),
    };
    if let Some(look) = &settings.look {
        f.values
            .insert(id(Level::Global, "Look"), Value::Struct(look_struct(look)));
    }
    if !settings.corrections.is_empty() {
        f.values.insert(
            id(Level::Global, "MaskGroupBasedCorrections"),
            Value::Corrections(settings.corrections.clone()),
        );
    }
    let table = J::Object(w.fields(&f, "")?);
    Ok(LuaWritten {
        table,
        skipped: Vec::new(),
        call: ApplyCall::default(),
    })
}

struct Writer<'a> {
    /// Apply mode: guard, complete, upper-case and check ids, validate
    /// compound strings (kept as spelled). Off in a test round trip.
    apply: bool,
    opt: &'a LuaOptions,
    /// The target photo's file kind (apply mode); `None` in a round trip.
    file_kind: Option<FileKind>,
    skipped: Vec<Skipped>,
    /// Something written needs `updateAISettings()`.
    ai: bool,
}

impl Writer<'_> {
    fn skip(&mut self, path: impl Into<String>, reason: SkipReason) {
        self.skipped.push(Skipped {
            path: path.into(),
            reason,
        });
    }

    /// One registry value at `path`, through the never-write guard in apply
    /// mode; `None` when it is not written.
    fn keyed(
        &mut self,
        spec: &'static KeySpec,
        v: &Value,
        path: &str,
    ) -> Result<Option<J>, WireError> {
        if self.apply && lua_never_written(spec) {
            self.skip(path, SkipReason::Policy(spec.policy));
            return Ok(None);
        }
        self.value(spec, v, path)
    }

    /// The registry values and opaque entries of one level as a table.
    fn fields(&mut self, f: &Fields, path: &str) -> Result<Map<String, J>, WireError> {
        let mut out = Map::new();
        for (kid, v) in &f.values {
            let spec = kid.spec();
            if let Some(j) = self.keyed(spec, v, &child(path, spec.name))? {
                out.insert(spec.name.to_owned(), j);
            }
        }
        for e in &f.opaque {
            if let Some(j) = self.opaque_entry(e, path)? {
                out.insert(e.name.clone(), j);
            }
        }
        Ok(out)
    }

    /// An opaque entry: verbatim in a test round trip (Lua content only),
    /// skipped and reported in apply mode (the policy filter removes them
    /// first; this is the guard).
    fn opaque_entry(&mut self, e: &OpaqueEntry, path: &str) -> Result<Option<J>, WireError> {
        let p = child(path, &e.name);
        if self.apply {
            self.skip(p, SkipReason::Opaque);
            return Ok(None);
        }
        match &e.value {
            Opaque::Json(j) if e.ns.is_none() => Ok(Some(j.clone())),
            _ => Err(WireError::NotLua { path: p }),
        }
    }

    fn integer(&self, spec: &KeySpec, i: i64) -> J {
        if spec.kind == ValueKind::IntFlag && self.opt.int_flag_as == FlagAs::Bool {
            J::Bool(i != 0)
        } else if spec.is_mask_enum() && self.opt.mask_enum_as == EnumAs::String {
            J::String(i.to_string())
        } else {
            J::from(i)
        }
    }

    /// One registry value as JSON; `None` when it is not written (an empty
    /// container is an absent key; opaque content in apply mode is skipped
    /// and reported).
    fn value(
        &mut self,
        spec: &'static KeySpec,
        v: &Value,
        path: &str,
    ) -> Result<Option<J>, WireError> {
        let kind = spec.kind.lua_form();
        let wrong = || WireError::WrongValue {
            path: path.to_owned(),
            expected: format!("{kind:?}"),
            found: variant(v),
        };
        let integer_kind = matches!(
            kind,
            ValueKind::Int | ValueKind::IntFlag | ValueKind::EnumInt(_) | ValueKind::VersionU32
        );
        let j = match (kind, v) {
            (_, Value::Opaque(_)) if self.apply => {
                self.skip(path, SkipReason::Opaque);
                return Ok(None);
            }
            (_, Value::Opaque(Opaque::Json(j))) => j.clone(),
            (_, Value::Opaque(Opaque::Xmp(_))) => {
                return Err(WireError::NotLua {
                    path: path.to_owned(),
                })
            }
            (_, Value::Int(i)) if integer_kind => self.integer(spec, *i),
            // Rule 2: an integer key takes an integer, rounded in the one
            // place that rounds.
            (_, Value::Real(x)) if integer_kind => match spec.coerce(*x).map(|c| c.value) {
                Ok(Scalar::Int(i)) => self.integer(spec, i),
                _ => return Err(wrong()),
            },
            (ValueKind::Real, Value::Real(x)) => real(*x),
            (ValueKind::Real, Value::Int(i)) => real(Finite::from_i64(*i)),
            (ValueKind::Bool(_), Value::Bool(b)) => J::Bool(*b),
            (ValueKind::Hex32, Value::Str(s)) => J::String(self.hex(s, path)?),
            (ValueKind::Enum(_) | ValueKind::Str | ValueKind::VersionStr, Value::Str(s)) => {
                J::String(self.compound(spec, s, path)?)
            }
            (ValueKind::Curve(curve), Value::Curve(points)) => {
                if points.is_empty() {
                    return Ok(None);
                }
                if self.apply && points.len() < 2 {
                    return Err(WireError::ShortCurve {
                        path: path.to_owned(),
                        points: points.len(),
                    });
                }
                J::Array(match curve {
                    CurveKind::Global => points
                        .iter()
                        .flat_map(|&(x, y)| [coordinate(x), coordinate(y)])
                        .collect(),
                    CurveKind::Local => points
                        .iter()
                        .map(|&(x, y)| {
                            J::String(format!("{},{}", coordinate_text(x), coordinate_text(y)))
                        })
                        .collect(),
                })
            }
            (ValueKind::StrSeq, Value::StrList(items)) => {
                if items.is_empty() {
                    return Ok(None);
                }
                let mut out = Vec::with_capacity(items.len());
                for (i, s) in items.iter().enumerate() {
                    out.push(J::String(self.compound(
                        spec,
                        s,
                        &format!("{path}[{i}]"),
                    )?));
                }
                J::Array(out)
            }
            (ValueKind::LangAlt, Value::Alt(s)) => {
                let mut m = Map::new();
                m.insert("x-default".into(), J::String(s.clone()));
                J::Object(m)
            }
            (ValueKind::Struct(_), Value::Struct(s)) => {
                let m = self.fields(&s.fields, path)?;
                if m.is_empty() {
                    return Ok(None);
                }
                J::Object(m)
            }
            (ValueKind::StructSeq(_), Value::StructList(items)) => {
                let mut out = Vec::new();
                for (i, s) in items.iter().enumerate() {
                    let m = self.fields(&s.fields, &format!("{path}[{i}]"))?;
                    if !m.is_empty() {
                        out.push(J::Object(m));
                    }
                }
                if out.is_empty() {
                    return Ok(None);
                }
                J::Array(out)
            }
            (ValueKind::ComponentSeq, Value::Tools(items)) => {
                let mut out = Vec::new();
                for (i, f) in items.iter().enumerate() {
                    let m = self.fields(f, &format!("{path}[{i}]"))?;
                    if !m.is_empty() {
                        out.push(J::Object(m));
                    }
                }
                if out.is_empty() {
                    return Ok(None);
                }
                J::Array(out)
            }
            (ValueKind::CorrectionSeq, Value::Corrections(items)) => {
                let mut out = Vec::new();
                for (i, c) in items.iter().enumerate() {
                    if let Some(j) = self.correction(c, &format!("{path}[{i}]"))? {
                        out.push(j);
                    }
                }
                if out.is_empty() {
                    return Ok(None);
                }
                J::Array(out)
            }
            _ => return Err(wrong()),
        };
        Ok(Some(j))
    }

    /// An id: upper-cased and checked in apply mode, verbatim in a round
    /// trip (the reader keeps ids as read).
    fn hex(&self, s: &str, path: &str) -> Result<String, WireError> {
        if !self.apply {
            return Ok(s.to_owned());
        }
        hex32_upper(s).ok_or_else(|| WireError::NotHex32 {
            path: path.to_owned(),
            value: s.to_owned(),
        })
    }

    /// Text, verbatim. In apply mode a compound number string
    /// (`ReferencePoint`, `LuminanceDepthSampleInfo`, ...) must be a list of
    /// numbers; it keeps its spelling (Lightroom's own tables mix `0` and
    /// `0.170630`), only what the writer builds itself is spelled `%.6f`.
    fn compound(&self, spec: &KeySpec, s: &str, path: &str) -> Result<String, WireError> {
        if self.apply && spec.fmt == NumFmt::CompoundFixed6 && reformat_compound(s).is_none() {
            return Err(WireError::NotCompound {
                path: path.to_owned(),
                value: s.to_owned(),
            });
        }
        Ok(s.to_owned())
    }

    /// White balance as a family (apply mode, rule 9). After the policy
    /// filter only the photo's family can be left (an unknown file kind
    /// leaves neither); the mode decides the rest:
    ///
    /// - numbers with `Custom` or no mode: the numbers, and `"Custom"` when
    ///   the settings say so or [`LuaOptions::wb_custom_with_numbers`];
    /// - numbers next to any other mode: they are what that mode resolved
    ///   to for the source, and are skipped ([`SkipReason::WhiteBalanceMode`]);
    /// - a named mode without numbers (or next to skipped ones): written
    ///   alone with [`LuaOptions::wb_mode_only`] on a raw (or unknown) photo,
    ///   otherwise skipped and reported (`"As Shot"`, the default, without a
    ///   report). Never alone on a non-raw photo, whatever the option says:
    ///   there `"Auto"` alone left the old incremental numbers in place
    ///   (E1f/E1g, Lightroom 15.6);
    /// - `Custom` without numbers, or a mode outside the registry's list:
    ///   always skipped and reported.
    fn white_balance(
        &mut self,
        s: &DevelopSettings,
        table: &mut Map<String, J>,
    ) -> Result<(), WireError> {
        const KEY: &str = "WhiteBalance";
        let mode = match s.get_by_name(KEY) {
            None => None,
            Some(Value::Str(m)) => Some(m.clone()),
            Some(other) => {
                // Not a mode the model can read (kept whole by a reader):
                // `value` skips and reports it, or refuses the variant.
                let spec = id(Level::Global, KEY).spec();
                self.value(spec, other, KEY)?;
                None
            }
        };
        let numbers: Vec<(&'static KeySpec, &Value)> = WB_NUMBERS
            .iter()
            .filter_map(|n| {
                let kid = id(Level::Global, n);
                s.get(kid).map(|v| (kid.spec(), v))
            })
            .collect();
        debug_assert!(
            numbers
                .iter()
                .filter_map(|(spec, _)| WbFamily::of_key(spec))
                .collect::<std::collections::HashSet<_>>()
                .len()
                <= 1,
            "the policy filter leaves at most one white-balance family"
        );
        let custom = mode.as_deref().is_none_or(|m| m == "Custom");
        if !numbers.is_empty() && custom {
            for (spec, v) in numbers {
                if let Some(j) = self.value(spec, v, spec.name)? {
                    table.insert(spec.name.to_owned(), j);
                }
            }
            if mode.is_some() || self.opt.wb_custom_with_numbers {
                table.insert(KEY.into(), J::from("Custom"));
            }
            return Ok(());
        }
        let Some(mode) = mode else {
            return Ok(());
        };
        for (spec, _) in numbers {
            self.skip(spec.name, SkipReason::WhiteBalanceMode(mode.clone()));
        }
        // `Custom` without numbers (they belonged to the other family, or
        // the file kind is unknown) is no mode Lightroom resolves: it would
        // pin whatever the photo has. Never written alone, and neither is a
        // mode outside the registry's list (kept with a reader warning).
        let named = mode != "Custom"
            && matches!(id(Level::Global, KEY).spec().kind,
                ValueKind::Enum(set) if set.contains(&mode.as_str()));
        // Non-raw: a mode alone did not recompute the numbers (E1f/E1g), so
        // the switch, which a raw answer (E1e) may turn on, stops at raw.
        let alone = self.opt.wb_mode_only && self.file_kind != Some(FileKind::NonRaw);
        if alone && named {
            table.insert(KEY.into(), J::String(mode));
        } else if mode != "As Shot" {
            self.skip(KEY, SkipReason::WhiteBalanceMode(mode.clone()));
        }
        Ok(())
    }

    /// The `Look` in apply mode, per [`LuaOptions::look_form`] (a round
    /// trip writes it whole through the global fields).
    fn look(&mut self, look: &Look) -> Result<Option<J>, WireError> {
        const L: Level = Level::Struct(StructKind::Look);
        let written = self.look_table(look)?;
        // An Adobe Adaptive profile needs an AI update to compute its
        // `AILook` state, which is never written (COMPUTED), in every form.
        // Applied by no experiment: E11 rejects adaptive Looks.
        let adaptive = look
            .rest
            .fields
            .values
            .get(&id(L, "isAdobeAdaptive"))
            .is_some_and(|v| *v == Value::Bool(true));
        if written.is_some() && adaptive {
            self.ai = true;
        }
        Ok(written)
    }

    fn look_table(&mut self, look: &Look) -> Result<Option<J>, WireError> {
        const L: Level = Level::Struct(StructKind::Look);
        if self.opt.look_form == LookForm::Full {
            let mut record = look_struct(look);
            let parameters = record.fields.take(L, "Parameters");
            let mut m = self.fields(&record.fields, "Look")?;
            match parameters {
                None => {}
                Some(Value::Opaque(Opaque::Json(j)))
                    if check_wire_at(&j, "Look.Parameters", false).is_ok() =>
                {
                    m.insert("Parameters".into(), j);
                }
                Some(_) => self.skip("Look.Parameters", SkipReason::Opaque),
            }
            return Ok((!m.is_empty()).then_some(J::Object(m)));
        }
        if look.name.is_none() && look.uuid.is_none() {
            // A stub without a name or a UUID refers to nothing.
            self.skip("Look", SkipReason::Policy(Policy::Unknown));
            return Ok(None);
        }
        let mut stub = Struct::new(StructKind::Look);
        let mut put = |name: &str, v: Option<Value>| {
            if let Some(v) = v {
                stub.fields.values.insert(id(L, name), v);
            }
        };
        put("Name", look.name.clone().map(Value::Str));
        put(
            "UUID",
            look.uuid
                .as_ref()
                .map(|u| Value::Str(u.as_str().to_owned())),
        );
        put("Amount", look.amount.map(Value::Real));
        if self.opt.look_form == LookForm::Stub {
            put("Stubbed", Some(Value::Bool(true)));
        }
        // The rest of the record: COMPUTED fields (`Parameters`, `Group`,
        // `Cluster`, `Copyright`, `Supports*`, `CameraModelRestriction`,
        // `isAdobeAdaptive`, ...) are copies Lightroom takes from its profile
        // library when it resolves the stub, so dropping them loses nothing
        // and is not reported (as in a preset). The policy filter has
        // already held a restricted Look back from any other camera, so the
        // restriction describes nothing lost here (a camera-restricted stub
        // is applied by no experiment: E11 uses Adobe Raw profiles).
        // `Stubbed` is the form's own; anything else that is not its
        // default is reported.
        for (kid, v) in &look.rest.fields.values {
            let spec = kid.spec();
            if spec.policy == Policy::Computed || spec.name == "Stubbed" {
                continue;
            }
            let default = matches!(spec.default.raw, Def::Value(lit) if v.equals_lit(&lit));
            let empty_restriction = spec.name == "CameraModelRestriction" && v.as_str() == Some("");
            if !default && !empty_restriction {
                self.skip(child("Look", spec.name), SkipReason::Policy(spec.policy));
            }
        }
        for o in &look.rest.fields.opaque {
            self.skip(child("Look", &o.name), SkipReason::Opaque);
        }
        let m = self.fields(&stub.fields, "Look")?;
        Ok((!m.is_empty()).then_some(J::Object(m)))
    }

    /// One correction as a table; `None` when it has no mask to write.
    fn correction(&mut self, c: &Correction, path: &str) -> Result<Option<J>, WireError> {
        if self.apply && c.masks.is_empty() {
            // The policy filter removes corrections without components
            // (their class is UNKNOWN); this is the guard for rule 6.
            self.skip(path, SkipReason::Policy(Policy::Unknown));
            return Ok(None);
        }
        let f = self.correction_fields(c);
        let m = self.fields(&f, path)?;
        if self.apply && !m.contains_key("CorrectionMasks") {
            self.skip(path, SkipReason::Policy(Policy::Unknown));
            return Ok(None);
        }
        Ok((!m.is_empty()).then_some(J::Object(m)))
    }

    /// A correction's typed parts back in their registry rows (the inverse
    /// of `Correction::from_fields`), plus, in apply mode, the form every
    /// applied correction carries.
    fn correction_fields(&mut self, c: &Correction) -> Fields {
        const C: Level = Level::Correction;
        let mut f = c.extra.clone();
        for (kid, value) in &c.local {
            f.values.insert(*kid, value.clone());
        }
        if let Some(n) = &c.name {
            f.values
                .insert(id(C, "CorrectionName"), Value::Str(n.clone()));
        }
        if let Some(s) = &c.sync_id {
            f.values
                .insert(id(C, "CorrectionSyncID"), Value::Str(s.as_str().to_owned()));
        }
        if let Some(a) = c.amount {
            f.values.insert(id(C, "CorrectionAmount"), Value::Real(a));
        }
        if let Some(a) = c.active {
            f.values.insert(id(C, "CorrectionActive"), Value::Bool(a));
        }
        if !c.masks.is_empty() {
            let tools = c.masks.iter().map(|m| self.component_fields(m)).collect();
            f.values
                .insert(id(C, "CorrectionMasks"), Value::Tools(tools));
        }
        if self.apply {
            for (k, d) in [
                ("What", Value::Str("Correction".into())),
                ("CorrectionAmount", Value::Real(Finite::new_const(1.0))),
                ("CorrectionActive", Value::Bool(true)),
            ] {
                f.values.entry(id(C, k)).or_insert(d);
            }
            if self.opt.local_form == LocalForm::AdaptivePreset {
                for k in ADAPTIVE_PRESET_LOCALS {
                    f.values
                        .entry(id(C, k))
                        .or_insert(Value::Real(Finite::ZERO));
                }
            }
        }
        f
    }

    /// A mask component's typed parts back in their registry rows (the
    /// inverse of `MaskComponent::from_fields`), plus, in apply mode,
    /// `MaskActive` and the form of [`LuaOptions::mask_form`].
    fn component_fields(&mut self, m: &MaskComponent) -> Fields {
        const M: Level = Level::MaskTool;
        let lossless = !self.apply;
        let preset_form = self.apply && self.opt.mask_form == MaskForm::PresetForm;
        let mut f = m.extra.clone();
        let mut put = |name: &str, value: Value| {
            f.values.insert(id(M, name), value);
        };
        if let Some(n) = &m.name {
            put("MaskName", Value::Str(n.clone()));
        }
        if let Some(s) = &m.sync_id {
            put("MaskSyncID", Value::Str(s.as_str().to_owned()));
        }
        if let Some(a) = m.active {
            put("MaskActive", Value::Bool(a));
        }
        if let Some((mode, value, inverted)) = m.combine.encode() {
            put("MaskBlendMode", Value::Int(mode));
            put("MaskValue", Value::Real(value));
            put("MaskInverted", Value::Bool(inverted));
        }
        match &m.tool {
            MaskTool::Semantic(s) => {
                put("What", Value::Str("Mask/Image".into()));
                let (sub, category) = s.encoding();
                put("MaskSubType", Value::Int(sub));
                if let Some(c) = category {
                    put("MaskSubCategoryID", Value::Int(c));
                }
                if let Semantic::PersonPartAt { point, .. } = s {
                    put(
                        "ReferencePoint",
                        Value::Str(format_compound(&[point.x, point.y], lossless)),
                    );
                }
            }
            MaskTool::Linear(g) => {
                put("What", Value::Str("Mask/Gradient".into()));
                put("ZeroX", Value::Real(g.zero.x));
                put("ZeroY", Value::Real(g.zero.y));
                put("FullX", Value::Real(g.full.x));
                put("FullY", Value::Real(g.full.y));
            }
            MaskTool::Radial(g) => {
                put("What", Value::Str("Mask/CircularGradient".into()));
                put("Top", Value::Real(g.top));
                put("Left", Value::Real(g.left));
                put("Bottom", Value::Real(g.bottom));
                put("Right", Value::Real(g.right));
                put("Angle", Value::Real(Finite::ZERO));
                put("Midpoint", Value::Int(g.midpoint));
                put("Roundness", Value::Int(g.roundness));
                put("Feather", Value::Int(g.feather));
                put("Flipped", Value::Bool(g.flipped));
            }
            MaskTool::LuminanceRange(lr) => {
                const R: Level = Level::Struct(StructKind::CorrectionRangeMask);
                put("What", Value::Str("Mask/RangeMask".into()));
                let mut crm = lr.rest.clone();
                let rv = &mut crm.fields.values;
                rv.insert(id(R, "Type"), Value::Int(2));
                rv.insert(id(R, "Invert"), Value::Bool(lr.invert));
                rv.insert(
                    id(R, "LumRange"),
                    Value::Str(format_compound(&lr.range, lossless)),
                );
                if preset_form {
                    rv.entry(id(R, "Version")).or_insert(Value::Int(3));
                    rv.entry(id(R, "SampleType")).or_insert(Value::Int(0));
                }
                put("CorrectionRangeMask", Value::Struct(crm));
            }
            MaskTool::Opaque { what } => {
                if let Some(w) = what {
                    put("What", Value::Str(w.clone()));
                }
            }
        }
        if self.apply {
            // An AI mask, typed or not (Select Object), needs Lightroom to
            // compute it.
            self.ai |= matches!(
                f.values.get(&id(M, "What")),
                Some(Value::Str(w)) if w == "Mask/Image"
            );
            let mut defaults = vec![("MaskActive", Value::Bool(true))];
            let semantic = matches!(
                &m.tool,
                MaskTool::Semantic(s) if !matches!(s, Semantic::PersonPartAt { .. })
            );
            if preset_form && semantic {
                defaults.extend([
                    ("MaskVersion", Value::Int(1)),
                    ("ReferencePoint", Value::Str("0.500000 0.500000".into())),
                    ("ErrorReason", Value::Int(0)),
                ]);
            }
            for (k, d) in defaults {
                f.values.entry(id(M, k)).or_insert(d);
            }
        }
        f
    }
}

/// Checks a finished table against wire rules 7–9: the top level a table,
/// no `null`, no empty container below the top level, every list of one
/// JSON type, no [`RETIRED_TEMP_KEY`] at any depth. [`to_lua_value`] runs it on every
/// table it hands out in apply mode; the plugin-side spec checks the same on
/// the wire goldens.
pub fn check_wire(table: &J) -> Result<(), WireError> {
    if !table.is_object() {
        return Err(WireError::NotWireSafe {
            path: String::new(),
            rule: "the top level must be a table with string keys",
        });
    }
    check_wire_at(table, "", true)
}

fn check_wire_at(j: &J, path: &str, top: bool) -> Result<(), WireError> {
    let fail = |rule| {
        Err(WireError::NotWireSafe {
            path: path.to_owned(),
            rule,
        })
    };
    match j {
        J::Null => fail("null (JSON.lua makes it a hole: a sparse array)"),
        J::Object(m) => {
            if m.is_empty() && !top {
                return fail("an empty table (JSON.lua encodes it as [], an absent key)");
            }
            for (k, v) in m {
                if k == RETIRED_TEMP_KEY {
                    return fail("Temp is never written (white balance goes as a family)");
                }
                check_wire_at(v, &child(path, k), false)?;
            }
            Ok(())
        }
        J::Array(a) => {
            let Some(first) = a.first() else {
                return fail(
                    "an empty list (an empty MaskGroupBasedCorrections deletes every mask)",
                );
            };
            let tag = std::mem::discriminant(first);
            if a.iter().any(|v| std::mem::discriminant(v) != tag) {
                return fail("a list mixing JSON types (a mixed Lua table)");
            }
            for (i, v) in a.iter().enumerate() {
                check_wire_at(v, &format!("{path}[{i}]"), false)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
