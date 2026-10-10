//! The key enumeration of plan §7.3 point 4, shared by the round trips of
//! both writers (`xmp_roundtrip.rs`, `lua_roundtrip.rs`): for every key, the
//! values to try (minimum, maximum, midpoint, default, a negative value for
//! a signed range, a value with more digits than any format keeps, every
//! member of a closed set) and where the reader would put each one in a
//! model, with the path [`crate::flat`] shows it under.
//!
//! Included by path next to `support/flat.rs` (as `crate::flat`).

use std::collections::BTreeMap;

use lrg_develop::model::{
    Combine, Correction, DevelopSettings, Fields, Finite, Hex32, Look, MaskComponent, MaskTool,
    Struct, Value,
};
use lrg_develop::registry::{
    self, CurveKind, Def, KeyId, KeySpec, Level, Lit, Policy, StructKind, ValueKind,
};

use crate::flat;

pub fn f(x: f64) -> Finite {
    Finite::new(x).unwrap()
}

/// The numeric default of `spec`, raw first.
pub fn numeric_default(spec: &KeySpec) -> Option<f64> {
    [spec.default.raw, spec.default.non_raw]
        .into_iter()
        .find_map(|d| match d {
            Def::Value(Lit::Int(i)) => Some(i as f64),
            Def::Value(Lit::Real(r)) => Some(r.get()),
            _ => None,
        })
}

pub fn numbers(spec: &KeySpec, integer: bool) -> Vec<f64> {
    let mut out = Vec::new();
    match spec.range {
        Some((min, max)) => {
            let (min, max) = (min.get(), max.get());
            out.extend([min, max, (min + max) / 2.0]);
            if min < 0.0 {
                out.push(min / 3.0);
            }
            if !integer {
                // More digits than any number format keeps.
                out.push(min + (max - min) * 0.123_456_789);
            }
        }
        None => {
            out.extend([0.0, 1.0, -1.0]);
            if !integer {
                out.extend([0.123_456_789, -0.25, 1.25]);
            }
        }
    }
    out.extend(numeric_default(spec));
    if integer {
        for x in &mut out {
            *x = x.round();
        }
    }
    out.sort_by(f64::total_cmp);
    out.dedup();
    out
}

pub fn curve(points: &[(f64, f64)]) -> Value {
    Value::Curve(points.iter().map(|&(x, y)| (f(x), f(y))).collect())
}

/// The values to try for one key in its Lua (`lua`) or XMP form; empty for the kinds that are containers
/// or typed elsewhere (structures, sequences of structures, corrections,
/// mask components), which the fixtures and builder tests cover.
pub fn samples(spec: &KeySpec, lua: bool) -> Vec<Value> {
    let int = |xs: Vec<f64>| xs.into_iter().map(|x| Value::Int(x as i64)).collect();
    let kind = if lua {
        spec.kind.lua_form()
    } else {
        spec.kind.xmp_form()
    };
    match kind {
        ValueKind::Int => int(numbers(spec, true)),
        ValueKind::Real => numbers(spec, false)
            .into_iter()
            .map(|x| Value::Real(f(x)))
            .collect(),
        ValueKind::IntFlag => vec![Value::Int(0), Value::Int(1)],
        ValueKind::EnumInt(values) => values.iter().map(|v| Value::Int(*v)).collect(),
        ValueKind::Bool(_) => vec![Value::Bool(false), Value::Bool(true)],
        ValueKind::Enum(labels) => labels.iter().map(|l| Value::Str((*l).into())).collect(),
        ValueKind::Hex32 => vec![Value::Str("0000000000000000000000000000ABCD".into())],
        ValueKind::Str => vec![Value::Str(
            match spec.name {
                "LumRange" => "0.000000 0.250000 0.750000 1.000000",
                "ReferencePoint" => "0.250000 0.750000",
                "CameraProfile" => "Adobe Standard",
                _ => "LrGenius sample",
            }
            .into(),
        )],
        ValueKind::StrSeq => vec![Value::StrList(vec!["1 2".into(), "3 4".into()])],
        ValueKind::Curve(kind) => {
            let mut out = vec![curve(&[
                (0.0, 0.0),
                (64.0, 50.0),
                (192.0, 210.0),
                (255.0, 255.0),
            ])];
            if let Def::Value(Lit::IntList(flat)) = spec.default.raw {
                let points: Vec<(f64, f64)> = flat
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| (p[0] as f64, p[1] as f64))
                    .collect();
                out.push(curve(&points));
            }
            assert!(matches!(kind, CurveKind::Global | CurveKind::Local));
            out
        }
        _ => Vec::new(),
    }
}

pub fn kid(level: Level, name: &str) -> KeyId {
    registry::lookup(level, name).unwrap_or_else(|| panic!("registry row {level}/{name}"))
}

pub fn one(kind: StructKind, id: KeyId, v: Value) -> Struct {
    let mut s = Struct::new(kind);
    s.fields.values.insert(id, v);
    s
}

/// A mask component that is neither typed nor combined: whatever lands in
/// `extra` stays there on reading.
pub fn bare_component(extra: Fields) -> MaskComponent {
    MaskComponent {
        tool: MaskTool::Opaque { what: None },
        combine: Combine::Unrecognised,
        name: None,
        sync_id: None,
        active: None,
        extra,
    }
}

