//! Rules the Lua and XMP readers share, so both land in the same model.
//!
//! Each reader walks its own syntax (a decoded JSON.lua table, an XMP tree)
//! and resolves keys through the registry. From there on the decisions are
//! made here, once: numbers and flags go through [`KeySpec::coerce`] (with
//! the same warnings), closed sets and ids are checked the same way, curve
//! points parse the same way, and [`finish`] turns the global fields into
//! [`DevelopSettings`]: typed corrections and `Look`, the file kind from the
//! white-balance family, the process-version check. Correction and mask
//! classification lives in [`crate::model::correction`] and is shared the
//! same way.

use crate::model::value::{Fields, Finite, Hex32, Opaque, OpaqueEntry, Value};
use crate::model::{DevelopSettings, FileKind, FileKindHint, Look, WbFamily, MIN_SUPPORTED_PV};
use crate::parse::{ParseWarning, WarningKind};
use crate::registry::{CoerceError, KeySpec, Level, ProcessVersion, Scalar, ValueKind};

/// Appends one warning.
pub(crate) fn warn(
    warnings: &mut Vec<ParseWarning>,
    path: &str,
    key: &str,
    raw: Option<String>,
    kind: WarningKind,
) {
    warnings.push(ParseWarning {
        path: path.to_owned(),
        key: key.to_owned(),
        raw,
        kind,
    });
}

/// `path.key`, or `key` at the top.
pub(crate) fn child(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

/// A scalar as the input spelled it, before the registry types it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ScalarIn {
    /// `true`/`false` (JSON), `True`/`true`/`False`/`false` (XMP).
    Bool(bool),
    /// A finite number.
    Num(Finite),
}

/// Types a scalar for `spec` (a numeric or boolean kind) through
/// [`KeySpec::coerce`]: a rounded integer is kept with a
/// [`WarningKind::NonIntegerForIntKey`], an integer outside its enum set is
/// kept with a [`WarningKind::ValueNotInSet`]; `None` when the value cannot
/// take the key's kind at all (the caller keeps it verbatim).
pub(crate) fn scalar(
    spec: &KeySpec,
    input: ScalarIn,
    path: &str,
    key: &str,
    raw: impl Fn() -> String,
    warnings: &mut Vec<ParseWarning>,
) -> Option<Value> {
    let coerced = match input {
        ScalarIn::Bool(b) => spec.coerce_bool(b),
        ScalarIn::Num(x) => spec.coerce(x),
    };
    match coerced {
        Ok(c) => {
            if let (Scalar::Int(i), true) = (c.value, c.rounded) {
                warn(
                    warnings,
                    path,
                    key,
                    Some(raw()),
                    WarningKind::NonIntegerForIntKey { rounded: i },
                );
            }
            Some(Value::from(c.value))
        }
        // Observed truth outside the known set: keep it, but say so.
        Err(CoerceError::NotInEnum { value, .. }) => {
            warn(warnings, path, key, Some(raw()), WarningKind::ValueNotInSet);
            Some(Value::Int(value))
        }
        Err(_) => None,
    }
}

/// Types a text value for a text kind (`Enum`, `Str`, `VersionStr`,
/// `Hex32`): a label outside the enum set or an id that is not 32 hex digits
/// is kept as text with a warning. `None` for any other kind.
pub(crate) fn text(
    kind: ValueKind,
    s: &str,
    path: &str,
    key: &str,
    raw: impl Fn() -> String,
    warnings: &mut Vec<ParseWarning>,
) -> Option<Value> {
    match kind {
        ValueKind::Enum(set) => {
            if !set.contains(&s) {
                warn(warnings, path, key, Some(raw()), WarningKind::ValueNotInSet);
            }
        }
        ValueKind::Hex32 => {
            if Hex32::parse(s).is_none() {
                warn(warnings, path, key, Some(raw()), WarningKind::NotHex32);
            }
        }
        ValueKind::Str | ValueKind::VersionStr => {}
        _ => return None,
    }
    Some(Value::Str(s.to_owned()))
}

/// One curve point, `"x, y"` (global curves in XMP) or `"x,y"` (local
/// curves, and every Lua curve string).
pub(crate) fn parse_point(s: &str) -> Option<(Finite, Finite)> {
    let (x, y) = s.split_once(',')?;
    let num = |t: &str| {
        t.trim()
            .parse::<f64>()
            .ok()
            .and_then(|v| Finite::new(v).ok())
    };
    Some((num(x)?, num(y)?))
}

