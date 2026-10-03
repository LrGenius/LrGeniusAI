//! The Lua writer in apply mode, checked both ways: one check for the
//! committed fixtures (`lua_roundtrip.rs`, asserted, under every option
//! flip), the wire goldens (`lua_wire_goldens.rs`, per case) and every
//! local sweep (`training_dump_local.rs` over the training dump,
//! `xmp_goldens_local.rs` over Lightroom's bundle, Camera Raw's folder and a
//! sidecar corpus), counts only.
//!
//! Each settings value is written for a photo (its own file kind and
//! process version unless a caller names one): no write error, a table
//! that passes `check_wire`, reads back without a warning, and
//!
//! - holds every policy-filtered value exactly or reports it in
//!   `LuaWritten::skipped` (a global value at its default and the `Look`
//!   stub's computed profile fields apart);
//! - holds nothing else than those values and the writer's own fixed form
//!   ([`writer_adds`]: `WhiteBalance = "Custom"`, `ToneCurveName2012`, the
//!   correction and component form, the adaptive-preset zero `Local*` keys,
//!   the panel switches the options ask for; a preset's amount flags), so a value
//!   that is dropped or changed without a report shows up as a difference
//!   either way.
//!
//! Included by path next to `support/flat.rs` (as `crate::flat`),
//! `support/preset.rs` (as `crate::preset`) and `support/mod.rs` (as
//! `crate::support`).

use std::collections::BTreeMap;

use lrg_develop::lua::{
    check_wire, from_lua_value, to_lua_value, EnumAs, FlagAs, LocalForm, LookForm, LuaMode,
    LuaOptions, LuaWritten, MaskForm, PanelSwitches, PhotoContext, ADAPTIVE_PRESET_LOCALS,
    MASK_SWITCH, PANEL_SWITCHES,
};
use lrg_develop::model::{DevelopSettings, FileKind, Value};
use lrg_develop::registry::{self, Def, Level};
use lrg_develop::FileKindHint;
use serde_json::Value as J;

use crate::flat::{self, Leaf};
use crate::preset;
use crate::support::generic_path;

/// The variant of a `Debug` text, without the value it may carry.
pub fn variant(debug: String) -> String {
    debug
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or("?")
        .to_owned()
}

/// Apply-mode counts over many settings values.
#[derive(Default)]
pub struct LuaApply {
    pub written: usize,
    pub write_errors: BTreeMap<String, usize>,
    pub not_wire_safe: usize,
    pub read_back_warnings: BTreeMap<String, usize>,
    /// Filtered values neither in the table nor reported.
    pub lost: BTreeMap<String, usize>,
    /// Filtered values in the table with another value.
    pub changed: BTreeMap<String, usize>,
    /// Values in the table that the filtered settings do not hold and that
    /// are not the writer's fixed form.
    pub added: BTreeMap<String, usize>,
    pub compared: usize,
    /// What was held back, per key (indices removed) and reason variant.
    pub skipped: BTreeMap<String, usize>,
}

fn bump(map: &mut BTreeMap<String, usize>, key: String) {
    *map.entry(key).or_default() += 1;
}

fn number(l: &Leaf) -> Option<f64> {
    match l {
        Leaf::Number(x) => Some(x.get()),
        Leaf::Value(v) => v.as_finite().map(|x| x.get()),
        Leaf::Opaque(_) => None,
    }
}

/// Whether `path` (or one of its ancestors) is among the `skipped` paths.
pub fn reported(skipped: &[String], path: &str) -> bool {
    skipped.iter().any(|k| {
        path == k
            || path
                .strip_prefix(k.as_str())
                .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
    })
}

/// What the writer adds to an applied table on its own, with the value it
/// writes, under `options` (a preset's amount flags are checked before the
/// read-back, in [`LuaApply::check`]).
pub fn writer_adds(path: &str, leaf: &Leaf, options: &LuaOptions) -> bool {
    let top = !path.contains('.') && !path.contains('[');
    let key = path.rsplit('.').next().unwrap_or(path);
    let text = |s: &str| *leaf == Leaf::Value(Value::Str(s.into()));
    let is = |x: f64| number(leaf) == Some(x);
    let yes = *leaf == Leaf::Value(Value::Bool(true));
    let range = path.contains(".CorrectionRangeMask.");
    // `MaskGroupBasedCorrections[i].<key>`, not a component's.
    let on_correction = path.starts_with("MaskGroupBasedCorrections[")
        && !path.contains(".CorrectionMasks[")
        && path.matches('.').count() == 1;
    if top {
        return match key {
            "WhiteBalance" => text("Custom"),
            "ToneCurveName2012" => true,
            k if k == MASK_SWITCH => options.panel_switches != PanelSwitches::None && yes,
            k if PANEL_SWITCHES.iter().any(|(s, _)| *s == k) => {
                options.panel_switches == PanelSwitches::All && yes
            }
            _ => false,
        };
    }
    match key {
        "Stubbed" if path == "Look.Stubbed" => yes,
        "What" if on_correction => text("Correction"),
        "CorrectionAmount" => is(1.0),
        "CorrectionActive" | "MaskActive" => yes,
        "MaskVersion" => is(1.0),
        "ReferencePoint" => text("0.500000 0.500000"),
        "ErrorReason" => is(0.0),
        "Version" if range => is(3.0),
        "SampleType" if range => is(0.0),
        k if on_correction && ADAPTIVE_PRESET_LOCALS.contains(&k) => {
            options.local_form == LocalForm::AdaptivePreset && is(0.0)
        }
        _ => false,
    }
}

