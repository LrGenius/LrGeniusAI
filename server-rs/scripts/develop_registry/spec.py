"""Hand-maintained registry specification: one entry per ``crs`` key or collapsed key group.

``gen.py`` renders these rows as Markdown tables and fills the "Observed" column
from ``inventory.json``. The rows are the maintainer's own notes, based on local
observation only: Lightroom Classic's bundled presets and profiles, the
maintainer's own sidecar corpus and the Lightroom SDK reference. A few rows are
key names known from Camera Raw that were never observed in Lightroom Classic's
output; they are marked "known from Camera Raw; never observed in LrC output",
carry no value semantics and are UNKNOWN or COMPUTED. No row is copied from
ExifTool or darktable; keys known only from ExifTool's tag tables and never
observed were deliberately left out (see the step-1 plan, "Lizenz/Herkunft").

Row tuple::

    (key_or_label, type, ui_range_unit, stored_vs_ui, default_raw_nonraw,
     pv_notes, policy, notes[, members])

``members`` (optional) lists the real key names a collapsed row stands for.
``policy`` is one of LEARN, LEARN\u2020 (gated), PHOTO, COMPUTED, NEVER, META,
UNKNOWN. "research \u00a7N" and "E<n>" refer to the maintainer's research notes and
the develop-settings experiments; the public summary is the wiki page
``Dev-AI-Edit-XMP-Findings.md``.

The table is kept in its compact one-row-per-line layout on purpose, so the
formatter is switched off below.
"""

# fmt: off

HSL = ['Red', 'Orange', 'Yellow', 'Green', 'Aqua', 'Blue', 'Purple', 'Magenta']

G = []  # (section, [rows]) for level=global


def sec(title, rows):
    G.append((title, rows))


sec('White balance', [
    ('WhiteBalance', 'enum', 'As Shot, Auto, Custom, Daylight, Cloudy, Shade, Tungsten, Fluorescent, Flash (non-raw: As Shot, Auto, Custom)', 'string = UI label (English, not localised)', 'As Shot / As Shot', 'all PV', 'PHOTO',
     'Never average. Serializer derives `Custom` whenever it writes Temperature/Tint. `Auto` is resolved per photo by LrC (see AutoWhiteVersion). Raw and non-raw have different preset lists, so a mixed batch must not share one value.'),
    ('Temperature', 'int', '2000..50000 K', 'stored = UI (Kelvin)', 'as shot (camera) / key absent', 'all PV; raw only', 'LEARN†',
     'Raw-only key. Gate: drop from presets that can hit non-raw targets; learn relative to the example\'s as-shot value, not as absolute Kelvin (lighting differs per scene). **Bug (f): code uses `Temp`**.'),
    ('Tint', 'int', '-150..+150', 'stored = UI', 'as shot / key absent', 'all PV; raw only', 'LEARN†',
     'Raw-only, same gate as Temperature. **Bug (f): non-raw must use IncrementalTint, the code writes `Tint` for both.**'),
    ('IncrementalTemperature', 'int', '-100..+100 (relative)', 'stored = UI', 'n/a / 0', 'all PV; non-raw only', 'LEARN†',
     'Non-raw counterpart of Temperature. Not in this raw-only corpus; seen in 5 profile Look.Parameters and (research) in every non-raw catalog row checked.'),
    ('IncrementalTint', 'int', '-100..+100 (relative)', 'stored = UI', 'n/a / 0', 'all PV; non-raw only', 'LEARN†', 'Non-raw counterpart of Tint.'),
    ('AutoWhiteVersion', 'uint', 'engine version', 'major<<24|minor<<16 (134348800 = 8.2)', 'absent', '', 'COMPUTED', 'Written by LrC when WhiteBalance=Auto was resolved.'),
    ('AsShotTemperature, AsShotTint', 'int', 'K / -150..150', '', '', 'seen only inside applied-Preset/Parameters (a few sidecars)', 'UNKNOWN',
     'Not a photo setting in XMP. Meaning inside a preset record unknown (research §8 open point).', ['AsShotTemperature', 'AsShotTint']),
])

sec('Basic tone and presence', [
    ('Exposure2012', 'real', '-5.00..+5.00 EV', 'stored = UI (EV), fixed 2 decimals', '0.00 / 0.00', 'PV2012+ (PV ≥ 2012; legacy `Exposure` for PV2003/2010)', 'LEARN', ''),
    ('Contrast2012', 'int', '-100..+100', 'stored = UI', '0 / 0', 'PV2012+', 'LEARN', ''),
    ('Highlights2012', 'int', '-100..+100', 'stored = UI', '0 / 0', 'PV2012+', 'LEARN', ''),
    ('Shadows2012', 'int', '-100..+100', 'stored = UI', '0 / 0', 'PV2012+', 'LEARN', ''),
    ('Whites2012', 'int', '-100..+100', 'stored = UI', '0 / 0', 'PV2012+', 'LEARN', ''),
    ('Blacks2012', 'int', '-100..+100', 'stored = UI', '0 / 0', 'PV2012+', 'LEARN', ''),
    ('Texture', 'int', '-100..+100', 'stored = UI', '0 / 0', 'LrC 8.3+', 'LEARN', ''),
    ('Clarity2012', 'int', '-100..+100', 'stored = UI', '0 / 0', 'PV2012+', 'LEARN', ''),
    ('Dehaze', 'int', '-100..+100', 'stored = UI', '0 / 0', 'PV2012+', 'LEARN', ''),
    ('Vibrance', 'int', '-100..+100', 'stored = UI', '0 / 0', 'all PV', 'LEARN', ''),
    ('Saturation', 'int', '-100..+100', 'stored = UI', '0 / 0', 'all PV', 'LEARN', ''),
    ('ToneMapStrength', 'real', '?', '?', '?', 'known from Camera Raw; never observed in LrC output', 'UNKNOWN', 'Not in LrC 15.5.1 UI as far as observed; do not write.'),
    ('Exposure, Contrast, Brightness, Shadows, FillLight, HighlightRecovery, Clarity (legacy PV2003/PV2010 tone)', 'real / int', '', '', '', 'PV2003/PV2010; a few bundled presets and every getDevelopSettings() table (as defaults)', 'COMPUTED',
     'Superseded by the *2012 keys. Read for fidelity, never learned or written; a PV2012+ photo ignores them.',
     ['Exposure', 'Contrast', 'Brightness', 'Shadows', 'FillLight', 'HighlightRecovery', 'Clarity']),
])

