//! AI Edit's retoucher loop end to end, on a scripted model: what the plugin
//! gets back at each step, and what the model is shown.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use lrg_api::edit_loop::{render, start, StartRequest};
use lrg_api::state::{AppState, ModelAssetPaths};
use lrg_providers::chat::{
    ChatDeltas, ChatModel, ChatPart, ChatRequest, ChatRole, ChatTurn, ToolCall,
};
use lrg_providers::edit_loop::LoopSetup;

static NO_MODELS: LazyLock<tempfile::TempDir> =
    LazyLock::new(|| tempfile::tempdir().expect("temp dir for the empty model directory"));

/// Every model family pointed at an empty directory, as in `contract.rs`: the
/// guardrails run without the face rules, and nothing loads.
fn models_absent() -> ModelAssetPaths {
    let dir = NO_MODELS.path();
    ModelAssetPaths {
        siglip: lrg_ml::siglip::ModelPaths {
            image_onnx: dir.join("siglip2_naflex_image.onnx"),
            text_onnx: dir.join("siglip2_naflex_text.onnx"),
            tokenizer_json: dir.join("siglip2_naflex_tokenizer.json"),
        },
        aesthetic: lrg_ml::aesthetic::AestheticPaths {
            onnx: dir.join("aesthetic_siglip.onnx"),
        },
        face: lrg_ml::faces::FaceModelPaths {
            det_onnx: dir.join("yunet_face_detection.onnx"),
            rec_onnx: dir.join("facenet_vggface2.onnx"),
            auraface_onnx: dir.join("auraface_glintr100.onnx"),
        },
        face_state: lrg_ml::face_state::FaceStatePaths {
            mesh_onnx: dir.join("face_mesh_v2.onnx"),
            blendshapes_onnx: dir.join("face_blendshapes_v2.onnx"),
        },
        subject: lrg_ml::subject::SubjectPaths {
            onnx: dir.join("subject_rfdetr_seg_nano.onnx"),
        },
        bioclip: lrg_ml::bioclip::BioclipModelPaths {
            image_onnx: dir.join("bioclip2_image.onnx"),
            taxa_bin: dir.join("bioclip2_taxa.bin"),
            taxa_json: dir.join("bioclip2_taxa.json"),
        },
    }
}

