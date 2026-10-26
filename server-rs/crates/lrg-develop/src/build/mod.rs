//! Builders for local corrections: one [`Correction`] of the Masking panel
//! from adjustments in UI units and a mask made of typed components.
//!
//! ```
//! use lrg_develop::build::{CorrectionBuilder, LuminanceRange, MaskTool, SyncNamespace};
//! use lrg_develop::model::Semantic;
//!
//! # fn main() -> Result<(), lrg_develop::build::BuildError> {
//! let subject = CorrectionBuilder::new("Subject", &SyncNamespace::lrgenius())
//!     .amount_ui(100.0)?                        // CorrectionAmount 1
//!     .local_ui("LocalExposure2012", 0.25)?     // stored 0.0625 (EV / 4)
//!     .local_ui("LocalShadows2012", 12.0)?      // stored 0.12
//!     .add(Semantic::Subject)
//!     .intersect(LuminanceRange::lows(0.45, 0.1)?)
//!     .build()?;
//! assert_eq!(subject.name.as_deref(), Some("LrGenius · Subject"));
//! // Intersecting sets MaskInverted, and the range's Invert with it.
//! assert!(matches!(&subject.masks[1].tool, MaskTool::LuminanceRange(r) if r.invert));
//! # Ok(())
//! # }
//! ```
//!
//! What a builder produces, and nothing else:
//!
//! - the correction's `CorrectionName` (`"LrGenius · <role>"`),
//!   `CorrectionSyncID`, `CorrectionAmount` (1 unless set), `CorrectionActive`
//!   and its adjustments in the **stored** unit, converted by
//!   [`registry::from_ui_value`] (the registry's `ui` module is the only
//!   place UI and stored units meet);
//! - per component its tool, its combination ([`Combine`]), a neutral English
//!   `MaskName` close to Adobe's (`"Subject"`, `"Iris and Pupil"`,
//!   `"Vegetation"`, ...; Adobe's own Subject and Sky presets say
//!   `"Subject 1"`/`"Sky 1"`), a `MaskSyncID` and `MaskActive`.
//!
//! The fixed per-tool fields of Adobe's preset form (`What`, an AI mask's
//! `MaskVersion`/`ReferencePoint`/`ErrorReason`, a range mask's `Version` and
//! `SampleType`) are completed by the XMP writer, not here. A radial gradient
//! gets its `Version` 2 here, because the reader keeps that field in
//! [`MaskComponent::extra`].
//!
//! Not built, by design: brush strokes, Select Object gestures, colour and
//! depth ranges, rotated radial gradients, and every COMPUTED field (digests,
//! runtime ids, mask bitmap sizes). Those are photo-specific or Lightroom's
//! own and have no place in a generated correction.
//!
//! # Sync ids
//!
//! `CorrectionSyncID` and each `MaskSyncID` are derived, not random: the
//! first 16 bytes of SHA-256 over `(namespace, role, slot)` as 32 upper-case
//! hex digits ([`SyncNamespace::id`]). The same role in the same namespace
//! always gets the same ids, so a later step can replace an earlier
//! correction instead of stacking a second one (whether Lightroom replaces or
//! duplicates on apply is what experiments E4/E5 settle). Two corrections of
//! one preset therefore need different roles: assemble a preset's
//! corrections with [`corrections`], which refuses a repeated role
//! ([`BuildError::DuplicateRole`]). The writer does not check this, because
//! Lightroom's own sidecars repeat `MaskSyncID`s across corrections.
//!
//! The derivation is pinned by known-answer tests: changing the domain tag,
//! the length prefixes or the slot texts changes every id, and a later
//! release could no longer replace the corrections an earlier one wrote.
//!
//! # Policy of what is built
//!
//! Semantic and luminance-range components are LEARN and reach a shared
//! preset. Linear and radial gradients and [`Semantic::PersonPartAt`] are
//! PHOTO (tied to one frame's geometry or to one person), so the preset
//! writer drops a correction containing one and reports it in
//! `Written::skipped`; they are for applying to one photo. `PersonPartAt` is
//! unverified until experiments E2/E8 show that Lightroom resolves a written
//! `ReferencePoint` to the person at that point.

