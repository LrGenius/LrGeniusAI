//! Key families the registry covers by name pattern instead of one row per key.
//!
//! Every family here is COMPUTED or NEVER: owned by Lightroom or authored per
//! photo, never learned and never written outside a test round trip. Their
//! values stay opaque ([`ValueKind::Any`]), so the readers keep them verbatim
//! and do not descend into them.

use super::{FrameScope, Level, Policy, Presence, StructKind, ValueKind};

/// How a pattern matches a key name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Matcher {
    /// The name starts with this prefix (and is longer than it).
    Prefix(&'static str),
    /// The prefix followed by exactly 32 hex characters (`Table_<md5>`).
    PrefixHex32(&'static str),
    /// The prefix followed by a decimal index (`UprightTransform_0`).
    PrefixIndex(&'static str),
    /// Any name (every field of an opaque structure).
    AnyName,
}

impl Matcher {
    /// Whether `name` belongs to this family.
    pub fn matches(self, name: &str) -> bool {
        match self {
            Matcher::Prefix(p) => name.len() > p.len() && name.starts_with(p),
            Matcher::PrefixHex32(p) => name.strip_prefix(p).is_some_and(|rest| {
                rest.len() == 32 && rest.bytes().all(|b| b.is_ascii_hexdigit())
            }),
            Matcher::PrefixIndex(p) => name
                .strip_prefix(p)
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())),
            Matcher::AnyName => true,
        }
    }
}

/// One key family.
#[derive(Clone, Copy, Debug)]
pub struct PatternSpec {
    /// Stable identifier, used in the registry snapshot and in warnings.
    pub id: &'static str,
    /// Levels the family occurs at; empty means every level.
    pub levels: &'static [Level],
    /// Name matcher.
    pub matcher: Matcher,
    /// Value type (always [`ValueKind::Any`] or a plain scalar).
    pub kind: ValueKind,
    /// Policy class (COMPUTED or NEVER).
    pub policy: Policy,
    /// Frame scope.
    pub frame: FrameScope,
    /// Lua, XMP or both.
    pub presence: Presence,
    /// What the family is.
    pub note: &'static str,
}

impl PatternSpec {
    /// Whether `name` at `level` belongs to this family.
    pub fn matches(&self, level: Level, name: &str) -> bool {
        (self.levels.is_empty() || self.levels.contains(&level)) && self.matcher.matches(name)
    }
}

/// The pattern families, checked in this order after the table.
pub static PATTERNS: &[PatternSpec] = &[
    PatternSpec {
        id: "table_blob",
        levels: &[],
        matcher: Matcher::PrefixHex32("Table_"),
        kind: ValueKind::Any,
        policy: Policy::Computed,
        frame: FrameScope::NotApplicable,
        presence: Presence::Both,
        note: "Base85 + zlib payloads named after their MD5: profile lookup tables, legacy \
               inline mask bitmaps, Denoise/Enhance results. Never written.",
    },
    PatternSpec {
        id: "brush_table_blob",
        levels: &[Level::MaskTool],
        matcher: Matcher::PrefixHex32("BrushTable_"),
        kind: ValueKind::Any,
        policy: Policy::Computed,
        frame: FrameScope::NotApplicable,
        presence: Presence::LuaOnly,
        note: "Brush-stroke payload of a Mask/Aggregate in getDevelopSettings(), next to \
               MaskBrushTable. Never written.",
    },
    PatternSpec {
        id: "patch_state",
        levels: &[Level::Struct(StructKind::RetouchArea)],
        matcher: Matcher::Prefix("pm_"),
        kind: ValueKind::Any,
        policy: Policy::Never,
        frame: FrameScope::PerFrameOnly,
        presence: Presence::Both,
        note: "Content-aware / generative patch state of a retouch or remove spot (pm_patch, \
               pm_patch_variations, pm_remap_info_*, pm_search_*, pm_target_*, ...).",
    },
    PatternSpec {
        id: "upright_transform",
        levels: &[Level::Global],
        matcher: Matcher::PrefixIndex("UprightTransform_"),
        kind: ValueKind::Str,
        policy: Policy::Computed,
        frame: FrameScope::NotApplicable,
        presence: Presence::Both,
        note: "Upright solver matrices (9 floats each), recomputed by Lightroom when \
               PerspectiveUpright is set. Never written.",
    },
    PatternSpec {
        id: "upright_guides",
        levels: &[Level::Global],
        matcher: Matcher::PrefixIndex("UprightFourSegments_"),
        kind: ValueKind::Str,
        policy: Policy::Never,
        frame: FrameScope::PerFrameOnly,
        presence: Presence::Both,
        note: "Guided Upright guide lines drawn by the user (4 floats each).",
    },
    PatternSpec {
        id: "filter_list_payload",
        levels: &[Level::Struct(StructKind::FilterList)],
        matcher: Matcher::AnyName,
        kind: ValueKind::Any,
        policy: Policy::Computed,
        frame: FrameScope::NotApplicable,
        presence: Presence::Both,
        note: "Everything below FilterList (Filters, Images, ImageGroup, Alpha, \
               ColorVariations, BlackLevels, Linearization, GenAIInfo, ...): Denoise, Raw \
               Details, Super Resolution and generative-remove payloads. Kept opaque, never \
               written; the generative provenance inside is NEVER as well.",
    },
];

