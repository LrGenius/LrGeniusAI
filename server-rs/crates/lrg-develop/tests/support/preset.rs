//! What a written develop preset may carry, and whether it carries what the
//! policy filter kept: one definition for every test that writes presets
//! (`xmp_roundtrip.rs` over the committed fixtures, `xmp_goldens_local.rs`
//! over Adobe's bundle and a sidecar corpus, `training_dump_local.rs` over
//! the training dump).
//!
//! Both checks work on a preset read back by the XMP reader:
//!
//! - [`violations`]: every registry key must have a policy that reaches a
//!   preset (LEARN, LEARN† or META, and not on the never-write list), apart
//!   from the values the writer sets itself in Adobe's fixed form; nothing
//!   may be kept whole ([`Value::Opaque`] at any depth, opaque entries).
//! - [`differences`]: the preset holds exactly the filtered settings
//!   (`DevelopSettings::filtered(Target::Preset { .. })`), every value
//!   rounded to its key's number format, apart from the writer's stamps and
//!   fixed form.
//!
//! Included by path next to `support/flat.rs` (as `crate::flat`).

use lrg_develop::model::{Correction, DevelopSettings, Fields, MaskTool, Value};
use lrg_develop::registry::{KeySpec, Level, NumFmt, Policy, StructKind};
use lrg_develop::xmp::format::reformat_compound;
use lrg_develop::xmp::write::never_written;
use lrg_develop::xmp::XmpDocument;

use crate::flat::{self, Leaf};

/// Values outside the preset policy that the preset writer sets itself,
/// with the one value it uses: `WhiteBalance="Custom"` next to
/// white-balance numbers, an AI mask's `ReferencePoint` and `ErrorReason`
/// and a range mask's `SampleType` in Adobe's preset form.
pub fn writer_fixed_form(spec: &KeySpec, v: &Value) -> bool {
    match (spec.level, spec.name) {
        (Level::Global, "WhiteBalance") => v.as_str() == Some("Custom"),
        (Level::MaskTool, "ReferencePoint") => v.as_str() == Some("0.500000 0.500000"),
        (Level::MaskTool, "ErrorReason") => v.as_int() == Some(0),
        (Level::Struct(StructKind::CorrectionRangeMask), "SampleType") => v.as_int() == Some(0),
        _ => false,
    }
}

/// Whether a key may appear in a preset at all.
pub fn preset_key(spec: &KeySpec) -> bool {
    !never_written(spec)
        && matches!(
            spec.policy,
            Policy::Learn | Policy::LearnGated(_) | Policy::Meta
        )
}

fn check_value(spec: &'static KeySpec, v: &Value, out: &mut Vec<String>) {
    let key = format!("{}/{}", spec.level, spec.name);
    if let Value::Opaque(_) = v {
        out.push(format!("kept whole {key}"));
    } else if !preset_key(spec) && !writer_fixed_form(spec, v) {
        out.push(format!("{} {key}", spec.policy.class_name()));
    }
    match v {
        Value::Struct(s) => check_fields(&s.fields, out),
        Value::StructList(items) => items.iter().for_each(|s| check_fields(&s.fields, out)),
        Value::Tools(items) => items.iter().for_each(|f| check_fields(f, out)),
        Value::Corrections(cs) => cs.iter().for_each(|c| check_correction(c, out)),
        _ => {}
    }
}

fn check_fields(f: &Fields, out: &mut Vec<String>) {
    for (id, v) in &f.values {
        check_value(id.spec(), v, out);
    }
    for o in &f.opaque {
        out.push(format!("opaque ?{}", o.name));
    }
}

fn check_correction(c: &Correction, out: &mut Vec<String>) {
    for (id, v) in &c.local {
        check_value(id.spec(), v, out);
    }
    check_fields(&c.extra, out);
    for m in &c.masks {
        check_fields(&m.extra, out);
        if let MaskTool::LuminanceRange(lr) = &m.tool {
            check_fields(&lr.rest.fields, out);
        }
    }
}

/// Every key of a read-back preset that must not be there, as
/// `"<class> <level>/<name>"`, `"kept whole <level>/<name>"` or
/// `"opaque ?<name>"`; one entry per occurrence. The reader's own count of
/// keys kept whole must be 0 too.
pub fn violations(doc: &XmpDocument) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(h) = &doc.header {
        check_fields(&h.rest, &mut out);
    }
    let d = &doc.develop;
    for (id, v) in d.values() {
        check_value(id.spec(), v, &mut out);
    }
    if let Some(l) = &d.look {
        check_fields(&l.rest.fields, &mut out);
    }
    for c in &d.corrections {
        check_correction(c, &mut out);
    }
    for o in &d.opaque {
        out.push(format!("opaque ?{}", o.name));
    }
    if doc.skipped_subtrees.kept_whole > 0 {
        out.push("kept whole (the reader's count)".into());
    }
    out
}

// --- the preset holds the filtered settings ----------------------------

fn quantize_fields(f: &mut Fields) {
    for (id, v) in f.values.iter_mut() {
        *v = quantize(id.spec(), v);
    }
}