mod gradient;
mod range;
mod semantic;

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

pub use semantic::{LandscapeClass, PeoplePart};

use crate::model::correction::is_local_adjustment;
pub use crate::model::correction::{
    Combine, Correction, LinearGradient, LuminanceRange, MaskComponent, MaskTool, RadialGradient,
    Semantic, SensorPoint,
};
use crate::model::value::{Fields, Finite, Hex32, Value};
use crate::registry::{
    self, from_ui, from_ui_value, CurveKind, KeyId, Level, UiError, UiScale, ValueKind,
};

/// Prefix of every correction name a builder writes (`"LrGenius · Sky"`).
pub const NAME_PREFIX: &str = "LrGenius · ";

/// `Version` of a radial gradient component, as Lightroom writes it.
pub const RADIAL_VERSION: i64 = 2;
/// `Midpoint` of a built radial gradient (the only value Lightroom writes).
pub const RADIAL_MIDPOINT: i64 = 50;
/// `Roundness` of a built radial gradient (the only value Lightroom writes).
pub const RADIAL_ROUNDNESS: i64 = 0;

/// Why a correction could not be built.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum BuildError {
    /// The correction has no mask component.
    #[error("the correction has no mask component")]
    Empty,
    /// The correction has no adjustment, so its mask would change nothing.
    #[error("the correction has no adjustment")]
    NoAdjustments,
    /// The role (the name after [`NAME_PREFIX`]) is empty.
    #[error("the correction needs a role")]
    EmptyRole,
    /// The first component must add: there is nothing yet to subtract from
    /// or intersect with.
    #[error("the first mask component must be added, not {0:?}")]
    FirstNotAdd(Combine),
    /// The key cannot be set by a builder.
    #[error("{key} cannot be set in a built correction: {reason}")]
    NotWritable {
        /// Key name as given.
        key: String,
        /// Why (unknown key, not an adjustment, not learnable, unverified
        /// UI scale, wrong kind).
        reason: &'static str,
    },
    /// A number was NaN or infinite.
    #[error("{what}: not a finite number")]
    NotFinite {
        /// Which input.
        what: &'static str,
    },
    /// A UI value outside the key's range, or another conversion error.
    #[error(transparent)]
    Ui(#[from] UiError),
    /// A people-part or landscape category the model does not know.
    #[error("{variant}: category {id} is not one of the known ids")]
    InvalidCategory {
        /// The [`Semantic`] variant.
        variant: &'static str,
        /// The id given.
        id: i64,
    },
    /// A luminance range whose points are not ordered `0 ≤ low feather ≤
    /// low ≤ high ≤ high feather ≤ 1`.
    #[error(
        "luminance range {0:?} is not ordered 0 <= low feather <= low <= high <= high feather <= 1"
    )]
    LumRangeOrder([f64; 4]),
    /// A gradient or ellipse without extent, or a feather outside 0..=100.
    #[error("{0}")]
    Geometry(&'static str),
    /// A local point curve that is not 2+ points with strictly increasing x.
    #[error("{key}: a curve needs at least two points with strictly increasing x")]
    Curve {
        /// Key name.
        key: String,
    },
    /// A tool a builder does not create ([`MaskTool::Opaque`], or a typed
    /// tool carrying fields a builder never sets).
    #[error("{0} cannot be built")]
    NotBuildable(&'static str),
    /// Two corrections of one preset with the same role, which would share
    /// their `CorrectionSyncID` and `MaskSyncID`s ([`corrections`]).
    #[error("two corrections have the role {0:?}; each needs its own")]
    DuplicateRole(String),
}

/// The namespace sync ids are derived in. [`SyncNamespace::lrgenius`] is
/// the one production uses; another namespace gives unrelated ids for the
/// same roles (tests, or a caller that must not collide with it).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SyncNamespace(String);

/// Which id of a correction [`SyncNamespace::id`] derives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IdSlot {
    /// `CorrectionSyncID`.
    Correction,
    /// `MaskSyncID` of the component at this index.
    Component(usize),
}

