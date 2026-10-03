//! Round trips 2 and 3 of the plan (§7.3) through the Lua writer, shared by
//! `lua_roundtrip.rs` (the committed fixtures, asserted per file) and the
//! local sweeps (`training_dump_local.rs`, `xmp_goldens_local.rs`, counts
//! only).
//!
//! - **Round trip 2**, model → Lua → model ([`LuaMode::TestRoundTrip`]):
//!   the same model back, and writing it again gives the same table. A model
//!   read from XMP goes in without what has no Lua form
//!   ([`without_xmp_only`]): XMP-only keys (`HasSettings`, `HasCrop`, ...;
//!   the preset header is not part of the model at all), `PerFormat` values
//!   in their XMP shape, content kept as XMP.
//! - **Round trip 3**, Lua → model → XMP preset → model → Lua, mixed and
//!   single-kind preset: the model the preset reads back into goes through
//!   Lua losslessly, and the values it shares with the source (policy
//!   filtered for the preset, rounded to the preset writer's number formats
//!   with `preset::quantized`, then through Lua) are equal
//!   (`preset::same`). Opaque content is excepted by definition: a preset
//!   never carries it.
//!
//! Included by path next to `support/flat.rs` (as `crate::flat`),
//! `support/preset.rs` (as `crate::preset`) and `support/mod.rs` (as
//! `crate::support`).

use std::collections::BTreeMap;

use lrg_develop::build::{IdSlot, SyncNamespace};
use lrg_develop::lua::{from_lua_value, to_lua_value, LuaMode, LuaOptions, LuaWritten};
use lrg_develop::model::{
    Correction, DevelopSettings, Fields, FileKind, MaskTool, Opaque, Target, Value,
};
use lrg_develop::registry::{KeyId, Presence, ProcessVersion, ValueKind};
use lrg_develop::xmp::{parse, write, PresetHeader, PresetSpec, WriteMode};
use lrg_develop::{FileKindHint, ParseWarning};
use serde_json::Value as J;

use crate::flat;
use crate::preset;
use crate::support::generic_path;

/// The reader's file-kind hint for `s`.
pub fn hint(s: &DevelopSettings) -> FileKindHint {
    FileKindHint::from(s.file_kind.map(|k| k == FileKind::Raw))
}

/// `s` written losslessly with the provisional encodings; panics on an
/// error (committed data only).
pub fn lossless(s: &DevelopSettings) -> LuaWritten {
    lossless_with(s, &LuaOptions::PROVISIONAL)
}

/// `s` written losslessly with the encodings of `options` (the only fields
/// the lossless mode reads: `mask_enum_as`, `int_flag_as`).
pub fn lossless_with(s: &DevelopSettings, options: &LuaOptions) -> LuaWritten {
    let w = to_lua_value(s, &LuaMode::TestRoundTrip, options).unwrap_or_else(|e| panic!("{e}"));
    assert!(w.skipped.is_empty());
    w
}

/// Writes `s` losslessly and reads it back.
pub fn round_trip(s: &DevelopSettings) -> (J, DevelopSettings, Vec<ParseWarning>) {
    round_trip_with(s, &LuaOptions::PROVISIONAL)
}

/// [`round_trip`] with the encodings of `options`.
pub fn round_trip_with(
    s: &DevelopSettings,
    options: &LuaOptions,
) -> (J, DevelopSettings, Vec<ParseWarning>) {
    let w = lossless_with(s, options);
    let (back, warnings) = from_lua_value(&w.table, hint(s)).unwrap();
    (w.table, back, warnings)
}

/// Whether a value has no Lua form: an XMP-only key, a `PerFormat` key in
/// its XMP shape, content kept as XMP.
fn xmp_only(id: KeyId, v: &Value) -> bool {
    let spec = id.spec();
    spec.presence == Presence::XmpOnly
        || matches!(spec.kind, ValueKind::PerFormat { .. })
        || matches!(v, Value::Opaque(Opaque::Xmp(_)))
}

fn strip_fields(f: &mut Fields) {
    f.values.retain(|id, v| !xmp_only(*id, v));
    for v in f.values.values_mut() {
        strip_value(v);
    }
    f.opaque.retain(|o| matches!(o.value, Opaque::Json(_)));
}

fn strip_value(v: &mut Value) {
    match v {
        Value::Struct(s) => strip_fields(&mut s.fields),
        Value::StructList(items) => items.iter_mut().for_each(|s| strip_fields(&mut s.fields)),
        Value::Tools(items) => items.iter_mut().for_each(strip_fields),
        Value::Corrections(cs) => cs.iter_mut().for_each(strip_correction),
        _ => {}
    }
}

