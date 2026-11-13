//! AI Edit's retoucher loop: what the model is told, and the tools it edits
//! with.
//!
//! The earlier LLM edit path asked once and took the answer: a model looked
//! at a photo and guessed absolute slider values for a file and a profile it
//! had never seen rendered. Vision models are poor at that guess and good at
//! the comparison that follows it, so the loop works the way a retoucher
//! does: the model edits ([`APPLY_EDIT`]), Lightroom renders the result, the
//! model sees the render next to the original with its measurements, and
//! corrects — until it calls [`FINISH`] or runs out of rounds.
//!
//! Every edit is the complete edit from the photo's starting point, not a
//! change on top of the last one. That keeps each round idempotent (the
//! plugin re-applies from the starting point) and lets the model go back to
//! an earlier round without having to undo anything.
//!
//! The tool schema is built from the develop registry
//! ([`lrg_develop::build::recipe`]): the ranges the model is given are
//! Lightroom's own, and every field it can send is one the recipe builder
//! turns into a develop key.

use serde_json::{json, Map, Value};

use lrg_develop::build::recipe::{
    recipe_globals, starting_values, ui_range, LOCAL_ADJUSTMENTS, MASK_TARGETS,
};
use lrg_develop::registry::{lookup, Level};
use lrg_develop::DevelopSettings;
use lrg_imaging::tone::ToneStats;

use crate::chat::ToolSpec;

/// The tool that applies an edit.
pub const APPLY_EDIT: &str = "apply_edit";
/// The tool that ends the loop.
pub const FINISH: &str = "finish";

/// Masks one edit may carry. Lightroom computes each AI mask on its own, and
/// a dozen of them is a dozen seconds per render for little gain.
pub const MAX_MASKS: usize = 6;

/// The global sliders the model is offered: recipe paths below `global`.
///
/// Deliberately fewer than the registry has: the sharpening radius, the
/// noise-reduction fine controls, the vignette shape and the curve splits
/// are rarely what makes an edit, and every extra field costs tokens and,
/// for a small local model, attention.
const OFFERED_GLOBALS: &[&str] = &[
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
    "tone_curve.highlights",
    "tone_curve.lights",
    "tone_curve.darks",
    "tone_curve.shadows",
    "hsl.red.hue",
    "hsl.red.saturation",
    "hsl.red.luminance",
    "hsl.orange.hue",
    "hsl.orange.saturation",
    "hsl.orange.luminance",
    "hsl.yellow.hue",
    "hsl.yellow.saturation",
    "hsl.yellow.luminance",
    "hsl.green.hue",
    "hsl.green.saturation",
    "hsl.green.luminance",
    "hsl.aqua.hue",
    "hsl.aqua.saturation",
    "hsl.aqua.luminance",
    "hsl.blue.hue",
    "hsl.blue.saturation",
    "hsl.blue.luminance",
    "hsl.purple.hue",
    "hsl.purple.saturation",
    "hsl.purple.luminance",
    "hsl.magenta.hue",
    "hsl.magenta.saturation",
    "hsl.magenta.luminance",
    "color_grading.shadows.hue",
    "color_grading.shadows.saturation",
    "color_grading.shadows.luminance",
    "color_grading.midtones.hue",
    "color_grading.midtones.saturation",
    "color_grading.midtones.luminance",
    "color_grading.highlights.hue",
    "color_grading.highlights.saturation",
    "color_grading.highlights.luminance",
    "color_grading.blending",
    "color_grading.balance",
    "sharpening",
    "noise_reduction",
    "vignette",
    "grain",
    "black_and_white_mix.red",
    "black_and_white_mix.orange",
    "black_and_white_mix.yellow",
    "black_and_white_mix.green",
    "black_and_white_mix.aqua",
    "black_and_white_mix.blue",
    "black_and_white_mix.purple",
    "black_and_white_mix.magenta",
];

/// A look the user can ask for, as the model is told it.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub id: &'static str,
    pub label: &'static str,
    pub brief: &'static str,
}