impl SyncNamespace {
    /// LrGeniusAI's namespace.
    pub fn lrgenius() -> SyncNamespace {
        SyncNamespace("LrGeniusAI".into())
    }

    /// A namespace of its own.
    pub fn new(name: impl Into<String>) -> SyncNamespace {
        SyncNamespace(name.into())
    }

    /// The namespace's name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The id of `slot` in the correction `role`: the first 16 bytes of
    /// SHA-256 over a domain tag and the length-prefixed namespace, role and
    /// slot, as 32 upper-case hex digits. The length prefixes keep
    /// `("a", "bc")` and `("ab", "c")` apart.
    pub fn id(&self, role: &str, slot: IdSlot) -> Hex32 {
        let slot = match slot {
            IdSlot::Correction => "correction".to_owned(),
            IdSlot::Component(i) => format!("component {i}"),
        };
        let mut h = Sha256::new();
        h.update(b"lrg-develop sync id v1");
        for part in [self.0.as_str(), role, slot.as_str()] {
            h.update((part.len() as u64).to_le_bytes());
            h.update(part.as_bytes());
        }
        let digest = h.finalize();
        let hex: String = digest[..16].iter().map(|b| format!("{b:02X}")).collect();
        Hex32::parse(&hex).expect("32 hex digits")
    }
}

/// Builds one [`Correction`]; see the module docs.
///
/// Adjustments are set with [`local_ui`](Self::local_ui) (UI units) and
/// [`local_curve`](Self::local_curve); setting a key twice keeps the last
/// value. Components are added in evaluation order with
/// [`add`](Self::add), [`add_inverted`](Self::add_inverted),
/// [`subtract`](Self::subtract) and [`intersect`](Self::intersect), and
/// checked in [`build`](Self::build).
#[derive(Clone, Debug)]
pub struct CorrectionBuilder {
    role: String,
    namespace: SyncNamespace,
    amount: Finite,
    local: BTreeMap<KeyId, Value>,
    components: Vec<(Combine, MaskTool)>,
}

const C: Level = Level::Correction;

impl CorrectionBuilder {
    /// A correction for `role` (`"Subject"`, `"Sky"`, ...), named
    /// `"LrGenius · <role>"`, with its sync ids derived in `namespace`.
    pub fn new(role: impl Into<String>, namespace: &SyncNamespace) -> CorrectionBuilder {
        CorrectionBuilder {
            role: role.into(),
            namespace: namespace.clone(),
            amount: Finite::new_const(1.0),
            local: BTreeMap::new(),
            components: Vec::new(),
        }
    }

    /// The correction's amount in percent (100 = as set; 0..=200). Out of
    /// range is an error, never clamped.
    pub fn amount_ui(mut self, percent: f64) -> Result<CorrectionBuilder, BuildError> {
        let spec = registry::lookup(C, "CorrectionAmount")
            .expect("registry row")
            .spec();
        let ui = Finite::new(percent).map_err(|_| BuildError::NotFinite { what: "amount" })?;
        self.amount = from_ui(spec, ui)?;
        Ok(self)
    }

    /// Sets the adjustment `key` (`"LocalExposure2012"`) from its UI value
    /// (`0.25` for +0.25 EV, `12.0` for +12 shadows), converted to the stored
    /// unit. Out of range is an error, never clamped.
    ///
    /// Only learnable numeric adjustments with a verified UI scale can be
    /// set: `LocalHue` and `LocalGrain` (unverified scales),
    /// `LocalPointColors`, the legacy `LocalExposure` family and every
    /// bookkeeping key are [`BuildError::NotWritable`].
    pub fn local_ui(mut self, key: &str, ui: f64) -> Result<CorrectionBuilder, BuildError> {
        let id = writable_local(key)?;
        let spec = id.spec();
        if !matches!(spec.kind, ValueKind::Real | ValueKind::Int) {
            return Err(BuildError::NotWritable {
                key: key.to_owned(),
                reason: "not a numeric adjustment",
            });
        }
        let ui = Finite::new(ui).map_err(|_| BuildError::NotFinite { what: "adjustment" })?;
        match from_ui_value(spec, ui)? {
            (value, None) => {
                self.local.insert(id, value);
                Ok(self)
            }
            (_, Some(err)) => Err(err.into()),
        }
    }