fn strip_correction(c: &mut Correction) {
    c.local.retain(|id, v| !xmp_only(*id, v));
    strip_fields(&mut c.extra);
    for m in &mut c.masks {
        strip_fields(&mut m.extra);
        if let MaskTool::LuminanceRange(lr) = &mut m.tool {
            strip_fields(&mut lr.rest.fields);
        }
    }
}

/// `s` without what has no Lua form (see the module docs).
pub fn without_xmp_only(s: &DevelopSettings) -> DevelopSettings {
    let mut out = DevelopSettings::new();
    out.file_kind = s.file_kind;
    for (id, v) in s.values() {
        if !xmp_only(id, v) {
            let mut v = v.clone();
            strip_value(&mut v);
            out.insert(id, v).unwrap();
        }
    }
    out.look = s.look.clone().map(|mut l| {
        strip_fields(&mut l.rest.fields);
        l
    });
    out.corrections = s.corrections.clone();
    out.corrections.iter_mut().for_each(strip_correction);
    out.opaque = s
        .opaque
        .iter()
        .filter(|o| matches!(o.value, Opaque::Json(_)))
        .cloned()
        .collect();
    out
}

/// The preset round trip 3 goes through.
pub fn preset_spec(mixed_file_kinds: bool) -> WriteMode {
    let uuid = SyncNamespace::new("lua_roundtrip").id("preset", IdSlot::Correction);
    WriteMode::Preset(PresetSpec {
        mixed_file_kinds,
        process_version: Some(ProcessVersion::V6),
        ..PresetSpec::new(PresetHeader::lrgenius(uuid, "Round trip"))
    })
}

/// The variant of a `Debug` text, without the value it may carry.
fn variant(debug: String) -> String {
    debug
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or("?")
        .to_owned()
}

fn bump(map: &mut BTreeMap<String, usize>, key: String) {
    *map.entry(key).or_default() += 1;
}

/// Round trips 2 and 3 over many settings values, counted; nothing here
/// prints a value.
#[derive(Default)]
pub struct LuaRoundTrips {
    /// Settings values through round trip 2.
    pub rt2_settings: usize,
    /// Lossless writes that failed, per error variant.
    pub write_errors: BTreeMap<String, usize>,
    /// Tables the reader rejected.
    pub read_errors: usize,
    /// Round trip 2: values compared (flattened leaves).
    pub rt2_values: usize,
    /// Round trip 2: paths (indices removed) that differ after reading back.
    pub rt2_changed: BTreeMap<String, usize>,
    /// Round trip 2: read-back warnings per kind and path.
    pub rt2_warnings: BTreeMap<String, usize>,
    /// Round trip 2: a second write that differs from the first.
    pub rt2_not_idempotent: usize,
    /// Round trip 3: presets written (two per settings value).
    pub rt3_presets: usize,
    /// Round trip 3: preset write or read errors, per variant.
    pub rt3_preset_errors: BTreeMap<String, usize>,
    /// Round trip 3: the preset's model not lossless through Lua, per path.
    pub rt3_not_lossless: BTreeMap<String, usize>,
    /// Round trip 3: read-back warnings of the preset's model, per kind and
    /// path.
    pub rt3_warnings: BTreeMap<String, usize>,
    /// Round trip 3: shared values compared after quantisation.
    pub rt3_compared: usize,
    /// Round trip 3: shared values that changed, per mode and path.
    pub rt3_changed: BTreeMap<String, usize>,
}

impl LuaRoundTrips {
    /// Lossless write, or the error variant counted.
    fn write(&mut self, s: &DevelopSettings) -> Option<J> {
        match to_lua_value(s, &LuaMode::TestRoundTrip, &LuaOptions::PROVISIONAL) {
            Ok(w) => Some(w.table),
            Err(e) => {
                bump(&mut self.write_errors, variant(format!("{e:?}")));
                None
            }
        }
    }

    fn read(
        &mut self,
        table: &J,
        s: &DevelopSettings,
    ) -> Option<(DevelopSettings, Vec<ParseWarning>)> {
        match from_lua_value(table, hint(s)) {
            Ok(r) => Some(r),
            Err(_) => {
                self.read_errors += 1;
                None
            }
        }
    }

    /// Both round trips for `s`, a model of Lua's form (read from a Lua
    /// table, or [`without_xmp_only`] of one read from XMP).
    pub fn settings(&mut self, s: &DevelopSettings) {
        self.round_trip_2(s);
        self.round_trip_3(s);
    }