/// The looks the plugin offers, `natural` first (the default).
pub const LOOKS: &[Look] = &[
    Look {
        id: "natural",
        label: "Natural",
        brief: "a clean, true-to-life edit. Correct the white balance and exposure, recover \
                highlights, open the shadows just enough, keep colours believable. The photo \
                should look like a better version of itself, not like an edit.",
    },
    Look {
        id: "bright_airy",
        label: "Bright & airy",
        brief: "high-key and soft. Lifted exposure and shadows, gentle contrast, clean whites \
                that do not clip, soft pastel colour, slightly warm skin, quieter greens.",
    },
    Look {
        id: "moody",
        label: "Moody",
        brief: "darker and atmospheric. Surroundings brought down, deep but not crushed blacks, \
                restrained highlights, muted and slightly cooler colour, the light kept on the \
                subject. Never muddy.",
    },
    Look {
        id: "cinematic",
        label: "Cinematic",
        brief: "filmic contrast with a teal-and-orange separation: cooler shadows, warm skin and \
                highlights, a soft highlight roll-off, rich blacks, controlled saturation.",
    },
    Look {
        id: "film",
        label: "Analog film",
        brief: "a print-film character: slightly faded blacks through the tone curve, warm \
                gentle highlights, softer contrast, the hue shifts of film stock, fine grain.",
    },
    Look {
        id: "punchy",
        label: "Vivid & punchy",
        brief: "bold colour and depth for landscapes and travel: strong but clean contrast, \
                rich skies, vibrance rather than saturation, crisp texture — no halos, no \
                oversaturation, no HDR look.",
    },
    Look {
        id: "black_and_white",
        label: "Black & white",
        brief: "a strong monochrome. Set `treatment` to black_and_white and shape the tones \
                with `black_and_white_mix` (darker blue skies, lighter skin), deep but not \
                crushed blacks, rich midtone contrast.",
    },
];

/// The look with this id, or the default.
pub fn look(id: &str) -> &'static Look {
    LOOKS.iter().find(|l| l.id == id).unwrap_or(&LOOKS[0])
}

/// What a loop is set up with.
#[derive(Clone, Debug)]
pub struct LoopSetup {
    /// The photo's white-balance family: raw (Kelvin), non-raw (offset), or
    /// unknown (no white balance offered).
    pub is_raw: Option<bool>,
    /// Whether the plugin can apply AI masks (Lightroom 15.3 and later).
    pub masks: bool,
    /// The language of `assessment` and `verdict`.
    pub language: String,
    /// One of [`LOOKS`].
    pub look: String,
    /// The user's own words about this run.
    pub instructions: Option<String>,
    /// Edits the model may apply, the first one included.
    pub max_edits: u32,
    /// Whether the loop starts from the photographer's own style.
    pub from_style: bool,
}

