//! What may leave the model for a given target.
//!
//! [`DevelopSettings::filtered`] applies the registry's policy classes to one
//! target (a shared preset, one photo, or a test round trip) and reports
//! every removed value that differs from its default as [`Skipped`], so a
//! caller can say what did not transfer instead of dropping it silently.
//! [`DevelopSettings::split_by_frame_scope`] sorts values by whether they can
//! be shared across the frames of a series.
//!
//! | Class | Preset | Apply to a photo | Test round trip |
//! |---|---|---|---|
//! | LEARN | yes | yes | yes |
//! | LEARN† | if the gate holds for every target | if the gate holds for the photo | yes |
//! | META, global (`ProcessVersion`, `Enable*`, ...) | no: set by the serializer | no: set by the serializer | yes |
//! | META, nested (correction, mask and structure bookkeeping) | yes | yes | yes |
//! | PHOTO | no | yes | yes |
//! | COMPUTED, NEVER, UNKNOWN, opaque | no | no | yes |
//!
//! The verdict is per key at every depth: a kept structure (`LensBlur`, a
//! mask's `CorrectionRangeMask`, a `Gesture` item, ...) is rebuilt from the
//! fields that pass at their own registry rows, so a shareable `LensBlur`
//! loses its sampled area and a luminance range its eyedropper point. The one
//! exception is the [`Look`], which goes whole or not at all: its
//! `Parameters` are the profile definition and mean nothing in part.
//!
//! Global META values are dropped without a report: the source's
//! `ProcessVersion`, `Version` stamps and panel switches describe the source,
//! not the settings, and a writer sets them from its own context (copying an
//! example's `ProcessVersion` would silently change the target photo's
//! process version). Nested META stays for now (a range mask's `Version`, sync
//! ids); what the writer sets there itself is decided with the XMP writer.

use super::correction::{Correction, LuminanceRange, MaskComponent, MaskTool, Semantic};
use super::value::{Fields, Struct, Value};
use super::{DevelopSettings, FileKind, Look};
use crate::registry::{
    self, Def, FileScope, FrameScope, Gate, KeyId, KeySpec, Level, Policy, ProcessVersion,
};

/// `CameraProfile` values every camera has: Adobe's standard profile and the
/// symbolic defaults Lightroom resolves per camera. Any other name (`Camera
/// Standard`, `Adobe Standard v2`, which only newer cameras have, ...) is
/// camera-specific.
pub const PORTABLE_PROFILES: &[&str] = &[
    "Adobe Standard",
    "Default Color",
    "Default Monochrome",
    "Default Profile",
];

/// Where filtered settings are going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A shared preset. `mixed_file_kinds`: it may be applied to raw and
    /// non-raw files alike.
    Preset {
        /// The preset may reach both raw and non-raw files.
        mixed_file_kinds: bool,
    },
    /// Applying to one photo with its own context.
    ///
    /// A camera-specific `CameraProfile` (outside [`PORTABLE_PROFILES`])
    /// passes: the model knows neither the source's camera nor the photo's
    /// make, so the caller must only apply settings from the same make until
    /// this target carries the make.
    ApplyPhoto {
        /// The photo's file kind, if known.
        file_kind: Option<FileKind>,
        /// The photo's process version, if known.
        process_version: Option<ProcessVersion>,
        /// The photo's camera model, if known (for camera-restricted
        /// profiles).
        camera: Option<String>,
    },
    /// Test round trip: everything, opaque content included.
    TestRoundTrip,
}

/// Why a value was filtered out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The key's policy class does not reach this target.
    Policy(Policy),
    /// The key's gate does not hold for this target.
    Gate(Gate),
    /// The key needs a newer process version than the photo has.
    ProcessVersion {
        /// The key's minimum.
        min: ProcessVersion,
        /// The photo's version.
        target: ProcessVersion,
    },
    /// The key exists only for the other file kind.
    FileKind(FileScope),
    /// A profile restricted to a camera the target does not have (or may
    /// not have).
    CameraRestricted {
        /// `CameraModelRestriction`.
        restriction: String,
    },
    /// Content the model keeps only for the round trip.
    Opaque,
}

/// One value [`DevelopSettings::filtered`] removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// Where it was (`"Temperature"`, `"LensBlur.SampledArea"`,
    /// `"MaskGroupBasedCorrections[1]"`,
    /// `"MaskGroupBasedCorrections[0].CorrectionID"`).
    pub path: String,
    /// Why.
    pub reason: SkipReason,
}