/// Builds the settings from the global fields a reader produced: the typed
/// `MaskGroupBasedCorrections` and `Look`, the file kind (the white-balance
/// family wins over `hint`, with a warning when they disagree) and the
/// process-version check.
pub(crate) fn finish(
    mut fields: Fields,
    hint: FileKindHint,
    warnings: &mut Vec<ParseWarning>,
) -> DevelopSettings {
    let mut settings = DevelopSettings::default();
    let mut kept_opaque = Vec::new();
    match fields.take(Level::Global, "MaskGroupBasedCorrections") {
        Some(Value::Corrections(corrections)) => settings.corrections = corrections,
        // A form the model keeps whole (an XMP `rdf:Bag`, qualifiers).
        Some(Value::Opaque(o)) => kept_opaque.push(opaque_entry("MaskGroupBasedCorrections", o)),
        Some(_) => unreachable!("the readers only build corrections for a CorrectionSeq key"),
        None => {}
    }
    match fields.take(Level::Global, "Look") {
        Some(Value::Struct(s)) => settings.look = Some(Look::from_struct(s)),
        Some(Value::Opaque(o)) => kept_opaque.push(opaque_entry("Look", o)),
        Some(_) => unreachable!("the readers only build a struct for a Struct key"),
        None => {}
    }
    for (id, v) in fields.values {
        settings
            .insert(id, v)
            .expect("global keys without the typed ones");
    }
    settings.opaque = fields.opaque;
    settings.opaque.extend(kept_opaque);
    file_kind(&mut settings, hint, warnings);
    process_version(&settings, warnings);
    settings
}

fn opaque_entry(name: &str, value: Opaque) -> OpaqueEntry {
    OpaqueEntry {
        ns: None,
        name: name.to_owned(),
        value,
    }
}

fn file_kind(settings: &mut DevelopSettings, hint: FileKindHint, warnings: &mut Vec<ParseWarning>) {
    let (raw, non_raw) = WbFamily::present_in(settings);
    let family = match (raw, non_raw) {
        (true, false) => Some(FileKind::Raw),
        (false, true) => Some(FileKind::NonRaw),
        (true, true) => {
            // Name the keys that clash, one of each family
            // ("Temperature/IncrementalTint").
            let first = |family| {
                WbFamily::keys_in(settings)
                    .into_iter()
                    .find(|(_, f)| *f == family)
                    .map_or("", |(k, _)| k)
            };
            let keys = format!("{}/{}", first(WbFamily::Raw), first(WbFamily::NonRaw));
            warn(
                warnings,
                &keys,
                &keys,
                None,
                WarningKind::ConflictingFileKind,
            );
            None
        }
        (false, false) => None,
    };
    if let (Some(family), Some(hinted)) = (family, hint.kind()) {
        if family != hinted {
            let key = WbFamily::from(family).temperature_key();
            warn(
                warnings,
                key,
                key,
                None,
                WarningKind::FileKindMismatch {
                    hint: hinted,
                    family,
                },
            );
        }
    }
    settings.file_kind = family.or(hint.kind());
}

fn process_version(settings: &DevelopSettings, warnings: &mut Vec<ParseWarning>) {
    const KEY: &str = "ProcessVersion";
    let Some(Value::Str(text)) = settings.get_by_name(KEY) else {
        return;
    };
    match ProcessVersion::parse(text) {
        None => warn(
            warnings,
            KEY,
            KEY,
            Some(text.clone()),
            WarningKind::InvalidProcessVersion,
        ),
        Some(pv) if pv < MIN_SUPPORTED_PV => warn(
            warnings,
            KEY,
            KEY,
            Some(text.clone()),
            WarningKind::UnsupportedProcessVersion { found: pv },
        ),
        Some(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_points_parse_with_and_without_a_space() {
        let p = |x: f64, y: f64| Some((Finite::new_const(x), Finite::new_const(y)));
        assert_eq!(parse_point("0, 0"), p(0.0, 0.0));
        assert_eq!(parse_point("64,58"), p(64.0, 58.0));
        assert_eq!(parse_point("0.25,0.5"), p(0.25, 0.5));
        assert_eq!(parse_point("1"), None);
        assert_eq!(parse_point("a, b"), None);
    }

    #[test]
    fn text_checks_sets_and_ids_but_keeps_the_value() {
        let mut w = Vec::new();
        let raw = || "x".to_owned();
        let v = text(ValueKind::Enum(&["A"]), "B", "k", "k", raw, &mut w);
        assert_eq!(v, Some(Value::Str("B".into())));
        let v = text(ValueKind::Hex32, "12", "k", "k", raw, &mut w);
        assert_eq!(v, Some(Value::Str("12".into())));
        let kinds: Vec<_> = w.iter().map(|w| w.kind.name()).collect();
        assert_eq!(kinds, ["ValueNotInSet", "NotHex32"]);
        assert_eq!(text(ValueKind::Int, "1", "k", "k", raw, &mut w), None);
    }
}
