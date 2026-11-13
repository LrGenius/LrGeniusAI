//! An edit recipe → [`DevelopSettings`]: what AI Edit's retoucher loop hands
//! to Lightroom.
//!
//! The recipe is the vocabulary a language model edits in: UI units, the
//! `global.*` names of the registry's recipe aliases, and semantic mask
//! targets by name. This module turns it into the model, so the Lua writer
//! ([`crate::lua::to_lua_value`]) can produce the table
//! `photo:applyDevelopSettings()` takes, masks included.
//!
//! ```json
//! {
//!   "global": {
//!     "exposure": 0.35, "highlights": -40, "temperature": 5600, "tint": 4,
//!     "tone_curve": {"lights": 8, "darks": -6},
//!     "hsl": {"orange": {"saturation": -6}},
//!     "color_grading": {"midtones": {"hue": 40, "saturation": 6}},
//!     "treatment": "black_and_white", "black_and_white_mix": {"red": 10}
//!   },
//!   "masks": [
//!     {"target": "sky", "adjustments": {"exposure": -0.4, "saturation": 12}}
//!   ]
//! }
//! ```
//!
//! What goes where:
//!
//! - Every global key with a recipe alias (`global.exposure`,
//!   `global.hsl.red.hue`, ...) is read at its alias path, plus
//!   [`EXTRA_GLOBALS`]: the colour-grading midtones, luminances and blending,
//!   and the black-and-white mix, which no training example teaches and so
//!   carry no alias.
//! - `temperature`/`tint` go into the photo's own white-balance family with
//!   `WhiteBalance = "Custom"`: Kelvin and an absolute tint on a raw file,
//!   the incremental offsets on everything else. A number on the other
//!   family's scale is dropped, never clamped (6200 clamped to +100 is a
//!   hard orange cast).
//! - `treatment` sets `ConvertToGrayscale`.
//! - Each mask becomes one correction from [`CorrectionBuilder`], named
//!   `LrGenius · <target>` with sync ids derived from the target, so a later
//!   round's sky replaces this round's instead of stacking a second one.
//!
//! Numbers outside a key's range are clamped and reported: the recipe has
//! been checked against the same ranges before it gets here, so a note from
//! this module means the two disagree.

use serde_json::{Map, Value as J};

use super::{CorrectionBuilder, LandscapeClass, PeoplePart, Semantic, SyncNamespace};
use crate::model::value::{Finite, Value};
use crate::model::{DevelopSettings, FileKind};
use crate::registry::{self, from_ui_value, to_ui, KeySpec, Level};

/// Mask targets a recipe may name, in the order a prompt lists them, with the
/// role their correction is named after.
pub const MASK_TARGETS: &[(&str, &str)] = &[
    ("subject", "Subject"),
    ("sky", "Sky"),
    ("background", "Background"),
    ("face_skin", "Face Skin"),
    ("body_skin", "Body Skin"),
    ("hair", "Hair"),
    ("lips", "Lips"),
    ("teeth", "Teeth"),
    ("iris_and_pupil", "Iris and Pupil"),
    ("eye_sclera", "Eye Sclera"),
    ("eyebrows", "Eyebrows"),
    ("facial_hair", "Facial Hair"),
    ("clothes", "Clothes"),
    ("architecture", "Architecture"),
    ("mountains", "Mountains"),
    ("vegetation", "Vegetation"),
    ("water", "Water"),
    ("natural_ground", "Natural Ground"),
    ("artificial_ground", "Artificial Ground"),
    ("snow", "Snow"),
];