/// The system prompt.
pub fn system_prompt(setup: &LoopSetup) -> String {
    let mut out = String::from(
        "You are a senior photo retoucher working in Adobe Lightroom Classic. You edit one \
photograph by setting develop sliders, then look at what Lightroom renders and refine it — the \
way a professional works: look, adjust, look again.

How the session works:
- You see the photo as it is now, with measurements of its tones and the slider values it \
starts from. Each time you call apply_edit, Lightroom applies your settings and you get back the \
new render with fresh measurements.
- Every apply_edit is the COMPLETE edit, measured from the starting point — not a change on top \
of your previous one. A slider you leave out goes back to its starting value. So when you refine, \
repeat every value you want to keep and change only what the render shows is wrong.
- Values are absolute Lightroom slider positions, as the Develop panel shows them.
- Trust the measurements over your impression for clipping, brightness levels and colour casts. \
Trust your eye for mood, skin, subject emphasis and taste.
- Call finish when another round would not clearly improve the photo. If an earlier round looked \
better than the latest one, name it in best_round.

How to edit:
1. Foundation: white balance first (greys neutral and skin natural, unless the look asks for \
warmth or coolness), then exposure, so the main subject sits right.
2. Tonal range: recover highlights, open shadows, then set whites and blacks so the tones use the \
range without clipping detail that matters (small specular highlights may clip). Prefer \
highlights/shadows and the tone curve to brute contrast.
3. Presence and colour: texture and clarity sparingly, dehaze only for real haze, vibrance \
before saturation, HSL for specific colours (skin is orange and red, sky blue and aqua, foliage \
yellow and green), colour grading for the look.
4. Effects last: a subtle vignette when it helps the eye to the subject, grain only for a film \
look.

Typical sizes: exposure ±0.1–1.5, highlights −20 to −80, shadows +10 to +50, whites and blacks \
±5–30, texture and clarity ±5–25, vibrance +5–25, saturation −10 to +10. Go further only when the \
measurements call for it. In later rounds change a few things by moderate amounts; if your last \
change made things worse, go back.

Never: clip large areas, halos from clarity or dehaze, orange or magenta skin, crushed shadows \
with no detail, an HDR look, or a white balance that swings between rounds.\n",
    );

    if setup.masks {
        out.push_str(
            "\nMasks: Lightroom's AI masks find their region themselves; you name the target and \
its adjustments. subject = the main subject; sky; background = everything but the subject; \
people parts (face_skin, body_skin, hair, lips, teeth, iris_and_pupil, eye_sclera, eyebrows, \
facial_hair, clothes) cover every person in the photo; landscape classes (architecture, \
mountains, vegetation, water, natural_ground, artificial_ground, snow). Use masks for what global \
sliders cannot do without hurting the rest — typically: sky darker and richer (exposure −0.2 to \
−0.6, highlights −20 to −50, saturation +5 to +15), subject lifted (exposure +0.1 to +0.4, \
shadows +10 to +25), background calmed (exposure −0.1 to −0.4, saturation −5 to −15), skin \
smoothed (face_skin: texture −10 to −25, clarity −5 to −10), eyes brighter (iris_and_pupil: \
exposure +0.2, clarity +10). Mask exposure is in EV. At most four masks, each with a clear \
purpose, and only for things the photo shows. A mask repeated in a later round replaces the \
earlier one.\n",
        );
    }

    let look = look(&setup.look);
    out.push_str(&format!(
        "\nThe look asked for: {} — {}\n",
        look.label, look.brief
    ));
    if let Some(words) = setup
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        out.push_str(&format!(
            "The photographer's own instructions, which win over the look where they differ: \
{words}\n"
        ));
    }
    if setup.from_style {
        out.push_str(
            "\nThe photo already carries the photographer's own style: the blend of their saved \
edits on similar photos, shown to you as the first render. Keep that style — its colour, \
contrast and choices — and fix only what does not suit this frame: exposure, white balance, \
clipping, emphasis, local work. Stay near their values unless the frame clearly needs \
otherwise.\n",
        );
    }
    out.push_str(&format!(
        "\nYou may apply at most {} edits. Write `assessment` and `verdict` in {}.",
        setup.max_edits.max(1),
        if setup.language.trim().is_empty() {
            "English"
        } else {
            setup.language.trim()
        }
    ));
    out
}

fn number(min: f64, max: f64) -> Value {
    json!({"type": "number", "minimum": min, "maximum": max})
}

/// Inserts `leaf` at the dotted `path` below `root`, creating objects on the
/// way.
fn insert_path(root: &mut Map<String, Value>, path: &str, leaf: Value) {
    let mut parts: Vec<&str> = path.split('.').collect();
    let last = parts.pop().expect("a path has a leaf");
    let mut node = root;
    for part in parts {
        let entry = node
            .entry(part.to_string())
            .or_insert_with(|| json!({"type": "object", "properties": {}}));
        node = entry
            .get_mut("properties")
            .and_then(Value::as_object_mut)
            .expect("an object schema has properties");
    }
    node.insert(last.to_string(), leaf);
}

