//! How the XMP writer spells values: numbers per the registry's [`NumFmt`]
//! and `plus_sign`, booleans per [`BoolStyle`], curve points, compound number
//! strings, ids, packed versions and XML escaping.
//!
//! Every rule here is what Lightroom Classic and Camera Raw write in their own
//! presets and sidecars (the registry's serialization notes, checked against
//! the bundled presets):
//!
//! | Rule | Example |
//! |---|---|
//! | `NumFmt::Int`: no decimal point | `Temperature` 3569 → `3569` |
//! | `NumFmt::Fixed(n)`: exactly `n` decimals | `Exposure2012` 0.5 → `+0.50`, `SharpenRadius` 1 → `+1.0` |
//! | `NumFmt::Trim6`: up to 6 decimals, trailing zeros trimmed | `LocalExposure2012` 0.0625 → `0.0625`, `CorrectionAmount` 1 → `1` |
//! | `NumFmt::CompoundFixed6`: every number `%.6f`, one space apart | `ReferencePoint` → `0.500000 0.500000` |
//! | `+` only on `plus_sign` keys at the global level, only above 0 | `Tint` 6 → `+6`, 0 → `0`, -6 → `-6` |
//! | never `-0` | -0.001 at 2 decimals → `0.00` |
//! | booleans: `True`/`False` global and header, `true`/`false` in structures | `CorrectionActive` → `true` |
//! | global curve points `x, y`, local curve points `x,y` | `0, 0` / `62,53` |
//! | ids: 32 upper-case hex digits | `CorrectionSyncID` |
//! | packed versions: `major << 24 \| minor << 16` | 15.3 → `251854848` |
//!
//! **Lossless mode** (the writer's test round trip): a number the registry
//! format would change (an `Exposure2012` of 0.333 at `Fixed(2)`) is written
//! as the shortest text that reads back as the same `f64` instead, so the
//! round trip compares equal. Lightroom never writes such a number; the
//! preset writer quantizes as the table says.

use crate::model::value::Finite;
use crate::registry::{round_half_even, BoolStyle, CurveKind, KeySpec, Level, NumFmt};

/// A string cannot be written into XML 1.0 at all (a control character other
/// than tab, line feed and carriage return; `U+FFFE`/`U+FFFF`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("character U+{0:04X} cannot appear in an XML 1.0 document")]
pub struct InvalidXmlChar(pub u32);

fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// Escapes `s` for an attribute value in double quotes: the five XML
/// entities, and tab, line feed and carriage return as character references
/// (a parser normalises them to spaces in an attribute otherwise).
pub fn escape_attr(s: &str) -> Result<String, InvalidXmlChar> {
    escape(s, true)
}

/// Escapes `s` for element text: the five XML entities, and carriage return
/// as a character reference (a parser turns a literal one into a line feed).
pub fn escape_text(s: &str) -> Result<String, InvalidXmlChar> {
    escape(s, false)
}

fn escape(s: &str, attr: bool) -> Result<String, InvalidXmlChar> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if !is_xml_char(c) {
            return Err(InvalidXmlChar(u32::from(c)));
        }
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => out.push_str("&#13;"),
            '\t' if attr => out.push_str("&#9;"),
            '\n' if attr => out.push_str("&#10;"),
            c => out.push(c),
        }
    }
    Ok(out)
}

/// `True`/`False` or `true`/`false`.
pub fn format_bool(style: BoolStyle, b: bool) -> &'static str {
    match (style, b) {
        (BoolStyle::TitleCase, true) => "True",
        (BoolStyle::TitleCase, false) => "False",
        (BoolStyle::Lower, true) => "true",
        (BoolStyle::Lower, false) => "false",
    }
}

/// Whether a positive value of `spec` gets a `+`: `plus_sign` keys at the
/// global level only (the registry allows the flag nowhere else, and a
/// structure never carries a sign).
fn signed(spec: &KeySpec) -> bool {
    spec.plus_sign && spec.level == Level::Global
}

/// The shortest text that parses back to exactly `x` (Rust's `Display` for
/// `f64`: round-trip exact and never in exponent notation).
fn shortest(x: Finite) -> String {
    format!("{}", x.get())
}