/// The AI mask a recipe's target names, or `None` for a name not in
/// [`MASK_TARGETS`].
///
/// The landscape class "sky" is left out on purpose: `sky` is Lightroom's
/// own Select Sky, which every version since masks has, and two masks for
/// the same thing would only confuse the model.
pub fn mask_target(name: &str) -> Option<Semantic> {
    Some(match name {
        "subject" => Semantic::Subject,
        "sky" => Semantic::Sky,
        "background" => Semantic::Background,
        "face_skin" => Semantic::people_part(PeoplePart::FaceSkin),
        "body_skin" => Semantic::people_part(PeoplePart::BodySkin),
        "hair" => Semantic::people_part(PeoplePart::Hair),
        "lips" => Semantic::people_part(PeoplePart::Lips),
        "teeth" => Semantic::people_part(PeoplePart::Teeth),
        "iris_and_pupil" => Semantic::people_part(PeoplePart::IrisAndPupil),
        "eye_sclera" => Semantic::people_part(PeoplePart::EyeSclera),
        "eyebrows" => Semantic::people_part(PeoplePart::Eyebrows),
        "facial_hair" => Semantic::people_part(PeoplePart::FacialHair),
        "clothes" => Semantic::people_part(PeoplePart::Clothes),
        "architecture" => Semantic::landscape(LandscapeClass::Architecture),
        "mountains" => Semantic::landscape(LandscapeClass::Mountains),
        "vegetation" => Semantic::landscape(LandscapeClass::Vegetation),
        "water" => Semantic::landscape(LandscapeClass::Water),
        "natural_ground" => Semantic::landscape(LandscapeClass::NaturalGround),
        "artificial_ground" => Semantic::landscape(LandscapeClass::ArtificialGround),
        "snow" => Semantic::landscape(LandscapeClass::Snow),
        _ => return None,
    })
}

/// A mask's adjustments: recipe name → correction key. Exposure is in EV
/// (±4), the rest are the panel's ±100 sliders.
pub const LOCAL_ADJUSTMENTS: &[(&str, &str)] = &[
    ("exposure", "LocalExposure2012"),
    ("contrast", "LocalContrast2012"),
    ("highlights", "LocalHighlights2012"),
    ("shadows", "LocalShadows2012"),
    ("whites", "LocalWhites2012"),
    ("blacks", "LocalBlacks2012"),
    ("temperature", "LocalTemperature"),
    ("tint", "LocalTint"),
    ("texture", "LocalTexture"),
    ("clarity", "LocalClarity2012"),
    ("dehaze", "LocalDehaze"),
    ("saturation", "LocalSaturation"),
    ("sharpness", "LocalSharpness"),
    ("noise", "LocalLuminanceNoise"),
];

/// Global keys a recipe may set that have no recipe alias: recipe path below
/// `global` → registry name.
pub const EXTRA_GLOBALS: &[(&str, &str)] = &[
    ("color_grading.midtones.hue", "ColorGradeMidtoneHue"),
    ("color_grading.midtones.saturation", "ColorGradeMidtoneSat"),
    ("color_grading.midtones.luminance", "ColorGradeMidtoneLum"),
    ("color_grading.shadows.luminance", "ColorGradeShadowLum"),
    (
        "color_grading.highlights.luminance",
        "ColorGradeHighlightLum",
    ),
    ("color_grading.blending", "ColorGradeBlending"),
    ("black_and_white_mix.red", "GrayMixerRed"),
    ("black_and_white_mix.orange", "GrayMixerOrange"),
    ("black_and_white_mix.yellow", "GrayMixerYellow"),
    ("black_and_white_mix.green", "GrayMixerGreen"),
    ("black_and_white_mix.aqua", "GrayMixerAqua"),
    ("black_and_white_mix.blue", "GrayMixerBlue"),
    ("black_and_white_mix.purple", "GrayMixerPurple"),
    ("black_and_white_mix.magenta", "GrayMixerMagenta"),
];

/// Above this, a temperature can only be Kelvin; below it, only an offset.
/// Lightroom's relative scale ends at ±100 and no daylight is under 1000 K.
const KELVIN_LOOKING: f64 = 500.0;

/// [`settings_from_recipe`]'s result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FromRecipe {
    /// The settings, with `file_kind` set to the photo's.
    pub settings: DevelopSettings,
    /// What did not make it, or not as given, in words for the user and the
    /// model.
    pub notes: Vec<String>,
}

/// Every global key a recipe can set: path below `global` (dots) → registry
/// spec, aliases first, then [`EXTRA_GLOBALS`].
pub fn recipe_globals() -> impl Iterator<Item = (&'static str, &'static KeySpec)> {
    let aliased = registry::iter()
        .filter(|(_, spec)| spec.level == Level::Global)
        .filter_map(|(_, spec)| Some((spec.recipe_alias?.strip_prefix("global.")?, spec)));
    let extra = EXTRA_GLOBALS.iter().map(|(path, name)| {
        let id = registry::lookup(Level::Global, name)
            .unwrap_or_else(|| panic!("EXTRA_GLOBALS names a key the registry lacks: {name}"));
        (*path, id.spec())
    });
    aliased.chain(extra)
}

