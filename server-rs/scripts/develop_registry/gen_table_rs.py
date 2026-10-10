"""Seed ``lrg-develop``'s key registry (``registry/table.rs``) from ``spec.py``.

``spec.py`` holds the maintainer's rows as free text (type, UI range, stored
unit, defaults, policy, notes). This script turns them into typed
``KeySpec`` literals:

* group rows are expanded into one row per member (``HueAdjustment*``,
  ``SDR*``, ...);
* families that stay patterns (``Table_<md5>``, ``pm_*``, the FilterList
  payload, ``UprightTransform_N``, ``UprightFourSegments_N``) are skipped here
  and live in ``registry/patterns.rs``;
* the per-key decisions the text cannot express (value kind of "mixed" rows,
  gates, frame scope, presence, number format, recipe aliases) are the tables
  at the top of this file.

It imports ``spec`` and nothing else: no inventory, no training dump, no
ExifTool data, and it never reads the rendered Markdown or its "Observed"
column. The output depends only on committed files.

The Rust table is the source of truth once seeded (docs/wiki/Dev-Develop-Model.md).
Re-running this script overwrites ``table.rs``, including hand edits made
since; use it for the initial seed or when ``spec.py`` changed and the diff is
reviewed. Afterwards regenerate the snapshot::

    python3 gen_table_rs.py --out ../../crates/lrg-develop/src/registry/table.rs
    LRG_BLESS=1 cargo test -p lrg-develop --test registry_snapshot

``rustfmt`` formats the output when it is on ``PATH``.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

import spec

HSL = spec.HSL

# ---------------------------------------------------------------------------
# Rows that are not seeded, with the reason.
# ---------------------------------------------------------------------------

SKIP_ROWS = {
    # Not a develop key at all: the plugin's white-balance bug (fixed in 1d).
    # A registry row would make readers accept it as known, and the planned
    # grep invariant forbids the literal in backend sources.
    ("Global", "Temp"),
}
# Row labels (prefix match) that stay pattern families (registry/patterns.rs).
PATTERN_ROWS = (
    "Table_<md5>",
    "pm_* (",
    "Filters payload",
    "GenAIInfo (",
)
# Members of a group row that are patterns; the rest of the row is seeded.
PATTERN_MEMBERS = re.compile(r"^(UprightTransform_\d+|UprightFourSegments_\d+)$")

# ---------------------------------------------------------------------------
# Levels
# ---------------------------------------------------------------------------

NESTED_LEVEL = {
    "nested:Look": "Struct(StructKind::Look)",
    "nested:LensBlur": "Struct(StructKind::LensBlur)",
    "nested:CorrectionRangeMask": "Struct(StructKind::CorrectionRangeMask)",
    "nested:Gesture": "Struct(StructKind::Gesture)",
    "nested:AILook": "Struct(StructKind::AiLook)",
    "nested:RangeMaskMapInfo": "Struct(StructKind::RangeMaskMapInfo)",
    "nested:DepthMapInfo": "Struct(StructKind::DepthMapInfo)",
    "nested:ISODependent": "Struct(StructKind::IsoDependent)",
    "nested:Preset": "Struct(StructKind::Preset)",
    "nested:RetouchArea": "Struct(StructKind::RetouchArea)",
    "nested:RetouchInfo": "Struct(StructKind::RetouchInfo)",
    "nested:PointColors": "Struct(StructKind::PointColor)",
    "nested:PointColorRange": "Struct(StructKind::PointColorRange)",
    # Look.Parameters holds global keys; ValueKind::Settings says so.
    "look-parameters": None,
    # Everything below FilterList is the filter_list_payload pattern.
    "nested:Filters": None,
}
# Members of a nested row that live one level further down.
SUB_LEVEL = {
    ("nested:Gesture", "X"): "Struct(StructKind::GesturePoint)",
    ("nested:Gesture", "Y"): "Struct(StructKind::GesturePoint)",
    ("nested:CorrectionRangeMask", "AreaComponents"): "Struct(StructKind::AreaModel)",
    ("nested:CorrectionRangeMask", "ColorRangeMaskAreaSampleInfo"): "Struct(StructKind::AreaModel)",
}
# The heal-spot shape row of a retouch area describes mask components.
MASKTOOL_ROWS = ("Mask/Ellipse",)

# ---------------------------------------------------------------------------
# Value kinds
# ---------------------------------------------------------------------------

B_TITLE = "Bool(BoolStyle::TitleCase)"
B_LOWER = "Bool(BoolStyle::Lower)"

# Explicit kinds, by (level, name) or name alone (checked in that order).
KIND = {
    # white balance
    "AutoWhiteVersion": "VersionU32",
    # tone
    ("Global", "Exposure"): "Real",
    ("Global", "Contrast"): "Int",
    ("Global", "Brightness"): "Int",
    ("Global", "Shadows"): "Int",
    ("Global", "FillLight"): "Int",
    ("Global", "HighlightRecovery"): "Int",
    ("Global", "Clarity"): "Int",
    "ToneMapStrength": "Any",
    "ExtendedToneCurvePV2012": "Curve(CurveKind::Global)",
    "ExtendedToneCurvePV2012Red": "Curve(CurveKind::Global)",
    "ExtendedToneCurvePV2012Green": "Curve(CurveKind::Global)",
    "ExtendedToneCurvePV2012Blue": "Curve(CurveKind::Global)",
    "ExtendedToneCurveName2012": "Any",
    "ToneCurve": "Curve(CurveKind::Global)",
    "ToneCurveRed": "Curve(CurveKind::Global)",
    "ToneCurveGreen": "Curve(CurveKind::Global)",
    "ToneCurveBlue": "Curve(CurveKind::Global)",
    "ToneCurveName": "Str",
    "ToneCurveName2012": "Str",
    # upright solver state
    "UprightVersion": "VersionU32",
    "UprightTransformCount": "Int",
    "UprightCenterMode": "Int",
    "UprightCenterNormX": "Real",
    "UprightCenterNormY": "Real",
    "UprightFocalMode": "Int",
    "UprightFocalLength35mm": "Real",
    "UprightPreview": B_TITLE,
    "UprightDependentDigest": "Hex32",
    "UprightGuidedDependentDigest": "Hex32",
    "UprightFourSegmentsCount": "Int",
    # crop / effects / profile (names only, never observed)
    "CropWidth": "Any",
    "CropHeight": "Any",
    "CropUnit": "Any",
    "CropUnits": "Any",
    "GlowRange": "Any",
    "GlowWarmth": "Any",
    "GlowSpread": "Any",
    "GlowStyle": "Any",
    "ReshapeAmount": "Any",
    "GrainSeed": "Int",
    "LookTable": "Hex32",
    "RGBTable": "Hex32",
    "LookTableAmount": "Any",
    "RGBTableAmount": "Any",
    "RGBTables": "Any",
    "ProfileGainTableMap": "Any",
    "ProfileGainTableMap2": "Any",
    "ProfileToneCurve": "Any",
    "MissingCameraProfile": "Any",
    "MissingCameraProfileDigest": "Hex32",
    "SourceAutoExposure2012": "Any",
    "SourceAutoContrast2012": "Any",
    "SourceAutoHighlights2012": "Any",
    "SourceAutoShadows2012": "Any",
    "SourceAutoWhites2012": "Any",
    "SourceAutoBlacks2012": "Any",
    "SourceAutoSaturation": "Any",
    "SourceAutoVibrance": "Any",
    "ToggleStyleAmount": "Real",
    # structures at the top level
    "Look": "Struct(StructKind::Look)",
    ("Global", "LensBlur"): "Struct(StructKind::LensBlur)",
    "AILook": "Struct(StructKind::AiLook)",
    "ISODependent": "StructSeq(StructKind::IsoDependent)",
    "RetouchAreas": "StructSeq(StructKind::RetouchArea)",
    "RemoveAreas": "StructSeq(StructKind::RetouchArea)",
    "RetouchInfo": "PerFormat { lua: &ValueKind::StructSeq(StructKind::RetouchInfo), xmp: &ValueKind::StrSeq }",
    "PointColors": "PerFormat { lua: &ValueKind::StructSeq(StructKind::PointColor), xmp: &ValueKind::StrSeq }",
    "FilterList": "Struct(StructKind::FilterList)",
    "AllowFilters": "Int",
    "DepthMapInfo": "Struct(StructKind::DepthMapInfo)",
    ("Global", "RangeMaskMapInfo"): "Struct(StructKind::RangeMaskMapInfo)",
    "DepthBasedCorrections": "CorrectionSeq",
    "MaskGroupBasedCorrections": "CorrectionSeq",
    "GradientBasedCorrections": "CorrectionSeq",
    "CircularGradientBasedCorrections": "CorrectionSeq",
    "PaintBasedCorrections": "CorrectionSeq",
    ("Global", "Preset"): "Struct(StructKind::Preset)",
    "AsShotTemperature": "Int",
    "AsShotTint": "Int",
    "ProcessVersion": "VersionStr",
    ("Global", "Version"): "VersionStr",
    "CompatibleVersion": "VersionU32",
    "RenderVersion": "VersionU32",
    "orientation": "Str",
    # header
    "Baseline": 'Enum(&["Adobe Default", "Camera Settings"])',
    # correction
    ("Correction", "What"): 'Enum(&["Correction"])',
    "CorrectionMasks": "ComponentSeq",
    "CorrectionID": "Str",
    "CorrectionReferenceX": "Real",
    "CorrectionReferenceY": "Real",
    # mask components
    ("MaskTool", "MaskID"): "Str",
    "MaskDigest": "Hex32",
    ("MaskTool", "InputDigest"): "Hex32",
    ("MaskTool", "InputDigestVersion"): "Int",
    "LocalInputDigest": "Hex32",
    "LocalInputDigestVersion": "Int",
    ("MaskTool", "ModelVersion"): "VersionU32",
    "WholeImageArea": "Str",
    "Origin": "Str",
    "FullMaskSize": "Str",
    "DidOverrideInputDigestMismatch": B_LOWER,
    "MaskSubCategoryID": "Int",
    ("MaskTool", "Gesture"): "StructSeq(StructKind::Gesture)",
    ("MaskTool", "Masks"): "ComponentSeq",
    ("MaskTool", "CorrectionRangeMask"): "Struct(StructKind::CorrectionRangeMask)",
    "MaskBrushTable": "Hex32",
    "MaskBrushUncompressedBytes": "Int",
    ("MaskTool", "Alpha"): "Real",
    ("MaskTool", "CenterValue"): "Real",
    ("MaskTool", "PerimeterValue"): "Real",
    ("MaskTool", "X"): "Real",
    ("MaskTool", "Y"): "Real",
    ("MaskTool", "SizeX"): "Real",
    ("MaskTool", "SizeY"): "Real",
    ("MaskTool", "Radius"): "Real",
    ("MaskTool", "Flow"): "Real",
    ("MaskTool", "CenterWeight"): "Real",
    ("MaskTool", "Midpoint"): "Int",
    ("MaskTool", "Roundness"): "Int",
    # Look
    ("Struct(StructKind::Look)", "Parameters"): "Settings",
    ("Struct(StructKind::Look)", "Cluster"): "Str",
    ("Struct(StructKind::Look)", "Copyright"): "Str",
    # LensBlur
    ("Struct(StructKind::LensBlur)", "SampledArea"): "Str",
    ("Struct(StructKind::LensBlur)", "SampledRange"): "Str",
    ("Struct(StructKind::LensBlur)", "SubjectRange"): "Str",
    ("Struct(StructKind::LensBlur)", "ImageOrientation"): "Int",
    ("Struct(StructKind::LensBlur)", "FocalRangeSource"): "Int",
    ("Struct(StructKind::LensBlur)", "BokehShape"): "Int",
    # CorrectionRangeMask
    ("Struct(StructKind::CorrectionRangeMask)", "AreaModels"): "StructSeq(StructKind::AreaModel)",
    ("Struct(StructKind::AreaModel)", "AreaComponents"): "StrSeq",
    ("Struct(StructKind::AreaModel)", "ColorRangeMaskAreaSampleInfo"): "Str",
    ("Struct(StructKind::CorrectionRangeMask)", "LumMin"): "Any",
    ("Struct(StructKind::CorrectionRangeMask)", "LumMax"): "Any",
    ("Struct(StructKind::CorrectionRangeMask)", "LumFeather"): "Any",
    ("Struct(StructKind::CorrectionRangeMask)", "DepthMin"): "Any",
    ("Struct(StructKind::CorrectionRangeMask)", "DepthMax"): "Any",
    ("Struct(StructKind::CorrectionRangeMask)", "DepthFeather"): "Any",
    ("Struct(StructKind::CorrectionRangeMask)", "DepthRange"): "Any",
    # Gesture
    ("Struct(StructKind::Gesture)", "Points"): "StructSeq(StructKind::GesturePoint)",
    ("Struct(StructKind::GesturePoint)", "X"): "Real",
    ("Struct(StructKind::GesturePoint)", "Y"): "Real",
    ("Struct(StructKind::Gesture)", "Dabs"): "StrSeq",
    ("Struct(StructKind::Gesture)", "Radius"): "Real",
    ("Struct(StructKind::Gesture)", "Flow"): "Real",
    ("Struct(StructKind::Gesture)", "CenterWeight"): "Real",
    ("Struct(StructKind::Gesture)", "BrushGestureInterpretation"): "Int",
    ("Struct(StructKind::Gesture)", "MaskActive"): B_LOWER,
    ("Struct(StructKind::Gesture)", "MaskBlendMode"): "EnumInt(&[0, 1])",
    ("Struct(StructKind::Gesture)", "MaskInverted"): B_LOWER,
    ("Struct(StructKind::Gesture)", "MaskSyncID"): "Hex32",
    ("Struct(StructKind::Gesture)", "MaskValue"): "Real",
    ("Struct(StructKind::Gesture)", "MaskID"): "Str",
    # AILook
    ("Struct(StructKind::AiLook)", "Active"): B_LOWER,
    ("Struct(StructKind::AiLook)", "AILookData"): "Str",
    ("Struct(StructKind::AiLook)", "InputDigest"): "Hex32",
    ("Struct(StructKind::AiLook)", "InputDigestVersion"): "Int",
    ("Struct(StructKind::AiLook)", "ModelVersion"): "VersionU32",
    ("Struct(StructKind::AiLook)", "Version"): "VersionU32",
    # RangeMaskMapInfo
    ("Struct(StructKind::RangeMaskMapInfo)", "RangeMaskMapInfo"): "Struct(StructKind::RangeMaskMapInfo)",
    ("Struct(StructKind::RangeMaskMapInfo)", "RGBMin"): "Str",
    ("Struct(StructKind::RangeMaskMapInfo)", "RGBMax"): "Str",
    ("Struct(StructKind::RangeMaskMapInfo)", "LabMin"): "Str",
    ("Struct(StructKind::RangeMaskMapInfo)", "LabMax"): "Str",
    ("Struct(StructKind::RangeMaskMapInfo)", "LumEq"): "StrSeq",
    # DepthMapInfo
    ("Struct(StructKind::DepthMapInfo)", "DepthSource"): "Int",
    ("Struct(StructKind::DepthMapInfo)", "BaseRawDepthTable"): "Hex32",
    ("Struct(StructKind::DepthMapInfo)", "BaseRawDepthInputDigest"): "Hex32",
    ("Struct(StructKind::DepthMapInfo)", "BaseRawDepthVersion"): "Int",
    ("Struct(StructKind::DepthMapInfo)", "BaseLayeredDepthTable"): "Hex32",
    ("Struct(StructKind::DepthMapInfo)", "BaseLayeredDepthInputDigest"): "Hex32",
    ("Struct(StructKind::DepthMapInfo)", "BaseLayeredDepthVersion"): "Int",
    ("Struct(StructKind::DepthMapInfo)", "BaseHighlightGuideTable"): "Hex32",
    ("Struct(StructKind::DepthMapInfo)", "BaseHighlightGuideInputDigest"): "Hex32",
    ("Struct(StructKind::DepthMapInfo)", "BaseHighlightGuideVersion"): "Int",
    # Preset record
    ("Struct(StructKind::Preset)", "Name"): "Str",
    ("Struct(StructKind::Preset)", "UUID"): "Hex32",
    ("Struct(StructKind::Preset)", "Amount"): "Real",
    ("Struct(StructKind::Preset)", "LookAmount"): "Real",
    ("Struct(StructKind::Preset)", "Group"): "LangAlt",
    ("Struct(StructKind::Preset)", "Cluster"): "Str",
    ("Struct(StructKind::Preset)", "Baseline"): "Str",
    ("Struct(StructKind::Preset)", "SupportsAmount"): B_LOWER,
    ("Struct(StructKind::Preset)", "SupportsMonochrome"): B_LOWER,
    ("Struct(StructKind::Preset)", "SupportsOutputReferred"): B_LOWER,
    ("Struct(StructKind::Preset)", "Parameters"): "Settings",
    # retouch spots
    ("Struct(StructKind::RetouchArea)", "SpotType"): "Str",
    ("Struct(StructKind::RetouchArea)", "Method"): "Str",
    ("Struct(StructKind::RetouchArea)", "SourceState"): "Str",
    ("Struct(StructKind::RetouchArea)", "Opacity"): "Real",
    ("Struct(StructKind::RetouchArea)", "Feather"): "Real",
    ("Struct(StructKind::RetouchArea)", "Seed"): "Int",
    ("Struct(StructKind::RetouchArea)", "OffsetY"): "Real",
    ("Struct(StructKind::RetouchArea)", "SourceX"): "Real",
    ("Struct(StructKind::RetouchArea)", "HealVersion"): "Int",
    ("Struct(StructKind::RetouchArea)", "IngestInfo"): "Str",
    ("Struct(StructKind::RetouchArea)", "fill_method"): "Str",
    ("Struct(StructKind::RetouchArea)", "Masks"): "ComponentSeq",
    ("Struct(StructKind::PointColor)", "HueRange"): "Struct(StructKind::PointColorRange)",
    ("Struct(StructKind::PointColor)", "SatRange"): "Struct(StructKind::PointColorRange)",
    ("Struct(StructKind::PointColor)", "LumRange"): "Struct(StructKind::PointColorRange)",
    # correction extras never observed
    "LocalColorGradeShadowHue": "Any",
    "DepthRange": "Any",
    ("MaskTool", "TargetImageArea"): "Any",
    ("MaskTool", "EdgeShift"): "Any",
    ("MaskTool", "Bidirectional"): "Any",
    ("MaskTool", "Zero2X"): "Any",
    ("MaskTool", "Zero2Y"): "Any",
    ("MaskTool", "ZeroFeather"): "Any",
    ("MaskTool", "Zero2Feather"): "Any",
    ("MaskTool", "FullPointDistance"): "Any",
}
# Lower-case legacy spot fields (retouch areas and RetouchInfo items).
for _lvl in ("Struct(StructKind::RetouchArea)", "Struct(StructKind::RetouchInfo)"):
    for _n, _k in (
        ("centerX", "Real"),
        ("centerY", "Real"),
        ("opacity", "Real"),
        ("radius", "Real"),
        ("seed", "Int"),
        ("sourceState", "Str"),
        ("sourceX", "Real"),
        ("sourceY", "Real"),
        ("spotType", "Str"),
        ("Method", "Str"),
    ):
        KIND[(_lvl, _n)] = _k
# Everything else in a "mixed"/"number" row that is only known by name.
ANY_TYPES = {"mixed", "number", "?"}

ENUMS = {
    "WhiteBalance": (
        'Enum(&["As Shot", "Auto", "Custom", "Daylight", "Cloudy", "Shade", "Tungsten", "Fluorescent", "Flash"])'
    ),
    "LensProfileSetup": 'Enum(&["LensDefaults", "Auto", "Custom"])',
    "PresetType": 'Enum(&["Normal", "Look"])',
    ("MaskTool", "What"): (
        'Enum(&["Mask/Image", "Mask/Gradient", "Mask/CircularGradient", "Mask/RangeMask", '
        '"Mask/Aggregate", "Mask/Paint", "Mask/Polygon", "Mask/Ellipse", "Mask/FlattenedGroup", '
        '"Mask/Clip", "Mask/Combine"])'
    ),
    ("Struct(StructKind::Gesture)", "What"): 'Enum(&["Mask/Polygon", "Mask/Paint"])',
    "PerspectiveUpright": "EnumInt(&[0, 1, 2, 3, 4, 5])",
    "PostCropVignetteStyle": "EnumInt(&[1, 2, 3])",
    ("MaskTool", "MaskBlendMode"): "EnumInt(&[0, 1])",
    "MaskSubType": "EnumInt(&[0, 1, 2, 3])",
    "ErrorReason": "EnumInt(&[0, 1])",
    ("Struct(StructKind::CorrectionRangeMask)", "Type"): "EnumInt(&[1, 2, 3])",
    ("Struct(StructKind::CorrectionRangeMask)", "SampleType"): "EnumInt(&[0, 1])",
}

SIMPLE_TYPES = {
    "int": "Int",
    "real": "Real",
    "int (0/1)": "IntFlag",
    "string": "Str",
    "hex32": "Hex32",
    "uint": "Int",
    "uint32": "Int",
    "string[]": "StrSeq",
    "lang-alt": "LangAlt",
    "runtime": "Str",
}

# ---------------------------------------------------------------------------
# Policies, gates, scopes
# ---------------------------------------------------------------------------

POLICY = {
    "LEARN": "Learn",
    "PHOTO": "Photo",
    "COMPUTED": "Computed",
    "NEVER": "Never",
    "META": "Meta",
    "UNKNOWN": "Unknown",
}
# The gate of every LEARN† key (by name, or (level, name)).
GATE = {
    "Temperature": "RawOnly",
    "Tint": "RawOnly",
    "IncrementalTemperature": "NonRawOnly",
    "IncrementalTint": "NonRawOnly",
    "ConvertToGrayscale": "Categorical",
    **{"GrayMixer" + c: 'DependsOn("ConvertToGrayscale")' for c in HSL},
    "Sharpness": "FileKindDefault",
    "ColorNoiseReduction": "FileKindDefault",
    "LensProfileSetup": 'OnlyValues(&["LensDefaults", "Auto"])',
    "LensProfileDistortionScale": 'DependsOn("LensProfileEnable")',
    "LensProfileVignettingScale": 'DependsOn("LensProfileEnable")',
    ("Global", "LensBlur"): "NeedsAiUpdate",
    "CameraProfile": "CameraRestricted",
    "Look": "CameraRestricted",
    "MaskGroupBasedCorrections": "ByMaskType",
    "CorrectionMasks": "ByMaskType",
    ("MaskTool", "What"): "ByMaskType",
    ("MaskTool", "CorrectionRangeMask"): "ByMaskType",
    ("Struct(StructKind::CorrectionRangeMask)", "Type"): "ByMaskType",
}
# LEARN keys the registry gates although spec.py lists them as plain LEARN:
# hue angles are averaged on the circle, weighted by the saturation of the
# same zone (a hue at saturation 0 is invisible, spec note "weight by
# saturation"); the vignette style is a category (spec note "Categorical
# (majority)").
EXTRA_GATE = {
    "SplitToningShadowHue": 'CircularHue("SplitToningShadowSaturation")',
    "SplitToningHighlightHue": 'CircularHue("SplitToningHighlightSaturation")',
    "ColorGradeMidtoneHue": 'CircularHue("ColorGradeMidtoneSat")',
    "ColorGradeGlobalHue": 'CircularHue("ColorGradeGlobalSat")',
    ("Correction", "LocalToningHue"): 'CircularHue("LocalToningSaturation")',
    "PostCropVignetteStyle": "Categorical",
}

# Adjusted per frame: exposure, white balance and the keys the edit guardrails
# scale per frame (lrg-api/src/edit_budget.rs: contrast, clarity, dehaze, the
# parametric curve, shadows, whites).
PER_FRAME_ADJUSTED = {
    "Exposure2012",
    "Temperature",
    "Tint",
    "IncrementalTemperature",
    "IncrementalTint",
    "WhiteBalance",
    "Contrast2012",
    "Clarity2012",
    "Dehaze",
    "Shadows2012",
    "Whites2012",
    "ParametricHighlights",
    "ParametricLights",
    "ParametricDarks",
    "ParametricShadows",
}

FILE_SCOPE = {
    "Temperature": "RawOnly",
    "Tint": "RawOnly",
    "IncrementalTemperature": "NonRawOnly",
    "IncrementalTint": "NonRawOnly",
}

LUA_ONLY = {
    *(
        "Enable" + p
        for p in (
            "Calibration",
            "ColorAdjustments",
            "Detail",
            "Effects",
            "GrayscaleMix",
            "LensCorrections",
            "RedEye",
            "Retouch",
            "SplitToning",
            "ToneCurve",
            "Transform",
            "MaskGroupBasedCorrections",
            "GradientBasedCorrections",
            "DistractionRemoval",
        )
    ),
    "orientation",
    "CropConstrainAspectRatio",
    "Defringe",
    "ChromaticAberrationR",
    "ChromaticAberrationB",
    ("Correction", "CorrectionID"),
    ("Correction", "CorrectionReferenceX"),
    ("Correction", "CorrectionReferenceY"),
    ("MaskTool", "MaskID"),
    ("Struct(StructKind::Gesture)", "MaskID"),
    ("Struct(StructKind::Look)", "isAdobeAdaptive"),
}
LUA_ONLY_LEVELS = {
    "Struct(StructKind::RetouchInfo)",
    "Struct(StructKind::PointColor)",
    "Struct(StructKind::PointColorRange)",
}
LUA_ONLY_RETOUCH = {"centerX", "centerY", "opacity", "radius", "seed", "sourceState", "sourceX", "sourceY", "spotType"}
XMP_ONLY = {"HasCrop", "HasSettings", "AlreadyApplied", "RawFileName", ("Global", "Preset")}
# Names from "never observed" rows that were seen after all (getDevelopSettings()).
OBSERVED_AFTER_ALL = {
    "ExtendedToneCurvePV2012",
    "ExtendedToneCurvePV2012Red",
    "ExtendedToneCurvePV2012Green",
    "ExtendedToneCurvePV2012Blue",
}

# Keys whose meaning depends on process version 2012 or later.
PV2012_NOTE = re.compile(r"PV2012\+")

# Number formats beyond the kind's default.
FIXED = {
    "Exposure2012": 2,
    "HDRMaxValue": 2,
    "PerspectiveX": 2,
    "PerspectiveY": 2,
    "SharpenRadius": 1,
    "PerspectiveRotate": 1,
    "ProcessVersion": 1,
}
COMPOUND = {
    ("MaskTool", "ReferencePoint"),
    ("Struct(StructKind::CorrectionRangeMask)", "LumRange"),
    ("Struct(StructKind::CorrectionRangeMask)", "LuminanceDepthSampleInfo"),
    ("Struct(StructKind::CorrectionRangeMask)", "PointModels"),
    ("Global", "PointColors"),
    ("Global", "ColorVariance"),
    ("Correction", "LocalPointColors"),
    ("Correction", "LocalColorVariance"),
    ("MaskTool", "Dabs"),
    ("Struct(StructKind::Gesture)", "Dabs"),
    ("Struct(StructKind::RangeMaskMapInfo)", "RGBMin"),
    ("Struct(StructKind::RangeMaskMapInfo)", "RGBMax"),
    ("Struct(StructKind::RangeMaskMapInfo)", "LabMin"),
    ("Struct(StructKind::RangeMaskMapInfo)", "LabMax"),
    ("Struct(StructKind::RangeMaskMapInfo)", "LumEq"),
}
# "+" is written although the value is never negative.
ALWAYS_PLUS = {"SharpenRadius", "HDRMaxValue"}
NEVER_PLUS = {"CropTop", "CropLeft", "CropBottom", "CropRight", "CropAngle", "Temperature"}

# Ranges the UI-range text does not give in a parsable form (stored unit).
RANGE = {
    "LensProfileEnable": (0, 1),
    "AutoLateralCA": (0, 1),
    "CropConstrainToWarp": (0, 1),
    "HDREditMode": (0, 1),
    ("Struct(StructKind::CorrectionRangeMask)", "ColorAmount"): (0, 1),
}
NO_RANGE = {
    # observed spreads, not limits
    ("MaskTool", "Angle"),
    ("Struct(StructKind::Look)", "Amount"),  # set below with the scale
}
UI_DIV = {("Struct(StructKind::CorrectionRangeMask)", "ColorAmount"): 100.0}

# Recipe aliases: the recipe fields the style engine blends and sends
# (lrg-analysis/src/training.rs `canonical_keys`), each one a field the
# installed plugin already applies (DevelopEditManager.lua: GLOBAL_KEY_MAP,
# buildHslDevelopSettings, buildColorGradingDevelopSettings,
# buildToneCurveSettings). White balance is left out on purpose: step 1d moves
# it out of the generic blend into its own policy. Not aliased although the
# plugin reads a recipe field for them: colour grading midtones/global/
# luminance/blending (the installed plugin warns and drops them) and the point
# curves (averaging curves is step 3).
ALIAS = {
    "Exposure2012": "global.exposure",
    "Contrast2012": "global.contrast",
    "Highlights2012": "global.highlights",
    "Shadows2012": "global.shadows",
    "Whites2012": "global.whites",
    "Blacks2012": "global.blacks",
    "Texture": "global.texture",
    "Clarity2012": "global.clarity",
    "Dehaze": "global.dehaze",
    "Vibrance": "global.vibrance",
    "Saturation": "global.saturation",
    "Sharpness": "global.sharpening",
    "SharpenRadius": "global.sharpen_radius",
    "SharpenDetail": "global.sharpen_detail",
    "SharpenEdgeMasking": "global.sharpen_masking",
    "LuminanceSmoothing": "global.noise_reduction",
    "LuminanceNoiseReductionDetail": "global.noise_reduction_detail",
    "LuminanceNoiseReductionContrast": "global.noise_reduction_contrast",
    "ColorNoiseReduction": "global.color_noise_reduction",
    "ColorNoiseReductionDetail": "global.color_noise_reduction_detail",
    "ColorNoiseReductionSmoothness": "global.color_noise_reduction_smoothness",
    "PostCropVignetteAmount": "global.vignette",
    "PostCropVignetteMidpoint": "global.vignette_midpoint",
    "PostCropVignetteRoundness": "global.vignette_roundness",
    "PostCropVignetteFeather": "global.vignette_feather",
    "PostCropVignetteHighlightContrast": "global.vignette_highlights",
    "GrainAmount": "global.grain",
    "GrainSize": "global.grain_size",
    "GrainFrequency": "global.grain_roughness",
    "ParametricHighlights": "global.tone_curve.highlights",
    "ParametricLights": "global.tone_curve.lights",
    "ParametricDarks": "global.tone_curve.darks",
    "ParametricShadows": "global.tone_curve.shadows",
    "ParametricShadowSplit": "global.tone_curve.shadow_split",
    "ParametricMidtoneSplit": "global.tone_curve.midtone_split",
    "ParametricHighlightSplit": "global.tone_curve.highlight_split",
    **{f"{slider}Adjustment{c}": f"global.hsl.{c.lower()}.{field}"
       for c in HSL
       for slider, field in (("Hue", "hue"), ("Saturation", "saturation"), ("Luminance", "luminance"))},
    "SplitToningShadowHue": "global.color_grading.shadows.hue",
    "SplitToningShadowSaturation": "global.color_grading.shadows.saturation",
    "SplitToningHighlightHue": "global.color_grading.highlights.hue",
    "SplitToningHighlightSaturation": "global.color_grading.highlights.saturation",
    "SplitToningBalance": "global.color_grading.balance",
}

# Default literals the free text does not give in parsable form.
STR_DEFAULT = {
    "ToneCurveName2012": "Linear",
    "CameraProfile": "Adobe Standard",
    ("Header", "CameraModelRestriction"): "",
}
NO_DEFAULT = {"Look", ("Global", "LensBlur"), "PointColors", "ColorVariance", "RedEyeInfo"}

# ---------------------------------------------------------------------------


def pick(table: dict, level: str, name: str):
    if (level, name) in table:
        return table[(level, name)]
    return table.get(name)


def has(s: set, level: str, name: str) -> bool:
    return (level, name) in s or name in s


@dataclass
class Row:
    level: str
    name: str
    typ: str
    ui_range: str
    stored: str
    default: str
    pv: str
    policy: str
    note: str
    out: dict = field(default_factory=dict)


def rows() -> list[Row]:
    out: list[Row] = []

    def add(level: str, r: tuple) -> None:
        key, typ, ui, st, dflt, pv, pol, notes = r[:8]
        if any(key.startswith(p) for p in PATTERN_ROWS):
            return
        members = r[8] if len(r) > 8 else [key]
        for m in members:
            if PATTERN_MEMBERS.match(m) or (level, m) in SKIP_ROWS:
                continue
            out.append(Row(level, m, typ, ui, st, dflt, pv, pol, notes))

    for _title, rs in spec.G:
        for r in rs:
            add("Global", r)
    for r in spec.HEADER:
        add("Header", r)
    for r in spec.CORR:
        add("Correction", r)
    for _title, rs in spec.MASK:
        for r in rs:
            add("MaskTool", r)
    for lvl, _title, rs in spec.NESTED:
        if lvl not in NESTED_LEVEL:
            raise SystemExit(f"no level mapping for {lvl}")
        level = NESTED_LEVEL[lvl]
        if level is None:
            continue
        for r in rs:
            if r[0].startswith(MASKTOOL_ROWS):
                add("MaskTool", r)
                continue
            before = len(out)
            add(level, r)
            for row in out[before:]:
                row.level = SUB_LEVEL.get((lvl, row.name), row.level)
    return out


NUM = r"[+-]?\d+(?:\.\d+)?"
RANGE_RE = re.compile(rf"^\s*({NUM})\.\.({NUM})(?!\?)(?![\d.])")


def kind_of(r: Row) -> str:
    k = pick(ENUMS, r.level, r.name) or pick(KIND, r.level, r.name)
    if k:
        return k
    t = r.typ.strip()
    if t == "bool":
        return B_TITLE if r.level in ("Global", "Header") else B_LOWER
    if t == "curve":
        return "Curve(CurveKind::Local)" if r.level == "Correction" else "Curve(CurveKind::Global)"
    if t in SIMPLE_TYPES:
        return SIMPLE_TYPES[t]
    if t in ANY_TYPES:
        return "Any"
    raise SystemExit(f"no value kind for {r.level}/{r.name} (type {t!r})")


def policy_of(r: Row) -> str:
    p = r.policy
    if p.endswith("†"):
        g = pick(GATE, r.level, r.name)
        if not g:
            raise SystemExit(f"LEARN† without a gate: {r.level}/{r.name}")
        return f"LearnGated(Gate::{g})"
    if p == "LEARN":
        g = pick(EXTRA_GATE, r.level, r.name)
        if g:
            return f"LearnGated(Gate::{g})"
    return POLICY[p]


def presence_of(r: Row) -> str:
    if has(LUA_ONLY, r.level, r.name) or r.level in LUA_ONLY_LEVELS:
        return "LuaOnly"
    if r.level == "Struct(StructKind::RetouchArea)" and r.name in LUA_ONLY_RETOUCH:
        return "LuaOnly"
    if r.level == "Header" or has(XMP_ONLY, r.level, r.name):
        return "XmpOnly"
    if "never observed in LrC output" in r.pv and r.name not in OBSERVED_AFTER_ALL:
        return "Unobserved"
    if "never observed in XMP or getDevelopSettings" in r.pv:
        return "Unobserved"
    return "Both"


def frame_of(r: Row, policy: str) -> str:
    if policy in ("Computed", "Meta", "Unknown"):
        return "NotApplicable"
    if policy == "Never":
        return "PerFrameOnly"
    if r.level == "Global" and r.name in PER_FRAME_ADJUSTED:
        return "PerFrameAdjusted"
    if policy == "Photo":
        return "PerFrameOnly"
    return "Shareable"


def scale_of(r: Row, kind: str) -> tuple[str, float]:
    """UiScale literal and the divisor applied to the UI range."""
    div = pick(UI_DIV, r.level, r.name)
    if div:
        return f"UiScale::Div({div:.1f})", 1.0  # its range text is already stored
    if kind not in ("Int", "Real", "IntFlag") and not kind.startswith("EnumInt") and kind != "VersionU32":
        return "UiScale::Identity", 1.0
    st = r.stored
    if "?" in st or (r.policy == "UNKNOWN" and "?" in r.ui_range):
        return "UiScale::Unknown", 1.0
    if "EV/4" in st:
        return "UiScale::Div(4.0)", 4.0
    if "UI/100" in st or "/100" in st:
        return "UiScale::Div(100.0)", 100.0
    return "UiScale::Identity", 1.0


def range_of(r: Row, kind: str, div: float) -> tuple[float, float] | None:
    if kind not in ("Int", "Real", "IntFlag"):
        return None
    if has(NO_RANGE, r.level, r.name):
        return None
    explicit = pick(RANGE, r.level, r.name)
    if explicit:
        return explicit
    m = RANGE_RE.match(r.ui_range)
    if not m:
        return None
    lo, hi = float(m.group(1)), float(m.group(2))
    return lo / div, hi / div


def parse_lit(kind: str, text: str):
    t = text.strip().strip('"')
    if kind in ("Int", "IntFlag", "VersionU32") or kind.startswith("EnumInt"):
        if re.fullmatch(r"[+-]?\d+", t):
            return ("Int", int(t))
        return None
    if kind == "Real":
        if re.fullmatch(NUM, t):
            return ("Real", float(t))
        return None
    if kind.startswith("Bool"):
        if t in ("True", "true"):
            return ("Bool", True)
        if t in ("False", "false"):
            return ("Bool", False)
        return None
    if kind.startswith("Enum("):
        values = re.findall(r'"([^"]*)"', kind)
        return ("Str", t) if t in values else None
    if kind == "Curve(CurveKind::Global)":
        m = re.fullmatch(r"\[([\d, ]+)\]", t)
        return ("IntList", [int(x) for x in m.group(1).split(",")]) if m else None
    return None


def default_of(r: Row, kind: str, policy: str) -> tuple:
    """(raw, non_raw) as ("Value", lit) | ("Absent",) | ("Unverified",) | ("NoDefault",)."""
    name = r.name
    if name in ("Temperature", "Tint"):
        return ("NoDefault",), ("Absent",)
    if name in ("IncrementalTemperature", "IncrementalTint"):
        return ("Absent",), ("Unverified",)
    s = pick(STR_DEFAULT, r.level, name)
    if s is not None:
        return ("Value", ("Str", s)), ("Unverified",)
    text = r.default.strip()
    if has(NO_DEFAULT, r.level, name) or not text:
        if not text and (policy == "Unknown" or "?" in r.typ):
            return ("Unverified",), ("Unverified",)
        return ("NoDefault",), ("NoDefault",)
    if text == "?":
        return ("Unverified",), ("Unverified",)
    left = text.split(" / ")[0]
    left = re.split(r"\s+\(|\s+in preset form|\s+in presets", left)[0]
    lit = parse_lit(kind, left) or parse_lit(kind, left.split()[0] if left.split() else "")
    if lit is None:
        return ("NoDefault",), ("NoDefault",)
    return ("Value", lit), ("Unverified",)


def fmt_of(r: Row, kind: str, presence: str) -> str:
    if presence == "LuaOnly":
        return "NumFmt::Text"
    if r.name in FIXED and r.level == "Global":
        return f"NumFmt::Fixed({FIXED[r.name]})"
    if (r.level, r.name) in COMPOUND:
        return "NumFmt::CompoundFixed6"
    if kind in ("Int", "IntFlag", "VersionU32") or kind.startswith("EnumInt") or kind.startswith("Curve"):
        return "NumFmt::Int"
    if kind == "Real":
        return "NumFmt::Trim6"
    return "NumFmt::Text"


def plus_of(r: Row, kind: str, rng, presence: str) -> bool:
    if r.level != "Global" or presence == "LuaOnly" or r.name in NEVER_PLUS:
        return False
    if r.name in ALWAYS_PLUS:
        return True
    return kind in ("Int", "Real") and rng is not None and rng[0] < 0


# ---------------------------------------------------------------------------
# Rust rendering
# ---------------------------------------------------------------------------


def rs_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def rs_f(x: float) -> str:
    return f"Finite::new_const({x!r})" if not float(x).is_integer() else f"Finite::new_const({float(x):.1f})"


def rs_def(d: tuple) -> str:
    if d[0] != "Value":
        return f"Def::{d[0]}"
    tag, v = d[1]
    if tag == "Int":
        return f"Def::Value(Lit::Int({v}))"
    if tag == "Real":
        return f"Def::Value(Lit::Real({rs_f(v)}))"
    if tag == "Bool":
        return f"Def::Value(Lit::Bool({'true' if v else 'false'}))"
    if tag == "Str":
        return f"Def::Value(Lit::Str({rs_str(v)}))"
    if tag == "IntList":
        return f"Def::Value(Lit::IntList(&[{', '.join(map(str, v))}]))"
    raise AssertionError(tag)


def note_of(r: Row) -> str:
    note, pv = r.note.strip(), r.pv.strip()
    text = f"{note} [{pv}]" if note and pv else note or (f"[{pv}]" if pv else "")
    text = text.replace("**", "")
    if '"Temp"' in text:
        raise SystemExit('a note must not contain the literal "Temp" (backend sources must not hold it)')
    return text


def render(all_rows: list[Row]) -> tuple[str, dict]:
    seen: set[tuple[str, str]] = set()
    lines = []
    counts: dict = {}
    for r in all_rows:
        if (r.level, r.name) in seen:
            raise SystemExit(f"duplicate row {r.level}/{r.name}")
        seen.add((r.level, r.name))
        kind = kind_of(r)
        policy = policy_of(r)
        presence = presence_of(r)
        ui, div = scale_of(r, kind)
        rng = range_of(r, kind, div)
        if r.level == "Struct(StructKind::Look)" and r.name == "Amount":
            ui, rng = "UiScale::Div(100.0)", (0.0, 2.0)
        raw, non_raw = default_of(r, kind, policy.split("(")[0])
        fmt = fmt_of(r, kind, presence)
        plus = plus_of(r, kind, rng, presence)
        frame = frame_of(r, policy.split("(")[0])
        file_scope = FILE_SCOPE.get(r.name, "Both") if r.level == "Global" else "Both"
        min_pv = "Some(ProcessVersion::PV2012)" if PV2012_NOTE.search(r.pv) else "None"
        alias = ALIAS.get(r.name) if r.level == "Global" else None
        if raw[0] == "Value" and rng is not None and raw[1][0] in ("Int", "Real"):
            v = raw[1][1]
            if not rng[0] <= v <= rng[1]:
                raise SystemExit(f"default {v} outside range {rng} for {r.level}/{r.name}")
        rng_rs = "None" if rng is None else f"Some(({rs_f(rng[0])}, {rs_f(rng[1])}))"
        lines.append(
            "    KeySpec {\n"
            f"        name: {rs_str(r.name)},\n"
            f"        level: Level::{r.level},\n"
            f"        kind: ValueKind::{kind},\n"
            f"        range: {rng_rs},\n"
            f"        ui: {ui},\n"
            f"        fmt: {fmt},\n"
            f"        plus_sign: {'true' if plus else 'false'},\n"
            f"        default: DefaultSpec {{ raw: {rs_def(raw)}, non_raw: {rs_def(non_raw)} }},\n"
            f"        policy: Policy::{policy},\n"
            f"        frame: FrameScope::{frame},\n"
            f"        file_kind: FileScope::{file_scope},\n"
            f"        presence: Presence::{presence},\n"
            f"        min_pv: {min_pv},\n"
            f"        recipe_alias: {'None' if alias is None else f'Some({rs_str(alias)})'},\n"
            f"        note: {rs_str(note_of(r))},\n"
            "    },"
        )
        lvl = r.level.replace("Struct(StructKind::", "struct:").rstrip(")")
        cls = policy.split("(")[0]
        counts.setdefault(lvl, {}).setdefault(cls, 0)
        counts[lvl][cls] += 1
    missing_alias = set(ALIAS) - {r.name for r in all_rows if r.level == "Global"}
    if missing_alias:
        raise SystemExit(f"aliases for unknown keys: {sorted(missing_alias)}")
    head = (
        "//! The key registry table: one [`KeySpec`] per develop key.\n"
        "//!\n"
        "//! Seeded from `server-rs/scripts/develop_registry/spec.py` by\n"
        "//! `gen_table_rs.py` in the same folder, and the source of truth since:\n"
        "//! edit this file by hand, then refresh the review snapshot with\n"
        "//! `LRG_BLESS=1 cargo test -p lrg-develop --test registry_snapshot`.\n"
        "//! Re-running the generator overwrites hand edits.\n"
        "//!\n"
        "//! Order is registry order (the sections of the registry: global keys by\n"
        "//! panel, header, correction, mask tool, nested structures); [`KeyId`]s\n"
        "//! are indices into this slice.\n"
        "//!\n"
        "//! [`KeyId`]: super::KeyId\n\n"
        "use super::{\n"
        "    BoolStyle, CurveKind, Def, DefaultSpec, Finite, FileScope, FrameScope, Gate, KeySpec, Level, Lit,\n"
        "    NumFmt, Policy, Presence, ProcessVersion, StructKind, UiScale, ValueKind,\n"
        "};\n\n"
        f"/// The {len(lines)} registry rows.\n"
        "pub(crate) static TABLE: &[KeySpec] = &[\n"
    )
    return head + "\n".join(lines) + "\n];\n", counts


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Seed registry/table.rs from spec.py.")
    ap.add_argument("--out", required=True, help="path of registry/table.rs")
    ap.add_argument("--counts", action="store_true", help="print row counts per level and policy class")
    args = ap.parse_args(argv)
    text, counts = render(rows())
    out = Path(args.out)
    out.write_text(text, encoding="utf-8")
    rustfmt = shutil.which("rustfmt")
    if rustfmt:
        subprocess.run([rustfmt, "--edition", "2021", str(out)], check=True)
    else:
        print("rustfmt not found; run cargo fmt", file=sys.stderr)
    if args.counts:
        total = 0
        for lvl, c in counts.items():
            print(f"{lvl:28} {dict(sorted(c.items()))}")
            total += sum(c.values())
        print("total rows", total)
    return 0


if __name__ == "__main__":
    sys.exit(main())
