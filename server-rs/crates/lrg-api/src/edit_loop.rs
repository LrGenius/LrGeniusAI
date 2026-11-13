//! AI Edit's retoucher loop, backend side: one session per photo.
//!
//! The loop needs Lightroom between its rounds — only Lightroom can apply a
//! develop table and render the result — so the backend holds the
//! conversation and the plugin drives it:
//!
//! 1. `start`: the plugin uploads the photo as it is and its develop
//!    settings. The backend measures it, opens the conversation and answers
//!    with the first edit as a table for `applyDevelopSettings` (or, starting
//!    from the photographer's style, with that style as round 0).
//! 2. `render`: the plugin applied it, rendered the photo and uploads the
//!    render. The model sees it with its measurements and either edits again
//!    or finishes.
//! 3. The plugin ends the session when it is done with it; an abandoned one
//!    expires.
//!
//! The model's tools and prompt are [`lrg_providers::edit_loop`]; the recipe
//! becomes Lightroom keys through [`lrg_develop::build::recipe`] and the Lua
//! writer, after the same guardrails every AI Edit recipe passes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::Mutex;

use lrg_analysis::edit_guardrails::CaptureConditions;
use lrg_analysis::style_engine::white_balance_into_global;
use lrg_develop::build::recipe::settings_from_recipe;
use lrg_develop::lua::{to_lua_value, LuaMode, LuaOptions, PhotoContext};
use lrg_develop::{DevelopSettings, FileKind, FileKindHint};
use lrg_imaging::tone::{measure, ToneStats};
use lrg_providers::chat::{
    ChatImage, ChatMessage, ChatModel, ChatPart, ChatRequest, ChatRole, ToolCall, ToolResult,
    ToolSpec,
};
use lrg_providers::edit_loop::{
    against_original, apply_edit_tool, finish_tool, opening_text, recipe_from_arguments,
    render_text, starting_text, system_prompt, EditArguments, LoopSetup, APPLY_EDIT, FINISH, NUDGE,
};

use crate::edit_budget::{measure_and_apply, FaceReaders};
use crate::state::AppState;

/// A session nobody has touched for this long is dropped: the plugin that
/// drove it is gone.
const IDLE_LIMIT: Duration = Duration::from_secs(30 * 60);

/// Long edge of every image the model sees. Larger costs tokens on every
/// turn and shows the model nothing an edit decision needs.
const MODEL_IMAGE_EDGE: u32 = 1536;

/// Tokens one answer may use. An edit with masks and colour grading is a
/// few hundred; the rest is room for a model that reasons first.
const MAX_ANSWER_TOKENS: u32 = 4096;

/// Low, for steady values from one round to the next; not zero, which makes
/// some models repeat their previous call verbatim.
const TEMPERATURE: f32 = 0.3;

/// Answers the loop asks for before it gives up on a model that will not
/// call a tool: the first, one nudge, one more.
const MAX_TURNS_PER_STEP: usize = 3;

/// The edits a session allows at most, whatever the plugin asks.
pub const MAX_EDITS_CAP: u32 = 5;

/// What the user is told when the backend itself failed on an edit; the
/// cause is in the log.
const INTERNAL_ERROR: &str = "An edit could not be prepared for Lightroom because of an error in \
     the LrGeniusAI backend. Please report it on GitHub with the backend log.";

/// The sessions in flight, by id.
#[derive(Default)]
pub struct EditSessions {
    sessions: StdMutex<HashMap<String, Arc<Mutex<EditSession>>>>,
}

impl EditSessions {
    fn purge(map: &mut HashMap<String, Arc<Mutex<EditSession>>>) {
        map.retain(|_, s| match s.try_lock() {
            Ok(s) => s.last_used.elapsed() < IDLE_LIMIT,
            // In use right now, so not idle.
            Err(_) => true,
        });
    }