/// The first pattern family `name` at `level` belongs to.
pub fn match_pattern(level: Level, name: &str) -> Option<&'static PatternSpec> {
    PATTERNS.iter().find(|p| p.matches(level, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_blob_needs_exactly_32_hex_digits() {
        let hex = "0123456789ABCDEF0123456789abcdef";
        assert!(Matcher::PrefixHex32("Table_").matches(&format!("Table_{hex}")));
        assert!(!Matcher::PrefixHex32("Table_").matches("Table_0123"));
        assert!(!Matcher::PrefixHex32("Table_").matches(&format!("Table_{hex}0")));
        assert!(!Matcher::PrefixHex32("Table_").matches(&format!("Table_{}G", &hex[..31])));
    }

    #[test]
    fn indexed_prefix_needs_digits() {
        let m = Matcher::PrefixIndex("UprightTransform_");
        assert!(m.matches("UprightTransform_0"));
        assert!(m.matches("UprightTransform_12"));
        assert!(!m.matches("UprightTransform_"));
        assert!(!m.matches("UprightTransformCount"));
        assert!(!m.matches("UprightTransform_x"));
    }

    #[test]
    fn prefix_needs_a_suffix() {
        assert!(Matcher::Prefix("pm_").matches("pm_patch"));
        assert!(!Matcher::Prefix("pm_").matches("pm_"));
        assert!(!Matcher::Prefix("pm_").matches("xpm_patch"));
    }

    #[test]
    fn patterns_are_scoped_to_their_levels() {
        let retouch = Level::Struct(StructKind::RetouchArea);
        assert_eq!(
            match_pattern(retouch, "pm_version").map(|p| p.id),
            Some("patch_state")
        );
        assert!(match_pattern(Level::Global, "pm_version").is_none());
        assert_eq!(
            match_pattern(Level::Struct(StructKind::FilterList), "Filters").map(|p| p.id),
            Some("filter_list_payload")
        );
        assert!(match_pattern(Level::Global, "Filters").is_none());
        let blob = "Table_00000000000000000000000000000001";
        assert_eq!(
            match_pattern(Level::MaskTool, blob).map(|p| p.id),
            Some("table_blob")
        );
    }

    #[test]
    fn families_are_never_learned_or_written() {
        for p in PATTERNS {
            assert!(
                matches!(p.policy, Policy::Computed | Policy::Never),
                "{} must be COMPUTED or NEVER",
                p.id
            );
        }
    }
}