/// A key's range in UI units, when it has one.
pub fn ui_range(spec: &KeySpec) -> Option<(f64, f64)> {
    let (lo, hi) = spec.range?;
    Some((to_ui(spec, lo).ok()?.get(), to_ui(spec, hi).ok()?.get()))
}

/// Builds the develop settings of `recipe` for a photo of `file_kind`.
///
/// `file_kind` decides the white-balance family; unknown means no white
/// balance is written, with a note.
pub fn settings_from_recipe(recipe: &J, file_kind: Option<FileKind>) -> FromRecipe {
    let mut out = FromRecipe::default();
    out.settings.file_kind = file_kind;
    let empty = Map::new();
    let global = recipe
        .get("global")
        .and_then(J::as_object)
        .unwrap_or(&empty);

    for (path, spec) in recipe_globals() {
        let Some(ui) = number_at(global, path) else {
            continue;
        };
        insert_ui(&mut out, spec, ui, path);
    }
    white_balance(global, file_kind, &mut out);
    match global.get("treatment").and_then(J::as_str) {
        Some("black_and_white") => insert_value(&mut out, "ConvertToGrayscale", Value::Bool(true)),
        Some("color") => insert_value(&mut out, "ConvertToGrayscale", Value::Bool(false)),
        Some(other) => out.notes.push(format!(
            "Treatment \"{other}\" is not one Lightroom has; it was left as it was."
        )),
        None => {}
    }

    if let Some(masks) = recipe.get("masks").and_then(J::as_array) {
        masks_into(masks, &mut out);
    }
    out
}

/// The sliders always reported by [`starting_values`], set or not: the ones a
/// model reasons from first.
const ALWAYS_REPORTED: &[&str] = &[
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "whites",
    "blacks",
    "texture",
    "clarity",
    "dehaze",
    "vibrance",
    "saturation",
];

/// Where a photo starts, in the recipe's vocabulary and UI units: the basic
/// sliders always, every other recipe global that is not 0, then the white
/// balance (`temperature`/`tint` on the photo's own scale) and
/// `treatment` = 1 for a black-and-white photo.
///
/// The counterpart of [`settings_from_recipe`] for telling a model the
/// values its edit replaces.
pub fn starting_values(settings: &DevelopSettings) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for (path, spec) in recipe_globals() {
        let ui = settings
            .get_by_name(spec.name)
            .and_then(Value::as_finite)
            .and_then(|stored| to_ui(spec, stored).ok())
            .map(Finite::get);
        match ui {
            Some(v) if v != 0.0 || ALWAYS_REPORTED.contains(&path) => {
                out.push((path.to_string(), v));
            }
            None if ALWAYS_REPORTED.contains(&path) => out.push((path.to_string(), 0.0)),
            _ => {}
        }
    }
    if let Some(wb) = settings.wb() {
        if let Some(t) = wb.temperature {
            out.push(("temperature".to_string(), t.get()));
        }
        if let Some(t) = wb.tint {
            out.push(("tint".to_string(), t.get()));
        }
    }
    if settings
        .get_by_name("ConvertToGrayscale")
        .and_then(Value::as_bool)
        == Some(true)
    {
        out.push(("treatment_black_and_white".to_string(), 1.0));
    }
    out
}

fn number_at(map: &Map<String, J>, path: &str) -> Option<f64> {
    let mut parts = path.split('.');
    let mut node = map.get(parts.next()?)?;
    for part in parts {
        node = node.get(part)?;
    }
    node.as_f64().filter(|v| v.is_finite())
}

fn insert_value(out: &mut FromRecipe, name: &str, value: Value) {
    let id = registry::lookup(Level::Global, name)
        .unwrap_or_else(|| panic!("registry row Global/{name}"));
    out.settings
        .insert(id, value)
        .unwrap_or_else(|e| panic!("{name} is a plain global key: {e}"));
}

fn insert_ui(out: &mut FromRecipe, spec: &'static KeySpec, ui: f64, label: &str) {
    let Ok(ui) = Finite::new(ui) else {
        return;
    };
    match from_ui_value(spec, ui) {
        Ok((value, clamped)) => {
            if clamped.is_some() {
                out.notes.push(format!(
                    "{label} {} is outside Lightroom's range and was limited to it.",
                    ui.get()
                ));
            }
            insert_value(out, spec.name, value);
        }
        // Every key reached here is a numeric slider with a known scale, so
        // this is a registry bug; the words say what happened to the edit.
        Err(e) => out.notes.push(format!(
            "{label} was left as it was: Lightroom's {} did not take the value ({e}).",
            spec.name
        )),
    }
}

