//! Tools that act in Lightroom, through the plugin and the agent bridge:
//! catalog queries, photo details and previews, collections, metadata,
//! develop settings and indexing. Every one of them needs the plugin's AI
//! Agent Bridge running; the ones that change photos also need the user to
//! allow changes.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, ErrorData as McpError, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use lrg_api::agent_bridge::CallOptions;
use lrg_api::routes::search;

use crate::server::{
    bridge_error, json_result, round2, string_list, tool_error, truncate, with_warnings,
    LrGeniusMcp, Photos, MAX_PHOTOS_PER_CALL,
};

/// Previews come back as a JPEG file the plugin rendered. Anything bigger
/// than this is not a preview.
const MAX_PREVIEW_BYTES: u64 = 25 * 1024 * 1024;

/// Most photos `get_photo_details` describes in one call.
const MAX_DETAILS: usize = 50;

/// How long a command that changes photos may take: a base plus a little
/// per photo, so a big batch is not cut off but a stuck one is noticed.
fn write_timeout(photos: usize) -> Duration {
    Duration::from_secs((60 + photos as u64 / 5).min(30 * 60))
}

fn too_many(count: usize) -> CallToolResult {
    tool_error(format!(
        "That is {count} photos; change at most {MAX_PHOTOS_PER_CALL} in one call. Split the set \
         (get_set pages through it) or narrow it down first."
    ))
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct FindInCatalogArgs {
    /// Lowest star rating to include, 0-5.
    #[serde(default)]
    pub rating_min: Option<u8>,
    /// Highest star rating to include, 0-5. rating_max 0 finds unrated photos.
    #[serde(default)]
    pub rating_max: Option<u8>,
    /// "pick", "reject" or "unflagged".
    #[serde(default)]
    pub flag: Option<String>,
    /// "red", "yellow", "green", "blue", "purple" or "none".
    #[serde(default)]
    pub color_label: Option<String>,
    /// Photos with any of these Lightroom keywords.
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    /// Text in the file name, title or caption.
    #[serde(default)]
    pub text: Option<String>,
    /// A folder path, or a folder name; includes subfolders.
    #[serde(default)]
    pub folder: Option<String>,
    /// A collection name.
    #[serde(default)]
    pub collection: Option<String>,
    /// Captured on or after this day, "YYYY-MM-DD".
    #[serde(default)]
    pub captured_after: Option<String>,
    /// Captured on or before this day, "YYYY-MM-DD".
    #[serde(default)]
    pub captured_before: Option<String>,
    /// Camera model contains this text, e.g. "R5" or "X-T5".
    #[serde(default)]
    pub camera: Option<String>,
    /// Lens contains this text, e.g. "85mm".
    #[serde(default)]
    pub lens: Option<String>,
    /// "raw", "dng", "jpeg", "tiff", "png", "psd", "psb", "avif", "jxl" or
    /// "video".
    #[serde(default)]
    pub file_type: Option<String>,
    /// Only photos with (true) or without (false) Develop adjustments.
    #[serde(default)]
    pub edited: Option<bool>,
    /// Most photos to return (default 1000, at most 5000).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DetailsArgs {
    #[serde(flatten)]
    pub photos: Photos,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ViewArgs {
    pub photo_id: String,
    /// Longest edge in pixels, 256-2048 (default 1024). Larger costs more
    /// context; 1024 is enough to judge sharpness and expression.
    #[serde(default)]
    pub size: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowArgs {
    #[serde(flatten)]
    pub photos: Photos,
    /// Name of the collection to fill (default "Agent results"). It lives in
    /// the "LrGeniusAI Agent" collection set; an existing one is replaced.
    #[serde(default)]
    pub collection: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateArgs {
    #[serde(flatten)]
    pub photos: Photos,
    /// Star rating, 0-5 (0 removes the rating).
    #[serde(default)]
    pub rating: Option<u8>,
    /// "pick", "reject" or "none".
    #[serde(default)]
    pub flag: Option<String>,
    /// "red", "yellow", "green", "blue", "purple" or "none".
    #[serde(default)]
    pub color_label: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
    /// Keywords to add. Use "Parent > Child" for a keyword inside another.
    #[serde(default)]
    pub add_keywords: Option<Vec<String>>,
    /// Keywords to remove (by name, or "Parent > Child").
    #[serde(default)]
    pub remove_keywords: Option<Vec<String>>,
    /// Other IPTC fields: headline, creator, copyright, location, city,
    /// state, country, iso_country_code, instructions, job_identifier,
    /// alt_text, extended_description.
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CollectionArgs {
    #[serde(flatten)]
    pub photos: Photos,
    /// Collection name. Created when it does not exist (unless removing).
    pub collection: String,
    /// Collection set to put a new collection in (optional; nested sets as
    /// "Parent > Child"). When removing without it, the collection is looked
    /// up anywhere in the catalog; name the set if several share the name.
    #[serde(default)]
    pub collection_set: Option<String>,
    /// Remove the photos from the collection instead of adding them.
    #[serde(default)]
    pub remove: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DevelopGetArgs {
    pub photo_id: String,
    /// Only these settings (e.g. ["Exposure2012", "Temperature"]). Default:
    /// the basic panel, tone, presence, HSL and detail settings.
    #[serde(default)]
    pub keys: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DevelopApplyArgs {
    #[serde(flatten)]
    pub photos: Photos,
    /// Lightroom develop settings by their SDK names, e.g. {"Exposure2012":
    /// 0.3, "Contrast2012": 10, "Highlights2012": -40, "Shadows2012": 30,
    /// "Whites2012": 5, "Blacks2012": -5, "Texture": 5, "Clarity2012": 8,
    /// "Dehaze": 5, "Vibrance": 10, "Saturation": 0, "Temperature": 5600,
    /// "Tint": 4}. Read get_develop_settings first to see the names and
    /// current values.
    pub settings: Map<String, Value>,
    /// Add the numbers to each photo's current values instead of setting
    /// them (e.g. {"Exposure2012": 0.3} brightens every photo by 0.3 EV).
    #[serde(default)]
    pub relative: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnalyzeArgs {
    #[serde(flatten)]
    pub photos: Photos,
    /// What to compute: "embeddings" (semantic search), "faces" (people),
    /// "species", "cull" (culling signals). Default ["embeddings"]. AI titles,
    /// captions and keywords are not offered here: Analyze & Index's own
    /// dialog writes those, with review; or write them yourself with
    /// update_photos after view_photo.
    #[serde(default)]
    pub tasks: Option<Vec<String>>,
    /// Redo photos that already have results (default false: only missing
    /// work is done).
    #[serde(default)]
    pub redo: Option<bool>,
}

const FLAGS: &[&str] = &["pick", "reject", "none"];
const LABELS: &[&str] = &["red", "yellow", "green", "blue", "purple", "none"];
const METADATA_FIELDS: &[&str] = &[
    "headline",
    "creator",
    "copyright",
    "location",
    "city",
    "state",
    "country",
    "iso_country_code",
    "instructions",
    "job_identifier",
    "alt_text",
    "extended_description",
];
const ANALYZE_TASKS: &[&str] = &["embeddings", "faces", "species", "cull"];

/// Checks `update_photos` arguments before anything reaches Lightroom, so a
/// typo is answered with the list of valid values rather than a half-applied
/// change. Returns the plugin's argument object.
fn update_payload(args: &UpdateArgs) -> Result<Value, String> {
    let mut changes = Map::new();
    if let Some(rating) = args.rating {
        if rating > 5 {
            return Err("rating must be 0-5.".into());
        }
        changes.insert("rating".into(), json!(rating));
    }
    if let Some(flag) = &args.flag {
        let flag = flag.trim().to_lowercase();
        if !FLAGS.contains(&flag.as_str()) {
            return Err(format!("flag must be one of {}.", FLAGS.join(", ")));
        }
        changes.insert("flag".into(), json!(flag));
    }
    if let Some(label) = &args.color_label {
        let label = label.trim().to_lowercase();
        if !LABELS.contains(&label.as_str()) {
            return Err(format!("color_label must be one of {}.", LABELS.join(", ")));
        }
        changes.insert("color_label".into(), json!(label));
    }
    if let Some(title) = &args.title {
        changes.insert("title".into(), json!(title));
    }
    if let Some(caption) = &args.caption {
        changes.insert("caption".into(), json!(caption));
    }
    for (key, list) in [
        ("add_keywords", &args.add_keywords),
        ("remove_keywords", &args.remove_keywords),
    ] {
        if let Some(list) = list {
            let cleaned: Vec<String> = list
                .iter()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect();
            if !cleaned.is_empty() {
                changes.insert(key.into(), json!(cleaned));
            }
        }
    }
    if let Some(metadata) = &args.metadata {
        let mut fields = Map::new();
        for (key, value) in metadata {
            let key = key.trim().to_lowercase();
            if !METADATA_FIELDS.contains(&key.as_str()) {
                return Err(format!(
                    "metadata field \"{key}\" is not supported; use one of {}.",
                    METADATA_FIELDS.join(", ")
                ));
            }
            fields.insert(key, json!(value));
        }
        if !fields.is_empty() {
            changes.insert("metadata".into(), Value::Object(fields));
        }
    }
    if changes.is_empty() {
        return Err(
            "Nothing to change: give at least one of rating, flag, color_label, title, \
                    caption, add_keywords, remove_keywords or metadata."
                .into(),
        );
    }
    Ok(Value::Object(changes))
}

/// A photo's aesthetic score and genre as get_photo_details reports them.
/// A score from another model export is still shown — it is that model's
/// honest answer — but flagged, because search does not rank by it and it is
/// not comparable with current scores.
fn aesthetic_fields(m: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(score) = m
        .get(lrg_store::meta::AESTHETIC_SCORE_FIELD)
        .and_then(Value::as_f64)
    {
        out.insert("aesthetic_score".into(), json!(round2(score)));
        if search::current_aesthetic(m).is_none() {
            out.insert("aesthetic_outdated".into(), json!(true));
        }
    }
    if let Some(genre) = search::genre_of(m) {
        out.insert("genre".into(), json!(genre));
        if let Some(label) = search::genre_label(genre) {
            out.insert("genre_label".into(), json!(label));
        }
    }
    out
}

/// The plugin's `find_photos` criteria, validated.
fn find_criteria(args: &FindInCatalogArgs) -> Result<Value, String> {
    let mut c = Map::new();
    for (key, value) in [
        ("rating_min", args.rating_min),
        ("rating_max", args.rating_max),
    ] {
        if let Some(v) = value {
            if v > 5 {
                return Err(format!("{key} must be 0-5."));
            }
            c.insert(key.into(), json!(v));
        }
    }
    if let Some(flag) = &args.flag {
        let flag = flag.trim().to_lowercase();
        if !["pick", "reject", "unflagged"].contains(&flag.as_str()) {
            return Err("flag must be pick, reject or unflagged.".into());
        }
        c.insert("flag".into(), json!(flag));
    }
    if let Some(label) = &args.color_label {
        let label = label.trim().to_lowercase();
        if !LABELS.contains(&label.as_str()) {
            return Err(format!("color_label must be one of {}.", LABELS.join(", ")));
        }
        c.insert("color_label".into(), json!(label));
    }
    for (key, value) in [
        ("captured_after", &args.captured_after),
        ("captured_before", &args.captured_before),
    ] {
        if let Some(day) = value {
            if chrono::NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d").is_err() {
                return Err(format!("{key} must be a date like 2025-07-14."));
            }
            c.insert(key.into(), json!(day.trim()));
        }
    }
    for (key, value) in [
        ("text", &args.text),
        ("folder", &args.folder),
        ("collection", &args.collection),
        ("camera", &args.camera),
        ("lens", &args.lens),
        ("file_type", &args.file_type),
    ] {
        if let Some(v) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            c.insert(key.into(), json!(v));
        }
    }
    if let Some(keywords) = &args.keywords {
        let list: Vec<&str> = keywords
            .iter()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .collect();
        if !list.is_empty() {
            c.insert("keywords".into(), json!(list));
        }
    }
    if let Some(edited) = args.edited {
        c.insert("edited".into(), json!(edited));
    }
    if c.is_empty() {
        return Err(
            "Give at least one condition (rating, flag, label, keywords, text, folder, \
             collection, dates, camera, lens, file_type or edited)."
                .into(),
        );
    }
    Ok(Value::Object(c))
}

/// A sentence describing the criteria, used as the result set's label.
fn describe_criteria(criteria: &Value) -> String {
    let parts: Vec<String> = criteria
        .as_object()
        .map(|c| {
            c.iter()
                .map(|(k, v)| match v {
                    Value::String(s) => format!("{k} {s}"),
                    other => format!("{k} {other}"),
                })
                .collect()
        })
        .unwrap_or_default();
    format!("catalog photos with {}", parts.join(", "))
}

#[tool_router(router = lightroom_tools, vis = "pub(crate)")]
impl LrGeniusMcp {
    /// Finds photos with Lightroom's own catalog filters: rating, flag,
    /// colour label, keywords, text, folder, collection, capture date,
    /// camera, lens, file type, edited or not. Covers every photo in the
    /// catalog, indexed or not. Returns a result set.
    #[tool(
        name = "find_photos_in_catalog",
        annotations(title = "Find photos in the catalog", read_only_hint = true)
    )]
    async fn find_photos_in_catalog(
        &self,
        Parameters(args): Parameters<FindInCatalogArgs>,
    ) -> Result<CallToolResult, McpError> {
        let criteria = match find_criteria(&args) {
            Ok(c) => c,
            Err(e) => return Ok(tool_error(e)),
        };
        let limit = args.limit.unwrap_or(1000).clamp(1, MAX_PHOTOS_PER_CALL);
        let reply = match self
            .lightroom(
                "find_photos",
                json!({"criteria": criteria, "limit": limit}),
                CallOptions::read(Duration::from_secs(180)),
                None,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(bridge_error(e)),
        };
        let ids = string_list(&reply.result["photo_ids"]);
        let set = self.sets().create(describe_criteria(&criteria), ids);
        let mut extra = json!({});
        if reply.result["truncated"].as_bool() == Some(true) {
            extra["truncated"] = json!(format!(
                "More than {limit} photos match; only the first {limit} are in the set."
            ));
        }
        if !reply.warnings.is_empty() {
            extra["warnings"] = json!(reply.warnings);
        }
        Ok(json_result(self.set_answer(&set, extra).await))
    }

    /// Everything known about up to 50 photos: Lightroom's metadata (file,
    /// capture time, camera and exposure, rating, flag, label, keywords,
    /// title, caption, whether it is edited) merged with LrGeniusAI's index
    /// (AI title, caption, keywords, species, aesthetic score 1-9, genre,
    /// search status).
    #[tool(
        name = "get_photo_details",
        annotations(title = "Photo details", read_only_hint = true)
    )]
    async fn get_photo_details(
        &self,
        Parameters(args): Parameters<DetailsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let resolved = match self.resolve_photos(&args.photos).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        let total = resolved.photo_ids.len();
        let ids: Vec<String> = resolved.photo_ids.into_iter().take(MAX_DETAILS).collect();

        let index: HashMap<String, Map<String, Value>> = match self.state().store() {
            Some(store) => store
                .get_meta(lrg_store::IMAGE_TABLE, &ids)
                .await
                .unwrap_or_default()
                .into_iter()
                .collect(),
            None => HashMap::new(),
        };

        let mut warnings = Vec::new();
        let mut lightroom: HashMap<String, Value> = HashMap::new();
        if self.state().agent_bridge.is_connected() {
            match self
                .lightroom(
                    "photo_info",
                    json!({"photo_ids": ids}),
                    CallOptions::read(Duration::from_secs(60)),
                    None,
                )
                .await
            {
                Ok(reply) => {
                    for p in reply.result["photos"].as_array().into_iter().flatten() {
                        if let Some(id) = p["photo_id"].as_str() {
                            lightroom.insert(id.to_string(), p.clone());
                        }
                    }
                    let missing = string_list(&reply.result["missing"]);
                    if !missing.is_empty() {
                        warnings.push(format!(
                            "{} photo(s) are not in the open Lightroom catalog: {}",
                            missing.len(),
                            missing.join(", ")
                        ));
                    }
                    warnings.extend(reply.warnings);
                }
                Err(e) => warnings.push(format!("Lightroom details unavailable: {e}")),
            }
        } else {
            warnings.push(
                "Lightroom is not connected, so only LrGeniusAI's index data is shown.".into(),
            );
        }

        let photos: Vec<Value> = ids
            .iter()
            .map(|id| {
                let mut out = Map::new();
                out.insert("photo_id".into(), json!(id));
                if let Some(lr) = lightroom.get(id).and_then(Value::as_object) {
                    let mut lr = lr.clone();
                    lr.remove("photo_id");
                    out.insert("lightroom".into(), Value::Object(lr));
                }
                match index.get(id) {
                    Some(m) => {
                        let text =
                            |k: &str| m.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
                        let mut ai = Map::new();
                        for key in ["title", "caption", "alt_text"] {
                            if let Some(v) = text(key) {
                                ai.insert(key.into(), json!(truncate(v, 1000)));
                            }
                        }
                        if let Some(k) = text("flattened_keywords") {
                            let list: Vec<&str> = k
                                .split(',')
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .collect();
                            ai.insert("keywords".into(), json!(list));
                        }
                        for (key, name) in [
                            ("species_common_name", "species"),
                            ("species_scientific_name", "scientific_name"),
                            ("species_rank", "species_rank"),
                        ] {
                            if let Some(v) = text(key) {
                                ai.insert(name.into(), json!(v));
                            }
                        }
                        ai.extend(aesthetic_fields(m));
                        ai.insert(
                            "searchable".into(),
                            json!(lrg_store::meta::has_embedding(m)),
                        );
                        out.insert("index".into(), Value::Object(ai));
                    }
                    None => {
                        out.insert("index".into(), json!({"indexed": false}));
                    }
                }
                Value::Object(out)
            })
            .collect();

        let mut answer = json!({"photos": photos});
        if total > ids.len() {
            answer["more"] = json!(total - ids.len());
            answer["hint"] = json!(format!(
                "Only the first {MAX_DETAILS} are described; ask for the rest in smaller groups."
            ));
        }
        Ok(json_result(with_warnings(answer, &warnings)))
    }

    /// Shows a photo the way Lightroom renders it right now, develop
    /// settings included, as an image you can look at. Use it to judge
    /// sharpness, expression and composition, and to check your own edits.
    #[tool(
        name = "view_photo",
        annotations(title = "View photo", read_only_hint = true)
    )]
    async fn view_photo(
        &self,
        Parameters(args): Parameters<ViewArgs>,
    ) -> Result<CallToolResult, McpError> {
        let size = args.size.unwrap_or(1024).clamp(256, 2048);
        let reply = match self
            .lightroom(
                "render_preview",
                json!({"photo_id": args.photo_id, "size": size}),
                CallOptions::read(Duration::from_secs(120)),
                None,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(bridge_error(e)),
        };
        let Some(path) = reply.result["path"].as_str() else {
            return Ok(tool_error("Lightroom did not return a preview file."));
        };
        let path = Path::new(path);
        let is_jpeg = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
        if !path.is_absolute() || !is_jpeg {
            return Ok(tool_error(format!(
                "Lightroom returned an unexpected preview path: {}",
                path.display()
            )));
        }
        let bytes = match tokio::fs::metadata(path).await {
            Ok(meta) if meta.len() <= MAX_PREVIEW_BYTES => tokio::fs::read(path).await,
            Ok(meta) => {
                return Ok(tool_error(format!(
                    "The preview is unexpectedly large ({} bytes).",
                    meta.len()
                )))
            }
            Err(e) => Err(e),
        };
        // The plugin renders into a temporary file for us alone.
        let _ = tokio::fs::remove_file(path).await;
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => {
                return Ok(tool_error(format!(
                    "Could not read the preview Lightroom rendered: {e}"
                )))
            }
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mut caption = json!({
            "photo_id": args.photo_id,
            "file_name": reply.result["file_name"],
            "rendered": format!("longest edge {size} px, current develop settings"),
        });
        if !reply.warnings.is_empty() {
            caption["warnings"] = json!(reply.warnings);
        }
        Ok(CallToolResult::success(vec![
            ContentBlock::image(encoded, "image/jpeg"),
            ContentBlock::text(caption.to_string()),
        ]))
    }

    /// Puts photos in a collection (inside the "LrGeniusAI Agent" set) and
    /// switches Lightroom to it, so the user sees exactly these photos.
    /// Replaces the collection's previous contents.
    #[tool(
        name = "show_in_lightroom",
        annotations(
            title = "Show in Lightroom",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn show_in_lightroom(
        &self,
        Parameters(args): Parameters<ShowArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let resolved = match self.resolve_photos(&args.photos).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        if resolved.photo_ids.len() > MAX_PHOTOS_PER_CALL {
            return Ok(too_many(resolved.photo_ids.len()));
        }
        let collection = args
            .collection
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "Agent results".into());
        let count = resolved.photo_ids.len();
        match self
            .lightroom(
                "show_photos",
                json!({"photo_ids": resolved.photo_ids, "collection": collection}),
                // Showing photos only adds them to the agent's own collection,
                // so it is allowed even when the plugin is read-only.
                CallOptions::read(write_timeout(count)),
                Some(&context),
            )
            .await
        {
            Ok(reply) => Ok(json_result(with_warnings(reply.result, &reply.warnings))),
            Err(e) => Ok(bridge_error(e)),
        }
    }

    /// Changes photo metadata in Lightroom: rating, pick/reject flag, colour
    /// label, title, caption, keywords and other IPTC fields — all in one
    /// undo step. Only the fields you give change.
    #[tool(
        name = "update_photos",
        annotations(
            title = "Update photos",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true
        )
    )]
    async fn update_photos(
        &self,
        Parameters(args): Parameters<UpdateArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let changes = match update_payload(&args) {
            Ok(c) => c,
            Err(e) => return Ok(tool_error(e)),
        };
        let resolved = match self.resolve_photos(&args.photos).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        let count = resolved.photo_ids.len();
        if count > MAX_PHOTOS_PER_CALL {
            return Ok(too_many(count));
        }
        let mut payload = changes;
        payload["photo_ids"] = json!(resolved.photo_ids);
        match self
            .lightroom(
                "update_photos",
                payload,
                CallOptions::write(write_timeout(count)),
                Some(&context),
            )
            .await
        {
            Ok(reply) => Ok(json_result(with_warnings(reply.result, &reply.warnings))),
            Err(e) => Ok(bridge_error(e)),
        }
    }

    /// Adds photos to a named collection (creating it if needed), or removes
    /// them from it.
    #[tool(
        name = "manage_collection",
        annotations(
            title = "Add to or remove from a collection",
            read_only_hint = false,
            destructive_hint = false
        )
    )]
    async fn manage_collection(
        &self,
        Parameters(args): Parameters<CollectionArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let name = args.collection.trim().to_string();
        if name.is_empty() {
            return Ok(tool_error("collection must not be empty."));
        }
        let resolved = match self.resolve_photos(&args.photos).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        let count = resolved.photo_ids.len();
        if count > MAX_PHOTOS_PER_CALL {
            return Ok(too_many(count));
        }
        let op = if args.remove.unwrap_or(false) {
            "remove"
        } else {
            "add"
        };
        match self
            .lightroom(
                "collection",
                json!({
                    "op": op,
                    "collection": name,
                    "collection_set": args.collection_set,
                    "photo_ids": resolved.photo_ids,
                }),
                CallOptions::write(write_timeout(count)),
                Some(&context),
            )
            .await
        {
            Ok(reply) => Ok(json_result(with_warnings(reply.result, &reply.warnings))),
            Err(e) => Ok(bridge_error(e)),
        }
    }

    /// The catalog's collections and collection sets, with photo counts.
    #[tool(
        name = "list_collections",
        annotations(title = "List collections", read_only_hint = true)
    )]
    async fn list_collections(&self) -> Result<CallToolResult, McpError> {
        match self
            .lightroom(
                "list_collections",
                json!({}),
                CallOptions::read(Duration::from_secs(60)),
                None,
            )
            .await
        {
            Ok(reply) => Ok(json_result(with_warnings(reply.result, &reply.warnings))),
            Err(e) => Ok(bridge_error(e)),
        }
    }

    /// A photo's current Lightroom develop settings, by their SDK names.
    #[tool(
        name = "get_develop_settings",
        annotations(title = "Get develop settings", read_only_hint = true)
    )]
    async fn get_develop_settings(
        &self,
        Parameters(args): Parameters<DevelopGetArgs>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .lightroom(
                "get_develop_settings",
                json!({"photo_id": args.photo_id, "keys": args.keys}),
                CallOptions::read(Duration::from_secs(60)),
                None,
            )
            .await
        {
            Ok(reply) => Ok(json_result(with_warnings(reply.result, &reply.warnings))),
            Err(e) => Ok(bridge_error(e)),
        }
    }

    /// Applies develop settings to photos, absolute or relative to each
    /// photo's current values. Before changing a photo it saves a develop
    /// snapshot, so every edit can be undone per photo even later; the whole
    /// call is also one undo step. Check the result with view_photo.
    #[tool(
        name = "apply_develop_settings",
        annotations(
            title = "Apply develop settings",
            read_only_hint = false,
            destructive_hint = true
        )
    )]
    async fn apply_develop_settings(
        &self,
        Parameters(args): Parameters<DevelopApplyArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if args.settings.is_empty() {
            return Ok(tool_error(
                "settings must name at least one develop setting.",
            ));
        }
        let relative = args.relative.unwrap_or(false);
        if relative {
            if let Some((key, _)) = args.settings.iter().find(|(_, v)| !v.is_number()) {
                return Ok(tool_error(format!(
                    "With relative, every setting needs a number; {key} is not one."
                )));
            }
        }
        let resolved = match self.resolve_photos(&args.photos).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        let count = resolved.photo_ids.len();
        if count > MAX_PHOTOS_PER_CALL {
            return Ok(too_many(count));
        }
        match self
            .lightroom(
                "apply_develop_settings",
                json!({
                    "photo_ids": resolved.photo_ids,
                    "settings": args.settings,
                    "relative": relative,
                    "snapshot": true,
                }),
                CallOptions::write(write_timeout(count * 3)),
                Some(&context),
            )
            .await
        {
            Ok(reply) => Ok(json_result(with_warnings(reply.result, &reply.warnings))),
            Err(e) => Ok(bridge_error(e)),
        }
    }

    /// Indexes photos with LrGeniusAI so search, people, species and culling
    /// can see them (only what is missing, unless redo). Runs on this
    /// computer and changes nothing in the catalog. Takes about a second per
    /// photo.
    #[tool(
        name = "analyze_photos",
        annotations(
            title = "Analyze and index photos",
            read_only_hint = false,
            destructive_hint = false
        )
    )]
    async fn analyze_photos(
        &self,
        Parameters(args): Parameters<AnalyzeArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(tasks) = &args.tasks {
            if tasks.iter().any(|t| t == "metadata") {
                return Ok(tool_error(
                    "AI titles, captions and keywords are written by the plugin's Analyze & Index \
                     dialog, which lets the user review them first. Ask the user to run it, or look \
                     at the photos with view_photo and write metadata with update_photos.",
                ));
            }
            if let Some(bad) = tasks.iter().find(|t| !ANALYZE_TASKS.contains(&t.as_str())) {
                return Ok(tool_error(format!(
                    "Unknown task \"{bad}\"; use any of {}.",
                    ANALYZE_TASKS.join(", ")
                )));
            }
        }
        let resolved = match self.resolve_photos(&args.photos).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        let count = resolved.photo_ids.len();
        if count > MAX_PHOTOS_PER_CALL {
            return Ok(too_many(count));
        }
        // Indexing writes to the backend only, so a read-only bridge allows it.
        let options = CallOptions::read(Duration::from_secs(4 * 3600));
        match self
            .lightroom(
                "analyze_photos",
                json!({
                    "photo_ids": resolved.photo_ids,
                    "tasks": args.tasks,
                    "redo": args.redo.unwrap_or(false),
                }),
                options,
                Some(&context),
            )
            .await
        {
            Ok(reply) => Ok(json_result(with_warnings(reply.result, &reply.warnings))),
            Err(e) => Ok(bridge_error(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scored(score: f64, model: &str, genre: Option<&str>) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(lrg_store::meta::AESTHETIC_SCORE_FIELD.into(), json!(score));
        m.insert(lrg_store::meta::AESTHETIC_MODEL_FIELD.into(), json!(model));
        if let Some(g) = genre {
            m.insert(lrg_store::meta::GENRE_FIELD.into(), json!(g));
        }
        m
    }

    #[test]
    fn photo_details_show_the_aesthetic_score_and_genre() {
        let f = aesthetic_fields(&scored(
            6.4567,
            search::AESTHETIC_MODEL_ID,
            Some("landscape"),
        ));
        assert_eq!(f["aesthetic_score"], 6.46);
        assert!(f.get("aesthetic_outdated").is_none());
        assert_eq!(f["genre"], "landscape");
        assert_eq!(f["genre_label"], search::genre_label("landscape").unwrap());
    }

    #[test]
    fn a_score_from_another_model_is_shown_but_flagged() {
        let f = aesthetic_fields(&scored(7.0, "an-older-export", None));
        assert_eq!(f["aesthetic_score"], 7.0);
        assert_eq!(f["aesthetic_outdated"], true);
        assert!(f.get("genre").is_none());
        assert!(aesthetic_fields(&Map::new()).is_empty());
    }

    fn update(f: impl FnOnce(&mut UpdateArgs)) -> UpdateArgs {
        let mut args = UpdateArgs {
            photos: Photos::default(),
            rating: None,
            flag: None,
            color_label: None,
            title: None,
            caption: None,
            add_keywords: None,
            remove_keywords: None,
            metadata: None,
        };
        f(&mut args);
        args
    }

    #[test]
    fn update_arguments_are_checked_before_lightroom_sees_them() {
        assert!(update_payload(&update(|_| {})).is_err());
        assert!(update_payload(&update(|a| a.rating = Some(6))).is_err());
        assert!(update_payload(&update(|a| a.flag = Some("maybe".into()))).is_err());
        let err = update_payload(&update(|a| {
            a.metadata = Some(HashMap::from([("gps".to_string(), "x".to_string())]))
        }))
        .unwrap_err();
        assert!(err.contains("not supported"));

        let ok = update_payload(&update(|a| {
            a.rating = Some(4);
            a.flag = Some(" Pick ".into());
            a.add_keywords = Some(vec!["Places > Hamburg".into(), "  ".into()]);
            a.metadata = Some(HashMap::from([("City".to_string(), "Hamburg".to_string())]));
        }))
        .unwrap();
        assert_eq!(ok["rating"], 4);
        assert_eq!(ok["flag"], "pick");
        assert_eq!(ok["add_keywords"], json!(["Places > Hamburg"]));
        assert_eq!(ok["metadata"]["city"], "Hamburg");
    }

    #[test]
    fn catalog_criteria_are_validated() {
        assert!(find_criteria(&FindInCatalogArgs::default()).is_err());
        assert!(find_criteria(&FindInCatalogArgs {
            captured_after: Some("14.07.2025".into()),
            ..Default::default()
        })
        .is_err());
        let c = find_criteria(&FindInCatalogArgs {
            rating_min: Some(3),
            flag: Some("PICK".into()),
            captured_after: Some("2025-07-14".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(c["flag"], "pick");
        assert_eq!(c["rating_min"], 3);
        assert!(describe_criteria(&c).contains("rating_min 3"));
    }
}