/// `v` as the preset writer spells it for `spec` and the reader reads it
/// back: numbers rounded to the key's format, a compound number string at
/// `%.6f`, containers field by field.
fn quantize(spec: &KeySpec, v: &Value) -> Value {
    match v {
        Value::Struct(s) => {
            let mut s = s.clone();
            quantize_fields(&mut s.fields);
            Value::Struct(s)
        }
        Value::StructList(items) => Value::StructList(
            items
                .iter()
                .map(|s| {
                    let mut s = s.clone();
                    quantize_fields(&mut s.fields);
                    s
                })
                .collect(),
        ),
        Value::Tools(items) => Value::Tools(
            items
                .iter()
                .map(|f| {
                    let mut f = f.clone();
                    quantize_fields(&mut f);
                    f
                })
                .collect(),
        ),
        Value::Corrections(cs) => Value::Corrections(cs.iter().map(quantize_correction).collect()),
        Value::Str(s) if spec.fmt == NumFmt::CompoundFixed6 => {
            Value::Str(reformat_compound(s).unwrap_or_else(|| s.clone()))
        }
        v => v.quantized(spec),
    }
}

fn quantize_correction(c: &Correction) -> Correction {
    let mut c = c.clone();
    for (id, v) in c.local.iter_mut() {
        *v = quantize(id.spec(), v);
    }
    quantize_fields(&mut c.extra);
    for m in &mut c.masks {
        quantize_fields(&mut m.extra);
        if let MaskTool::LuminanceRange(lr) = &mut m.tool {
            quantize_fields(&mut lr.rest.fields);
        }
    }
    c
}

/// `s` with every registry value quantized as the writer writes it.
pub fn quantized(s: &DevelopSettings) -> DevelopSettings {
    let mut out = s.clone();
    for (id, v) in s.values() {
        out.insert(id, quantize(id.spec(), v))
            .expect("same key, same kind");
    }
    if let Some(l) = &mut out.look {
        quantize_fields(&mut l.rest.fields);
    }
    out.corrections = s.corrections.iter().map(quantize_correction).collect();
    out
}

/// The number of a leaf, if it is one.
fn number(l: &Leaf) -> Option<f64> {
    match l {
        Leaf::Number(x) => Some(x.get()),
        Leaf::Value(Value::Int(i)) => Some(*i as f64),
        Leaf::Value(Value::Real(x)) => Some(x.get()),
        _ => None,
    }
}

/// Leaves equal as the writer and reader treat them: numbers by value
/// (within `%.6f`, the finest format), a 32-digit hex id in any case.
pub fn same(a: &Leaf, b: &Leaf) -> bool {
    if let (Some(x), Some(y)) = (number(a), number(b)) {
        return (x - y).abs() <= 5e-7;
    }
    let hex = |l: &Leaf| match l {
        Leaf::Value(Value::Str(s)) if s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Some(s.to_ascii_uppercase())
        }
        _ => None,
    };
    match (hex(a), hex(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// What the writer adds to a preset on its own, with the one value it
/// writes: the version stamps and derived names at the top, the fixed form
/// of corrections, components and range masks.
fn writer_adds(path: &str, leaf: &Leaf) -> bool {
    let top = !path.contains('.');
    let key = path.rsplit('.').next().unwrap_or(path);
    let text = |s: &str| *leaf == Leaf::Value(Value::Str(s.into()));
    let is = |x: f64| number(leaf) == Some(x);
    let yes = *leaf == Leaf::Value(Value::Bool(true));
    let range = path.contains(".CorrectionRangeMask.");
    match key {
        "Version" | "CompatibleVersion" | "ProcessVersion" | "HasSettings"
        | "ToneCurveName2012"
            if top =>
        {
            true
        }
        "WhiteBalance" if top => text("Custom"),
        "What" => text("Correction"),
        "CorrectionAmount" => is(1.0),
        "CorrectionActive" | "MaskActive" => yes,
        "MaskVersion" => is(1.0),
        "ReferencePoint" => text("0.500000 0.500000"),
        "ErrorReason" => is(0.0),
        "Version" if range => is(3.0),
        "SampleType" if range => is(0.0),
        _ => false,
    }
}

/// How a read-back preset differs from the settings it was written from
/// after the policy filter (`filtered`): `"missing <path>"`,
/// `"changed <path>"`, `"extra <path>"`, indices kept. Values are compared
/// as the writer spells them; the writer's stamps and fixed form and the
/// derived file kind are allowed.
pub fn differences(filtered: &DevelopSettings, back: &DevelopSettings) -> Vec<String> {
    let want = flat::settings(&quantized(filtered));
    let got = flat::settings(back);
    let mut out = Vec::new();
    for (path, leaf) in &want {
        if path == "(file kind)" {
            continue;
        }
        match got.get(path) {
            None => out.push(format!("missing {path}")),
            Some(g) if !same(leaf, g) => out.push(format!("changed {path}")),
            Some(_) => {}
        }
    }
    for (path, leaf) in &got {
        if path != "(file kind)" && !want.contains_key(path) && !writer_adds(path, leaf) {
            out.push(format!("extra {path}"));
        }
    }
    out
}
