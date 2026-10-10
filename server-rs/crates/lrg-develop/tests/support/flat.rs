//! A develop model flattened to `path → leaf`, for comparing two models key
//! by key: which keys differ after a round trip, or between a rebuilt
//! correction and Adobe's.
//!
//! The typed parts of the model are expanded back into the XMP keys they
//! stand for (a [`Combine`] into `MaskBlendMode`/`MaskValue`/`MaskInverted`,
//! a [`Semantic`] into `What`/`MaskSubType`/`MaskSubCategoryID`, ...), so a
//! difference is reported under the key a reader of the file would name.
//! Paths look like `Exposure2012`, `Look.Amount`,
//! `MaskGroupBasedCorrections[0].CorrectionMasks[1].MaskSubType`; opaque
//! entries are `?<name>` (`?<namespace>#<name>` outside `crs:`), numbered
//! when a name repeats.
//!
//! Included by path from the tests that use all of it (a file of its own,
//! apart from `support/mod.rs`, whose other users would see dead code).

use std::collections::BTreeMap;

use lrg_develop::model::{
    Correction, DevelopSettings, Fields, Finite, Look, MaskComponent, MaskTool, Opaque, Semantic,
    Value,
};
use lrg_develop::xmp::PresetHeader;

/// One flattened value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Leaf {
    /// A registry key's value (never a container).
    Value(Value),
    /// A number the model holds in a typed field.
    Number(Finite),
    /// Content kept verbatim.
    Opaque(Opaque),
}

/// `path → leaf`, sorted by path.
pub type Flat = BTreeMap<String, Leaf>;

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn value(out: &mut Flat, path: String, v: &Value) {
    match v {
        Value::Struct(s) => fields(out, &path, &s.fields),
        Value::StructList(items) if !items.is_empty() => {
            for (i, s) in items.iter().enumerate() {
                fields(out, &format!("{path}[{i}]"), &s.fields);
            }
        }
        Value::Tools(items) if !items.is_empty() => {
            for (i, f) in items.iter().enumerate() {
                fields(out, &format!("{path}[{i}]"), f);
            }
        }
        Value::Corrections(cs) if !cs.is_empty() => {
            for (i, c) in cs.iter().enumerate() {
                correction(out, &format!("{path}[{i}]"), c);
            }
        }
        leaf => {
            out.insert(path, Leaf::Value(leaf.clone()));
        }
    }
}

fn fields(out: &mut Flat, path: &str, f: &Fields) {
    for (id, v) in &f.values {
        value(out, join(path, id.spec().name), v);
    }
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for o in &f.opaque {
        let name = match &o.ns {
            Some(ns) => format!("?{ns}#{}", o.name),
            None => format!("?{}", o.name),
        };
        let n = seen.entry(name.clone()).or_default();
        let key = if *n == 0 { name } else { format!("{name}#{n}") };
        *n += 1;
        out.insert(join(path, &key), Leaf::Opaque(o.value.clone()));
    }
}

fn put(out: &mut Flat, path: &str, key: &str, leaf: Leaf) {
    out.insert(join(path, key), leaf);
}

fn text(s: &str) -> Leaf {
    Leaf::Value(Value::Str(s.to_owned()))
}

fn int(i: i64) -> Leaf {
    Leaf::Number(Finite::from_i64(i))
}

fn boolean(b: bool) -> Leaf {
    Leaf::Value(Value::Bool(b))
}

/// One correction below `path` (`MaskGroupBasedCorrections[2]`).
fn correction(out: &mut Flat, path: &str, c: &Correction) {
    if let Some(n) = &c.name {
        put(out, path, "CorrectionName", text(n));
    }
    if let Some(s) = &c.sync_id {
        put(out, path, "CorrectionSyncID", text(s.as_str()));
    }
    if let Some(a) = c.amount {
        put(out, path, "CorrectionAmount", Leaf::Number(a));
    }
    if let Some(a) = c.active {
        put(out, path, "CorrectionActive", boolean(a));
    }
    for (id, v) in &c.local {
        value(out, join(path, id.spec().name), v);
    }
    fields(out, path, &c.extra);
    for (i, m) in c.masks.iter().enumerate() {
        component(out, &format!("{path}.CorrectionMasks[{i}]"), m);
    }
}