pub fn correction(local: BTreeMap<KeyId, Value>, masks: Vec<MaskComponent>) -> Correction {
    Correction {
        name: None,
        sync_id: None,
        amount: None,
        active: None,
        local,
        masks,
        extra: Fields::default(),
    }
}

pub fn in_component(id: KeyId, v: Value) -> DevelopSettings {
    let mut extra = Fields::default();
    extra.values.insert(id, v);
    let mut s = DevelopSettings::new();
    s.corrections = vec![correction(BTreeMap::new(), vec![bare_component(extra)])];
    s
}

/// `value` of `id` placed where the reader would put it, and the path
/// [`flat::settings`] shows it under; `None` for a key this test does not
/// place (its row says why it is covered elsewhere).
pub fn place(id: KeyId, v: &Value) -> Option<(DevelopSettings, String)> {
    let spec = id.spec();
    let mut s = DevelopSettings::new();
    let m = "MaskGroupBasedCorrections[0].CorrectionMasks[0]";
    let path = match spec.level {
        Level::Global => {
            s.insert(id, v.clone()).ok()?;
            spec.name.to_owned()
        }
        Level::Correction => {
            if spec.name == "CorrectionAmount" {
                let mut c = correction(BTreeMap::new(), Vec::new());
                c.amount = v.as_finite();
                s.corrections = vec![c];
            } else {
                s.corrections = vec![correction([(id, v.clone())].into(), Vec::new())];
            }
            format!("MaskGroupBasedCorrections[0].{}", spec.name)
        }
        Level::MaskTool if spec.name == "What" => {
            let mut c = bare_component(Fields::default());
            c.tool = MaskTool::Opaque {
                what: Some(v.as_str()?.to_owned()),
            };
            s.corrections = vec![correction(BTreeMap::new(), vec![c])];
            format!("{m}.What")
        }
        Level::MaskTool => {
            s = in_component(id, v.clone());
            format!("{m}.{}", spec.name)
        }
        Level::Struct(StructKind::Look) => {
            let mut look = Look {
                name: None,
                uuid: None,
                amount: None,
                camera_restriction: None,
                rest: Struct::new(StructKind::Look),
            };
            match spec.name {
                "Name" => look.name = Some(v.as_str()?.to_owned()),
                "UUID" => look.uuid = Some(Hex32::parse(v.as_str()?)?),
                "Amount" => look.amount = v.as_finite(),
                _ => return None,
            }
            s.look = Some(look);
            format!("Look.{}", spec.name)
        }
        Level::Struct(StructKind::LensBlur) => {
            let lens_blur = one(StructKind::LensBlur, id, v.clone());
            s.insert(kid(Level::Global, "LensBlur"), Value::Struct(lens_blur))
                .unwrap();
            format!("LensBlur.{}", spec.name)
        }
        Level::Struct(StructKind::CorrectionRangeMask) => {
            let crm = one(StructKind::CorrectionRangeMask, id, v.clone());
            s = in_component(
                kid(Level::MaskTool, "CorrectionRangeMask"),
                Value::Struct(crm),
            );
            format!("{m}.CorrectionRangeMask.{}", spec.name)
        }
        Level::Struct(StructKind::AreaModel) => {
            let area = one(StructKind::AreaModel, id, v.clone());
            let crm = one(
                StructKind::CorrectionRangeMask,
                kid(Level::Struct(StructKind::CorrectionRangeMask), "AreaModels"),
                Value::StructList(vec![area]),
            );
            s = in_component(
                kid(Level::MaskTool, "CorrectionRangeMask"),
                Value::Struct(crm),
            );
            format!("{m}.CorrectionRangeMask.AreaModels[0].{}", spec.name)
        }
        Level::Struct(StructKind::Gesture) => {
            let gesture = one(StructKind::Gesture, id, v.clone());
            s = in_component(
                kid(Level::MaskTool, "Gesture"),
                Value::StructList(vec![gesture]),
            );
            format!("{m}.Gesture[0].{}", spec.name)
        }
        Level::Struct(StructKind::GesturePoint) => {
            let point = one(StructKind::GesturePoint, id, v.clone());
            let gesture = one(
                StructKind::Gesture,
                kid(Level::Struct(StructKind::Gesture), "Points"),
                Value::StructList(vec![point]),
            );
            s = in_component(
                kid(Level::MaskTool, "Gesture"),
                Value::StructList(vec![gesture]),
            );
            format!("{m}.Gesture[0].Points[0].{}", spec.name)
        }
        _ => return None,
    };
    Some((s, path))
}

/// The leaf [`flat`] shows for `v` at `id` (typed fields are numbers).
pub fn leaf(id: KeyId, v: &Value) -> flat::Leaf {
    let spec = id.spec();
    let typed_number = matches!(
        (spec.level, spec.name),
        (Level::Correction, "CorrectionAmount") | (Level::Struct(StructKind::Look), "Amount")
    );
    match v.as_finite() {
        Some(x) if typed_number => flat::Leaf::Number(x),
        _ => flat::Leaf::Value(v.clone()),
    }
}

pub fn learn_or_photo(spec: &KeySpec) -> bool {
    spec.policy.is_learnable() || spec.policy == Policy::Photo
}