    /// Sets a local point curve (`"MainCurve"`, `"RedCurve"`, `"GreenCurve"`,
    /// `"BlueCurve"`): at least two points, x strictly increasing.
    pub fn local_curve(
        mut self,
        key: &str,
        points: &[(u8, u8)],
    ) -> Result<CorrectionBuilder, BuildError> {
        let id = writable_local(key)?;
        if id.spec().kind != ValueKind::Curve(CurveKind::Local) {
            return Err(BuildError::NotWritable {
                key: key.to_owned(),
                reason: "not a local point curve",
            });
        }
        let increasing = points.windows(2).all(|w| w[0].0 < w[1].0);
        if points.len() < 2 || !increasing {
            return Err(BuildError::Curve {
                key: key.to_owned(),
            });
        }
        let curve = points
            .iter()
            .map(|&(x, y)| (Finite::from_i64(x.into()), Finite::from_i64(y.into())))
            .collect();
        self.local.insert(id, Value::Curve(curve));
        Ok(self)
    }

    fn push(mut self, combine: Combine, tool: impl Into<MaskTool>) -> CorrectionBuilder {
        self.components.push((combine, tool.into()));
        self
    }

    /// Adds a component (`MaskBlendMode` 0, `MaskValue` 1).
    // Lightroom's "Add" mask operation, a sibling of `subtract` and
    // `intersect`; not arithmetic, so no `std::ops::Add`.
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, tool: impl Into<MaskTool>) -> CorrectionBuilder {
        self.push(Combine::Add { inverted: false }, tool)
    }

    /// Adds the inverse of a component (`MaskInverted` true).
    pub fn add_inverted(self, tool: impl Into<MaskTool>) -> CorrectionBuilder {
        self.push(Combine::Add { inverted: true }, tool)
    }

    /// Subtracts a component from the mask so far.
    pub fn subtract(self, tool: impl Into<MaskTool>) -> CorrectionBuilder {
        self.push(Combine::Subtract, tool)
    }

    /// Intersects the mask so far with a component.
    pub fn intersect(self, tool: impl Into<MaskTool>) -> CorrectionBuilder {
        self.push(Combine::Intersect, tool)
    }

    /// Checks everything and builds the correction.
    ///
    /// Invariants of the result: at least one component, the first one
    /// added; `MaskValue` 0 exactly when `MaskBlendMode` 1 (by
    /// [`Combine::encode`]); at least one adjustment; every id derived from
    /// the namespace and role; a radial gradient's `Flipped` is
    /// `!MaskInverted` and a luminance range's `CorrectionRangeMask.Invert`
    /// is `MaskInverted` (the relations every radial gradient and range mask
    /// Lightroom writes holds).
    pub fn build(self) -> Result<Correction, BuildError> {
        let role = self.role.trim();
        if role.is_empty() {
            return Err(BuildError::EmptyRole);
        }
        let Some((first, _)) = self.components.first() else {
            return Err(BuildError::Empty);
        };
        if !matches!(first, Combine::Add { .. }) {
            return Err(BuildError::FirstNotAdd(*first));
        }
        if self.local.is_empty() {
            return Err(BuildError::NoAdjustments);
        }
        let masks = self
            .components
            .into_iter()
            .enumerate()
            .map(|(i, (combine, tool))| {
                component(combine, tool, self.namespace.id(role, IdSlot::Component(i)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Correction {
            name: Some(format!("{NAME_PREFIX}{role}")),
            sync_id: Some(self.namespace.id(role, IdSlot::Correction)),
            amount: Some(self.amount),
            active: Some(true),
            local: self.local,
            masks,
            extra: Fields::default(),
        })
    }
}

/// Builds a preset's corrections, in order: every builder's
/// [`build`](CorrectionBuilder::build), and no role twice (two corrections
/// with one role would share their sync ids, see the module docs). The
/// way to assemble the corrections of one preset.
pub fn corrections(
    builders: impl IntoIterator<Item = CorrectionBuilder>,
) -> Result<Vec<Correction>, BuildError> {
    let mut roles = std::collections::HashSet::new();
    let mut out = Vec::new();
    for b in builders {
        let role = b.role.trim().to_owned();
        if !role.is_empty() && !roles.insert(role.clone()) {
            return Err(BuildError::DuplicateRole(role));
        }
        out.push(b.build()?);
    }
    Ok(out)
}

/// The registry id of an adjustment a builder may set.
fn writable_local(key: &str) -> Result<KeyId, BuildError> {
    let not = |reason| BuildError::NotWritable {
        key: key.to_owned(),
        reason,
    };
    let id = registry::lookup(C, key).ok_or_else(|| not("not a correction key"))?;
    let spec = id.spec();
    if !is_local_adjustment(spec) {
        return Err(not("not an adjustment"));
    }
    if !spec.policy.is_learnable() {
        return Err(not("its value semantics are not established (policy)"));
    }
    if matches!(spec.ui, UiScale::Unknown) {
        return Err(not("its UI scale is not verified"));
    }
    Ok(id)
}

/// Whether `MaskInverted` is set for this combination.
fn is_inverted(combine: Combine) -> bool {
    combine.encode().is_some_and(|(_, _, inverted)| inverted)
}

/// One checked component with its neutral name and id.
fn component(
    combine: Combine,
    tool: MaskTool,
    sync_id: Hex32,
) -> Result<MaskComponent, BuildError> {
    if combine == Combine::Unrecognised {
        return Err(BuildError::NotBuildable("an unrecognised combination"));
    }
    let mut extra = Fields::default();
    let (name, tool) = match tool {
        MaskTool::Semantic(s) => (semantic::mask_name(s)?, MaskTool::Semantic(s)),
        MaskTool::Linear(g) => {
            gradient::check_linear(&g)?;
            ("Linear Gradient", MaskTool::Linear(g))
        }
        MaskTool::Radial(g) => {
            gradient::check_radial(&g)?;
            extra.values.insert(
                registry::lookup(Level::MaskTool, "Version").expect("registry row"),
                Value::Int(RADIAL_VERSION),
            );
            let g = RadialGradient {
                flipped: !is_inverted(combine),
                ..g
            };
            ("Radial Gradient", MaskTool::Radial(g))
        }
        MaskTool::LuminanceRange(lr) => {
            range::check(&lr)?;
            // Lightroom writes `Invert` equal to `MaskInverted` on every range
            // mask of the maintainer's sidecar corpus, intersections included
            // (the local sweep counts it); a range that disagreed could be
            // read as its opposite.
            let lr = LuminanceRange {
                invert: is_inverted(combine),
                ..lr
            };
            ("Luminance Range", MaskTool::LuminanceRange(lr))
        }
        MaskTool::Opaque { .. } => {
            return Err(BuildError::NotBuildable(
                "an opaque mask tool (brush, object, colour range)",
            ))
        }
    };
    Ok(MaskComponent {
        tool,
        combine,
        name: Some(name.to_owned()),
        sync_id: Some(sync_id),
        active: Some(true),
        extra,
    })
}

impl From<Semantic> for MaskTool {
    fn from(s: Semantic) -> MaskTool {
        MaskTool::Semantic(s)
    }
}

impl From<LinearGradient> for MaskTool {
    fn from(g: LinearGradient) -> MaskTool {
        MaskTool::Linear(g)
    }
}

impl From<RadialGradient> for MaskTool {
    fn from(g: RadialGradient) -> MaskTool {
        MaskTool::Radial(g)
    }
}

impl From<LuminanceRange> for MaskTool {
    fn from(r: LuminanceRange) -> MaskTool {
        MaskTool::LuminanceRange(r)
    }
}

#[cfg(test)]
mod tests;
