//! Helpers shared by the local-only sweeps (`training_dump_local.rs`,
//! `xmp_goldens_local.rs`): counting what a parsed model holds without ever
//! printing a value.
//!
//! The sweeps run over private data (the maintainer's training dump, a
//! sidecar corpus) and Adobe's own preset files, so everything here reduces
//! a model to key names and counts.

use std::collections::BTreeMap;

use lrg_develop::model::{
    Combine, Correction, DevelopSettings, Fields, Finite, MaskTool, Semantic, Value,
};
use lrg_develop::registry::{lookup, KeySpec, Level, StructKind};

/// A mask tool as a counting label (`"semantic:sky"`, `"linear"`, ...).
pub fn tool_name(tool: &MaskTool) -> String {
    match tool {
        MaskTool::Semantic(Semantic::Subject) => "semantic:subject".into(),
        MaskTool::Semantic(Semantic::Sky) => "semantic:sky".into(),
        MaskTool::Semantic(Semantic::Background) => "semantic:background".into(),
        MaskTool::Semantic(Semantic::PeoplePart(_)) => "semantic:people-part".into(),
        MaskTool::Semantic(Semantic::Landscape(_)) => "semantic:landscape".into(),
        MaskTool::Semantic(Semantic::PersonPartAt { .. }) => "semantic:person-part-at".into(),
        MaskTool::Linear(_) => "linear".into(),
        MaskTool::Radial(_) => "radial".into(),
        MaskTool::LuminanceRange(_) => "luminance-range".into(),
        MaskTool::Opaque { what } => format!("opaque:{}", what.as_deref().unwrap_or("?")),
    }
}

/// A mask combination as a counting label.
pub fn combine_name(c: Combine) -> &'static str {
    match c {
        Combine::Add { inverted: false } => "add",
        Combine::Add { inverted: true } => "add-inverted",
        Combine::Subtract => "subtract",
        Combine::Intersect => "intersect",
        Combine::Unrecognised => "unrecognised",
    }
}

/// A warning path with its indices removed
/// (`MaskGroupBasedCorrections[2].CorrectionMasks[0].What` →
/// `MaskGroupBasedCorrections[].CorrectionMasks[].What`), so one key counts
/// once however often it appears.
pub fn generic_path(path: &str) -> String {
    path.split('[')
        .map(|part| part.split_once(']').map_or(part, |(_, rest)| rest))
        .collect::<Vec<_>>()
        .join("[]")
}

/// Mask tools and combinations over the corrections of one model, counted.
#[derive(Default)]
pub struct MaskCounts {
    /// Corrections seen.
    pub corrections: usize,
    /// Mask components per tool label.
    pub tools: BTreeMap<String, usize>,
    /// Mask components per combination label.
    pub combines: BTreeMap<&'static str, usize>,
    /// The fixed relations and forms the builders and the preset writer
    /// rely on, counted over Lightroom's own components: a radial
    /// gradient's `Flipped` against `MaskInverted`, a luminance range's
    /// `CorrectionRangeMask.Invert` against `MaskInverted`, its `Version`
    /// and its `SampleType` (a typed luminance range is `Type` 2).
    pub forms: BTreeMap<String, usize>,
}

impl MaskCounts {
    /// Counts the corrections of `s`.
    pub fn add(&mut self, s: &DevelopSettings) {
        self.corrections += s.corrections.len();
        for m in s.corrections.iter().flat_map(|c| &c.masks) {
            *self.tools.entry(tool_name(&m.tool)).or_default() += 1;
            *self.combines.entry(combine_name(m.combine)).or_default() += 1;
            let inverted = m.combine.encode().map(|(_, _, inverted)| inverted);
            let mut form = |f: String| *self.forms.entry(f).or_default() += 1;
            match &m.tool {
                MaskTool::Radial(g) => form(
                    if Some(!g.flipped) == inverted {
                        "radial: Flipped = !MaskInverted"
                    } else {
                        "radial: Flipped != !MaskInverted"
                    }
                    .into(),
                ),
                MaskTool::LuminanceRange(lr) => {
                    form(
                        if Some(lr.invert) == inverted {
                            "luminance range: Invert = MaskInverted"
                        } else {
                            "luminance range: Invert != MaskInverted"
                        }
                        .into(),
                    );
                    let int = |n| {
                        lr.rest
                            .get(n)
                            .and_then(Value::as_int)
                            .map_or_else(|| "none".to_owned(), |v| v.to_string())
                    };
                    form(format!("luminance range: Version {}", int("Version")));
                    form(format!("luminance range: SampleType {}", int("SampleType")));
                }
                _ => {}
            }
        }
    }

    /// The counted forms that break a relation the builders rely on:
    /// `Flipped` other than `!MaskInverted`, `Invert` other than
    /// `MaskInverted`, a range mask `Version` other than 3.
    pub fn broken_forms(&self) -> Vec<(&String, &usize)> {
        self.forms
            .iter()
            .filter(|(f, _)| {
                f.contains("!= ")
                    || (f.starts_with("luminance range: Version ") && !f.ends_with(" 3"))
            })
            .collect()
    }
}

/// Counts, per key (`level/name`), the numbers outside the key's registry
/// range. Keys and counts only.
#[derive(Default)]
pub struct RangeCheck {
    /// Numbers outside their key's range, per key.
    pub outside: BTreeMap<String, usize>,
}

impl RangeCheck {
    fn number(&mut self, spec: &KeySpec, x: Finite) {
        if let Some((min, max)) = spec.range {
            if x < min || x > max {
                *self
                    .outside
                    .entry(format!("{}/{}", spec.level, spec.name))
                    .or_default() += 1;
            }
        }
    }

    fn value(&mut self, spec: &KeySpec, v: &Value) {
        match v {
            Value::Int(_) | Value::Real(_) => self.number(spec, v.as_finite().unwrap()),
            Value::Struct(s) => self.fields(&s.fields),
            Value::StructList(items) => items.iter().for_each(|s| self.fields(&s.fields)),
            Value::Tools(items) => items.iter().for_each(|f| self.fields(f)),
            Value::Corrections(cs) => cs.iter().for_each(|c| self.correction(c)),
            _ => {}
        }
    }

    fn fields(&mut self, f: &Fields) {
        for (id, v) in &f.values {
            self.value(id.spec(), v);
        }
    }

    fn correction(&mut self, c: &Correction) {
        if let Some(a) = c.amount {
            self.number(
                lookup(Level::Correction, "CorrectionAmount")
                    .unwrap()
                    .spec(),
                a,
            );
        }
        for (id, v) in &c.local {
            self.value(id.spec(), v);
        }
        self.fields(&c.extra);
        for m in &c.masks {
            self.fields(&m.extra);
            if let MaskTool::LuminanceRange(lr) = &m.tool {
                self.fields(&lr.rest.fields);
            }
        }
    }

    /// Checks every number in `s`, corrections and `Look` included.
    pub fn settings(&mut self, s: &DevelopSettings) {
        for (id, v) in s.values() {
            self.value(id.spec(), v);
        }
        for c in &s.corrections {
            self.correction(c);
        }
        if let Some(look) = &s.look {
            if let Some(a) = look.amount {
                let spec = lookup(Level::Struct(StructKind::Look), "Amount").unwrap();
                self.number(spec.spec(), a);
            }
            self.fields(&look.rest.fields);
        }
    }
}