    fn insert(&self, session: EditSession) -> String {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut map = self.sessions.lock().expect("edit sessions lock");
        Self::purge(&mut map);
        map.insert(id.clone(), Arc::new(Mutex::new(session)));
        id
    }

    pub fn get(&self, id: &str) -> Option<Arc<Mutex<EditSession>>> {
        let mut map = self.sessions.lock().expect("edit sessions lock");
        Self::purge(&mut map);
        map.get(id).cloned()
    }

    pub fn remove(&self, id: &str) -> bool {
        self.sessions
            .lock()
            .expect("edit sessions lock")
            .remove(id)
            .is_some()
    }
}

/// One edit the session handed out, kept so a later `best_round` can point
/// back at it.
#[derive(Clone)]
struct Round {
    /// 0 for the photographer's style, then 1, 2, ... for the model's edits,
    /// as the model is told them.
    number: u32,
    recipe: Value,
    assessment: Option<String>,
    /// What did not reach the photo as given, told to the model with the
    /// render.
    notes: Vec<String>,
}

/// What the session waits for: the render of the edit it handed out last.
enum Pending {
    /// Round 0, the photographer's style: no model turn yet. The opening
    /// message goes out with this render.
    Style { opening: String },
    /// An `apply_edit` call to answer with the render. `others` are any
    /// further calls in the same turn, answered as not taken.
    Edit {
        call: ToolCall,
        others: Vec<ToolCall>,
    },
    /// Nothing: the session has finished.
    Nothing,
}

pub struct EditSession {
    model: Arc<dyn ChatModel>,
    system: String,
    tools: Vec<ToolSpec>,
    messages: Vec<ChatMessage>,
    setup: LoopSetup,
    photo: PhotoContext,
    /// The photo as uploaded at the start, for the guardrails.
    original_bytes: Vec<u8>,
    /// The same, at the size the model sees.
    original_image: ChatImage,
    original_tones: ToneStats,
    capture: CaptureConditions,
    pending: Pending,
    /// Round 0 (the style) when there is one, then every edit in order.
    rounds: Vec<Round>,
    /// Edits the model applied (round 0 not counted).
    edits: u32,
    last_used: Instant,
    pub photo_id: String,
    pub filename: Option<String>,
    /// The start request's fields, for persisting the finished recipe.
    fields: HashMap<String, String>,
    input_tokens: u64,
    output_tokens: u64,
}

/// One answer to the plugin.
pub struct Step {
    pub round: u32,
    /// `"style"` or `"model"`.
    pub source: &'static str,
    pub done: bool,
    pub recipe: Value,
    pub develop_settings: Value,
    pub update_ai_settings: bool,
    pub assessment: Option<String>,
    pub verdict: Option<String>,
    /// The round to keep when the session is done: the latest unless the
    /// model named an earlier one.
    pub keep_round: Option<u32>,
    /// What did not reach the photo as given, and other notes for the user.
    pub warnings: Vec<String>,
    pub guardrail_reasons: Vec<String>,
}

impl Step {
    pub fn to_json(&self, session_id: &str, session: &EditSession) -> Value {
        json!({
            "status": "success",
            "session_id": session_id,
            "photo_id": session.photo_id,
            "round": self.round,
            "source": self.source,
            "done": self.done,
            "edit": self.recipe,
            "develop_settings": self.develop_settings,
            "update_ai_settings": self.update_ai_settings,
            "assessment": self.assessment,
            "verdict": self.verdict,
            "keep_round": self.keep_round,
            "edits_left": session.setup.max_edits.saturating_sub(session.edits),
            "warnings": self.warnings,
            "guardrail_reasons": self.guardrail_reasons,
            "guardrail_explanations": crate::routes::edit::guardrail_explanations(&self.guardrail_reasons),
            "input_tokens": session.input_tokens,
            "output_tokens": session.output_tokens,
        })
    }
}