/// Answers with the turns it was given, in order, and keeps every request.
struct Scripted {
    turns: Mutex<VecDeque<ChatTurn>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl Scripted {
    fn new(turns: Vec<ChatTurn>) -> Arc<Scripted> {
        Arc::new(Scripted {
            turns: Mutex::new(turns.into()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChatModel for Scripted {
    fn name(&self) -> String {
        "scripted".into()
    }
    fn context_window(&self) -> Option<u32> {
        None
    }
    fn supports_images(&self) -> bool {
        true
    }
    async fn complete(
        &self,
        request: &ChatRequest,
        _deltas: Option<ChatDeltas>,
    ) -> Result<ChatTurn, String> {
        self.requests.lock().unwrap().push(request.clone());
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "the script ran out".to_string())
    }
}

fn call(id: &str, name: &str, arguments: Value) -> ChatTurn {
    ChatTurn {
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        }],
        input_tokens: 100,
        output_tokens: 20,
        ..ChatTurn::default()
    }
}

fn jpeg(level: u8) -> Vec<u8> {
    let img = image::RgbImage::from_fn(64, 48, |x, _| {
        image::Rgb([level.saturating_add((x % 16) as u8); 3])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .unwrap();
    out.into_inner()
}

fn request(model: Arc<Scripted>, style: Option<Value>, masks: bool) -> StartRequest {
    StartRequest {
        photo_id: "photo-1".into(),
        filename: Some("IMG_1.CR3".into()),
        image: jpeg(60),
        develop_settings: Some(json!({
            "ProcessVersion": "15.4",
            "WhiteBalance": "As Shot",
            "Temperature": 5100,
            "Tint": 4,
            "Exposure2012": 0.2,
        })),
        is_raw: Some(true),
        setup: LoopSetup {
            is_raw: None,
            masks,
            language: "English".into(),
            look: "moody".into(),
            instructions: None,
            max_edits: 3,
            from_style: false,
        },
        style_recipe: style,
        model,
        fields: HashMap::new(),
    }
}

fn state() -> Arc<AppState> {
    Arc::new(AppState::with_model_paths(None, false, models_absent()))
}

#[tokio::test]
async fn edit_render_refine_finish() {
    let state = state();
    let model = Scripted::new(vec![
        call(
            "c1",
            "apply_edit",
            json!({
                "assessment": "Dark and flat; lift and give the sky depth.",
                "global": {"exposure": 0.6, "highlights": -40, "temperature": 5600},
                "masks": [{"target": "sky", "adjustments": {"exposure": -0.3}}]
            }),
        ),
        call(
            "c2",
            "apply_edit",
            json!({
                "assessment": "A touch too bright now.",
                "global": {"exposure": 0.45, "highlights": -40, "temperature": 5600}
            }),
        ),
        call(
            "c3",
            "finish",
            json!({"verdict": "Round 1 had the better sky.", "best_round": 1}),
        ),
    ]);

    let (id, first) = start(&state, request(model.clone(), None, true))
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(first.round, 1);
    assert!(!first.done);
    assert_eq!(first.source, "model");
    assert_eq!(first.develop_settings["Exposure2012"], json!(0.6));
    assert_eq!(first.develop_settings["Temperature"], json!(5600));
    assert_eq!(first.develop_settings["WhiteBalance"], json!("Custom"));
    assert_eq!(
        first.develop_settings["MaskGroupBasedCorrections"][0]["CorrectionName"],
        json!("LrGenius · Sky")
    );
    assert!(first.update_ai_settings);

    // The model saw the photo, its starting values and its measurements.
    let opening = &model.requests()[0];
    assert!(opening.system.contains("Moody"));
    assert!(opening.system.contains("Masks:"));
    let parts = &opening.messages[0].parts;
    assert!(matches!(parts[0], ChatPart::Image(_)));
    let ChatPart::Text(text) = &parts[1] else {
        panic!("the opening has text after the image")
    };
    assert!(text.contains("It is a raw file."), "{text}");
    assert!(text.contains("exposure +0.20"), "{text}");
    assert!(text.contains("temperature 5100"), "{text}");
    assert!(text.contains("Luminance (0-100)"), "{text}");

    let session = state.edit_sessions.get(&id).unwrap();
    let mut session = session.lock().await;
    let second = render(
        &state,
        &mut session,
        &jpeg(120),
        vec!["The sky mask found nothing in this photo.".into()],
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(second.round, 2);
    assert_eq!(second.develop_settings["Exposure2012"], json!(0.45));
    assert!(second
        .develop_settings
        .get("MaskGroupBasedCorrections")
        .is_none());

    // The render went back as the result of the first call, with an image
    // and the comparison against the original.
    let after_render = &model.requests()[1];
    let last = after_render.messages.last().unwrap();
    assert_eq!(last.role, ChatRole::User);
    let ChatPart::ToolResult(result) = &last.parts[0] else {
        panic!("the render answers the tool call")
    };
    assert_eq!(result.call_id, "c1");
    assert_eq!(result.images.len(), 1);
    assert!(
        result.text.contains("Against the original: median"),
        "{}",
        result.text
    );
    assert!(
        result.text.contains("- The sky mask found nothing"),
        "{}",
        result.text
    );
    assert!(
        result.text.contains("2 more edits allowed"),
        "{}",
        result.text
    );

    let done = render(&state, &mut session, &jpeg(140), Vec::new())
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert!(done.done);
    assert_eq!(done.keep_round, Some(1));
    assert_eq!(done.verdict.as_deref(), Some("Round 1 had the better sky."));
    assert_eq!(done.recipe["global"]["exposure"], json!(0.6));
    let (persisted, _) = session.result(1, done.verdict.as_deref()).unwrap();
    assert_eq!(persisted["summary"], json!("Round 1 had the better sky."));
}

#[tokio::test]
async fn the_style_is_round_zero_and_the_model_sees_both_images() {
    let state = state();
    let model = Scripted::new(vec![
        call(
            "c1",
            "apply_edit",
            json!({"assessment": "Keep the style, open the face.",
                   "global": {"exposure": 0.3, "contrast": 12}}),
        ),
        call("c2", "finish", json!({"verdict": "Done."})),
    ]);
    let style = json!({"global": {"contrast": 12, "vibrance": 10},
                       "white_balance": {"mode": "Custom", "family": "raw",
                                         "temperature": 5300, "tint": 6},
                       "masks": []});

    let (id, zero) = start(&state, request(model.clone(), Some(style), false))
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(zero.round, 0);
    assert_eq!(zero.source, "style");
    assert_eq!(zero.develop_settings["Vibrance"], json!(10));
    assert_eq!(zero.develop_settings["Temperature"], json!(5300));
    assert!(
        model.requests().is_empty(),
        "round 0 needs no model, only Lightroom"
    );

    let session = state.edit_sessions.get(&id).unwrap();
    let mut session = session.lock().await;
    let first = render(&state, &mut session, &jpeg(90), Vec::new())
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(first.round, 1);
    // The model sent exposure and contrast only; the style's vibrance stays.
    assert_eq!(first.develop_settings["Exposure2012"], json!(0.3));
    assert_eq!(first.develop_settings["Vibrance"], json!(10));
    assert_eq!(first.develop_settings["Temperature"], json!(5300));
    let opening = &model.requests()[0];
    assert!(opening.system.contains("photographer's own style"));
    let ChatPart::Text(style_text) = &opening.messages[0].parts[3] else {
        panic!("the style's render is followed by its text")
    };
    assert!(
        style_text.contains("The edit this render shows:\n") && style_text.contains("vibrance 10"),
        "{style_text}"
    );
    let images = opening.messages[0]
        .parts
        .iter()
        .filter(|p| matches!(p, ChatPart::Image(_)))
        .count();
    assert_eq!(images, 2, "the original and the style's render");

    let done = render(&state, &mut session, &jpeg(100), Vec::new())
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert!(done.done);
    assert_eq!(done.keep_round, Some(1));
}

#[tokio::test]
async fn a_model_that_will_not_call_a_tool_is_nudged_then_refused() {
    let state = state();
    let chatter = || ChatTurn {
        text: "This photo looks lovely.".into(),
        ..ChatTurn::default()
    };
    let model = Scripted::new(vec![chatter(), chatter(), chatter()]);
    let Err(e) = start(&state, request(model.clone(), None, false)).await else {
        panic!("a model without tool calls cannot edit")
    };
    assert!(
        e.message.contains("did not call apply_edit"),
        "{}",
        e.message
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    let ChatPart::Text(nudge) = &requests[1].messages.last().unwrap().parts[0] else {
        panic!("the nudge is text")
    };
    assert!(nudge.contains("apply_edit"));
}

#[tokio::test]
async fn an_edit_past_the_budget_is_refused_and_finish_is_asked_for() {
    let state = state();
    let mut turns = vec![call(
        "c1",
        "apply_edit",
        json!({"assessment": "a", "global": {"exposure": 0.1}}),
    )];
    turns.push(call(
        "c2",
        "apply_edit",
        json!({"assessment": "b", "global": {"exposure": 0.2}}),
    ));
    turns.push(call("c3", "finish", json!({"verdict": "ok"})));
    let model = Scripted::new(turns);
    let mut req = request(model.clone(), None, false);
    req.setup.max_edits = 1;

    let (id, first) = start(&state, req)
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(first.round, 1);
    let session = state.edit_sessions.get(&id).unwrap();
    let mut session = session.lock().await;
    let done = render(&state, &mut session, &jpeg(90), Vec::new())
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert!(done.done);
    assert_eq!(done.keep_round, Some(1));
    let refused = &model.requests()[2];
    let ChatPart::ToolResult(result) = &refused.messages.last().unwrap().parts[0] else {
        panic!("the refusal answers the call")
    };
    assert!(result.is_error);
    assert!(result.text.contains("No edits are left"));
}

#[tokio::test]
async fn a_jpeg_gets_an_offset_white_balance() {
    let state = state();
    let model = Scripted::new(vec![call(
        "c1",
        "apply_edit",
        json!({"assessment": "Cool it down.", "global": {"temperature": -12, "tint": 3}}),
    )]);
    let mut req = request(model.clone(), None, false);
    req.develop_settings = Some(json!({
        "ProcessVersion": "15.4",
        "WhiteBalance": "As Shot",
        "IncrementalTemperature": 0,
        "IncrementalTint": 0,
    }));
    req.is_raw = Some(false);
    let (_, first) = start(&state, req)
        .await
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(first.develop_settings["IncrementalTemperature"], json!(-12));
    assert!(first.develop_settings.get("Temperature").is_none());
    let schema = &model.requests()[0].tools[0].input_schema;
    assert_eq!(
        schema["properties"]["global"]["properties"]["temperature"]["maximum"],
        json!(100.0)
    );
}

#[tokio::test]
async fn edits_lightroom_cannot_use_are_named_as_the_cause() {
    let state = state();
    let unusable = || {
        call(
            "c",
            "apply_edit",
            json!({"assessment": "Brighten the moon.", "global": {},
                   "masks": [{"target": "moon", "adjustments": {"exposure": 1.0}}]}),
        )
    };
    let model = Scripted::new(vec![unusable(), unusable(), unusable()]);
    let Err(e) = start(&state, request(model.clone(), None, true)).await else {
        panic!("no usable edit, no session")
    };
    assert!(
        e.message.contains("Lightroom has no AI mask by that name"),
        "{}",
        e.message
    );
    assert!(!e.message.contains("tool calling"), "{}", e.message);
    // The model heard why each time.
    let requests = model.requests();
    let ChatPart::ToolResult(result) = &requests[1].messages.last().unwrap().parts[0] else {
        panic!("the rejection answers the call")
    };
    assert!(result.is_error);
    assert!(result.text.contains("moon"), "{}", result.text);
}