/// [`DevelopSettings::split_by_frame_scope`]'s result: one settings value per
/// [`FrameScope`], each with the source's file kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameSplit {
    /// Frame-independent (tone, HSL, colour grading, detail, profile,
    /// semantic and luminance masks).
    pub shareable: DevelopSettings,
    /// Shared but adjusted per frame (exposure, white balance, ...).
    pub per_frame_adjusted: DevelopSettings,
    /// Belongs to one frame (crop, geometry, lens identity, retouch,
    /// geometric and hand-drawn masks).
    pub per_frame_only: DevelopSettings,
    /// Not a transferable setting (computed, meta, unknown, opaque).
    pub not_applicable: DevelopSettings,
}

impl FrameSplit {
    fn part(&mut self, scope: FrameScope) -> &mut DevelopSettings {
        match scope {
            FrameScope::Shareable => &mut self.shareable,
            FrameScope::PerFrameAdjusted => &mut self.per_frame_adjusted,
            FrameScope::PerFrameOnly => &mut self.per_frame_only,
            FrameScope::NotApplicable => &mut self.not_applicable,
        }
    }
}

/// How restrictive a class is when components are combined: the most
/// restrictive component decides for the whole correction.
fn rank(p: Policy) -> u8 {
    match p {
        Policy::Learn | Policy::LearnGated(_) | Policy::Meta => 0,
        Policy::Photo => 1,
        Policy::Unknown | Policy::Computed | Policy::Never => 2,
    }
}

impl MaskComponent {
    /// The component's policy class, from its tool.
    pub fn policy(&self) -> Policy {
        match &self.tool {
            MaskTool::Semantic(Semantic::PersonPartAt { .. }) => Policy::Photo,
            MaskTool::Semantic(_) | MaskTool::LuminanceRange(_) => Policy::Learn,
            MaskTool::Linear(_) | MaskTool::Radial(_) => Policy::Photo,
            MaskTool::Opaque { what } => match what.as_deref() {
                Some("Mask/Aggregate" | "Mask/Paint" | "Mask/Brush") => Policy::Never,
                // Select Object (a gesture on an AI mask), colour/depth
                // ranges and rotated radial gradients: tied to one frame.
                Some("Mask/Image") if self.extra.get(Level::MaskTool, "Gesture").is_some() => {
                    Policy::Photo
                }
                Some("Mask/RangeMask" | "Mask/Gradient" | "Mask/CircularGradient") => Policy::Photo,
                _ => Policy::Unknown,
            },
        }
    }

    /// The component's frame scope: shareable for semantic and luminance
    /// masks, per frame for everything tied to image positions.
    pub fn frame_scope(&self) -> FrameScope {
        match self.policy() {
            Policy::Learn | Policy::LearnGated(_) | Policy::Meta => FrameScope::Shareable,
            Policy::Photo | Policy::Never => FrameScope::PerFrameOnly,
            Policy::Computed | Policy::Unknown => FrameScope::NotApplicable,
        }
    }
}

impl Correction {
    /// The most restrictive policy of its components (UNKNOWN without any).
    pub fn policy(&self) -> Policy {
        self.masks
            .iter()
            .map(MaskComponent::policy)
            .max_by_key(|p| rank(*p))
            .unwrap_or(Policy::Unknown)
    }

    /// The least shareable frame scope of its components.
    pub fn frame_scope(&self) -> FrameScope {
        let order = |s: FrameScope| match s {
            FrameScope::Shareable => 0,
            FrameScope::PerFrameAdjusted => 1,
            FrameScope::PerFrameOnly => 2,
            FrameScope::NotApplicable => 3,
        };
        self.masks
            .iter()
            .map(MaskComponent::frame_scope)
            .max_by_key(|s| order(*s))
            .unwrap_or(FrameScope::NotApplicable)
    }
}

/// Whether a policy class reaches the target, before gates.
fn class_verdict(policy: Policy, target: &Target) -> Result<(), SkipReason> {
    match (policy, target) {
        (_, Target::TestRoundTrip) => Ok(()),
        (Policy::Learn | Policy::LearnGated(_) | Policy::Meta, _) => Ok(()),
        (Policy::Photo, Target::ApplyPhoto { .. }) => Ok(()),
        (p, _) => Err(SkipReason::Policy(p)),
    }
}