/// `x` with `decimals` fixed decimals, never `-0…`.
fn fixed(x: Finite, decimals: u8) -> String {
    let s = format!("{:.*}", usize::from(decimals), x.get());
    match s.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|b| b == b'0' || b == b'.') => rest.to_owned(),
        _ => s,
    }
}

/// Up to 6 decimals, trailing zeros (and a trailing point) trimmed.
fn trim6(x: Finite) -> String {
    let s = fixed(x, 6);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

/// Keeps `text` when it reads back as `x`; otherwise (lossless mode only)
/// the shortest exact text.
fn exact_or(text: String, x: Finite, lossless: bool) -> String {
    if lossless && text.parse::<f64>().ok() != Some(x.get()) {
        shortest(x)
    } else {
        text
    }
}

/// An integer as a number of `spec`, with its `+` rule.
pub fn format_int(spec: &KeySpec, i: i64) -> String {
    if i > 0 && signed(spec) {
        format!("+{i}")
    } else {
        i.to_string()
    }
}

/// A number of `spec` per its [`NumFmt`] and `+` rule.
///
/// `NumFmt::Int` rounds half-to-even (a real value on an integer format only
/// reaches the writer through a curve point); `NumFmt::Text` (keys that are
/// no number in XMP, or not written to XMP at all) uses the shortest exact
/// text. With `lossless`, a format that would change the value falls back to
/// the shortest exact text.
pub fn format_real(spec: &KeySpec, x: Finite, lossless: bool) -> String {
    let body = number_body(spec.fmt, x, lossless);
    if x > Finite::ZERO && signed(spec) {
        format!("+{body}")
    } else {
        body
    }
}

/// A number per `fmt`, without a sign rule.
fn number_body(fmt: NumFmt, x: Finite, lossless: bool) -> String {
    let text = match fmt {
        NumFmt::Int => match round_half_even(x) {
            Some(i) => i.to_string(),
            None => shortest(x),
        },
        NumFmt::Fixed(n) => fixed(x, n),
        NumFmt::Trim6 => trim6(x),
        NumFmt::CompoundFixed6 => fixed(x, 6),
        NumFmt::Text => shortest(x),
    };
    exact_or(text, x, lossless)
}

/// One curve point: `"x, y"` for a global curve, `"x,y"` for a local one;
/// each coordinate in the curve key's number format (integers).
pub fn format_curve_point(
    kind: CurveKind,
    fmt: NumFmt,
    (x, y): (Finite, Finite),
    lossless: bool,
) -> String {
    let sep = match kind {
        CurveKind::Global => ", ",
        CurveKind::Local => ",",
    };
    format!(
        "{}{sep}{}",
        number_body(fmt, x, lossless),
        number_body(fmt, y, lossless)
    )
}

/// Numbers as one compound string: each `%.6f`, one space apart
/// (`ReferencePoint`, `LumRange`).
pub fn format_compound(values: &[Finite], lossless: bool) -> String {
    values
        .iter()
        .map(|x| exact_or(fixed(*x, 6), *x, lossless))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Re-spells a compound number string (`"0.5 0.5"` → `"0.500000
/// 0.500000"`); `None` when a part is not a finite number or there is none.
pub fn reformat_compound(s: &str) -> Option<String> {
    let values = s
        .split_whitespace()
        .map(|t| t.parse::<f64>().ok().and_then(|x| Finite::new(x).ok()))
        .collect::<Option<Vec<_>>>()?;
    (!values.is_empty()).then(|| format_compound(&values, false))
}

/// 32 hex digits upper-cased; `None` for anything else.
pub fn hex32_upper(s: &str) -> Option<String> {
    (s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_ascii_uppercase())
}

/// A Camera Raw engine version (`crs:Version`, `crs:CompatibleVersion`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EngineVersion {
    /// Major part (`18` in 18.5).
    pub major: u8,
    /// Minor part (`5` in 18.5).
    pub minor: u8,
}

impl EngineVersion {
    /// `major.minor`.
    pub const fn new(major: u8, minor: u8) -> EngineVersion {
        EngineVersion { major, minor }
    }

    /// The packed form of `crs:CompatibleVersion`: `major << 24 | minor << 16`
    /// (15.3 → 251854848).
    pub const fn packed(self) -> i64 {
        ((self.major as i64) << 24) | ((self.minor as i64) << 16)
    }
}

impl std::fmt::Display for EngineVersion {
    /// `crs:Version`'s form, `"18.5"`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::lookup;

    fn spec(level: Level, name: &str) -> &'static KeySpec {
        lookup(level, name).unwrap().spec()
    }

    fn g(name: &str) -> &'static KeySpec {
        spec(Level::Global, name)
    }

    fn f(x: f64) -> Finite {
        Finite::new(x).unwrap()
    }

    #[test]
    fn plan_examples_are_spelled_as_lightroom_does() {
        assert_eq!(format_real(g("Exposure2012"), f(0.5), false), "+0.50");
        assert_eq!(format_int(g("Tint"), 6), "+6");
        assert_eq!(format_int(g("Temperature"), 3569), "3569");
        assert_eq!(
            format_real(
                spec(Level::Correction, "LocalExposure2012"),
                f(0.0625),
                false
            ),
            "0.0625"
        );
        assert_eq!(format_real(g("SharpenRadius"), f(1.0), false), "+1.0");
    }

    #[test]
    fn fixed_formats_keep_their_decimals_and_sign_rule() {
        let exposure = g("Exposure2012");
        assert_eq!(format_real(exposure, f(0.0), false), "0.00");
        assert_eq!(format_real(exposure, f(-0.5), false), "-0.50");
        assert_eq!(format_real(exposure, f(1.234), false), "+1.23");
        assert_eq!(format_real(exposure, f(-0.001), false), "0.00", "never -0");
        assert_eq!(format_real(g("SharpenRadius"), f(0.5), false), "+0.5");
        assert_eq!(format_real(g("PerspectiveRotate"), f(0.0), false), "0.0");
        assert_eq!(format_real(g("HDRMaxValue"), f(2.0), false), "+2.00");
    }

    #[test]
    fn integers_are_signed_only_on_plus_sign_globals_above_zero() {
        assert_eq!(format_int(g("Contrast2012"), 15), "+15");
        assert_eq!(format_int(g("Contrast2012"), 0), "0");
        assert_eq!(format_int(g("Contrast2012"), -50), "-50");
        assert_eq!(format_int(g("IncrementalTint"), 34), "+34");
        assert_eq!(format_int(g("Temperature"), 12500), "12500");
        // Inside a structure a key never gets a sign.
        assert_eq!(format_int(spec(Level::MaskTool, "Feather"), 50), "50");
        assert_eq!(format_int(g("CompatibleVersion"), 251854848), "251854848");
    }

    #[test]
    fn no_key_below_the_global_level_is_ever_signed() {
        for (_, s) in crate::registry::iter().filter(|(_, s)| s.level != Level::Global) {
            assert_eq!(format_int(s, 7), "7", "{}/{}", s.level, s.name);
            assert!(
                !format_real(s, f(0.5), false).starts_with('+'),
                "{}",
                s.name
            );
        }
    }

    #[test]
    fn trim6_trims_trailing_zeros_and_rounds_at_six_decimals() {
        let local = spec(Level::Correction, "LocalClarity2012");
        assert_eq!(format_real(local, f(0.153914), false), "0.153914");
        assert_eq!(format_real(local, f(1.0), false), "1");
        assert_eq!(format_real(local, f(0.0), false), "0");
        assert_eq!(format_real(local, f(-0.25), false), "-0.25");
        assert_eq!(format_real(local, f(0.1234567), false), "0.123457");
        assert_eq!(format_real(local, f(-0.0000001), false), "0", "never -0");
        assert_eq!(format_real(g("CropTop"), f(0.1), false), "0.1", "no sign");
        assert_eq!(
            format_real(spec(Level::Correction, "CorrectionAmount"), f(1.0), false),
            "1"
        );
    }

    #[test]
    fn lossless_mode_keeps_what_the_format_would_round() {
        let exposure = g("Exposure2012");
        assert_eq!(format_real(exposure, f(0.333), false), "+0.33");
        assert_eq!(format_real(exposure, f(0.333), true), "+0.333");
        assert_eq!(
            format_real(exposure, f(0.5), true),
            "+0.50",
            "an exact value keeps its format"
        );
        let local = spec(Level::Correction, "LocalClarity2012");
        assert_eq!(format_real(local, f(0.1234567), true), "0.1234567");
        assert_eq!(
            format_real(local, f(1e-7), true),
            "0.0000001",
            "no exponent"
        );
    }

    #[test]
    fn booleans_follow_the_level_style() {
        assert_eq!(format_bool(BoolStyle::TitleCase, true), "True");
        assert_eq!(format_bool(BoolStyle::TitleCase, false), "False");
        assert_eq!(format_bool(BoolStyle::Lower, true), "true");
        assert_eq!(format_bool(BoolStyle::Lower, false), "false");
    }

    #[test]
    fn curve_points_use_a_space_only_on_global_curves() {
        let p = (f(0.0), f(0.0));
        assert_eq!(
            format_curve_point(CurveKind::Global, NumFmt::Int, p, false),
            "0, 0"
        );
        assert_eq!(
            format_curve_point(CurveKind::Local, NumFmt::Int, (f(62.0), f(53.0)), false),
            "62,53"
        );
        assert_eq!(
            format_curve_point(CurveKind::Global, NumFmt::Int, (f(64.4), f(58.5)), false),
            "64, 58",
            "rounded half-to-even in a preset"
        );
        assert_eq!(
            format_curve_point(CurveKind::Local, NumFmt::Int, (f(64.4), f(58.0)), true),
            "64.4,58"
        );
    }

    #[test]
    fn compound_strings_are_six_decimals_space_separated() {
        assert_eq!(
            format_compound(&[f(0.5), f(0.5)], false),
            "0.500000 0.500000"
        );
        assert_eq!(
            format_compound(&[f(0.0), f(0.2), f(0.8), f(1.0)], false),
            "0.000000 0.200000 0.800000 1.000000"
        );
        assert_eq!(
            reformat_compound("0.5  0.25").as_deref(),
            Some("0.500000 0.250000")
        );
        assert_eq!(reformat_compound("0.5 x"), None);
        assert_eq!(reformat_compound(""), None);
        assert_eq!(format_compound(&[f(0.1234567)], true), "0.1234567");
    }

    #[test]
    fn ids_are_upper_case_hex32() {
        assert_eq!(
            hex32_upper("00000000000000000000000000000a0b").as_deref(),
            Some("00000000000000000000000000000A0B")
        );
        assert_eq!(hex32_upper("0000"), None);
        assert_eq!(hex32_upper("0000000000000000000000000000000g"), None);
    }

    #[test]
    fn engine_versions_pack_like_compatible_version() {
        assert_eq!(EngineVersion::new(15, 3).packed(), 251_854_848);
        assert_eq!(EngineVersion::new(14, 0).packed(), 234_881_024);
        assert_eq!(EngineVersion::new(17, 4).packed(), 285_474_816);
        assert_eq!(EngineVersion::new(18, 5).to_string(), "18.5");
    }

    #[test]
    fn escaping_covers_the_five_entities_and_whitespace_in_attributes() {
        assert_eq!(
            escape_attr(r#"a&b<c>d"e'f"#).unwrap(),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
        assert_eq!(
            escape_text(r#"a&b<c>d"e'f"#).unwrap(),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
        assert_eq!(escape_attr("a\tb\nc\rd").unwrap(), "a&#9;b&#10;c&#13;d");
        assert_eq!(escape_text("a\tb\nc\rd").unwrap(), "a\tb\nc&#13;d");
        assert_eq!(
            escape_text("Äpfel · 🍎").unwrap(),
            "Äpfel · 🍎",
            "UTF-8 unchanged"
        );
        assert_eq!(escape_text("a\u{1}b"), Err(InvalidXmlChar(1)));
        assert_eq!(escape_attr("\u{FFFE}"), Err(InvalidXmlChar(0xFFFE)));
    }
}