    fn round_trip_2(&mut self, s: &DevelopSettings) {
        self.rt2_settings += 1;
        let Some(table) = self.write(s) else { return };
        let Some((back, warnings)) = self.read(&table, s) else {
            return;
        };
        for w in &warnings {
            bump(
                &mut self.rt2_warnings,
                format!("{} {}", w.kind.name(), generic_path(&w.path)),
            );
        }
        let (a, b) = (flat::settings(s), flat::settings(&back));
        self.rt2_values += a.len();
        for path in flat::diff(&a, &b) {
            bump(&mut self.rt2_changed, generic_path(&path));
        }
        if self.write(&back).is_some_and(|again| again != table) {
            self.rt2_not_idempotent += 1;
        }
    }

    /// Round trip 3 alone, for sources whose round trip 2 is checked
    /// elsewhere.
    pub fn round_trip_3_only(&mut self, s: &DevelopSettings) {
        self.round_trip_3(s);
    }

    fn round_trip_3(&mut self, s: &DevelopSettings) {
        for mixed in [false, true] {
            let mode = if mixed { "mixed" } else { "single-kind" };
            self.rt3_presets += 1;
            let written = match write(s, &preset_spec(mixed)) {
                Ok(w) => w,
                Err(e) => {
                    bump(
                        &mut self.rt3_preset_errors,
                        format!("write {}", variant(format!("{e:?}"))),
                    );
                    continue;
                }
            };
            let m2 = match parse(written.xmp.as_bytes()) {
                Ok(doc) => doc.develop,
                Err(e) => {
                    bump(
                        &mut self.rt3_preset_errors,
                        format!("parse {}", variant(format!("{e:?}"))),
                    );
                    continue;
                }
            };
            // The preset's model through Lua: lossless ...
            let Some(t2) = self.write(&m2) else { continue };
            let Some((m3, warnings)) = self.read(&t2, &m2) else {
                continue;
            };
            for w in &warnings {
                bump(
                    &mut self.rt3_warnings,
                    format!("{} {}", w.kind.name(), generic_path(&w.path)),
                );
            }
            for path in flat::diff(&flat::settings(&m2), &flat::settings(&m3)) {
                bump(&mut self.rt3_not_lossless, generic_path(&path));
            }
            // ... and equal to the source on every value both hold, the
            // source rounded as the preset writer rounds.
            let (filtered, _) = s.filtered(&Target::Preset {
                mixed_file_kinds: mixed,
            });
            let q = preset::quantized(&filtered);
            let Some(t1) = self.write(&q) else { continue };
            let Some((m1, _)) = self.read(&t1, &q) else {
                continue;
            };
            let b = flat::settings(&m3);
            for (path, leaf) in &flat::settings(&m1) {
                if let Some(other) = b.get(path) {
                    self.rt3_compared += 1;
                    if !preset::same(leaf, other) {
                        bump(
                            &mut self.rt3_changed,
                            format!("{mode} {}", generic_path(path)),
                        );
                    }
                }
            }
        }
    }

    pub fn report(&self, indent: &str) {
        if self.rt2_settings > 0 {
            eprintln!(
                "{indent}round trip 2 (model -> Lua -> model): {} settings, {} values compared, \
             write errors {:?}, read errors {}, changed {:?}, read-back warnings {:?}, \
             not idempotent {}",
                self.rt2_settings,
                self.rt2_values,
                self.write_errors,
                self.read_errors,
                self.rt2_changed,
                self.rt2_warnings,
                self.rt2_not_idempotent
            );
        }
        eprintln!(
            "{indent}round trip 3 (Lua -> model -> XMP preset -> model -> Lua): {} presets, \
             {} shared values compared, preset errors {:?}, preset model not lossless {:?}, \
             read-back warnings {:?}, changed {:?}",
            self.rt3_presets,
            self.rt3_compared,
            self.rt3_preset_errors,
            self.rt3_not_lossless,
            self.rt3_warnings,
            self.rt3_changed
        );
    }

    pub fn assert_clean(&self, title: &str) {
        assert!(
            self.write_errors.is_empty()
                && self.read_errors == 0
                && self.rt2_changed.is_empty()
                && self.rt2_warnings.is_empty()
                && self.rt2_not_idempotent == 0
                && self.rt3_preset_errors.is_empty()
                && self.rt3_not_lossless.is_empty()
                && self.rt3_warnings.is_empty()
                && self.rt3_changed.is_empty(),
            "{title}: round trips 2 and 3 through Lua are not clean (see the report above)"
        );
    }
}