fn gate_verdict(gate: Gate, value: Option<&Value>, target: &Target) -> Result<(), SkipReason> {
    let fails = match (gate, target) {
        (_, Target::TestRoundTrip) => false,
        (Gate::RawOnly | Gate::FileKindDefault, Target::Preset { mixed_file_kinds }) => {
            *mixed_file_kinds
        }
        (Gate::NonRawOnly, Target::Preset { mixed_file_kinds }) => *mixed_file_kinds,
        (Gate::RawOnly, Target::ApplyPhoto { file_kind, .. }) => *file_kind != Some(FileKind::Raw),
        (Gate::NonRawOnly, Target::ApplyPhoto { file_kind, .. }) => {
            *file_kind != Some(FileKind::NonRaw)
        }
        (Gate::FileKindDefault, Target::ApplyPhoto { .. }) => false,
        (Gate::OnlyValues(allowed), _) => {
            !matches!(value, Some(Value::Str(s)) if allowed.contains(&s.as_str()))
        }
        // Adobe Standard and the symbolic defaults exist for every camera.
        (Gate::CameraRestricted, _) if matches!(value, Some(Value::Str(s)) if PORTABLE_PROFILES.contains(&s.as_str())) => {
            false
        }
        // Any other profile name is camera-specific, and a preset may reach
        // any camera.
        (Gate::CameraRestricted, Target::Preset { .. }) => true,
        // One photo: the model has no make to compare, so the same-make rule
        // is the caller's (see `Target::ApplyPhoto`). The Look's own
        // restriction is checked in `Filter::look`, where the camera is known.
        (Gate::CameraRestricted, Target::ApplyPhoto { .. }) => false,
        // Learning-side gates: they shape how values are blended, not
        // whether a value may be written.
        (
            Gate::Categorical
            | Gate::CircularHue(_)
            | Gate::ByMaskType
            | Gate::NeedsAiUpdate
            | Gate::DependsOn(_),
            _,
        ) => false,
    };
    if fails {
        Err(SkipReason::Gate(gate))
    } else {
        Ok(())
    }
}

/// Whether a registry key with this value reaches the target.
fn key_verdict(spec: &KeySpec, value: Option<&Value>, target: &Target) -> Result<(), SkipReason> {
    if let Target::TestRoundTrip = target {
        return Ok(());
    }
    class_verdict(spec.policy, target)?;
    if let Some(gate) = spec.policy.gate() {
        gate_verdict(gate, value, target)?;
    }
    if let Target::ApplyPhoto {
        file_kind,
        process_version,
        ..
    } = target
    {
        let wrong_kind = matches!(
            (spec.file_kind, file_kind),
            (FileScope::RawOnly, Some(FileKind::NonRaw))
                | (FileScope::NonRawOnly, Some(FileKind::Raw))
        );
        if wrong_kind {
            return Err(SkipReason::FileKind(spec.file_kind));
        }
        if let (Some(min), Some(pv)) = (spec.min_pv, *process_version) {
            if min > pv {
                return Err(SkipReason::ProcessVersion { min, target: pv });
            }
        }
    }
    Ok(())
}

/// Whether `value` is the key's default for `kind` (so removing it loses
/// nothing and needs no report).
fn is_default(spec: &KeySpec, value: &Value, kind: Option<FileKind>) -> bool {
    let def = match kind {
        Some(FileKind::NonRaw) => spec.default.non_raw,
        _ => spec.default.raw,
    };
    matches!(def, Def::Value(lit) if value.equals_lit(&lit))
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

struct Filter<'a> {
    target: &'a Target,
    kind: Option<FileKind>,
    skipped: Vec<Skipped>,
}