/// What a session is started with.
pub struct StartRequest {
    pub photo_id: String,
    pub filename: Option<String>,
    pub image: Vec<u8>,
    /// `photo:getDevelopSettings()` as JSON.lua encoded it, or `None`.
    pub develop_settings: Option<Value>,
    /// The plugin's guess, used only when the settings carry no
    /// white-balance keys.
    pub is_raw: Option<bool>,
    pub setup: LoopSetup,
    /// The photographer's style for round 0: the `edit` of a
    /// `/v1/edit/style` answer.
    pub style_recipe: Option<Value>,
    pub model: Arc<dyn ChatModel>,
    pub fields: HashMap<String, String>,
}

/// Why a step failed, for the HTTP answer.
pub struct StepError {
    pub status: axum::http::StatusCode,
    pub message: String,
}

impl StepError {
    fn bad_request(message: impl Into<String>) -> StepError {
        StepError {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn model(message: impl Into<String>) -> StepError {
        StepError {
            status: axum::http::StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }
}

/// An uploaded JPEG at the model's size, and its tones.
fn prepare_image(bytes: &[u8]) -> Result<(ChatImage, ToneStats), String> {
    let decoded =
        image::load_from_memory(bytes).map_err(|e| format!("could not decode the image: {e}"))?;
    let rgb = decoded.to_rgb8();
    let tones = measure(rgb.as_raw(), rgb.width() as usize, rgb.height() as usize)
        .ok_or_else(|| "the image is empty".to_string())?;
    let long_edge = rgb.width().max(rgb.height());
    let data = if long_edge <= MODEL_IMAGE_EDGE {
        bytes.to_vec()
    } else {
        let resized = image::DynamicImage::ImageRgb8(rgb).resize(
            MODEL_IMAGE_EDGE,
            MODEL_IMAGE_EDGE,
            image::imageops::FilterType::Triangle,
        );
        let mut out = std::io::Cursor::new(Vec::new());
        resized
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .map_err(|e| format!("could not encode the image: {e}"))?;
        out.into_inner()
    };
    Ok((
        ChatImage {
            data,
            mime_type: "image/jpeg".to_string(),
        },
        tones,
    ))
}

/// Turns a recipe into the table Lightroom takes, after the guardrails.
struct Built {
    recipe: Value,
    table: Value,
    update_ai_settings: bool,
    /// What did not reach the photo as given, for the user and the model.
    warnings: Vec<String>,
    /// The guardrails' explanations: for the model with the render, and for
    /// the user only through `guardrail_explanations`, where the review shows
    /// them under "What the frame allowed" — as warnings they would be
    /// counted for every photo of a run and push real problems out of the
    /// summary.
    frame_notes: Vec<String>,
    guardrail_reasons: Vec<String>,
}

impl Built {
    /// Everything the model should hear about this edit.
    fn model_notes(&self) -> Vec<String> {
        self.frame_notes
            .iter()
            .chain(&self.warnings)
            .cloned()
            .collect()
    }
}

async fn build(state: &AppState, session: &EditSession, mut recipe: Value) -> Built {
    let bytes = session.original_bytes.clone();
    let capture = session.capture;
    let faces = state.face.clone();
    let face_state = state.face_state.clone();
    let guarded = tokio::task::spawn_blocking(move || {
        let applied = measure_and_apply(
            &mut recipe,
            &bytes,
            &capture,
            FaceReaders::when_downloaded(&faces, &face_state),
        );
        (recipe, applied)
    })
    .await;
    let (recipe, applied) = match guarded {
        Ok(result) => result,
        Err(e) => {
            log::error!("Edit loop guardrail task failed: {e}");
            return Built {
                recipe: Value::Null,
                table: json!({}),
                update_ai_settings: false,
                warnings: vec![INTERNAL_ERROR.to_string()],
                frame_notes: Vec::new(),
                guardrail_reasons: Vec::new(),
            };
        }
    };
    let mut warnings: Vec<String> = applied.warning.into_iter().collect();
    let built = settings_from_recipe(&recipe, session.photo.file_kind);
    warnings.extend(built.notes);
    let (table, update_ai_settings) = match to_lua_value(
        &built.settings,
        &LuaMode::Apply(session.photo.clone()),
        &LuaOptions::default(),
    ) {
        Ok(written) => {
            warnings.extend(written.skipped.iter().map(|s| s.sentence()));
            (written.table, written.call.update_ai_settings)
        }
        Err(e) => {
            log::error!("Edit loop: the develop table could not be written: {e}");
            warnings.push(INTERNAL_ERROR.to_string());
            (json!({}), false)
        }
    };
    Built {
        recipe,
        table,
        update_ai_settings,
        warnings,
        frame_notes: crate::routes::edit::guardrail_explanations(&applied.reasons),
        guardrail_reasons: applied.reasons,
    }
}

fn is_empty_table(table: &Value) -> bool {
    table.as_object().is_none_or(|m| m.is_empty())
}

/// Opens a session and returns its id with the first step.
pub async fn start(state: &AppState, request: StartRequest) -> Result<(String, Step), StepError> {
    if !request.model.supports_images() {
        return Err(StepError::bad_request(format!(
            "{} cannot see images, and AI Edit has to look at the photo. Choose a vision model.",
            request.model.name()
        )));
    }
    let (original_image, original_tones) =
        prepare_image(&request.image).map_err(StepError::bad_request)?;

    let hint = match request.is_raw {
        Some(true) => FileKindHint::Raw,
        Some(false) => FileKindHint::NonRaw,
        None => FileKindHint::Unknown,
    };
    let current = match &request.develop_settings {
        Some(blob) => match lrg_develop::lua::from_lua_value(blob, hint) {
            Ok((settings, _)) => settings,
            Err(e) => {
                return Err(StepError::bad_request(format!(
                    "the photo's develop settings could not be read: {e}"
                )))
            }
        },
        None => DevelopSettings::default(),
    };
    let file_kind = current.file_kind.or(match request.is_raw {
        Some(true) => Some(FileKind::Raw),
        Some(false) => Some(FileKind::NonRaw),
        None => None,
    });
    let photo = PhotoContext {
        file_kind,
        process_version: current.process_version(),
        camera: None,
    };
    let mut setup = request.setup;
    setup.is_raw = file_kind.map(|k| k == FileKind::Raw);
    setup.max_edits = setup.max_edits.clamp(1, MAX_EDITS_CAP);
    setup.from_style = request.style_recipe.is_some();

    let capture = CaptureConditions {
        iso: lrg_imaging::capture::read_iso(&request.image),
        is_raw: file_kind == Some(FileKind::Raw),
    };
    let frame_notes = frame_notes(state, &request.image, &capture).await;
    let opening = opening_text(
        &starting_text(&current),
        &original_tones.describe(),
        &frame_notes,
        &setup,
    );

    let mut session = EditSession {
        model: request.model,
        system: system_prompt(&setup),
        tools: vec![apply_edit_tool(&setup), finish_tool()],
        messages: Vec::new(),
        setup,
        photo,
        original_bytes: request.image,
        original_image,
        original_tones,
        capture,
        pending: Pending::Nothing,
        rounds: Vec::new(),
        edits: 0,
        last_used: Instant::now(),
        photo_id: request.photo_id,
        filename: request.filename,
        fields: request.fields,
        input_tokens: 0,
        output_tokens: 0,
    };

    let step = if let Some(style) = request.style_recipe {
        let (style, mut notes) = white_balance_into_global(style, file_kind);
        let built = build(state, &session, style).await;
        notes.extend(built.warnings.iter().cloned());
        if is_empty_table(&built.table) {
            // Nothing to show the model as round 0: go straight to the model.
            session.setup.from_style = false;
            session.system = system_prompt(&session.setup);
            let opening = opening_text(
                &starting_text(&current),
                &session.original_tones.describe(),
                &frame_notes,
                &session.setup,
            );
            session
                .messages
                .push(opening_message(&session, &opening, None));
            let mut step = advance(state, &mut session).await?;
            step.warnings.splice(0..0, notes);
            step
        } else {
            session.rounds.push(Round {
                number: 0,
                recipe: built.recipe.clone(),
                assessment: None,
                notes: Vec::new(),
            });
            session.pending = Pending::Style { opening };
            Step {
                round: 0,
                source: "style",
                done: false,
                recipe: built.recipe,
                develop_settings: built.table,
                update_ai_settings: built.update_ai_settings,
                assessment: None,
                verdict: None,
                keep_round: None,
                warnings: notes,
                guardrail_reasons: built.guardrail_reasons,
            }
        }
    } else {
        session
            .messages
            .push(opening_message(&session, &opening, None));
        advance(state, &mut session).await?
    };

    let id = state.edit_sessions.insert(session);
    Ok((id, step))
}

/// The first user message: the original, what the model works from, and,
/// from a style, the style's render.
fn opening_message(
    session: &EditSession,
    opening: &str,
    style: Option<(ChatImage, String)>,
) -> ChatMessage {
    let mut parts = vec![
        ChatPart::Image(session.original_image.clone()),
        ChatPart::Text(opening.to_string()),
    ];
    if let Some((image, text)) = style {
        parts.push(ChatPart::Image(image));
        parts.push(ChatPart::Text(text));
    }
    ChatMessage {
        role: ChatRole::User,
        parts,
    }
}

/// What the frame allows, found by asking the guardrails about an edit that
/// pushes every capped slider to the end of its range.
async fn frame_notes(state: &AppState, image: &[u8], capture: &CaptureConditions) -> Vec<String> {
    let bytes = image.to_vec();
    let capture = *capture;
    let faces = state.face.clone();
    let face_state = state.face_state.clone();
    let probed = tokio::task::spawn_blocking(move || {
        let mut probe = json!({"global": {
            "contrast": 100, "clarity": 100, "dehaze": 100, "shadows": 100, "whites": 100
        }});
        let applied = measure_and_apply(
            &mut probe,
            &bytes,
            &capture,
            FaceReaders::when_downloaded(&faces, &face_state),
        );
        (probe, applied.reasons)
    })
    .await;
    let Ok((probe, reasons)) = probed else {
        return Vec::new();
    };
    let mut notes = crate::routes::edit::guardrail_explanations(&reasons);
    let caps: Vec<String> = ["contrast", "clarity", "dehaze", "shadows", "whites"]
        .iter()
        .filter_map(|k| {
            let v = probe["global"][k].as_f64()?;
            (v < 100.0).then(|| format!("{k} at most {v:.0}"))
        })
        .collect();
    if !caps.is_empty() {
        notes.push(format!("Upper limits for this frame: {}.", caps.join(", ")));
    }
    notes
}

/// Takes the render of the pending edit and asks the model what comes next.
///
/// `lightroom_notes` are what the plugin saw while applying the edit — an AI
/// mask that found nothing, say — and reach the model with the render.
pub async fn render(
    state: &AppState,
    session: &mut EditSession,
    image: &[u8],
    lightroom_notes: Vec<String>,
) -> Result<Step, StepError> {
    session.last_used = Instant::now();
    let (render_image, tones) = prepare_image(image).map_err(StepError::bad_request)?;
    let comparison = against_original(&session.original_tones, &tones);
    let left = session.setup.max_edits.saturating_sub(session.edits);

    match std::mem::replace(&mut session.pending, Pending::Nothing) {
        Pending::Nothing => {
            return Err(StepError::bad_request(
                "this edit session is not waiting for a render",
            ))
        }
        Pending::Style { opening } => {
            let text = render_text(0, &tones.describe(), &comparison, &lightroom_notes, left);
            let message = opening_message(session, &opening, Some((render_image, text)));
            session.messages.push(message);
        }
        Pending::Edit { call, others } => {
            let mut notes = session
                .rounds
                .last()
                .map(|r| r.notes.clone())
                .unwrap_or_default();
            notes.extend(lightroom_notes);
            let text = render_text(session.edits, &tones.describe(), &comparison, &notes, left);
            let mut parts = vec![ChatPart::ToolResult(ToolResult {
                call_id: call.id,
                name: call.name,
                text,
                images: vec![render_image],
                is_error: false,
            })];
            parts.extend(others.into_iter().map(not_taken));
            session.messages.push(ChatMessage {
                role: ChatRole::User,
                parts,
            });
        }
    }
    advance(state, session).await
}

fn not_taken(call: ToolCall) -> ChatPart {
    ChatPart::ToolResult(ToolResult {
        call_id: call.id,
        name: call.name,
        text: "Not done: one tool call per turn. The first call of the turn was taken.".to_string(),
        images: Vec::new(),
        is_error: true,
    })
}

fn tool_error(call: &ToolCall, text: &str) -> ChatPart {
    ChatPart::ToolResult(ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        text: text.to_string(),
        images: Vec::new(),
        is_error: true,
    })
}

/// The step that ends the session, keeping `keep_round` when it names a
/// round the session had and the latest round otherwise.
fn finished(
    session: &mut EditSession,
    verdict: Option<String>,
    keep_round: Option<u32>,
    warnings: Vec<String>,
) -> Step {
    session.pending = Pending::Nothing;
    let kept = keep_round
        .and_then(|n| session.rounds.iter().find(|r| r.number == n))
        .or_else(|| session.rounds.last());
    Step {
        round: kept.map_or(0, |r| r.number),
        source: "model",
        done: true,
        recipe: kept.map_or(Value::Null, |r| r.recipe.clone()),
        develop_settings: json!({}),
        update_ai_settings: false,
        assessment: kept.and_then(|r| r.assessment.clone()),
        verdict,
        keep_round: kept.map(|r| r.number),
        warnings,
        guardrail_reasons: Vec::new(),
    }
}

/// Asks the model for its next move and turns the answer into a step.
async fn advance(state: &AppState, session: &mut EditSession) -> Result<Step, StepError> {
    let mut warnings = Vec::new();
    // Why edits the model sent this step did not reach Lightroom at all.
    let mut rejected: Vec<String> = Vec::new();
    for _ in 0..MAX_TURNS_PER_STEP {
        let request = ChatRequest {
            system: session.system.clone(),
            messages: session.messages.clone(),
            tools: session.tools.clone(),
            max_tokens: MAX_ANSWER_TOKENS,
            temperature: TEMPERATURE,
        };
        let turn = session
            .model
            .complete(&request, None)
            .await
            .map_err(|e| StepError::model(format!("{}: {e}", session.model.name())))?;
        session.input_tokens += u64::from(turn.input_tokens);
        session.output_tokens += u64::from(turn.output_tokens);
        session.messages.push(turn.to_message());

        let mut calls = turn.tool_calls.into_iter();
        let Some(call) = calls.next() else {
            if session.edits == 0 {
                session.messages.push(ChatMessage::user_text(NUDGE));
                continue;
            }
            warnings.push(
                "The AI model stopped without saying it was done; the latest edit was kept."
                    .to_string(),
            );
            return Ok(finished(session, None, None, warnings));
        };
        let others: Vec<ToolCall> = calls.collect();

        match call.name.as_str() {
            FINISH if session.edits > 0 => {
                let verdict = call
                    .arguments
                    .get("verdict")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let keep = call
                    .arguments
                    .get("best_round")
                    .and_then(Value::as_u64)
                    .map(|r| r as u32);
                return Ok(finished(session, verdict, keep, warnings));
            }
            FINISH => {
                let mut parts = vec![tool_error(
                    &call,
                    "Nothing has been applied yet. Call apply_edit with your edit first.",
                )];
                parts.extend(others.into_iter().map(not_taken));
                session.messages.push(ChatMessage {
                    role: ChatRole::User,
                    parts,
                });
            }
            APPLY_EDIT if session.edits >= session.setup.max_edits => {
                let mut parts = vec![tool_error(&call, "No edits are left. Call finish.")];
                parts.extend(others.into_iter().map(not_taken));
                session.messages.push(ChatMessage {
                    role: ChatRole::User,
                    parts,
                });
            }
            APPLY_EDIT => {
                let EditArguments {
                    recipe,
                    assessment,
                    notes: argument_notes,
                } = recipe_from_arguments(&call.arguments);
                let mut built = build(state, session, recipe).await;
                built.warnings.splice(0..0, argument_notes);
                if is_empty_table(&built.table) {
                    let mut text = "Nothing in this edit reaches Lightroom.".to_string();
                    if !built.warnings.is_empty() {
                        text.push_str(&format!(" {}", built.warnings.join(" ")));
                    }
                    // Kept for the user: if the model never gets an edit
                    // through, these are why, not its tool calling.
                    rejected.extend(built.warnings.iter().cloned());
                    let mut parts = vec![tool_error(&call, &text)];
                    parts.extend(others.into_iter().map(not_taken));
                    session.messages.push(ChatMessage {
                        role: ChatRole::User,
                        parts,
                    });
                    continue;
                }
                session.edits += 1;
                session.rounds.push(Round {
                    number: session.edits,
                    recipe: built.recipe.clone(),
                    assessment: assessment.clone(),
                    notes: built.model_notes(),
                });
                session.pending = Pending::Edit { call, others };
                warnings.extend(built.warnings);
                return Ok(Step {
                    round: session.edits,
                    source: "model",
                    done: false,
                    recipe: built.recipe,
                    develop_settings: built.table,
                    update_ai_settings: built.update_ai_settings,
                    assessment,
                    verdict: None,
                    keep_round: None,
                    warnings,
                    guardrail_reasons: built.guardrail_reasons,
                });
            }
            other => {
                let mut parts = vec![tool_error(
                    &call,
                    &format!("There is no tool named {other}. Use apply_edit or finish."),
                )];
                parts.extend(others.into_iter().map(not_taken));
                session.messages.push(ChatMessage {
                    role: ChatRole::User,
                    parts,
                });
            }
        }
    }
    rejected.dedup();
    if session.edits > 0 {
        warnings.push(if rejected.is_empty() {
            "The AI model did not answer with an edit or a finish; the latest edit was kept."
                .to_string()
        } else {
            format!(
                "The AI model's last edits could not be applied ({}); the latest edit that could \
                 was kept.",
                rejected.join(" ")
            )
        });
        return Ok(finished(session, None, None, warnings));
    }
    Err(StepError::model(if rejected.is_empty() {
        format!(
            "{} did not call apply_edit after {MAX_TURNS_PER_STEP} answers. Models without tool \
             calling cannot run AI Edit; choose another model.",
            session.model.name()
        )
    } else {
        format!(
            "{} sent edits Lightroom could not use: {} Try again, or choose a larger model.",
            session.model.name(),
            rejected.join(" ")
        )
    }))
}

impl EditSession {
    /// Round `number`'s recipe with the model's words on it as `summary`,
    /// and the fields the session was started with: what persisting the
    /// finished edit needs.
    pub fn result(
        &self,
        number: u32,
        verdict: Option<&str>,
    ) -> Option<(Value, HashMap<String, String>)> {
        let round = self.rounds.iter().find(|r| r.number == number)?;
        let mut recipe = round.recipe.clone();
        if let Some(object) = recipe.as_object_mut() {
            if let Some(summary) = verdict.or(round.assessment.as_deref()) {
                object.insert("summary".to_string(), json!(summary));
            }
        }
        Some((recipe, self.fields.clone()))
    }
}