sec('Tone curve', [
    ('ToneCurveName2012', 'string', 'Linear, Medium Contrast, Strong Contrast, Custom (+ localised names, e.g. "Matter Kontrast")', 'label', 'Linear / Linear', 'PV2012+', 'META',
     'Display label, sometimes localised. Serializer derives: `Linear` for the identity curve, else `Custom`. Never learn from it.'),
    ('ToneCurvePV2012', 'curve', 'points (x,y), ints 0..255, x strictly increasing, ≥ 2 points', 'XMP Seq "x, y"; Lua flat array {x1,y1,x2,y2,...}', '[0,0,255,255]', 'PV2012+', 'LEARN', 'Learn by resampling to a fixed x grid; write back as a small point set.'),
    ('ToneCurvePV2012Red', 'curve', 'as above', 'as above', '[0,0,255,255]', 'PV2012+', 'LEARN', ''),
    ('ToneCurvePV2012Green', 'curve', 'as above', 'as above', '[0,0,255,255]', 'PV2012+', 'LEARN', ''),
    ('ToneCurvePV2012Blue', 'curve', 'as above', 'as above', '[0,0,255,255]', 'PV2012+', 'LEARN', ''),
    ('ParametricShadows', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('ParametricDarks', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('ParametricLights', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('ParametricHighlights', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('ParametricShadowSplit', 'int', '0..100, must stay < MidtoneSplit', 'stored = UI', '25 / 25', '', 'LEARN', 'Keep the ordering Shadow < Midtone < Highlight when averaging.'),
    ('ParametricMidtoneSplit', 'int', '0..100', 'stored = UI', '50 / 50', '', 'LEARN', ''),
    ('ParametricHighlightSplit', 'int', '0..100, must stay > MidtoneSplit', 'stored = UI', '75 / 75', '', 'LEARN', ''),
    ('CurveRefineSaturation', 'int', '0..100', 'stored = UI', '100 / 100', 'written by recent LrC only (about a fifth of the sampled sidecars)', 'LEARN', 'Low priority.'),
    ('ExtendedToneCurvePV2012, ExtendedToneCurvePV2012Red, ExtendedToneCurvePV2012Green, ExtendedToneCurvePV2012Blue', 'curve', '?', 'Lua: flat number array like ToneCurvePV2012', '', 'in a few getDevelopSettings() tables (3 of 1,294 training rows); never observed in XMP', 'UNKNOWN', 'Probably HDR-extended curves. Research: no XMP counterpart found.',
     ['ExtendedToneCurvePV2012', 'ExtendedToneCurvePV2012Red', 'ExtendedToneCurvePV2012Green', 'ExtendedToneCurvePV2012Blue']),
    ('ExtendedToneCurveName2012', 'string', '?', '?', '', 'known from Camera Raw; never observed in LrC output', 'UNKNOWN', 'Name for the extended curves above.'),
    ('ToneCurve, ToneCurveRed, ToneCurveGreen, ToneCurveBlue, ToneCurveName', 'curve / string', '', '', '', 'legacy PV2003/2010 (11 Classic presets)', 'COMPUTED', 'Legacy. Never write; the parser keeps them for round-trip only.', ['ToneCurve', 'ToneCurveRed', 'ToneCurveGreen', 'ToneCurveBlue', 'ToneCurveName']),
])

sec('Colour mixer (HSL) and B&W', [
    ('HueAdjustment{8}', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', '8 keys: ' + ', '.join('HueAdjustment' + c for c in HSL), ['HueAdjustment' + c for c in HSL]),
    ('SaturationAdjustment{8}', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', '8 keys.', ['SaturationAdjustment' + c for c in HSL]),
    ('LuminanceAdjustment{8}', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', '8 keys.', ['LuminanceAdjustment' + c for c in HSL]),
    ('ConvertToGrayscale', 'bool', 'True/False', '"True"/"False"', 'False / False', '', 'LEARN†',
     'Categorical: weighted majority, never a mean. Gate: B&W and colour examples must not be averaged into one recipe (today they are, research §2).'),
    ('GrayMixer{8}', 'int', '-100..+100', 'stored = UI', 'auto-mix computed on conversion / 0 in presets', '', 'LEARN†', 'Only meaningful with ConvertToGrayscale=True. 8 keys.', ['GrayMixer' + c for c in HSL]),
    ('AutoGrayscaleMix', 'bool', 'True', '"True"', 'absent', 'presets only (13)', 'UNKNOWN', 'Preset instruction "compute auto B&W mix on apply". Behaviour through applyDevelopSettings unverified.'),
])

sec('Colour grading (split toning keys are shared)', [
    ('SplitToningShadowHue', 'int', '0..360 deg (Colour Grading "Shadows" hue)', 'stored = UI', '0 / 0', 'LrC 10+ reuses the split-toning keys', 'LEARN', 'Circular mean; weight by saturation.'),
    ('SplitToningShadowSaturation', 'int', '0..100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('SplitToningHighlightHue', 'int', '0..360 deg', 'stored = UI', '0 / 0', '', 'LEARN', 'Circular mean.'),
    ('SplitToningHighlightSaturation', 'int', '0..100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('SplitToningBalance', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('ColorGradeShadowLum', 'int', '-100..+100', 'stored = UI', '0 / 0', 'LrC 10+', 'LEARN', '**Bug (f): plugin warns "not supported by Lightroom" and drops it.**'),
    ('ColorGradeHighlightLum', 'int', '-100..+100', 'stored = UI', '0 / 0', 'LrC 10+', 'LEARN', '**Bug (f): same false warning.**'),
    ('ColorGradeMidtoneHue', 'int', '0..360 deg', 'stored = UI', '0 / 0', 'LrC 10+', 'LEARN', 'Circular mean.'),
    ('ColorGradeMidtoneSat', 'int', '0..100', 'stored = UI', '0 / 0', 'LrC 10+', 'LEARN', ''),
    ('ColorGradeMidtoneLum', 'int', '-100..+100', 'stored = UI', '0 / 0', 'LrC 10+', 'LEARN', ''),
    ('ColorGradeGlobalHue', 'int', '0..360 deg', 'stored = UI', '0 / 0', 'LrC 10+', 'LEARN', 'Circular mean.'),
    ('ColorGradeGlobalSat', 'int', '0..100', 'stored = UI', '0 / 0', 'LrC 10+', 'LEARN', ''),
    ('ColorGradeGlobalLum', 'int', '-100..+100', 'stored = UI', '0 / 0', 'LrC 10+', 'LEARN', 'Missing from the LLM schema today.'),
    ('ColorGradeBlending', 'int', '0..100', 'stored = UI', '50 / 50', 'LrC 10+', 'LEARN', 'Non-zero default.'),
])

sec('Detail', [
    ('Sharpness', 'int', '0..150', 'stored = UI', '40 / 0', '', 'LEARN†', 'Default differs raw/non-raw: learn per file type (or as delta to the type default).'),
    ('SharpenRadius', 'real', '0.5..3.0 px', 'stored = UI, fixed 1 decimal, always "+" ("+1.0")', '+1.0 / +1.0', '', 'LEARN', ''),
    ('SharpenDetail', 'int', '0..100', 'stored = UI', '25 / 25', '', 'LEARN', 'Non-zero default.'),
    ('SharpenEdgeMasking', 'int', '0..100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('LuminanceSmoothing', 'int', '0..100 (UI "Noise Reduction")', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('LuminanceNoiseReductionDetail', 'int', '0..100', 'stored = UI', '50 / 50', 'written only when LuminanceSmoothing was touched', 'LEARN', ''),
    ('LuminanceNoiseReductionContrast', 'int', '0..100', 'stored = UI', '0 / 0', 'as above', 'LEARN', ''),
    ('ColorNoiseReduction', 'int', '0..100', 'stored = UI', '25 / 0', '', 'LEARN†', 'Default differs raw/non-raw.'),
    ('ColorNoiseReductionDetail', 'int', '0..100', 'stored = UI', '50 / 50', '', 'LEARN', ''),
    ('ColorNoiseReductionSmoothness', 'int', '0..100', 'stored = UI', '50 / 50', '', 'LEARN', ''),
    ('ISODependent', 'struct[]', 'Seq of {ISO, ColorNoiseReduction, LuminanceSmoothing}', 'ints', 'absent', 'Adobe ISO-adaptive presets only (8)', 'UNKNOWN', 'Preset-only table. Whether applyDevelopSettings/plugin presets accept it is unverified.'),
])

sec('Lens corrections', [
    ('LensProfileEnable', 'int (0/1)', '0 = off, 1 = on', 'integer, not bool', '0 / 0 (Adobe); 1 = this user\'s default', '', 'LEARN',
     '**Bug (f): plugin writes `EnableLensCorrections` (panel switch) instead.**'),
    ('LensProfileSetup', 'enum', 'LensDefaults, Auto, Custom', 'string', 'LensDefaults', '', 'LEARN†', 'Gate: only `LensDefaults`/`Auto` are transferable; `Custom` pins a specific profile (PHOTO).'),
    ('LensProfileName', 'string', 'profile display name', '', 'filled by LrC', '', 'PHOTO', 'Lens identity. LrC fills it itself for LensDefaults/Auto.'),
    ('LensProfileFilename', 'string', '.lcp file name', '', 'filled by LrC', '', 'PHOTO', 'Lens identity.'),
    ('LensProfileDigest', 'hex32', '', '', 'filled by LrC', '', 'PHOTO', 'Lens identity digest.'),
    ('LensProfileIsEmbedded', 'bool', 'True/False', '"True"/"False"', 'filled by LrC', '', 'PHOTO', 'True for in-camera (embedded) profiles.'),
    ('LensProfileDistortionScale', 'int', '0..200', 'stored = UI', '100 / 100', '', 'LEARN†', 'Only meaningful with LensProfileEnable=1.'),
    ('LensProfileVignettingScale', 'int', '0..200', 'stored = UI', '100 / 100', '', 'LEARN†', 'Only meaningful with LensProfileEnable=1 (0 seen = profile vignetting off).'),
    ('AutoLateralCA', 'int (0/1)', '0/1', 'integer', '0 / 0 (Adobe); 1 = this user\'s default', '', 'LEARN', ''),
    ('LensManualDistortionAmount', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'PHOTO', 'Geometry: disables geometric masks (research §4.3).'),
    ('VignetteAmount', 'int', '-100..+100 (lens vignetting)', 'stored = UI', '0 / 0', '', 'LEARN', 'Low priority.'),
    ('VignetteMidpoint', 'int', '0..100', 'stored = UI', '50 / 50', 'written only when VignetteAmount ≠ 0 (2 sidecars in full corpus)', 'LEARN', ''),
    ('DefringePurpleAmount', 'int', '0..20', 'stored = UI', '0 / 0', '', 'LEARN', 'Low priority, lens-dependent.'),
    ('DefringePurpleHueLo', 'int', '0..100, gap ≥ 10 to Hi', 'stored = UI', '30 / 30', '', 'LEARN', ''),
    ('DefringePurpleHueHi', 'int', '0..100', 'stored = UI', '70 / 70', '', 'LEARN', ''),
    ('DefringeGreenAmount', 'int', '0..20', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('DefringeGreenHueLo', 'int', '0..100', 'stored = UI', '40 / 40', '', 'LEARN', ''),
    ('DefringeGreenHueHi', 'int', '0..100', 'stored = UI', '60 / 60', '', 'LEARN', ''),
    ('Defringe, ChromaticAberrationR, ChromaticAberrationB', 'int', '', '', '', 'legacy; in every getDevelopSettings() table (Lua) and the Zeroed template, never observed in XMP', 'COMPUTED', 'Legacy CA controls. Never write.',
     ['Defringe', 'ChromaticAberrationR', 'ChromaticAberrationB']),
    ('LensProfileChromaticAberrationScale', 'int', '', '', '', 'legacy (Zeroed template only; never observed in XMP or getDevelopSettings())', 'COMPUTED', 'Legacy CA control. Never write.'),
])

sec('Transform / Upright', [
    ('PerspectiveUpright', 'enum int', '0 Off, 1 Auto, 2 Full, 3 Level, 4 Vertical, 5 Guided', 'integer', '0 / 0', '', 'PHOTO', 'Write 1–4 only, never 5 and never the UprightTransform_* matrices; LrC recomputes. Disables geometric masks.'),
    ('PerspectiveVertical', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'PHOTO', 'Geometry.'),
    ('PerspectiveHorizontal', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'PHOTO', 'Geometry.'),
    ('PerspectiveRotate', 'real', '-10.0..+10.0 deg', 'fixed 1 decimal ("0.0", "+0.8")', '0.0 / 0.0', '', 'PHOTO', 'Geometry.'),
    ('PerspectiveScale', 'int', '50..150', 'stored = UI', '100 / 100', '', 'PHOTO', 'Geometry.'),
    ('PerspectiveAspect', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'PHOTO', 'Geometry.'),
    ('PerspectiveX', 'real', '-100..+100 (UI "Offset X")', 'fixed 2 decimals ("0.00")', '0.00 / 0.00', '', 'PHOTO', 'Geometry.'),
    ('PerspectiveY', 'real', '-100..+100', 'fixed 2 decimals', '0.00 / 0.00', '', 'PHOTO', 'Geometry.'),
    ('Upright solver state (17 keys)', 'mixed', '', '', 'absent', 'written when PerspectiveUpright ≠ 0', 'COMPUTED',
     'UprightVersion, UprightTransformCount, UprightTransform_0..5 (9-float matrices), UprightCenterMode, UprightCenterNormX/Y, UprightFocalMode, UprightFocalLength35mm, UprightPreview, UprightDependentDigest, UprightGuidedDependentDigest, UprightFourSegmentsCount.',
     ['UprightVersion', 'UprightTransformCount', 'UprightTransform_0', 'UprightTransform_1', 'UprightTransform_2', 'UprightTransform_3', 'UprightTransform_4', 'UprightTransform_5', 'UprightCenterMode', 'UprightCenterNormX', 'UprightCenterNormY', 'UprightFocalMode', 'UprightFocalLength35mm', 'UprightPreview', 'UprightDependentDigest', 'UprightGuidedDependentDigest', 'UprightFourSegmentsCount']),
    ('UprightFourSegments_0..3', 'string', '4 floats per guide', '', 'absent', 'Guided Upright only', 'NEVER', 'User-drawn guide lines (per-photo strokes).', ['UprightFourSegments_0', 'UprightFourSegments_1', 'UprightFourSegments_2', 'UprightFourSegments_3']),
])

sec('Crop', [
    ('CropTop', 'real', '0..1', 'normalised, uncropped sensor frame; up to 6 decimals trimmed', '0 / 0', '', 'PHOTO', 'Crop corners are opposite corners of a rectangle rotated by CropAngle (research §4).'),
    ('CropLeft', 'real', '0..1', 'as above', '0 / 0', '', 'PHOTO', ''),
    ('CropBottom', 'real', '0..1', 'as above', '1 / 1', '', 'PHOTO', ''),
    ('CropRight', 'real', '0..1', 'as above', '1 / 1', '', 'PHOTO', ''),
    ('CropAngle', 'real', '-45..+45 deg', 'no "+" sign, up to 6 decimals', '0 / 0', '', 'PHOTO', ''),
    ('CropConstrainToWarp', 'int (0/1)', '0/1', 'integer', '0 / 0', '', 'PHOTO', ''),
    ('CropConstrainToUnitSquare', 'int', '1', 'integer', '1 / 1', '', 'PHOTO', 'Always 1 when written.'),
    ('HasCrop', 'bool', 'True', '"True"', 'absent', 'XMP only (not in the Lua table)', 'META', 'Derived: written iff the crop differs from the full frame.'),
    ('CropConstrainAspectRatio', 'bool', 'true/false (Lua)', 'Lua boolean', 'absent', 'Lua only (getDevelopSettings(), 66 of 1,294 training rows); never observed in XMP', 'PHOTO', 'Aspect-ratio lock of the crop tool: per-photo UI state.'),
    ('CropWidth, CropHeight, CropUnit, CropUnits', 'real / enum', 'output size; units unknown', '', '', 'known from Camera Raw; never observed in LrC output', 'UNKNOWN', 'ACR crop-to-size settings; not an LrC feature as far as observed.', ['CropWidth', 'CropHeight', 'CropUnit', 'CropUnits']),
])

sec('Effects', [
    ('PostCropVignetteAmount', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('PostCropVignetteMidpoint', 'int', '0..100', 'stored = UI', '50 / 50', '', 'LEARN', ''),
    ('PostCropVignetteFeather', 'int', '0..100', 'stored = UI', '50 / 50', '', 'LEARN', ''),
    ('PostCropVignetteRoundness', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('PostCropVignetteStyle', 'enum int', '1 Highlight Priority, 2 Color Priority, 3 Paint Overlay', 'integer', '1 / 1', '', 'LEARN', 'Categorical (majority).'),
    ('PostCropVignetteHighlightContrast', 'int', '0..100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('OverrideLookVignette', 'bool', 'True/False', '"True"/"False"', 'False / False', '', 'UNKNOWN', 'Set by LrC when the user vignette overrides a vignette built into the Look; whether we must set it when writing PostCropVignette* is unverified.'),
    ('GrainAmount', 'int', '0..100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('GrainSize', 'int', '0..100', 'stored = UI', '25 / 25', '', 'LEARN', 'Zeroed template uses 50; the Adobe default and the corpus mode are 25.'),
    ('GrainFrequency', 'int', '0..100 (UI "Roughness")', 'stored = UI', '50 / 50', '', 'LEARN', ''),
    ('GrainSeed', 'uint32', 'random seed', '', 'absent', 'recent LrC', 'COMPUTED', 'Grain pattern seed chosen by LrC.'),
    ('GlowRange, GlowWarmth, GlowSpread, GlowStyle, ReshapeAmount', 'number', '?', '?', '', 'known from Camera Raw 18.6; never observed in LrC output', 'UNKNOWN', 'Newer engine features not seen in LrC 15.5.1 output.', ['GlowRange', 'GlowWarmth', 'GlowSpread', 'GlowStyle', 'ReshapeAmount']),
])

sec('Lens blur', [
    ('LensBlur', 'struct', 'see sub-table 7.3', '', 'Active=false / same', 'LrC 13+ (Adobe ships lens-blur-only presets)', 'LEARN\u2020',
     'Gate: needs a depth map, i.e. an AI update after applying; FocalRange semantics are UNKNOWN, so v1 writes only Active + the look sliders.'),
])

sec('Calibration', [
    ('ShadowTint', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('RedHue', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('RedSaturation', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('GreenHue', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('GreenSaturation', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('BlueHue', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', ''),
    ('BlueSaturation', 'int', '-100..+100', 'stored = UI', '0 / 0', '', 'LEARN', '`BluePrimarySaturation` does not exist (silently dropped by LrC, research §7).'),
])

sec('Profile', [
    ('CameraProfile', 'string', 'Adobe Standard, Adobe Standard v2, symbolic Default Color / Default Monochrome / Default Profile, camera-specific DCP names (Camera Standard, Camera PROVIA/Standard, ...), Embedded (non-raw)', 'string', 'Adobe Standard / Embedded', '', 'LEARN†',
     'Gate: transfer only Adobe Standard and the symbolic Default* values; camera-specific names only to the same make. The creative profile is the `Look`. **Bug (f): dead `profile` branch writes `CameraProfile="Adobe Color"`, which is a Look name and is silently dropped.**'),
    ('CameraProfileDigest', 'hex32', '', '', 'filled by LrC', '', 'COMPUTED', ''),
    ('Look', 'struct', 'see sub-table', '', 'raw: Adobe Color (UUID B952C231111CD8E0ECCF14B86BAA7077) / none', 'LrC 7.3+', 'LEARN†', 'Identity = Look.Name + Look.UUID, strength = Look.Amount. Gate: camera-restricted Looks only to the same camera; Adaptive looks need an AI update (AILook).'),
    ('LookTable, RGBTable', 'hex32', 'reference to a Table_<md5> blob', '', '', '', 'COMPUTED', 'Profile lookup-table references.', ['LookTable', 'RGBTable']),
    ('Table_<md5>', 'blob', 'base85 + zlib', '', '', 'one attribute per referenced table, name carries the MD5', 'COMPUTED', 'Profile LUTs, legacy inline mask bitmaps (crs:Version 16), Denoise/Enhance payloads. Decodable (research §11) but never written by us.'),
    ('LookTableAmount, RGBTableAmount, RGBTables, ProfileGainTableMap, ProfileGainTableMap2, ProfileToneCurve, MissingCameraProfile, MissingCameraProfileDigest', 'mixed', '', '', '', 'profile-definition fields (Look.Parameters) / known from Camera Raw', 'COMPUTED', 'Part of a profile definition, not a photo setting.',
     ['LookTableAmount', 'RGBTableAmount', 'RGBTables', 'ProfileGainTableMap', 'ProfileGainTableMap2', 'ProfileToneCurve', 'MissingCameraProfile', 'MissingCameraProfileDigest']),
    ('AILook', 'struct', 'see sub-table', '', 'absent', 'Adaptive Color profile', 'COMPUTED', 'Computed state of an adaptive profile.'),
])

sec('Point colour', [
    ('PointColors', 'string[] (XMP) / struct[] (Lua)', 'XMP: Seq of swatches, 19 floats each "%.6f, ..."; Lua: one table per swatch (see PointColors items)', 'sentinel swatch of 19 × -1.000000 = none', 'one sentinel swatch', 'LrC 13+', 'UNKNOWN',
     'Layout only partly decoded; values are sampled from one photo. Round-trip only.'),
    ('ColorVariance', 'string[]', 'Seq of floats "%.6f"', '', '[-50.000000]', 'LrC 13+', 'UNKNOWN', 'Meaning of -50 unknown (research §8).'),
])

sec('HDR', [
    ('HDREditMode', 'int (0/1)', '0/1', 'integer', '0 / 0', 'LrC 12+', 'PHOTO', 'Never learn: the 8-bit JPEG export cannot show HDR. Keep the photo\'s value.'),
    ('HDRMaxValue', 'real', 'EV headroom', 'fixed 2 decimals, always "+" ("+4.00")', '+4.00 / +4.00', 'LrC 12+', 'PHOTO', ''),
    ('SDRBrightness, SDRContrast, SDRClarity, SDRHighlights, SDRShadows, SDRWhites, SDRBlend', 'int', '-100..+100 (SDR rendition sliders)', 'stored = UI', '0 / 0', 'LrC 12+, only with HDREditMode=1', 'PHOTO', '', ['SDRBrightness', 'SDRContrast', 'SDRClarity', 'SDRHighlights', 'SDRShadows', 'SDRWhites', 'SDRBlend']),
])

sec('Auto and style state', [
    ('AutoTone', 'bool', 'True', '"True"', 'absent', 'presets (23) + 1 sidecar', 'UNKNOWN', 'Preset instruction "run Auto Tone on apply"; collides with explicit tone values; applyDevelopSettings has its own `flattenAutoNow` flag. Not for v1.'),
    ('AutoToneDigest, AutoToneDigestNoSat, AutoToneDigestPV2', 'hex32', '', '', '', '', 'COMPUTED', 'Cache keys of the last auto-tone evaluation.', ['AutoToneDigest', 'AutoToneDigestNoSat', 'AutoToneDigestPV2']),
    ('SourceAuto* (8 keys)', 'number', '', '', '', 'known from Camera Raw; never observed in LrC output', 'COMPUTED', 'SourceAutoExposure2012 ... SourceAutoVibrance.',
     ['SourceAutoExposure2012', 'SourceAutoContrast2012', 'SourceAutoHighlights2012', 'SourceAutoShadows2012', 'SourceAutoWhites2012', 'SourceAutoBlacks2012', 'SourceAutoSaturation', 'SourceAutoVibrance']),
    ('AutoExposure, AutoShadows, AutoBrightness, AutoContrast', 'bool', '', '', 'false (Zeroed)', 'legacy PV2003/2010', 'COMPUTED', 'Legacy auto flags.', ['AutoExposure', 'AutoShadows', 'AutoBrightness', 'AutoContrast']),
    ('ToggleStyleDigest', 'hex32', '', '', 'absent', 'recent LrC (a few sampled sidecars)', 'UNKNOWN', 'Probably identifies an applied "style"/preset for toggling; meaning unverified.'),
    ('ToggleStyleAmount', 'real', '0..1?', '', 'absent', 'as above', 'UNKNOWN', 'Observed 0.67..1.'),
])

sec('Retouch, remove, red eye, AI filters', [
    ('RetouchAreas', 'struct[]', 'see sub-table', '', 'absent', '', 'NEVER', 'Heal/clone/content-aware spots.'),
    ('RemoveAreas', 'struct[]', 'see sub-table', '', 'absent', 'LrC 13+', 'NEVER', 'Remove tool / generative remove.'),
    ('RetouchInfo', 'string[] (XMP) / struct[] (Lua)', 'XMP: "centerX = ..., centerY = ..." items; Lua: one table per spot (see RetouchInfo items)', '', 'absent', 'legacy spot removal', 'NEVER', ''),
    ('RedEyeInfo', 'string[]', '', '', 'empty', 'legacy', 'NEVER', 'Present (empty) in the Zeroed Lua template.'),
    ('FilterList', 'struct', 'see sub-table', '', 'absent', 'LrC 12.3+ (Denoise, Raw Details, Super Resolution)', 'COMPUTED', 'Enhance/AI filter payloads; FilterIDs 3/4/6 are remove/heal/distraction removal (NEVER).'),
    ('AllowFilters', 'int', '1', '', 'absent', 'with FilterList', 'COMPUTED', ''),
    ('DepthMapInfo', 'struct', 'see sub-table', '', 'absent', 'lens blur / depth', 'COMPUTED', ''),
    ('RangeMaskMapInfo', 'struct', 'see sub-table', '', 'absent', 'range masks', 'COMPUTED', 'Per-photo image statistics for range masks.'),
    ('DepthBasedCorrections', 'struct[]', 'Correction struct + LocalCorrectedDepth', '', 'absent', 'known from Camera Raw; never observed in LrC output', 'UNKNOWN', 'Old depth-based local corrections.'),
])

sec('Masks (container) and legacy local containers', [
    ('MaskGroupBasedCorrections', 'struct[]', 'Seq of Correction structs (one per mask in the Masks panel), order = panel order', '', 'absent', 'crs:Version ≥ 14 (LrC 11+)', 'LEARN†',
     'Per-item policy follows the mask components (sub-tables below): semantic AI components are LEARN, geometry PHOTO, brush NEVER.'),
    ('GradientBasedCorrections, CircularGradientBasedCorrections, PaintBasedCorrections', 'struct[]', 'pre-LrC 11 containers', '', 'absent', 'legacy; 0 in corpus', 'NEVER', 'Never write (no AI masks, no add/subtract). A parser may migrate them read-only.',
     ['GradientBasedCorrections', 'CircularGradientBasedCorrections', 'PaintBasedCorrections']),
])

sec('Versioning and bookkeeping', [
    ('ProcessVersion', 'string', '"5.0" PV2003, "5.7" PV2010, "6.7" PV2012, "10.0" PV4, "11.0" PV5, "15.4" PV6', 'one decimal', 'current: 15.4', '', 'META', 'Write only when switching PV on purpose; omitting it keeps the photo\'s PV. Learning must refuse to mix PV2003/2010 examples.'),
    ('Version', 'string', 'writer engine version "18.5", "18.5.1"', '', '', '', 'META', 'Must be ≤ the engine of the target LrC (LrC 15.5.1 = 18.5.1).'),
    ('CompatibleVersion', 'uint', 'minimum reader version', 'major<<24 | minor<<16 (285212672 = 17.0)', 'absent', '', 'META', 'Feature → version mapping unknown; omit unless copying Adobe\'s value for the same feature.'),
    ('HasSettings', 'bool', 'True', '"True"', 'True', 'XMP only', 'META', 'Always True.'),
    ('AlreadyApplied', 'bool', 'False', '"False"', 'False', 'sidecars only', 'META', ''),
    ('RawFileName', 'string', 'file name of the raw', '', '', 'sidecars only', 'META', 'Photo identity; set from the target file.'),
    ('RenderVersion', 'uint', '', '', '', 'known from Camera Raw; never observed in LrC output', 'COMPUTED', ''),
    ('Preset', 'struct', 'record of the last applied preset (see 7.10)', '', 'absent', 'sidecars only (about a tenth of them)', 'COMPUTED', 'Written by LrC; its Parameters subtree is a preset, not the photo\'s settings.'),
])

sec('Lua-table-only keys (getDevelopSettings / applyDevelopSettings, never in XMP)', [
    ('Enable* panel switches (14)', 'bool', 'true/false (Lua)', 'Lua boolean', 'true', 'in Zeroed.lrtemplate and the SDK table; 0 occurrences in the sidecar corpus or the bundled preset/profile files', 'META',
     'EnableCalibration, EnableColorAdjustments, EnableDetail, EnableEffects, EnableGrayscaleMix, EnableLensCorrections, EnableRedEye, EnableRetouch, EnableSplitToning, EnableToneCurve, EnableTransform, EnableMaskGroupBasedCorrections, EnableGradientBasedCorrections, EnableDistractionRemoval. Never write `false`; write `true` only for panels we set (panel switches, not features).',
     ['EnableCalibration', 'EnableColorAdjustments', 'EnableDetail', 'EnableEffects', 'EnableGrayscaleMix', 'EnableLensCorrections', 'EnableRedEye', 'EnableRetouch', 'EnableSplitToning', 'EnableToneCurve', 'EnableTransform', 'EnableMaskGroupBasedCorrections', 'EnableGradientBasedCorrections', 'EnableDistractionRemoval']),
    ('orientation', 'enum string', 'AB, BC, CD, DA, CB, ... (catalog codes)', '', '', 'catalog / Lua only', 'PHOTO', 'Read-only input for the coordinate transform (E13 decides the runtime format). Never write.'),
    ('Temp', 'n/a', '', '', '', 'not a crs key (in no catalog row and no XMP file checked)', 'NEVER', '**Bug (f)**: written/trained today. E1 decides whether LrC rejects or ignores it. Map to Temperature (raw) / IncrementalTemperature (non-raw).'),
])

# Preset header (META)
HEADER = [
    ('PresetType', 'enum', 'Normal (preset), Look (profile)', '', '', '', 'META', ''),
    ('UUID', 'hex32', '32 uppercase hex, no dashes', '', '', '', 'META', 'Stable per generated preset; LrDevelopPreset:getUuid() differs from it (research §5).'),
    ('Cluster', 'string', 'e.g. "Adobe", "Classic", "Premium 1.50", ""', '', '', '', 'META', ''),
    ('Name', 'lang-alt', 'x-default only', '', '', '', 'META', ''),
    ('ShortName', 'lang-alt', '', '', '', '', 'META', 'Empty Alt allowed.'),
    ('SortName', 'lang-alt', '', '', '', '', 'META', ''),
    ('Group', 'lang-alt', 'preset group shown in the panel', '', '', '', 'META', 'Use "LrGeniusAI".'),
    ('Description', 'lang-alt', '', '', '', '', 'META', ''),
    ('Copyright', 'string', '', '', '', '', 'META', ''),
    ('ContactInfo', 'string', '', '', '', '', 'META', ''),
    ('CameraModelRestriction', 'string', '"" or camera model', '', '""', '', 'META', 'Set to the model when a camera-restricted Look is included.'),
    ('SupportsAmount', 'bool', 'True/False', '"True"/"False"', '', '', 'META', 'Enables the amount slider (0..200 %).'),
    ('SupportsAmount2', 'bool', 'True/False', '', '', 'newer presets (330)', 'META', 'Meaning vs SupportsAmount unverified (E4 compares).'),
    ('SupportsColor', 'bool', 'True', '', '', '', 'META', ''),
    ('SupportsMonochrome', 'bool', 'True/False', '', '', '', 'META', 'False when the preset forces colour/B&W semantics.'),
    ('SupportsHighDynamicRange', 'bool', 'True', '', '', '', 'META', ''),
    ('SupportsNormalDynamicRange', 'bool', 'True', '', '', '', 'META', ''),
    ('SupportsSceneReferred', 'bool', 'True', '', '', '', 'META', ''),
    ('SupportsOutputReferred', 'bool', 'True/False', '', '', '', 'META', ''),
    ('RequiresRGBTables', 'bool', 'False', '', '', 'newer presets', 'META', ''),
    ('ShowInPresets', 'bool', 'True', '', '', 'few presets', 'META', ''),
    ('ShowInQuickActions', 'bool', 'False', '', '', 'few presets', 'META', ''),
    ('Baseline', 'enum', 'Adobe Default, Camera Settings', '', 'absent', 'presets (20) and applied-Preset records', 'UNKNOWN', 'Probably the reference state for the amount slider (amount 0 % = baseline). Unverified.'),
]

# Correction level (one MaskGroupBasedCorrections item)
CORR = [
    ('What', 'const', '"Correction"', '', '', '', 'META', ''),
    ('CorrectionAmount', 'real', '0..200 % (mask "Amount")', 'UI/100 → 0..2', '1', '', 'LEARN', 'Research: observed 0.21–2.'),
    ('CorrectionActive', 'bool', 'true/false', '"true"/"false"', 'true', '', 'META', 'Always true when we write.'),
    ('CorrectionName', 'string', 'free text, localised by LrC', '', '', '', 'META', 'We write "LrGenius · <role>". Never infer semantics from names.'),
    ('CorrectionSyncID', 'hex32', '', '', '', '', 'META', 'Stable, role-derived id; whether an equal id replaces or duplicates is E5.'),
    ('CorrectionMasks', 'struct[]', 'Seq of mask components, evaluation order', '', '', '', 'LEARN†', 'Policy per component (mask-tool table).'),
    ('LocalExposure2012', 'real', '-4..+4 EV', '**EV/4** → -1..+1', '0', 'PV2012+', 'LEARN', 'Dodge 0.0625 = +0.25 EV (Adobe local presets).'),
    ('LocalContrast2012', 'real', '-100..+100', 'UI/100', '0', 'PV2012+', 'LEARN', ''),
    ('LocalHighlights2012', 'real', '-100..+100', 'UI/100', '0', 'PV2012+', 'LEARN', ''),
    ('LocalShadows2012', 'real', '-100..+100', 'UI/100', '0', 'PV2012+', 'LEARN', ''),
    ('LocalWhites2012', 'real', '-100..+100', 'UI/100', '0', 'PV2012+', 'LEARN', ''),
    ('LocalBlacks2012', 'real', '-100..+100', 'UI/100', '0', 'PV2012+', 'LEARN', ''),
    ('LocalClarity2012', 'real', '-100..+100', 'UI/100', '0', 'PV2012+', 'LEARN', ''),
    ('LocalDehaze', 'real', '-100..+100', 'UI/100', '0', 'no "2012" suffix', 'LEARN', ''),
    ('LocalTexture', 'real', '-100..+100', 'UI/100', '0', 'no suffix', 'LEARN', ''),
    ('LocalSaturation', 'real', '-100..+100', 'UI/100', '0', 'no suffix', 'LEARN', ''),
    ('LocalSharpness', 'real', '-100..+100', 'UI/100', '0', '', 'LEARN', ''),
    ('LocalTemperature', 'real', '-100..+100 (relative)', 'UI/100', '0', 'no suffix', 'LEARN', 'The LLM schema range is right in UI units; the serializer must divide by 100.'),
    ('LocalTint', 'real', '-100..+100 (relative)', 'UI/100', '0', 'no suffix', 'LEARN', ''),
    ('LocalLuminanceNoise', 'real', '-100..+100 (UI "Noise")', 'UI/100', '0', '', 'LEARN', 'UI range from knowledge; only 0..1 observed. The plugin\'s `local_Noise` must be `local_LuminanceNoise`.'),
    ('LocalMoire', 'real', '-100..+100', 'UI/100', '0', '', 'LEARN', ''),
    ('LocalDefringe', 'real', '-100..+100', 'UI/100', '0', '', 'LEARN', ''),
    ('LocalHue', 'real', '-180..+180? deg', 'UI/180? (one third-party measurement)', '0', '', 'UNKNOWN', 'Scale unverified; 2 non-zero values in the sample.'),
    ('LocalToningHue', 'real', '0..360 deg (mask "Color" swatch)', 'degrees, raw', '0 (Adobe local presets use 240 with saturation 0)', '', 'LEARN', 'Circular; ignore when LocalToningSaturation = 0.'),
    ('LocalToningSaturation', 'real', '0..100', 'UI/100', '0', '', 'LEARN', ''),
    ('LocalCurveRefineSaturation', 'real', '0..100', 'stored = UI', '100', 'LrC 13+', 'LEARN', 'Only 100 and 30 observed.'),
    ('LocalGrain', 'real', '0..100?', 'UI/100?', '0', 'LrC 13+', 'UNKNOWN', 'Only 0 and 1 observed; scale unverified.'),
    ('MainCurve, RedCurve, GreenCurve, BlueCurve', 'curve', 'points 0..255', 'XMP Seq "x,y" (**no space**); Lua list of "x,y" strings (research)', 'absent', 'LrC 13+', 'LEARN', 'Low priority.', ['MainCurve', 'RedCurve', 'GreenCurve', 'BlueCurve']),
    ('LocalPointColors', 'string[]', 'as PointColors', '', 'absent', 'LrC 13+', 'UNKNOWN', 'As global PointColors.'),
    ('LocalColorVariance', 'string[]', 'as ColorVariance', '', 'absent', 'LrC 13+', 'UNKNOWN', ''),
    ('LocalExposure, LocalContrast, LocalClarity, LocalBrightness', 'real', '', '', '0', 'legacy PV2010 local keys', 'COMPUTED', 'Adobe presets write them as 0; the serializer may emit 0 for fidelity, never other values. (LocalClarity 0.655 appears only in legacy local presets.)',
     ['LocalExposure', 'LocalContrast', 'LocalClarity', 'LocalBrightness']),
    ('LocalCorrectedDepth', 'real', '', '', '0', 'depth corrections', 'COMPUTED', 'Always 0.'),
    ('LocalColorGrade* (14), LocalToningLuminance, LocalGlow, CorrectionColorTable, CorrectionLUTName, CorrectionLUTAmount', 'mixed', '', '', '', 'known from Camera Raw 18.6; never observed in LrC output', 'UNKNOWN', 'Do not use (research §3.4).',
     ['LocalColorGradeShadowHue', 'LocalColorGradeShadowSat', 'LocalColorGradeShadowLum', 'LocalColorGradeHighlightHue', 'LocalColorGradeHighlightSat', 'LocalColorGradeHighlightLum', 'LocalColorGradeMidtoneHue', 'LocalColorGradeMidtoneSat', 'LocalColorGradeMidtoneLum', 'LocalColorGradeGlobalHue', 'LocalColorGradeGlobalSat', 'LocalColorGradeGlobalLum', 'LocalColorGradeBalance', 'LocalColorGradeBlending', 'LocalToningLuminance', 'LocalGlow', 'CorrectionColorTable', 'CorrectionLUTName', 'CorrectionLUTAmount']),
    ('CorrectionID, CorrectionReferenceX, CorrectionReferenceY', 'runtime', '', '', '', 'Lua/catalog only', 'COMPUTED', 'Runtime ids; never write.', ['CorrectionID', 'CorrectionReferenceX', 'CorrectionReferenceY']),
]

# Mask-tool level: (What, [rows])
MASK = [
    ('All component types', [
        ('What', 'enum', 'Mask/Image, Mask/Gradient, Mask/CircularGradient, Mask/RangeMask, Mask/Aggregate, Mask/Paint (inside Aggregate/Gesture), Mask/Polygon (Gesture), Mask/Ellipse (retouch); Camera Raw also knows Mask/FlattenedGroup, Mask/Clip, Mask/Combine', '', '', '', 'LEARN†', 'Policy follows the type: Image = LEARN, Gradient/CircularGradient/Polygon = PHOTO, Paint/Aggregate = NEVER.'),
        ('MaskActive', 'bool', 'true/false', '', 'true', '', 'META', ''),
        ('MaskName', 'string', 'localised, version-dependent ("Radialverlauf" vs "Radialer Verlauf")', '', '', '', 'META', 'Never key on it.'),
        ('MaskBlendMode', 'enum int', '0 add, 1 subtract/intersect', '', '0', '', 'LEARN', 'Intersect = 1 + MaskValue 0 + MaskInverted true (105 corpus cases).'),
        ('MaskInverted', 'bool', 'true/false', '', 'false', '', 'LEARN', ''),
        ('MaskSyncID', 'hex32', '', '', '', '', 'META', ''),
        ('MaskValue', 'real', '0..1', '', '1', '', 'LEARN', '0 iff MaskBlendMode = 1 (235/235).'),
        ('MaskID', 'runtime', '', '', '', 'Lua/catalog only', 'COMPUTED', 'Runtime id; never write.'),
    ]),
    ('Mask/Image (AI, semantic)', [
        ('MaskVersion', 'int', '1', '', '1', '', 'META', ''),
        ('MaskSubType', 'enum int', '0 category/object/person part, 1 Subject, 2 Sky, 3 people part (preset form)', 'XMP: integer string; Lua: number (every mask in the training dump)', '', 'LrC 11+', 'LEARN', 'Subtype 3 = "all people" is unconfirmed; 0 + part id = one specific person.'),
        ('MaskSubCategoryID', 'enum int', '22 Background; 2–9, 11, 12 people parts; 20036 whole person; 50001–50008 landscape classes', '', 'absent for subtype 1/2', 'landscape classes LrC 13+', 'LEARN', 'Full tables in Dev-AI-Edit-XMP-Findings.md. 20036 generic form unknown.'),
        ('ReferencePoint', 'string', '"x y", "%.6f %.6f", sensor frame', '', '"0.500000 0.500000" in preset form', '', 'PHOTO', 'Write the constant 0.5 0.5 for preset-form semantic masks (META); a real point selects a person/object (PHOTO).'),
        ('ErrorReason', 'enum int', '0 ok / not computed, 1 computed but nothing found', '', '0 in preset form', '', 'COMPUTED', 'Adobe presets write 0; read back after an AI update and surface 1 as a warning.'),
        ('Gesture', 'struct[]', 'hint geometry: Mask/Polygon (4 points) or Mask/Paint strokes', '', 'absent', '"Select Object"', 'PHOTO', 'Frame-dependent (research §4.2). Polygon form only; never paint strokes.'),
        ('MaskDigest, InputDigest, InputDigestVersion, LocalInputDigest, LocalInputDigestVersion, ModelVersion, WholeImageArea, Origin, FullMaskSize, DidOverrideInputDigestMismatch', 'mixed', '', '', 'absent in preset form', '', 'COMPUTED',
         'Computed-mask bookkeeping (bitmap lives in .acr / catalog). FullMaskSize ~ "2880,1920"; WholeImageArea rationals.',
         ['MaskDigest', 'InputDigest', 'InputDigestVersion', 'LocalInputDigest', 'LocalInputDigestVersion', 'ModelVersion', 'WholeImageArea', 'Origin', 'FullMaskSize', 'DidOverrideInputDigestMismatch']),
        ('TargetImageArea, EdgeShift', 'mixed', '', '', '', 'known from Camera Raw; never observed in LrC output', 'COMPUTED', '', ['TargetImageArea', 'EdgeShift']),
        ('SemanticObjectPrompt, ExtendedSemanticName, ExtendedInstanceID, InstanceIDs, InstanceID, InstanceBounds', 'mixed', '', '', '', 'known from Camera Raw 18.6; never observed in LrC output', 'UNKNOWN',
         'SemanticObjectPrompt suggests text-prompted object masks; relevant for step 4 (dodge & burn tier 2).',
         ['SemanticObjectPrompt', 'ExtendedSemanticName', 'ExtendedInstanceID', 'InstanceIDs', 'InstanceID', 'InstanceBounds']),
    ]),
    ('Mask/Gradient (linear)', [
        ('ZeroX', 'real', 'normalised sensor frame, may leave [0,1]', 'up to 6 decimals', '', '', 'PHOTO', '0 % point.'),
        ('ZeroY', 'real', 'as above', '', '', '', 'PHOTO', ''),
        ('FullX', 'real', 'as above', '', '', '', 'PHOTO', '100 % point.'),
        ('FullY', 'real', 'as above', '', '', '', 'PHOTO', 'Direction/orthogonality in pixel space is derived, not measured.'),
        ('Bidirectional, Zero2X, Zero2Y, ZeroFeather, Zero2Feather, FullPointDistance', 'mixed', '', '', '', 'known from Camera Raw; never observed in LrC output', 'UNKNOWN', 'Newer gradient variants.',
         ['Bidirectional', 'Zero2X', 'Zero2Y', 'ZeroFeather', 'Zero2Feather', 'FullPointDistance']),
    ]),
    ('Mask/CircularGradient (radial)', [
        ('Top', 'real', 'normalised sensor frame, may leave [0,1]', '', '', '', 'PHOTO', ''),
        ('Left', 'real', 'as above', '', '', '', 'PHOTO', ''),
        ('Bottom', 'real', 'as above', '', '', '', 'PHOTO', ''),
        ('Right', 'real', 'as above', '', '', '', 'PHOTO', ''),
        ('Angle', 'real', '-45..+45 deg observed', '', '0', '', 'PHOTO', 'Convention (sign, rotated corners) uncalibrated: write 0 only.'),
        ('Midpoint', 'int', '50', '', '50', '', 'PHOTO', 'Always 50.'),
        ('Roundness', 'int', '0 observed', '', '0', '', 'PHOTO', ''),
        ('Feather', 'int', '0..100', 'stored = UI', '50', '', 'PHOTO', ''),
        ('Flipped', 'bool', 'true/false', '', '', '', 'PHOTO', 'Always = !MaskInverted (276 + 186 corpus radials).'),
        ('Version', 'int', '2', '', '2', '', 'META', ''),
    ]),
    ('Mask/RangeMask', [
        ('CorrectionRangeMask', 'struct', 'see sub-table', '', '', '', 'LEARN†', 'Luminance (Type 2) is LEARN, colour (Type 1) PHOTO.'),
    ]),
    ('Mask/Aggregate and Mask/Paint (brush)', [
        ('Masks', 'struct[]', 'Seq of Mask/Paint strokes', '', '', '', 'NEVER', ''),
        ('Dabs', 'string[]', 'tokens "M x y" (stroke start), "d x y" (dab), "r R", "f F", "h H"', '', '', '', 'NEVER', 'Up to 218 KB per sidecar.'),
        ('Radius', 'real', 'normalised by sensor width', '', '', '', 'NEVER', ''),
        ('Flow', 'real', '0..1', '', '', '', 'NEVER', ''),
        ('CenterWeight', 'real', '0', '', '', '', 'NEVER', ''),
        ('MaskBrushTable, MaskBrushUncompressedBytes', 'hex32 / int', '', '', '', 'strokes moved to .acr/catalog', 'COMPUTED', '', ['MaskBrushTable', 'MaskBrushUncompressedBytes']),
    ]),
]

# Nested structs
NESTED = [
    ('nested:Look', 'Look (profile reference)', [
        ('Name', 'string', 'profile name (plain attribute, not Alt)', '', '', '', 'LEARN', ''),
        ('UUID', 'hex32', '', '', '', '', 'LEARN', 'Must match an installed profile.'),
        ('Amount', 'real', '0..200 %', 'UI/100 → 0..2 ("1", "1.19", "1.000000")', '1', '', 'LEARN', ''),
        ('Stubbed', 'bool', 'true', '', 'presets only', '', 'META', 'Presets carry a stub (no Parameters); whether applyDevelopSettings accepts a stub is E11.'),
        ('Parameters', 'struct', 'the profile definition (see Look.Parameters)', '', '', 'sidecars only', 'COMPUTED', 'LrC fills it from its profile library.'),
        ('Group', 'lang-alt', '', '', '', '', 'COMPUTED', 'Copied from the profile.'),
        ('SortName', 'lang-alt', '', '', '', '', 'COMPUTED', ''),
        ('Cluster, Copyright', 'string', '', '', '', '', 'COMPUTED', '', ['Cluster', 'Copyright']),
        ('CameraModelRestriction', 'string', 'e.g. "Nikon Z 9" (bundled camera-matching profiles)', '', '', '', 'COMPUTED', 'Also the learning gate: never transfer such a Look to another camera.'),
        ('isAdobeAdaptive', 'bool', 'true', 'Lua boolean', '', 'Lua only (getDevelopSettings() of adaptive profiles); never observed in XMP', 'COMPUTED', 'Marks an Adobe Adaptive profile (needs an AI update, see AILook).'),
        ('SupportsAmount, SupportsMonochrome, SupportsOutputReferred', 'bool', '', '', '', '', 'COMPUTED', '', ['SupportsAmount', 'SupportsMonochrome', 'SupportsOutputReferred']),
    ]),
    ('look-parameters', 'Look.Parameters (profile definition; also the top level of a profile .xmp)', [
        ('all keys (profile definition)', 'mixed', '', '', '', '', 'COMPUTED',
         'Observed: CameraProfile, ConvertToGrayscale, LookTable, RGBTable, RGBTables, RGBTableAmount, ProfileGainTableMap, ProfileToneCurve, Table_<md5>, ToneCurvePV2012*, ProcessVersion, Version, CompatibleVersion, HasSettings, Clarity2012, Highlights2012, Shadows2012, Contrast2012, Exposure2012, Whites2012, Blacks2012, Saturation, Vibrance, HSL, Parametric*, SplitToning*, ColorGrade*, GrayMixer*, PostCropVignette*, IncrementalTemperature/Tint, CurveRefineSaturation, PointColors, ColorVariance, LensBlur. Same types and serialization as the global keys, but never merged into the photo\'s own settings.', []),
    ]),
    ('nested:LensBlur', 'LensBlur', [
        ('Active', 'bool', 'true/false', '', 'false', 'LrC 13+', 'LEARN', 'Needs a depth map (AI update).'),
        ('BlurAmount', 'int', '0..100', '', '50', '', 'LEARN', ''),
        ('BokehShape', 'enum int', '0.. (0 circle; others unverified)', '', '0', '', 'LEARN', ''),
        ('BokehShapeDetail', 'int', '0..?', '', '0', '', 'LEARN', ''),
        ('BokehAspect', 'int', '-100..+100', '', '0', '', 'LEARN', ''),
        ('BokehRotation', 'int', '?', '', '0', '', 'LEARN', ''),
        ('CatEyeAmount', 'int', '0..100', '', '0', '', 'LEARN', ''),
        ('CatEyeScale', 'int', '100', '', '100', '', 'LEARN', ''),
        ('HighlightsBoost', 'int', '0..100', '', '50', '', 'LEARN', ''),
        ('HighlightsThreshold', 'int', '0..100', '', '50', '', 'LEARN', ''),
        ('SphericalAberration', 'int', '0..100?', '', '0', '', 'LEARN', ''),
        ('Version', 'int', '1', '', '1', '', 'META', ''),
        ('FocalRange', 'string', '4 ints "a b c d" (depth 0..100, may exceed)', '', '"0 0 100 100" in presets', '', 'UNKNOWN', 'Preset value vs per-photo focus range; whether LrC re-focuses on the subject when applied from a preset is unverified.'),
        ('FocalRangeSource', 'enum int', '1 observed', '', '', '', 'UNKNOWN', ''),
        ('SampledArea, SampledRange, SubjectRange, ImageOrientation', 'string / int', '', '', '', '', 'COMPUTED', 'Per-photo focus sampling.', ['SampledArea', 'SampledRange', 'SubjectRange', 'ImageOrientation']),
    ]),
    ('nested:CorrectionRangeMask', 'CorrectionRangeMask (inside a Mask/RangeMask component)', [
        ('Version', 'int', '3', '', '3', '', 'META', ''),
        ('Type', 'enum int', '1 colour, 2 luminance, 3 depth (unobserved)', '', '', '', 'LEARN†', 'Type 2 is LEARN; Type 1 is PHOTO (photo-specific samples).'),
        ('Invert', 'bool', 'true/false', '', 'false', '', 'LEARN', ''),
        ('SampleType', 'enum int', '0 / 1', '', '0', '', 'UNKNOWN', 'Meaning unverified.'),
        ('LumRange', 'string', '4 floats "%.6f": feather-low, low, high, feather-high (0..1)', '', '', '', 'LEARN', 'Threshold-only, frame-independent.'),
        ('LuminanceDepthSampleInfo', 'string', '"0 x y" eyedropper sample', '', '', '', 'PHOTO', ''),
        ('ColorAmount', 'real', '0..1 (UI "Refine" /100)', '', '', 'colour ranges', 'PHOTO', ''),
        ('PointModels', 'string[]', 'Lab sample + tolerances', '', '', 'colour ranges', 'PHOTO', ''),
        ('AreaModels', 'struct[]', '{AreaComponents: string[], ColorRangeMaskAreaSampleInfo}', '', '', 'colour ranges', 'PHOTO', ''),
        ('AreaComponents, ColorRangeMaskAreaSampleInfo', 'string', '', '', '', 'inside AreaModels', 'PHOTO', '', ['AreaComponents', 'ColorRangeMaskAreaSampleInfo']),
        ('LumMin, LumMax, LumFeather, DepthMin, DepthMax, DepthFeather, DepthRange', 'real', '', '', '', 'known from Camera Raw; never observed in LrC output (legacy LrC 7 range masks)', 'UNKNOWN', '', ['LumMin', 'LumMax', 'LumFeather', 'DepthMin', 'DepthMax', 'DepthFeather', 'DepthRange']),
    ]),
    ('nested:Gesture', 'Gesture (hint for Mask/Image subtype 0 "Select Object")', [
        ('What', 'enum', 'Mask/Polygon, Mask/Paint', '', '', '', 'PHOTO', ''),
        ('Points', 'struct[]', 'Seq of {X, Y}, normalised sensor frame; order TL, TR, BL, BR (display order)', '', '', '', 'PHOTO', 'The only gesture form we may write (a box as prompt).'),
        ('X, Y', 'real', '', '', '', 'inside Points', 'PHOTO', '', ['X', 'Y']),
        ('Dabs, Radius, Flow, CenterWeight, BrushGestureInterpretation', 'mixed', '', '', '', 'paint-stroke gestures', 'NEVER', '', ['Dabs', 'Radius', 'Flow', 'CenterWeight', 'BrushGestureInterpretation']),
        ('MaskActive, MaskBlendMode, MaskInverted, MaskSyncID, MaskValue', 'mixed', 'as mask components', '', '', '', 'META', '', ['MaskActive', 'MaskBlendMode', 'MaskInverted', 'MaskSyncID', 'MaskValue']),
        ('MaskID', 'runtime', '', '', '', 'Lua/catalog only', 'COMPUTED', 'Runtime id; never write.'),
    ]),
    ('nested:AILook', 'AILook (Adaptive Color state)', [
        ('Active, AILookData, InputDigest, InputDigestVersion, ModelVersion, Version', 'mixed', '', '', '', '', 'COMPUTED', 'Produced by the AI update.', ['Active', 'AILookData', 'InputDigest', 'InputDigestVersion', 'ModelVersion', 'Version']),
    ]),
    ('nested:RangeMaskMapInfo', 'RangeMaskMapInfo', [
        ('RangeMaskMapInfo (inner wrapper), RGBMin, RGBMax, LabMin, LabMax, LumEq', 'string / string[]', 'image statistics', '', '', '', 'COMPUTED', 'Written as rdf:parseType="Resource" wrapper around an inner RangeMaskMapInfo struct.', ['RangeMaskMapInfo', 'RGBMin', 'RGBMax', 'LabMin', 'LabMax', 'LumEq']),
    ]),
    ('nested:DepthMapInfo', 'DepthMapInfo', [
        ('DepthSource, Base{RawDepth,LayeredDepth,HighlightGuide}{Table,InputDigest,Version}', 'mixed', '', '', '', '', 'COMPUTED', '10 keys.',
         ['DepthSource', 'BaseRawDepthTable', 'BaseRawDepthInputDigest', 'BaseRawDepthVersion', 'BaseLayeredDepthTable', 'BaseLayeredDepthInputDigest', 'BaseLayeredDepthVersion', 'BaseHighlightGuideTable', 'BaseHighlightGuideInputDigest', 'BaseHighlightGuideVersion']),
    ]),
    ('nested:ISODependent', 'ISODependent items (Adobe ISO-adaptive presets)', [
        ('ISO, ColorNoiseReduction, LuminanceSmoothing', 'int', 'ISO breakpoint + NR values', '', '', 'presets only', 'UNKNOWN', 'Interpolated by ISO on apply (inferred). Not for v1.', ['ISO', 'ColorNoiseReduction', 'LuminanceSmoothing']),
    ]),
    ('nested:Preset', 'Preset (record of the last applied preset, sidecars only)', [
        ('Parameters', 'struct', 'the applied preset\'s own settings (same keys as the top level)', '', '', 'sidecars only', 'COMPUTED', 'Kept opaque; never merged into the photo\'s settings.'),
        ('Name, UUID, Amount, LookAmount, Group, Cluster, Baseline, SupportsAmount, SupportsMonochrome, SupportsOutputReferred', 'mixed', '', '', '', 'about a tenth of the sidecars; Parameters subtree excluded from this inventory', 'COMPUTED',
         'Written by LrC. Useful as a learning signal ("which preset did the user start from"), never written by us.',
         ['Name', 'UUID', 'Amount', 'LookAmount', 'Group', 'Cluster', 'Baseline', 'SupportsAmount', 'SupportsMonochrome', 'SupportsOutputReferred']),
    ]),
    ('nested:RetouchArea', 'RetouchAreas / RemoveAreas items', [
        ('SpotType, Method, SourceState, Opacity, Feather, Seed, OffsetY, SourceX, HealVersion, IngestInfo, fill_method, Masks', 'mixed', '', '', '', '', 'NEVER', 'Heal/clone/remove spots (Masks = Mask/Ellipse or Mask/Paint).',
         ['SpotType', 'Method', 'SourceState', 'Opacity', 'Feather', 'Seed', 'OffsetY', 'SourceX', 'HealVersion', 'IngestInfo', 'fill_method', 'Masks']),
        ('centerX, centerY, opacity, radius, seed, sourceState, sourceX, sourceY, spotType', 'mixed', '', '', '', 'Lua only: legacy spot fields next to the new ones in a few getDevelopSettings() retouch areas', 'NEVER', 'Same fields as a RetouchInfo item.',
         ['centerX', 'centerY', 'opacity', 'radius', 'seed', 'sourceState', 'sourceX', 'sourceY', 'spotType']),
        ('pm_* (content-aware / generative patch state, 38 keys)', 'mixed', '', '', '', '', 'NEVER', 'pm_patch, pm_patch_mask, pm_patch_variations, pm_remap_info_*, pm_search_*, pm_target_*, pm_whole_image_*, pm_full_image_*, pm_input_digest, ...', ['pm_%d' % i for i in range(38)]),
        ('Mask/Ellipse: X, Y, SizeX, SizeY, Alpha, CenterValue, PerimeterValue', 'real', '', '', '', 'heal spot shape', 'NEVER', '', ['X', 'Y', 'SizeX', 'SizeY', 'Alpha', 'CenterValue', 'PerimeterValue']),
    ]),
    ('nested:RetouchInfo', 'RetouchInfo items (Lua table form of the legacy spot list)', [
        ('Method, centerX, centerY, opacity, radius, sourceState, sourceX, sourceY, spotType', 'mixed', '', '', '', 'Lua only (the XMP form is one string per spot)', 'NEVER', 'Legacy heal/clone spot.',
         ['Method', 'centerX', 'centerY', 'opacity', 'radius', 'sourceState', 'sourceX', 'sourceY', 'spotType']),
    ]),
    ('nested:PointColors', 'PointColors items (Lua table form of one swatch)', [
        ('SrcHue, SrcSat, SrcLum, HueShift, SatScale, LumScale, RangeAmount, Variance', 'real', '', '', '', 'Lua only (the XMP form is one string of 19 floats per swatch)', 'UNKNOWN', 'Sampled colour and its shift; values come from one photo. Round-trip only.',
         ['SrcHue', 'SrcSat', 'SrcLum', 'HueShift', 'SatScale', 'LumScale', 'RangeAmount', 'Variance']),
        ('HueRange, SatRange, LumRange', 'struct', 'see PointColors ranges', '', '', 'Lua only', 'UNKNOWN', '', ['HueRange', 'SatRange', 'LumRange']),
    ]),
    ('nested:PointColorRange', 'HueRange / SatRange / LumRange inside a PointColors item', [
        ('LowerNone, LowerFull, UpperFull, UpperNone', 'real', '', '', '', 'Lua only', 'UNKNOWN', 'Range with feathered edges.', ['LowerNone', 'LowerFull', 'UpperFull', 'UpperNone']),
    ]),
    ('nested:Filters', 'FilterList.Filters (Denoise / Raw Details / Super Resolution / generative)', [
        ('Filters payload (Filters, Images, ImageGroup, Alpha, ColorVariations, BlackLevels, Linearization, ...)', 'mixed', '', '', '', '', 'COMPUTED',
         'FilterID, Name, Title, CompressedSettings, Src/Dst bounds, MinEditVersion, OrderIndex*, ShouldDeleteImagesDueToUpstream, IsSignalForDelete, Images[] (BlendType, Matrix*, Reference*, GlobalInputDigest, ...), ImageGroup[] (GroupDigest, PixelType, Planes, Size*, Stage3*), BlackLevels, MapPolynomial. About 60 keys.', ['f%d' % i for i in range(60)]),
        ('GenAIInfo (Name, UUID, SoftwareAgent, Timestamp, ParameterPairs)', 'struct[]', '', '', '', 'generative provenance', 'NEVER', '', ['GName', 'GUUID', 'GSoftwareAgent', 'GTimestamp', 'GParameterPairs']),
    ]),
]
# fmt: on