fn white_balance(global: &Map<String, J>, file_kind: Option<FileKind>, out: &mut FromRecipe) {
    let temperature = global.get("temperature").and_then(J::as_f64);
    let tint = global.get("tint").and_then(J::as_f64);
    if temperature.is_none() && tint.is_none() {
        return;
    }
    let Some(kind) = file_kind else {
        out.notes.push(
            "White balance was not applied: it is not known whether this photo is a raw file, \
             so it is unclear which white-balance scale it uses."
                .to_string(),
        );
        return;
    };
    let (temperature_key, tint_key) = match kind {
        FileKind::Raw => ("Temperature", "Tint"),
        FileKind::NonRaw => ("IncrementalTemperature", "IncrementalTint"),
    };
    if let Some(t) = temperature {
        let kelvin = t.abs() > KELVIN_LOOKING;
        let wrong_scale = match kind {
            FileKind::Raw => !kelvin,
            FileKind::NonRaw => kelvin,
        };
        if wrong_scale {
            out.notes.push(match kind {
                FileKind::Raw => format!(
                    "White balance was not applied: temperature {t} is a relative value, but this \
                     raw file takes Kelvin."
                ),
                FileKind::NonRaw => format!(
                    "White balance was not applied: temperature {t} is in Kelvin, but this photo \
                     is not a raw file and takes a relative -100 to +100."
                ),
            });
            return;
        }
    }
    let spec = |name| {
        registry::lookup(Level::Global, name)
            .unwrap_or_else(|| panic!("registry row Global/{name}"))
            .spec()
    };
    if let Some(t) = temperature {
        insert_ui(out, spec(temperature_key), t, "temperature");
    }
    if let Some(t) = tint {
        insert_ui(out, spec(tint_key), t, "tint");
    }
    insert_value(out, "WhiteBalance", Value::Str("Custom".to_string()));
}