/// `base` with one field at a time set to each of its other values: the
/// option sets an experiment's answer can lead to, named `field=value`.
/// `wb_mode_only` and `flatten_auto_now` also flip together (the second
/// matters only with the first). A new `LuaOptions` field fails to compile
/// here until it is listed.
pub fn flips(base: LuaOptions) -> Vec<(String, LuaOptions)> {
    let LuaOptions {
        mask_enum_as,
        int_flag_as,
        panel_switches,
        mask_form,
        local_form,
        wb_custom_with_numbers,
        wb_mode_only,
        flatten_auto_now,
        ai_update,
        look_form,
        preset_amount_flags,
    } = base;
    let mut out = Vec::new();
    let mut add = |name: String, o: LuaOptions| out.push((name, o));
    for v in [EnumAs::Number, EnumAs::String] {
        if v != mask_enum_as {
            add(
                format!("mask_enum_as={v:?}"),
                LuaOptions {
                    mask_enum_as: v,
                    ..base
                },
            );
        }
    }
    for v in [FlagAs::Number, FlagAs::Bool] {
        if v != int_flag_as {
            add(
                format!("int_flag_as={v:?}"),
                LuaOptions {
                    int_flag_as: v,
                    ..base
                },
            );
        }
    }
    for v in [
        PanelSwitches::None,
        PanelSwitches::MaskOnly,
        PanelSwitches::All,
    ] {
        if v != panel_switches {
            add(
                format!("panel_switches={v:?}"),
                LuaOptions {
                    panel_switches: v,
                    ..base
                },
            );
        }
    }
    for v in [MaskForm::PresetForm, MaskForm::Minimal] {
        if v != mask_form {
            add(
                format!("mask_form={v:?}"),
                LuaOptions {
                    mask_form: v,
                    ..base
                },
            );
        }
    }
    for v in [LocalForm::AdaptivePreset, LocalForm::Sparse] {
        if v != local_form {
            add(
                format!("local_form={v:?}"),
                LuaOptions {
                    local_form: v,
                    ..base
                },
            );
        }
    }
    for v in [LookForm::Stub, LookForm::BareStub, LookForm::Full] {
        if v != look_form {
            add(
                format!("look_form={v:?}"),
                LuaOptions {
                    look_form: v,
                    ..base
                },
            );
        }
    }
    add(
        format!("wb_custom_with_numbers={}", !wb_custom_with_numbers),
        LuaOptions {
            wb_custom_with_numbers: !wb_custom_with_numbers,
            ..base
        },
    );
    add(
        format!("wb_mode_only={}", !wb_mode_only),
        LuaOptions {
            wb_mode_only: !wb_mode_only,
            ..base
        },
    );
    add(
        format!("flatten_auto_now={}", !flatten_auto_now),
        LuaOptions {
            flatten_auto_now: !flatten_auto_now,
            ..base
        },
    );
    add(
        format!(
            "wb_mode_only={} flatten_auto_now={}",
            !wb_mode_only, !flatten_auto_now
        ),
        LuaOptions {
            wb_mode_only: !wb_mode_only,
            flatten_auto_now: !flatten_auto_now,
            ..base
        },
    );
    add(
        format!("ai_update={}", !ai_update),
        LuaOptions {
            ai_update: !ai_update,
            ..base
        },
    );
    add(
        format!("preset_amount_flags={}", !preset_amount_flags),
        LuaOptions {
            preset_amount_flags: !preset_amount_flags,
            ..base
        },
    );
    out
}

impl LuaApply {
    /// Writes `settings` for its own photo with the provisional options and
    /// checks the table.
    pub fn settings(&mut self, settings: &DevelopSettings) {
        let photo = PhotoContext {
            file_kind: settings.file_kind,
            process_version: settings.process_version(),
            camera: None,
        };
        self.settings_for(settings, &LuaMode::Apply(photo), &LuaOptions::PROVISIONAL);
    }