/// One mask component below `path`.
fn component(out: &mut Flat, path: &str, m: &MaskComponent) {
    let n = Leaf::Number;
    if let Some(name) = &m.name {
        put(out, path, "MaskName", text(name));
    }
    if let Some(s) = &m.sync_id {
        put(out, path, "MaskSyncID", text(s.as_str()));
    }
    if let Some(a) = m.active {
        put(out, path, "MaskActive", boolean(a));
    }
    if let Some((mode, v, inverted)) = m.combine.encode() {
        put(out, path, "MaskBlendMode", int(mode));
        put(out, path, "MaskValue", n(v));
        put(out, path, "MaskInverted", boolean(inverted));
    }
    match &m.tool {
        MaskTool::Semantic(s) => {
            put(out, path, "What", text("Mask/Image"));
            let (sub, category) = s.encoding();
            put(out, path, "MaskSubType", int(sub));
            if let Some(c) = category {
                put(out, path, "MaskSubCategoryID", int(c));
            }
            if let Semantic::PersonPartAt { point, .. } = s {
                put(out, path, "ReferencePoint[0]", n(point.x));
                put(out, path, "ReferencePoint[1]", n(point.y));
            }
        }
        MaskTool::Linear(g) => {
            put(out, path, "What", text("Mask/Gradient"));
            put(out, path, "ZeroX", n(g.zero.x));
            put(out, path, "ZeroY", n(g.zero.y));
            put(out, path, "FullX", n(g.full.x));
            put(out, path, "FullY", n(g.full.y));
        }
        MaskTool::Radial(g) => {
            put(out, path, "What", text("Mask/CircularGradient"));
            put(out, path, "Top", n(g.top));
            put(out, path, "Left", n(g.left));
            put(out, path, "Bottom", n(g.bottom));
            put(out, path, "Right", n(g.right));
            put(out, path, "Angle", n(Finite::ZERO));
            put(out, path, "Feather", int(g.feather));
            put(out, path, "Midpoint", int(g.midpoint));
            put(out, path, "Roundness", int(g.roundness));
            put(out, path, "Flipped", boolean(g.flipped));
        }
        MaskTool::LuminanceRange(lr) => {
            put(out, path, "What", text("Mask/RangeMask"));
            let crm = join(path, "CorrectionRangeMask");
            put(out, &crm, "Type", int(2));
            put(out, &crm, "Invert", boolean(lr.invert));
            for (i, x) in lr.range.iter().enumerate() {
                put(out, &crm, &format!("LumRange[{i}]"), n(*x));
            }
            fields(out, &crm, &lr.rest.fields);
        }
        MaskTool::Opaque { what } => {
            if let Some(w) = what {
                put(out, path, "What", text(w));
            }
        }
    }
    fields(out, path, &m.extra);
}

fn look(out: &mut Flat, l: &Look) {
    let path = "Look";
    if let Some(name) = &l.name {
        put(out, path, "Name", text(name));
    }
    if let Some(u) = &l.uuid {
        put(out, path, "UUID", text(u.as_str()));
    }
    if let Some(a) = l.amount {
        put(out, path, "Amount", Leaf::Number(a));
    }
    if let Some(r) = &l.camera_restriction {
        put(out, path, "CameraModelRestriction", text(r));
    }
    fields(out, path, &l.rest.fields);
}

/// The whole model: globals, `Look`, corrections, opaque entries, and the
/// file kind as `(file kind)`.
pub fn settings(s: &DevelopSettings) -> Flat {
    let mut out = Flat::new();
    for (id, v) in s.values() {
        value(&mut out, id.spec().name.to_owned(), v);
    }
    if let Some(l) = &s.look {
        look(&mut out, l);
    }
    for (i, c) in s.corrections.iter().enumerate() {
        correction(&mut out, &format!("MaskGroupBasedCorrections[{i}]"), c);
    }
    let top = Fields {
        values: BTreeMap::new(),
        opaque: s.opaque.clone(),
    };
    fields(&mut out, "", &top);
    if let Some(k) = s.file_kind {
        out.insert("(file kind)".into(), text(&format!("{k:?}")));
    }
    out
}

/// A preset header, every path below `header`.
pub fn header(h: &PresetHeader) -> Flat {
    let mut out = Flat::new();
    let p = "header";
    let texts = [
        ("PresetType", &h.preset_type),
        ("Name", &h.name),
        ("ShortName", &h.short_name),
        ("SortName", &h.sort_name),
        ("Group", &h.group),
        ("Description", &h.description),
        ("Cluster", &h.cluster),
        ("CameraModelRestriction", &h.camera_model_restriction),
    ];
    for (k, v) in texts {
        if let Some(v) = v {
            put(&mut out, p, k, text(v));
        }
    }
    if let Some(u) = &h.uuid {
        put(&mut out, p, "UUID", text(u.as_str()));
    }
    let flags = [
        ("SupportsAmount", h.supports_amount),
        ("SupportsAmount2", h.supports_amount2),
        ("SupportsColor", h.supports_color),
        ("SupportsMonochrome", h.supports_monochrome),
    ];
    for (k, v) in flags {
        if let Some(v) = v {
            put(&mut out, p, k, boolean(v));
        }
    }
    fields(&mut out, p, &h.rest);
    out
}

/// The paths whose leaves differ: present on one side only, or unequal.
pub fn diff(a: &Flat, b: &Flat) -> Vec<String> {
    let mut out: Vec<String> = a
        .iter()
        .filter(|(k, v)| b.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect();
    out.extend(b.keys().filter(|k| !a.contains_key(*k)).cloned());
    out.sort();
    out
}