fn global_schema(setup: &LoopSetup) -> Value {
    let mut properties = Map::new();
    for (path, spec) in recipe_globals() {
        if !OFFERED_GLOBALS.contains(&path) {
            continue;
        }
        let Some((min, max)) = ui_range(spec) else {
            continue;
        };
        insert_path(&mut properties, path, number(min, max));
    }
    let white_balance = match setup.is_raw {
        Some(true) => Some(("Temperature", "Tint", "Kelvin, e.g. 5500 for daylight")),
        Some(false) => Some((
            "IncrementalTemperature",
            "IncrementalTint",
            "a relative offset; 0 keeps the photo's own white balance, negative is cooler",
        )),
        None => None,
    };
    if let Some((temperature, tint, unit)) = white_balance {
        for (field, key, description) in [
            ("temperature", temperature, unit),
            ("tint", tint, "negative is greener, positive more magenta"),
        ] {
            let spec = lookup(Level::Global, key)
                .expect("a white-balance key of the registry")
                .spec();
            if let Some((min, max)) = ui_range(spec) {
                let mut leaf = number(min, max);
                leaf["description"] = json!(description);
                properties.insert(field.to_string(), leaf);
            }
        }
    }
    properties.insert(
        "treatment".to_string(),
        json!({"type": "string", "enum": ["color", "black_and_white"]}),
    );
    json!({
        "type": "object",
        "description": "Global sliders. Leave out what stays at its starting value.",
        "properties": properties,
    })
}

fn mask_schema() -> Value {
    let mut adjustments = Map::new();
    for (name, key) in LOCAL_ADJUSTMENTS {
        let spec = lookup(Level::Correction, key)
            .expect("a correction key of the registry")
            .spec();
        if let Some((min, max)) = ui_range(spec) {
            adjustments.insert((*name).to_string(), number(min, max));
        }
    }
    let targets: Vec<&str> = MASK_TARGETS.iter().map(|(name, _)| *name).collect();
    json!({
        "type": "array",
        "maxItems": MAX_MASKS,
        "description": "Local corrections through Lightroom's AI masks, one per target.",
        "items": {
            "type": "object",
            "properties": {
                "target": {"type": "string", "enum": targets},
                "adjustments": {"type": "object", "properties": adjustments},
            },
            "required": ["target", "adjustments"],
        },
    })
}

/// The tool the model edits with.
pub fn apply_edit_tool(setup: &LoopSetup) -> ToolSpec {
    let mut properties = Map::new();
    properties.insert(
        "assessment".to_string(),
        json!({
            "type": "string",
            "description": "Two or three sentences: what you see in the latest image and what \
                            this edit changes.",
        }),
    );
    properties.insert("global".to_string(), global_schema(setup));
    if setup.masks {
        properties.insert("masks".to_string(), mask_schema());
    }
    ToolSpec {
        name: APPLY_EDIT.to_string(),
        description: "Apply the complete edit, measured from the photo's starting point, and \
                      get back Lightroom's render of it."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": ["assessment", "global"],
        }),
    }
}

/// The tool that ends the loop.
pub fn finish_tool() -> ToolSpec {
    ToolSpec {
        name: FINISH.to_string(),
        description: "End the session: the photo is done.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "verdict": {
                    "type": "string",
                    "description": "One or two sentences on the finished edit.",
                },
                "best_round": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Only when an earlier round looked better than the latest: \
                                    its number.",
                },
            },
            "required": ["verdict"],
        }),
    }
}

/// An `apply_edit` call's arguments, split up.
#[derive(Clone, Debug, PartialEq)]
pub struct EditArguments {
    /// `{"global": {...}, "masks": [...]}`.
    pub recipe: Value,
    /// What the model saw and changed.
    pub assessment: Option<String>,
    /// What was left out of the call before it reached the recipe builder.
    pub notes: Vec<String>,
}