fn masks_into(masks: &[J], out: &mut FromRecipe) {
    let namespace = SyncNamespace::lrgenius();
    let mut seen: Vec<&str> = Vec::new();
    'masks: for mask in masks {
        let Some(object) = mask.as_object() else {
            continue;
        };
        let Some(target) = object.get("target").and_then(J::as_str) else {
            out.notes
                .push("A mask without a target was left out.".to_string());
            continue;
        };
        let (Some(semantic), Some((_, role))) = (
            mask_target(target),
            MASK_TARGETS.iter().find(|(name, _)| *name == target),
        ) else {
            out.notes.push(format!(
                "A mask on \"{target}\" was left out: Lightroom has no AI mask by that name."
            ));
            continue;
        };
        if seen.contains(&target) {
            out.notes.push(format!(
                "A second mask on {target} was left out: one correction per target."
            ));
            continue;
        }
        seen.push(target);

        let adjustments = object
            .get("adjustments")
            .and_then(J::as_object)
            .unwrap_or(object);
        let mut builder = CorrectionBuilder::new(*role, &namespace);
        let mut any = false;
        for (name, key) in LOCAL_ADJUSTMENTS {
            let Some(mut ui) = adjustments.get(*name).and_then(J::as_f64) else {
                continue;
            };
            if !ui.is_finite() || ui == 0.0 {
                continue;
            }
            let spec = registry::lookup(Level::Correction, key)
                .unwrap_or_else(|| panic!("registry row Correction/{key}"))
                .spec();
            if let Some((lo, hi)) = ui_range(spec) {
                if ui < lo || ui > hi {
                    out.notes.push(format!(
                        "The {target} mask's {name} {ui} is outside Lightroom's range and was \
                         limited to it."
                    ));
                    ui = ui.clamp(lo, hi);
                }
            }
            builder = match builder.local_ui(key, ui) {
                Ok(b) => b,
                Err(e) => {
                    out.notes
                        .push(format!("The {target} mask was left out: {e}."));
                    continue 'masks;
                }
            };
            any = true;
        }
        if !any {
            continue;
        }
        match builder.add(semantic).build() {
            Ok(correction) => out.settings.corrections.push(correction),
            Err(e) => out
                .notes
                .push(format!("The {target} mask was left out: {e}.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::MaskTool;
    use crate::lua::{to_lua_value, LuaMode, LuaOptions, PhotoContext};
    use serde_json::json;

    fn get(f: &FromRecipe, name: &str) -> Option<Value> {
        f.settings.get_by_name(name).cloned()
    }

    #[test]
    fn globals_land_on_their_registry_keys_in_the_stored_unit() {
        let f = settings_from_recipe(
            &json!({"global": {
                "exposure": 0.35,
                "contrast": 12,
                "tone_curve": {"lights": 8},
                "hsl": {"orange": {"saturation": -6}},
                "color_grading": {"shadows": {"hue": 210, "saturation": 9},
                                  "midtones": {"hue": 40, "saturation": 6}}
            }}),
            Some(FileKind::Raw),
        );
        assert!(f.notes.is_empty(), "{:?}", f.notes);
        assert_eq!(
            get(&f, "Exposure2012"),
            Some(Value::Real(Finite::new(0.35).unwrap()))
        );
        assert_eq!(get(&f, "Contrast2012"), Some(Value::Int(12)));
        assert_eq!(get(&f, "ParametricLights"), Some(Value::Int(8)));
        assert_eq!(get(&f, "SaturationAdjustmentOrange"), Some(Value::Int(-6)));
        assert_eq!(get(&f, "SplitToningShadowHue"), Some(Value::Int(210)));
        assert_eq!(get(&f, "ColorGradeMidtoneHue"), Some(Value::Int(40)));
        assert_eq!(f.settings.file_kind, Some(FileKind::Raw));
    }

    #[test]
    fn a_number_out_of_range_is_limited_and_reported() {
        let f = settings_from_recipe(&json!({"global": {"contrast": 180}}), None);
        assert_eq!(get(&f, "Contrast2012"), Some(Value::Int(100)));
        assert_eq!(f.notes.len(), 1);
    }

    #[test]
    fn white_balance_follows_the_photos_family() {
        let raw = settings_from_recipe(
            &json!({"global": {"temperature": 5600, "tint": 6}}),
            Some(FileKind::Raw),
        );
        assert_eq!(get(&raw, "Temperature"), Some(Value::Int(5600)));
        assert_eq!(get(&raw, "Tint"), Some(Value::Int(6)));
        assert_eq!(get(&raw, "WhiteBalance"), Some(Value::Str("Custom".into())));
        assert!(get(&raw, "IncrementalTemperature").is_none());

        let jpeg = settings_from_recipe(
            &json!({"global": {"temperature": -8, "tint": 3}}),
            Some(FileKind::NonRaw),
        );
        assert_eq!(get(&jpeg, "IncrementalTemperature"), Some(Value::Int(-8)));
        assert_eq!(get(&jpeg, "IncrementalTint"), Some(Value::Int(3)));
        assert!(get(&jpeg, "Temperature").is_none());
    }

    #[test]
    fn a_temperature_on_the_other_scale_is_dropped_not_clamped() {
        for (t, kind) in [(6200.0, FileKind::NonRaw), (12.0, FileKind::Raw)] {
            let f = settings_from_recipe(&json!({"global": {"temperature": t}}), Some(kind));
            assert!(get(&f, "WhiteBalance").is_none(), "{t}");
            assert!(get(&f, "Temperature").is_none(), "{t}");
            assert!(get(&f, "IncrementalTemperature").is_none(), "{t}");
            assert_eq!(f.notes.len(), 1, "{t}");
        }
    }

    #[test]
    fn an_unknown_file_kind_gets_no_white_balance() {
        let f = settings_from_recipe(&json!({"global": {"temperature": 5600}}), None);
        assert!(get(&f, "WhiteBalance").is_none());
        assert_eq!(f.notes.len(), 1);
    }

    #[test]
    fn treatment_switches_black_and_white() {
        let f = settings_from_recipe(
            &json!({"global": {"treatment": "black_and_white",
                               "black_and_white_mix": {"blue": -20}}}),
            Some(FileKind::Raw),
        );
        assert_eq!(get(&f, "ConvertToGrayscale"), Some(Value::Bool(true)));
        assert_eq!(get(&f, "GrayMixerBlue"), Some(Value::Int(-20)));
    }

    #[test]
    fn masks_become_named_corrections_with_stable_ids() {
        let recipe = json!({"masks": [
            {"target": "sky", "adjustments": {"exposure": -0.4, "saturation": 12}},
            {"target": "face_skin", "adjustments": {"exposure": 0.2}},
            {"target": "sky", "adjustments": {"exposure": 1.0}},
            {"target": "moon", "adjustments": {"exposure": 1.0}},
            {"target": "subject", "adjustments": {"exposure": 0}}
        ]});
        let f = settings_from_recipe(&recipe, Some(FileKind::Raw));
        let names: Vec<_> = f
            .settings
            .corrections
            .iter()
            .map(|c| c.name.clone().unwrap())
            .collect();
        assert_eq!(names, ["LrGenius · Sky", "LrGenius · Face Skin"]);
        // The duplicate sky and the unknown target are reported; an all-zero
        // mask changes nothing and is dropped without a word.
        assert_eq!(f.notes.len(), 2, "{:?}", f.notes);
        assert!(matches!(
            f.settings.corrections[0].masks[0].tool,
            MaskTool::Semantic(Semantic::Sky)
        ));

        let again = settings_from_recipe(&recipe, Some(FileKind::Raw));
        assert_eq!(
            f.settings.corrections[0].sync_id,
            again.settings.corrections[0].sync_id
        );
    }

    #[test]
    fn every_target_builds() {
        for (name, _) in MASK_TARGETS {
            let f = settings_from_recipe(
                &json!({"masks": [{"target": name, "adjustments": {"exposure": 0.1}}]}),
                Some(FileKind::Raw),
            );
            assert_eq!(f.settings.corrections.len(), 1, "{name}: {:?}", f.notes);
        }
    }

    #[test]
    fn the_settings_write_as_an_apply_table_with_the_ai_update() {
        let f = settings_from_recipe(
            &json!({"global": {"exposure": 0.2, "temperature": 5400},
                    "masks": [{"target": "subject", "adjustments": {"shadows": 15}}]}),
            Some(FileKind::Raw),
        );
        let photo = PhotoContext {
            file_kind: Some(FileKind::Raw),
            ..PhotoContext::default()
        };
        let written =
            to_lua_value(&f.settings, &LuaMode::Apply(photo), &LuaOptions::default()).unwrap();
        assert!(written.call.update_ai_settings);
        assert_eq!(written.table["Exposure2012"], json!(0.2));
        assert_eq!(written.table["Temperature"], json!(5400));
        assert_eq!(written.table["WhiteBalance"], json!("Custom"));
        let corrections = written.table["MaskGroupBasedCorrections"]
            .as_array()
            .unwrap();
        assert_eq!(corrections.len(), 1);
        assert_eq!(
            corrections[0]["CorrectionName"],
            json!("LrGenius · Subject")
        );
        assert!(written.skipped.is_empty(), "{:?}", written.skipped);
    }

    #[test]
    fn starting_values_read_back_what_a_recipe_wrote() {
        let f = settings_from_recipe(
            &json!({"global": {"exposure": 0.5, "hsl": {"blue": {"luminance": -12}},
                               "temperature": 5200, "tint": 3}}),
            Some(FileKind::Raw),
        );
        let values = starting_values(&f.settings);
        let get = |name: &str| values.iter().find(|(p, _)| p == name).map(|(_, v)| *v);
        assert_eq!(get("exposure"), Some(0.5));
        assert_eq!(get("contrast"), Some(0.0), "the basics are always reported");
        assert_eq!(get("hsl.blue.luminance"), Some(-12.0));
        assert_eq!(
            get("hsl.red.hue"),
            None,
            "a zero beyond the basics is left out"
        );
        assert_eq!(get("temperature"), Some(5200.0));
        assert_eq!(get("tint"), Some(3.0));
    }

    #[test]
    fn starting_values_read_a_lightroom_table() {
        let (settings, _) = crate::lua::from_lua_value(
            &json!({
                "ProcessVersion": "15.4",
                "Exposure2012": 0.7,
                "Contrast2012": 15,
                "WhiteBalance": "As Shot",
                "IncrementalTemperature": 0,
                "IncrementalTint": 0,
                "ConvertToGrayscale": true
            }),
            crate::FileKindHint::Unknown,
        )
        .unwrap();
        let values = starting_values(&settings);
        assert!(values.contains(&("exposure".to_string(), 0.7)));
        assert!(values.contains(&("contrast".to_string(), 15.0)));
        assert!(values.contains(&("temperature".to_string(), 0.0)));
        assert!(values.contains(&("treatment_black_and_white".to_string(), 1.0)));
    }

    #[test]
    fn every_extra_global_is_a_registry_key() {
        assert_eq!(
            recipe_globals()
                .filter(|(path, _)| path.starts_with("black_and_white_mix"))
                .count(),
            8
        );
    }
}