impl Filter<'_> {
    fn skip(&mut self, path: String, reason: SkipReason) {
        self.skipped.push(Skipped { path, reason });
    }

    /// One registry key's value at `path` (the key's own path): its
    /// verdict first, then, when it stays, what is inside it.
    fn keyed(&mut self, id: KeyId, v: &Value, path: &str) -> Option<Value> {
        let spec = id.spec();
        match key_verdict(spec, Some(v), self.target) {
            Ok(()) => self.nested(v, path),
            Err(reason) => {
                if !is_default(spec, v, self.kind) {
                    self.skip(path.to_owned(), reason);
                }
                None
            }
        }
    }

    /// The part of a kept value that may go: structures field by field at
    /// their own registry rows, corrections by their components. A structure
    /// or list left with nothing in it is dropped; every other value is kept
    /// as it is.
    fn nested(&mut self, v: &Value, path: &str) -> Option<Value> {
        match v {
            Value::Struct(s) => self.structure(s, path).map(Value::Struct),
            Value::StructList(items) => {
                let kept: Vec<Struct> = items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| self.structure(s, &format!("{path}[{i}]")))
                    .collect();
                (!kept.is_empty()).then_some(Value::StructList(kept))
            }
            Value::Tools(items) => {
                let kept: Vec<Fields> = items
                    .iter()
                    .enumerate()
                    .map(|(i, f)| self.fields(f, &format!("{path}[{i}]")))
                    .filter(|f| !f.is_empty())
                    .collect();
                (!kept.is_empty()).then_some(Value::Tools(kept))
            }
            Value::Corrections(items) => {
                let kept: Vec<Correction> = items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| self.correction(c, &format!("{path}[{i}]")))
                    .collect();
                (!kept.is_empty()).then_some(Value::Corrections(kept))
            }
            other => Some(other.clone()),
        }
    }

    fn structure(&mut self, s: &Struct, path: &str) -> Option<Struct> {
        let fields = self.fields(&s.fields, path);
        (!fields.is_empty()).then_some(Struct {
            kind: s.kind,
            fields,
        })
    }

    fn fields(&mut self, f: &Fields, path: &str) -> Fields {
        let mut out = Fields::default();
        for (id, v) in &f.values {
            if let Some(kept) = self.keyed(*id, v, &join(path, id.spec().name)) {
                out.values.insert(*id, kept);
            }
        }
        for o in &f.opaque {
            self.skip(join(path, &o.name), SkipReason::Opaque);
        }
        out
    }

    fn look(&mut self, look: &Look) -> Option<Look> {
        let spec = registry::lookup(Level::Global, "Look")
            .expect("registry row")
            .spec();
        let mut verdict = class_verdict(spec.policy, self.target);
        if verdict.is_ok() {
            if let Some(restriction) = &look.camera_restriction {
                let fits = match self.target {
                    Target::ApplyPhoto { camera, .. } => camera.as_deref() == Some(restriction),
                    Target::Preset { .. } => false,
                    Target::TestRoundTrip => true,
                };
                if !fits {
                    verdict = Err(SkipReason::CameraRestricted {
                        restriction: restriction.clone(),
                    });
                }
            }
        }
        match verdict {
            Ok(()) => Some(look.clone()),
            Err(reason) => {
                self.skip("Look".into(), reason);
                None
            }
        }
    }

    fn correction(&mut self, c: &Correction, path: &str) -> Option<Correction> {
        let policy = c.policy();
        if let Err(reason) = class_verdict(policy, self.target) {
            self.skip(path.to_owned(), reason);
            return None;
        }
        let mut local = std::collections::BTreeMap::new();
        for (id, v) in &c.local {
            if let Some(kept) = self.keyed(*id, v, &join(path, id.spec().name)) {
                local.insert(*id, kept);
            }
        }
        let masks = c
            .masks
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let mask_path = format!("{path}.CorrectionMasks[{i}]");
                let tool = match &m.tool {
                    // The typed range stays; the rest of the range mask
                    // (eyedropper sample, sample type, ...) is checked field
                    // by field like any other structure.
                    MaskTool::LuminanceRange(lr) => MaskTool::LuminanceRange(LuminanceRange {
                        rest: Struct {
                            kind: lr.rest.kind,
                            fields: self.fields(
                                &lr.rest.fields,
                                &format!("{mask_path}.CorrectionRangeMask"),
                            ),
                        },
                        ..lr.clone()
                    }),
                    other => other.clone(),
                };
                MaskComponent {
                    tool,
                    extra: self.fields(&m.extra, &mask_path),
                    ..m.clone()
                }
            })
            .collect();
        Some(Correction {
            local,
            masks,
            extra: self.fields(&c.extra, path),
            ..c.clone()
        })
    }
}

impl DevelopSettings {
    /// The settings that may go to `target`, and what was removed.
    ///
    /// Every removed value that differs from its default (for this
    /// settings' file kind) is reported; removing a default loses nothing.
    /// The exception is global META (see the module docs), which is dropped
    /// without a report because a writer sets it from its own context.
    ///
    /// A correction goes or stays whole, decided by its most restrictive
    /// component ([`Correction::policy`]); inside a kept correction,
    /// bookkeeping fields (runtime ids, digests) are removed one by one.
    /// Structures are filtered field by field at their own registry rows,
    /// at any depth. The `Look` goes whole or not at all.
    pub fn filtered(&self, target: &Target) -> (DevelopSettings, Vec<Skipped>) {
        if let Target::TestRoundTrip = target {
            return (self.clone(), Vec::new());
        }
        let mut f = Filter {
            target,
            kind: self.file_kind,
            skipped: Vec::new(),
        };
        let mut out = DevelopSettings {
            file_kind: self.file_kind,
            ..DevelopSettings::default()
        };
        for (id, v) in self.values() {
            let spec = id.spec();
            if spec.policy == Policy::Meta {
                // Not reported on purpose: these describe the source
                // (`ProcessVersion`, `Version`, `Enable*`, `HasCrop`, ...) and
                // the writer sets them from the target's context. Nothing is
                // lost that the user could act on.
                continue;
            }
            if let Some(kept) = f.keyed(id, v, spec.name) {
                out.values.insert(id, kept);
            }
        }
        if let Some(look) = &self.look {
            out.look = f.look(look);
        }
        for (i, c) in self.corrections.iter().enumerate() {
            let path = format!("MaskGroupBasedCorrections[{i}]");
            if let Some(kept) = f.correction(c, &path) {
                out.corrections.push(kept);
            }
        }
        for o in &self.opaque {
            f.skip(o.name.clone(), SkipReason::Opaque);
        }
        (out, f.skipped)
    }