/// An `apply_edit` call's arguments, split into the recipe and the model's
/// assessment.
///
/// Only `global` and `masks` reach the recipe; anything else the model sent
/// is dropped here, and the recipe builder reports what it cannot use.
/// A model that wrote the global sliders flat, beside `assessment`, has its
/// numbers picked up as globals rather than lost.
pub fn recipe_from_arguments(arguments: &Value) -> EditArguments {
    let assessment = arguments
        .get("assessment")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut global = arguments
        .get("global")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if global.is_empty() {
        if let Some(object) = arguments.as_object() {
            for (key, value) in object {
                if !matches!(key.as_str(), "assessment" | "masks" | "global") {
                    global.insert(key.clone(), value.clone());
                }
            }
        }
    }
    let all_masks = arguments
        .get("masks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut notes = Vec::new();
    if all_masks.len() > MAX_MASKS {
        notes.push(format!(
            "Only the first {MAX_MASKS} of {} masks were applied: each AI mask costs Lightroom \
             time on every render.",
            all_masks.len()
        ));
    }
    let masks: Vec<Value> = all_masks.into_iter().take(MAX_MASKS).collect();
    EditArguments {
        recipe: json!({"global": global, "masks": masks}),
        assessment,
        notes,
    }
}

fn format_value(path: &str, v: f64) -> String {
    if path == "exposure" {
        format!("{v:+.2}")
    } else if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// A photo's starting values for the opening message: the basic sliders on
/// one line, the rest (`tone_curve.lights 10`, ...) on the next, and how many
/// masks of its own it has.
pub fn starting_text(settings: &DevelopSettings) -> String {
    let mut basics = Vec::new();
    let mut rest = Vec::new();
    for (path, v) in starting_values(settings) {
        if path == "treatment_black_and_white" {
            rest.push("treatment black_and_white".to_string());
            continue;
        }
        let item = format!("{path} {}", format_value(&path, v));
        if path.contains('.') {
            rest.push(item);
        } else {
            basics.push(item);
        }
    }
    let mut out = basics.join(", ");
    if !rest.is_empty() {
        out.push('\n');
        out.push_str(&rest.join(", "));
    }
    if !settings.corrections.is_empty() {
        out.push_str(&format!(
            "\nThe photo already has {} mask(s) of its own; they stay as they are.",
            settings.corrections.len()
        ));
    }
    out
}

fn delta(label: &str, before: f64, after: f64, digits: usize, unit: &str) -> Option<String> {
    let (b, a) = (
        format!("{before:.digits$}{unit}"),
        format!("{after:.digits$}{unit}"),
    );
    (b != a).then(|| format!("{label} {b} → {a}"))
}

/// How a render's tones moved from the original's: only the measurements
/// that changed at the precision they are shown with.
pub fn against_original(original: &ToneStats, render: &ToneStats) -> String {
    let pct = |v: f64| v * 100.0;
    let parts: Vec<String> = [
        delta(
            "median",
            pct(original.percentiles[3]),
            pct(render.percentiles[3]),
            0,
            "",
        ),
        delta(
            "p99",
            pct(original.percentiles[6]),
            pct(render.percentiles[6]),
            0,
            "",
        ),
        delta(
            "p1",
            pct(original.percentiles[0]),
            pct(render.percentiles[0]),
            0,
            "",
        ),
        delta(
            "clipped highlights",
            pct(original.clipped_highlights),
            pct(render.clipped_highlights),
            1,
            "%",
        ),
        delta(
            "black",
            pct(original.clipped_shadows),
            pct(render.clipped_shadows),
            1,
            "%",
        ),
        delta("warmth", original.warmth, render.warmth, 1, ""),
        delta("tint", original.tint, render.tint, 1, ""),
        delta(
            "midtone saturation",
            pct(original.midtone_saturation),
            pct(render.midtone_saturation),
            0,
            "%",
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    parts.join(", ")
}

/// The first message: the photo as it is, and what the model works from.
///
/// `starting_values` are the photo's current sliders, one per line;
/// `measurements` come from `lrg_imaging::tone`; `frame_notes` are the
/// guardrails' explanations of what the frame can take.
pub fn opening_text(
    starting_values: &str,
    measurements: &str,
    frame_notes: &[String],
    setup: &LoopSetup,
) -> String {
    let mut out = String::from("This is the photo to edit, as it is now.\n");
    out.push_str(match setup.is_raw {
        Some(true) => "It is a raw file.\n",
        Some(false) => {
            "It is not a raw file (JPEG, TIFF or similar): white balance is an offset.\n"
        }
        None => "Whether it is a raw file is unknown, so white balance cannot be set.\n",
    });
    out.push_str("\nStarting values (anything not listed is at its default):\n");
    out.push_str(if starting_values.trim().is_empty() {
        "(all at their defaults)"
    } else {
        starting_values
    });
    out.push_str("\n\nMeasurements of this image:\n");
    out.push_str(measurements);
    if !frame_notes.is_empty() {
        out.push_str("\n\nWhat this frame can take (the edit is held to it):\n- ");
        out.push_str(&frame_notes.join("\n- "));
    }
    out.push_str(if setup.from_style {
        "\n\nThe photographer's style has been applied to it; its render follows."
    } else {
        "\n\nCall apply_edit with your first complete edit."
    });
    out
}

/// What the model is told about a render.
pub fn render_text(
    edit_number: u32,
    measurements: &str,
    against_original: &str,
    notes: &[String],
    edits_left: u32,
) -> String {
    let mut out = if edit_number == 0 {
        "The photographer's own style, applied and rendered by Lightroom. It counts as round 0; \
         your edits continue from the starting values, not from these."
            .to_string()
    } else {
        format!("Round {edit_number}, rendered by Lightroom.")
    };
    out.push_str("\n\nMeasurements of this render:\n");
    out.push_str(measurements);
    if !against_original.is_empty() {
        out.push_str("\n\nAgainst the original: ");
        out.push_str(against_original);
    }
    if !notes.is_empty() {
        out.push_str("\n\nWhat Lightroom could not take as given:\n- ");
        out.push_str(&notes.join("\n- "));
    }
    out.push_str(&if edits_left == 0 {
        "\n\nNo edits are left: call finish.".to_string()
    } else {
        format!(
            "\n\n{edits_left} more edit{} allowed. Call apply_edit with the complete corrected \
             edit, or finish if the photo is done.",
            if edits_left == 1 { "" } else { "s" }
        )
    });
    out
}

/// Said when the model answered without calling a tool.
pub const NUDGE: &str = "Please answer by calling apply_edit with the complete edit, or finish.";

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(is_raw: Option<bool>, masks: bool) -> LoopSetup {
        LoopSetup {
            is_raw,
            masks,
            language: "German".into(),
            look: "cinematic".into(),
            instructions: Some("keep the skin warm".into()),
            max_edits: 3,
            from_style: false,
        }
    }

    #[test]
    fn every_offered_global_is_in_the_schema() {
        let tool = apply_edit_tool(&setup(Some(true), true));
        let global = &tool.input_schema["properties"]["global"]["properties"];
        for path in OFFERED_GLOBALS {
            let mut node = global;
            let mut parts = path.split('.').peekable();
            while let Some(part) = parts.next() {
                node = &node[part];
                if parts.peek().is_some() {
                    node = &node["properties"];
                }
            }
            assert_eq!(node["type"], "number", "{path}");
        }
        assert_eq!(global["exposure"]["minimum"], json!(-5.0));
        assert_eq!(global["contrast"]["maximum"], json!(100.0));
    }

    #[test]
    fn the_white_balance_offered_is_the_photos_own_scale() {
        let raw = apply_edit_tool(&setup(Some(true), false));
        let t = &raw.input_schema["properties"]["global"]["properties"]["temperature"];
        assert!(t["maximum"].as_f64().unwrap() > 10_000.0);

        let jpeg = apply_edit_tool(&setup(Some(false), false));
        let t = &jpeg.input_schema["properties"]["global"]["properties"]["temperature"];
        assert_eq!(t["maximum"], json!(100.0));

        let unknown = apply_edit_tool(&setup(None, false));
        assert!(
            unknown.input_schema["properties"]["global"]["properties"]["temperature"].is_null()
        );
    }

    #[test]
    fn masks_are_offered_only_when_the_plugin_can_apply_them() {
        let with = apply_edit_tool(&setup(Some(true), true));
        let targets =
            &with.input_schema["properties"]["masks"]["items"]["properties"]["target"]["enum"];
        assert!(targets.as_array().unwrap().contains(&json!("face_skin")));
        let exposure = &with.input_schema["properties"]["masks"]["items"]["properties"]
            ["adjustments"]["properties"]["exposure"];
        assert_eq!(exposure["maximum"], json!(4.0));

        let without = apply_edit_tool(&setup(Some(true), false));
        assert!(without.input_schema["properties"]["masks"].is_null());
        assert!(!system_prompt(&setup(Some(true), false)).contains("Masks:"));
    }

    #[test]
    fn the_prompt_carries_the_look_the_words_and_the_language() {
        let prompt = system_prompt(&setup(Some(true), true));
        assert!(prompt.contains("Cinematic"));
        assert!(prompt.contains("keep the skin warm"));
        assert!(prompt.contains("in German"));
        assert!(prompt.contains("at most 3 edits"));
        assert_eq!(look("nonsense").id, "natural");
    }

    #[test]
    fn arguments_split_into_recipe_and_assessment() {
        let EditArguments {
            recipe, assessment, ..
        } = recipe_from_arguments(&json!({
            "assessment": "  Too dark. ",
            "global": {"exposure": 0.5},
            "masks": [{"target": "sky", "adjustments": {"exposure": -0.3}}],
            "mood": "nice"
        }));
        assert_eq!(assessment.as_deref(), Some("Too dark."));
        assert_eq!(recipe["global"]["exposure"], json!(0.5));
        assert_eq!(recipe["masks"][0]["target"], json!("sky"));
        assert!(recipe.get("mood").is_none());
    }

    #[test]
    fn flat_sliders_are_read_as_globals() {
        let args = recipe_from_arguments(&json!({"assessment": "x", "exposure": 0.3}));
        assert_eq!(args.recipe["global"]["exposure"], json!(0.3));
    }

    #[test]
    fn the_masks_are_capped() {
        let many: Vec<Value> = (0..10)
            .map(|_| json!({"target": "sky", "adjustments": {"exposure": -0.1}}))
            .collect();
        let args = recipe_from_arguments(&json!({"global": {}, "masks": many}));
        assert_eq!(args.recipe["masks"].as_array().unwrap().len(), MAX_MASKS);
        assert_eq!(args.notes.len(), 1, "the left-out masks are reported");
    }

    fn tones(median: f64, clipped: f64, warmth: f64) -> ToneStats {
        ToneStats {
            percentiles: [0.02, 0.05, 0.2, median, 0.7, 0.9, 0.97],
            bands: [0.1, 0.2, 0.4, 0.2, 0.1],
            clipped_highlights: clipped,
            clipped_shadows: 0.01,
            midtone_saturation: 0.3,
            warmth,
            tint: 0.0,
            cast_from_neutrals: true,
            contrast: 0.2,
        }
    }

    #[test]
    fn the_comparison_names_only_what_moved() {
        let text = against_original(&tones(0.38, 0.004, 3.0), &tones(0.47, 0.004, 0.4));
        assert!(text.contains("median 38 → 47"), "{text}");
        assert!(text.contains("warmth 3.0 → 0.4"), "{text}");
        assert!(!text.contains("clipped highlights"), "{text}");
        assert!(!text.contains("p99"), "{text}");
    }

    #[test]
    fn starting_text_lists_the_basics_and_the_rest() {
        let (settings, _) = lrg_develop::lua::from_lua_value(
            &json!({"ProcessVersion": "15.4", "Exposure2012": 0.7, "ParametricLights": 10}),
            lrg_develop::FileKindHint::Raw,
        )
        .unwrap();
        let text = starting_text(&settings);
        assert!(text.starts_with("exposure +0.70, contrast 0"), "{text}");
        assert!(text.contains("tone_curve.lights 10"), "{text}");
    }

    #[test]
    fn the_render_text_counts_down_and_says_when_to_stop() {
        let text = render_text(2, "m", "median 30 → 41", &["note".into()], 1);
        assert!(text.contains("Round 2"));
        assert!(text.contains("1 more edit allowed"));
        assert!(text.contains("- note"));
        assert!(render_text(3, "m", "", &[], 0).contains("call finish"));
        assert!(render_text(0, "m", "", &[], 2).contains("round 0"));
    }
}