    /// Writes `settings` in `mode` (`Apply` or `Preset`, which names the
    /// photo) with `options`, and checks the table both ways. The written
    /// table, unless the write failed.
    pub fn settings_for(
        &mut self,
        settings: &DevelopSettings,
        mode: &LuaMode,
        options: &LuaOptions,
    ) -> Option<LuaWritten> {
        let (photo, preset) = match mode {
            LuaMode::Apply(p) => (p, false),
            LuaMode::Preset(p) => (p, true),
            _ => panic!("settings_for checks the apply and preset modes"),
        };
        self.written += 1;
        let w = match to_lua_value(settings, mode, options) {
            Ok(w) => w,
            Err(e) => {
                bump(&mut self.write_errors, variant(format!("{e:?}")));
                return None;
            }
        };
        for k in &w.skipped {
            bump(
                &mut self.skipped,
                format!(
                    "{} {}",
                    generic_path(&k.path),
                    variant(format!("{:?}", k.reason))
                ),
            );
        }
        let skipped: Vec<String> = w.skipped.iter().map(|k| k.path.clone()).collect();
        self.check(settings, photo, preset, options, &w.table, &skipped);
        Some(w)
    }

    /// Checks `table`, written from `settings` for `photo` with `options`
    /// (`preset`: for a plugin preset) and reporting the `skipped` paths:
    /// wire-safe, read back without a warning, and both directions of the
    /// module docs. The wire-goldens test hands it a golden file's table.
    pub fn check(
        &mut self,
        settings: &DevelopSettings,
        photo: &PhotoContext,
        preset: bool,
        options: &LuaOptions,
        table: &J,
        skipped: &[String],
    ) {
        if check_wire(table).is_err() {
            self.not_wire_safe += 1;
        }
        // A plugin preset's amount flags are no develop setting (no registry
        // row; `getDevelopSettings()` never has them): checked here, then
        // left out of the read-back.
        let mut table = table.clone();
        if let Some(m) = table.as_object_mut() {
            for flag in ["SupportsAmount", "SupportsAmount2"] {
                if let Some(v) = m.remove(flag) {
                    let wanted = preset && options.preset_amount_flags && v == J::Bool(true);
                    if !wanted {
                        bump(&mut self.added, flag.to_owned());
                    }
                }
            }
        }
        let hint = FileKindHint::from(photo.file_kind.map(|k| k == FileKind::Raw));
        let Ok((back, warnings)) = from_lua_value(&table, hint) else {
            self.not_wire_safe += 1;
            return;
        };
        for x in &warnings {
            bump(
                &mut self.read_back_warnings,
                format!("{} {}", x.kind.name(), generic_path(&x.path)),
            );
        }
        let (filtered, _) = settings.filtered(&photo.target());
        let want = flat::settings(&filtered);
        let got = flat::settings(&back);
        for (path, leaf) in &want {
            if path == "(file kind)" {
                continue;
            }
            // The Look goes as a stub: Lightroom fills in its computed
            // profile fields itself.
            if path.starts_with("Look.")
                && !matches!(path.as_str(), "Look.Name" | "Look.UUID" | "Look.Amount")
            {
                continue;
            }
            match got.get(path) {
                Some(g) => {
                    self.compared += 1;
                    if !preset::same(g, leaf) {
                        bump(&mut self.changed, generic_path(path));
                    }
                }
                None => {
                    // A global value at its default (`WhiteBalance` "As
                    // Shot") goes without a report: dropping it loses nothing.
                    let default = registry::lookup(Level::Global, path).is_some_and(|id| {
                        let spec = id.spec();
                        [spec.default.raw, spec.default.non_raw].iter().any(|d| {
                            matches!((d, leaf), (Def::Value(lit), Leaf::Value(v)) if v.equals_lit(lit))
                        })
                    });
                    if !reported(skipped, path) && !default {
                        bump(&mut self.lost, generic_path(path));
                    }
                }
            }
        }
        for (path, leaf) in &got {
            if path != "(file kind)"
                && !want.contains_key(path)
                && !writer_adds(path, leaf, options)
            {
                bump(&mut self.added, generic_path(path));
            }
        }
    }

    pub fn report(&self, indent: &str) {
        eprintln!(
            "{indent}as Lua tables for their own photo: {} written, write errors {:?}, \
             not wire-safe {}, read-back warnings {:?}",
            self.written, self.write_errors, self.not_wire_safe, self.read_back_warnings
        );
        eprintln!(
            "{indent}applied table against the filtered settings: {} values compared, \
             changed {:?}, lost without a report {:?}, added beyond the writer's form {:?}",
            self.compared, self.changed, self.lost, self.added
        );
        eprintln!("{indent}held back from the applied tables (key, reason: count):");
        for (what, n) in &self.skipped {
            eprintln!("{indent}  {n:>6}  {what}");
        }
    }

    pub fn is_clean(&self) -> bool {
        self.write_errors.is_empty()
            && self.not_wire_safe == 0
            && self.read_back_warnings.is_empty()
            && self.changed.is_empty()
            && self.lost.is_empty()
            && self.added.is_empty()
    }

    pub fn assert_clean(&self, title: &str) {
        assert!(
            self.is_clean(),
            "{title}: writing as Lua tables is not clean (see the report above)"
        );
    }
}