    /// Sorts the settings by frame scope. Global values follow their
    /// registry row, corrections their least shareable component, the Look
    /// its registry row, opaque entries go to `not_applicable`.
    pub fn split_by_frame_scope(&self) -> FrameSplit {
        let mut split = FrameSplit::default();
        for part in [
            &mut split.shareable,
            &mut split.per_frame_adjusted,
            &mut split.per_frame_only,
            &mut split.not_applicable,
        ] {
            part.file_kind = self.file_kind;
        }
        for (id, v) in self.values() {
            split.part(id.spec().frame).values.insert(id, v.clone());
        }
        if let Some(look) = &self.look {
            let frame = registry::lookup(Level::Global, "Look")
                .expect("registry row")
                .spec()
                .frame;
            split.part(frame).look = Some(look.clone());
        }
        for c in &self.corrections {
            split.part(c.frame_scope()).corrections.push(c.clone());
        }
        split.not_applicable.opaque = self.opaque.clone();
        split
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::correction::{Combine, LinearGradient, SensorPoint};
    use crate::model::value::{Finite, Opaque, OpaqueEntry, Struct};
    use crate::registry::{lookup, StructKind};

    fn set(s: &mut DevelopSettings, name: &str, v: Value) {
        s.insert(lookup(Level::Global, name).unwrap(), v).unwrap();
    }

    fn component(tool: MaskTool) -> MaskComponent {
        MaskComponent {
            tool,
            combine: Combine::Add { inverted: false },
            name: None,
            sync_id: None,
            active: Some(true),
            extra: Fields::default(),
        }
    }

    fn correction(masks: Vec<MaskComponent>) -> Correction {
        let mut local = std::collections::BTreeMap::new();
        local.insert(
            lookup(Level::Correction, "LocalExposure2012").unwrap(),
            Value::Real(Finite::new_const(0.25)),
        );
        let mut extra = Fields::default();
        extra.values.insert(
            lookup(Level::Correction, "CorrectionID").unwrap(),
            Value::Str("00000000-0000-4000-8000-000000000001".into()),
        );
        Correction {
            name: Some("Correction 1".into()),
            sync_id: None,
            amount: Some(Finite::new_const(1.0)),
            active: Some(true),
            local,
            masks,
            extra,
        }
    }

    fn linear() -> MaskTool {
        let p = SensorPoint {
            x: Finite::ZERO,
            y: Finite::ZERO,
        };
        MaskTool::Linear(LinearGradient { zero: p, full: p })
    }

    fn sample() -> DevelopSettings {
        let mut s = DevelopSettings::new();
        s.file_kind = Some(FileKind::Raw);
        set(&mut s, "Exposure2012", Value::Real(Finite::new_const(0.5)));
        set(&mut s, "Temperature", Value::Int(5600));
        set(&mut s, "CropTop", Value::Real(Finite::new_const(0.1)));
        set(&mut s, "ProcessVersion", Value::Str("15.4".into()));
        set(
            &mut s,
            "FilterList",
            Value::Struct(Struct::new(StructKind::FilterList)),
        );
        set(
            &mut s,
            "RetouchAreas",
            Value::StructList(vec![Struct::new(StructKind::RetouchArea)]),
        );
        let mut subject = component(MaskTool::Semantic(Semantic::Subject));
        subject.extra.values.insert(
            lookup(Level::MaskTool, "MaskDigest").unwrap(),
            Value::Str("00000000000000000000000000000001".into()),
        );
        s.corrections.push(correction(vec![subject]));
        s.corrections.push(correction(vec![component(linear())]));
        s.corrections
            .push(correction(vec![component(MaskTool::Opaque {
                what: Some("Mask/Aggregate".into()),
            })]));
        s.opaque.push(OpaqueEntry {
            ns: None,
            name: "SomethingNew".into(),
            value: Opaque::Json(serde_json::json!(1)),
        });
        s
    }

    fn paths(skipped: &[Skipped]) -> Vec<&str> {
        skipped.iter().map(|s| s.path.as_str()).collect()
    }

    #[test]
    fn test_round_trip_keeps_everything() {
        let s = sample();
        let (out, skipped) = s.filtered(&Target::TestRoundTrip);
        assert_eq!(out, s);
        assert!(skipped.is_empty());
    }

    #[test]
    fn a_mixed_preset_drops_photo_computed_raw_only_and_hand_drawn_content() {
        let s = sample();
        let (out, skipped) = s.filtered(&Target::Preset {
            mixed_file_kinds: true,
        });
        assert!(out.get_by_name("Exposure2012").is_some());
        assert!(
            out.get_by_name("ProcessVersion").is_none(),
            "the writer sets the process version, never the example"
        );
        assert!(!paths(&skipped).contains(&"ProcessVersion"), "not reported");
        for gone in ["Temperature", "CropTop", "FilterList", "RetouchAreas"] {
            assert!(out.get_by_name(gone).is_none(), "{gone}");
        }
        assert_eq!(
            out.corrections.len(),
            1,
            "only the subject mask is shareable"
        );
        assert!(matches!(
            out.corrections[0].masks[0].tool,
            MaskTool::Semantic(Semantic::Subject)
        ));
        assert!(
            out.corrections[0].extra.values.is_empty(),
            "CorrectionID removed"
        );
        assert!(
            out.corrections[0].masks[0].extra.values.is_empty(),
            "MaskDigest removed"
        );
        assert!(out.opaque.is_empty());
        let p = paths(&skipped);
        for want in [
            "Temperature",
            "CropTop",
            "FilterList",
            "MaskGroupBasedCorrections[0].CorrectionID",
            "MaskGroupBasedCorrections[0].CorrectionMasks[0].MaskDigest",
            "RetouchAreas",
            "MaskGroupBasedCorrections[1]",
            "MaskGroupBasedCorrections[2]",
            "SomethingNew",
        ] {
            assert!(p.contains(&want), "{want} not reported in {p:?}");
        }
        let temp = skipped.iter().find(|s| s.path == "Temperature").unwrap();
        assert_eq!(temp.reason, SkipReason::Gate(Gate::RawOnly));
        let brush = skipped
            .iter()
            .find(|s| s.path == "MaskGroupBasedCorrections[2]")
            .unwrap();
        assert_eq!(brush.reason, SkipReason::Policy(Policy::Never));
    }

    #[test]
    fn a_single_kind_preset_keeps_raw_only_keys() {
        let (out, _) = sample().filtered(&Target::Preset {
            mixed_file_kinds: false,
        });
        assert!(out.get_by_name("Temperature").is_some());
    }

    #[test]
    fn applying_to_a_photo_keeps_photo_content_but_checks_its_context() {
        let s = sample();
        let (out, skipped) = s.filtered(&Target::ApplyPhoto {
            file_kind: Some(FileKind::NonRaw),
            process_version: Some(ProcessVersion::V6),
            camera: None,
        });
        assert!(out.get_by_name("CropTop").is_some());
        assert!(out.get_by_name("Temperature").is_none());
        assert!(
            out.get_by_name("ProcessVersion").is_none(),
            "the example's 15.4 must not reach a photo of another version"
        );
        assert_eq!(
            out.corrections.len(),
            2,
            "subject and gradient, not the brush"
        );
        let temp = skipped.iter().find(|s| s.path == "Temperature").unwrap();
        assert_eq!(temp.reason, SkipReason::Gate(Gate::RawOnly));
    }

    #[test]
    fn keys_newer_than_the_photo_process_version_are_skipped() {
        let s = sample();
        let (out, skipped) = s.filtered(&Target::ApplyPhoto {
            file_kind: Some(FileKind::Raw),
            process_version: Some(ProcessVersion::PV2010),
            camera: None,
        });
        assert!(out.get_by_name("Exposure2012").is_none());
        let e = skipped.iter().find(|s| s.path == "Exposure2012").unwrap();
        assert_eq!(
            e.reason,
            SkipReason::ProcessVersion {
                min: ProcessVersion::PV2012,
                target: ProcessVersion::PV2010
            }
        );
        // Without a known file kind the PV check still applies.
        let (out, _) = s.filtered(&Target::ApplyPhoto {
            file_kind: None,
            process_version: Some(ProcessVersion::PV2010),
            camera: None,
        });
        assert!(out.get_by_name("Exposure2012").is_none());
    }

    #[test]
    fn defaults_are_removed_without_a_report() {
        let mut s = DevelopSettings::new();
        s.file_kind = Some(FileKind::Raw);
        set(&mut s, "CropTop", Value::Real(Finite::ZERO));
        let (out, skipped) = s.filtered(&Target::Preset {
            mixed_file_kinds: true,
        });
        assert!(out.get_by_name("CropTop").is_none());
        assert!(skipped.is_empty(), "{skipped:?}");
    }

    fn look(restriction: Option<&str>) -> Look {
        Look {
            name: Some("Synthetic Profile".into()),
            uuid: None,
            amount: Some(Finite::new_const(1.0)),
            camera_restriction: restriction.map(str::to_owned),
            rest: Struct::new(StructKind::Look),
        }
    }

    #[test]
    fn camera_restricted_looks_only_go_to_that_camera() {
        let mut s = DevelopSettings::new();
        s.look = Some(look(Some("Synthetic Camera")));
        let apply = |camera: Option<&str>| {
            s.filtered(&Target::ApplyPhoto {
                file_kind: None,
                process_version: None,
                camera: camera.map(str::to_owned),
            })
        };
        assert!(apply(Some("Synthetic Camera")).0.look.is_some());
        let (out, skipped) = apply(Some("Other Camera"));
        assert!(out.look.is_none());
        assert!(matches!(
            skipped[0].reason,
            SkipReason::CameraRestricted { .. }
        ));
        assert!(apply(None).0.look.is_none());
        let (out, _) = s.filtered(&Target::Preset {
            mixed_file_kinds: false,
        });
        assert!(out.look.is_none());

        let mut open = DevelopSettings::new();
        open.look = Some(look(None));
        let (out, skipped) = open.filtered(&Target::Preset {
            mixed_file_kinds: true,
        });
        assert_eq!(out.look, open.look, "taken whole");
        assert!(skipped.is_empty());
    }

    #[test]
    fn only_values_gate_keeps_listed_values() {
        let mut s = DevelopSettings::new();
        set(&mut s, "LensProfileSetup", Value::Str("Custom".into()));
        let (out, skipped) = s.filtered(&Target::Preset {
            mixed_file_kinds: false,
        });
        assert!(out.get_by_name("LensProfileSetup").is_none());
        assert_eq!(skipped.len(), 1);
        set(&mut s, "LensProfileSetup", Value::Str("Auto".into()));
        let (out, _) = s.filtered(&Target::Preset {
            mixed_file_kinds: false,
        });
        assert!(out.get_by_name("LensProfileSetup").is_some());
    }

    #[test]
    fn split_by_frame_scope_sorts_values_and_corrections() {
        let s = sample();
        let split = s.split_by_frame_scope();
        assert!(split
            .per_frame_adjusted
            .get_by_name("Exposure2012")
            .is_some());
        assert!(split
            .per_frame_adjusted
            .get_by_name("Temperature")
            .is_some());
        assert!(split.per_frame_only.get_by_name("CropTop").is_some());
        assert!(split.not_applicable.get_by_name("ProcessVersion").is_some());
        assert!(split.not_applicable.get_by_name("FilterList").is_some());
        assert_eq!(split.shareable.corrections.len(), 1);
        assert_eq!(split.per_frame_only.corrections.len(), 2);
        assert_eq!(split.not_applicable.opaque.len(), 1);
        let total = split.shareable.value_count()
            + split.per_frame_adjusted.value_count()
            + split.per_frame_only.value_count()
            + split.not_applicable.value_count();
        assert_eq!(
            total,
            s.value_count(),
            "every value lands in exactly one part"
        );
        assert_eq!(split.shareable.file_kind, Some(FileKind::Raw));
    }

    fn field(kind: StructKind, name: &str) -> KeyId {
        lookup(Level::Struct(kind), name).unwrap()
    }

    #[test]
    fn nested_fields_are_filtered_at_their_own_rows() {
        let mut blur = Struct::new(StructKind::LensBlur);
        let lb = |n| field(StructKind::LensBlur, n);
        blur.fields.values.insert(lb("Active"), Value::Bool(true));
        blur.fields.values.insert(lb("BlurAmount"), Value::Int(50));
        blur.fields
            .values
            .insert(lb("SampledArea"), Value::Str("0 0 1 1".into()));
        blur.fields
            .values
            .insert(lb("FocalRange"), Value::Str("0 0 100 100".into()));
        blur.fields.opaque.push(OpaqueEntry {
            ns: None,
            name: "Mystery".into(),
            value: Opaque::Json(serde_json::json!(5)),
        });
        let mut s = DevelopSettings::new();
        set(&mut s, "LensBlur", Value::Struct(blur));
        let (out, skipped) = s.filtered(&Target::Preset {
            mixed_file_kinds: true,
        });
        let Some(Value::Struct(kept)) = out.get_by_name("LensBlur") else {
            panic!("LensBlur dropped: {out:?}");
        };
        let names: Vec<&str> = kept.fields.values.keys().map(|id| id.spec().name).collect();
        assert_eq!(names, ["Active", "BlurAmount"]);
        assert!(kept.fields.opaque.is_empty());
        let mut p = paths(&skipped);
        p.sort_unstable();
        assert_eq!(
            p,
            [
                "LensBlur.FocalRange",
                "LensBlur.Mystery",
                "LensBlur.SampledArea"
            ]
        );
        let sampled = skipped
            .iter()
            .find(|k| k.path == "LensBlur.SampledArea")
            .unwrap();
        assert_eq!(sampled.reason, SkipReason::Policy(Policy::Computed));
        let mystery = skipped
            .iter()
            .find(|k| k.path == "LensBlur.Mystery")
            .unwrap();
        assert_eq!(mystery.reason, SkipReason::Opaque);
        // The round trip keeps all of it.
        assert_eq!(s.filtered(&Target::TestRoundTrip).0, s);
    }

    #[test]
    fn a_luminance_range_loses_its_eyedropper_sample_in_a_preset() {
        let crm = |n| field(StructKind::CorrectionRangeMask, n);
        let mut rest = Struct::new(StructKind::CorrectionRangeMask);
        rest.fields.values.insert(crm("Version"), Value::Int(3));
        rest.fields.values.insert(
            crm("LuminanceDepthSampleInfo"),
            Value::Str("0 0.5 0.5".into()),
        );
        let range = MaskTool::LuminanceRange(LuminanceRange {
            range: [0.0, 0.2, 0.8, 1.0].map(Finite::new_const),
            invert: false,
            rest,
        });
        let mut s = DevelopSettings::new();
        s.corrections.push(correction(vec![component(range)]));
        let (out, skipped) = s.filtered(&Target::Preset {
            mixed_file_kinds: true,
        });
        let MaskTool::LuminanceRange(lr) = &out.corrections[0].masks[0].tool else {
            panic!("range mask lost");
        };
        let names: Vec<&str> = lr
            .rest
            .fields
            .values
            .keys()
            .map(|id| id.spec().name)
            .collect();
        assert_eq!(names, ["Version"], "nested META stays");
        assert!(paths(&skipped).contains(
            &"MaskGroupBasedCorrections[0].CorrectionMasks[0].CorrectionRangeMask.LuminanceDepthSampleInfo"
        ));
    }

    #[test]
    fn global_meta_is_left_to_the_writer() {
        let mut s = DevelopSettings::new();
        set(&mut s, "ProcessVersion", Value::Str("11.0".into()));
        set(&mut s, "EnableDetail", Value::Bool(false));
        set(&mut s, "Exposure2012", Value::Real(Finite::new_const(0.5)));
        for target in [
            Target::Preset {
                mixed_file_kinds: false,
            },
            Target::ApplyPhoto {
                file_kind: Some(FileKind::Raw),
                process_version: Some(ProcessVersion::V6),
                camera: None,
            },
        ] {
            let (out, skipped) = s.filtered(&target);
            assert!(out.get_by_name("ProcessVersion").is_none(), "{target:?}");
            assert!(out.get_by_name("EnableDetail").is_none(), "{target:?}");
            assert!(out.get_by_name("Exposure2012").is_some());
            assert!(skipped.is_empty(), "{target:?}: {skipped:?}");
        }
        let (all, _) = s.filtered(&Target::TestRoundTrip);
        assert!(all.get_by_name("ProcessVersion").is_some());
    }

    #[test]
    fn portable_camera_profiles_reach_presets_and_camera_specific_ones_do_not() {
        let preset = Target::Preset {
            mixed_file_kinds: false,
        };
        for name in PORTABLE_PROFILES {
            let mut s = DevelopSettings::new();
            set(&mut s, "CameraProfile", Value::Str((*name).into()));
            let (out, skipped) = s.filtered(&preset);
            assert_eq!(
                out.get_by_name("CameraProfile"),
                Some(&Value::Str((*name).into()))
            );
            assert!(skipped.is_empty());
        }
        let mut s = DevelopSettings::new();
        set(
            &mut s,
            "CameraProfile",
            Value::Str("Camera Standard".into()),
        );
        let (out, skipped) = s.filtered(&preset);
        assert!(out.get_by_name("CameraProfile").is_none());
        assert_eq!(
            skipped,
            [Skipped {
                path: "CameraProfile".into(),
                reason: SkipReason::Gate(Gate::CameraRestricted)
            }]
        );
        // One photo: passed on; the caller keeps to the same make.
        let (out, _) = s.filtered(&Target::ApplyPhoto {
            file_kind: None,
            process_version: None,
            camera: None,
        });
        assert!(out.get_by_name("CameraProfile").is_some());
    }
}
