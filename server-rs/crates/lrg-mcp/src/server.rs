//! The MCP server object: what every tool shares — the backend state, the
//! in-process API, the result sets, the Lightroom bridge — plus the server's
//! self-description and its prompts. The tools themselves live in
//! `tools_index.rs` (the backend's index), `tools_lightroom.rs` (the catalog,
//! through the plugin) and `tools_dev.rs` (testing the plugin).

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ProgressNotificationParam, PromptMessage, Role,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{prompt, prompt_handler, prompt_router, tool_handler, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use lrg_api::agent_bridge::{BridgeError, CallOptions, CommandReply, Progress};
use lrg_api::routes::search;
use lrg_api::state::AppState;
use lrg_store::IMAGE_TABLE;

use crate::api::Api;
use crate::sets::{ResultSet, ResultSets};

/// How many photos a tool lists inline before it says "and N more".
pub(crate) const PREVIEW_PHOTOS: usize = 20;

/// The most photos one tool call resolves. A set can be larger; tools that
/// change photos refuse beyond this and say so, so a runaway agent cannot
/// rewrite a whole catalog in one call.
pub const MAX_PHOTOS_PER_CALL: usize = 5000;

/// Everything the tools share. One per backend process; each MCP session
/// gets its own [`LrGeniusMcp`] around the same `Shared`.
pub struct Shared {
    pub state: Arc<AppState>,
    pub api: Api,
    pub sets: ResultSets,
}

#[derive(Clone)]
pub struct LrGeniusMcp {
    pub(crate) shared: Arc<Shared>,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

impl LrGeniusMcp {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            tool_router: Self::index_tools() + Self::lightroom_tools() + Self::dev_tools(),
            prompt_router: Self::prompt_router(),
        }
    }

    /// Names of the tools that test and change the plugin itself
    /// (`tools_dev.rs`). The in-app chat never offers them to a model.
    pub fn developer_tool_names() -> Vec<String> {
        Self::dev_tools()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    /// Names of every tool, for tests and the docs check.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }
}

/// Which photos a tool works on. Exactly one of the two fields.
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct Photos {
    /// A result set from an earlier tool ("s1", "s2", ...), or "selection"
    /// for the photos selected in Lightroom right now, or "view" for every
    /// photo in the folder or collection Lightroom is showing.
    #[serde(default)]
    pub set: Option<String>,
    /// Photo IDs as other tools return them. Prefer `set` when you have one.
    #[serde(default)]
    pub photo_ids: Option<Vec<String>>,
}

/// Photos a tool resolved, with a phrase saying where they came from.
pub(crate) struct Resolved {
    pub photo_ids: Vec<String>,
    pub label: String,
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// A tool's answer as pretty JSON text — what every MCP client can show and
/// every model can read.
pub(crate) fn json_result(value: Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    CallToolResult::success(vec![ContentBlock::text(text)])
}

/// A failure the model should read and act on (wrong arguments, Lightroom
/// not connected), as opposed to a protocol error.
pub(crate) fn tool_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

/// A bridge failure as a tool error, keeping the plugin's warnings: a command
/// that failed halfway may still have changed some photos.
pub(crate) fn bridge_error(error: BridgeError) -> CallToolResult {
    let mut text = error.to_string();
    if let BridgeError::Plugin { warnings, .. } = &error {
        if !warnings.is_empty() {
            text.push_str("\n\nWarnings:\n- ");
            text.push_str(&warnings.join("\n- "));
        }
    }
    tool_error(text)
}

/// Adds the plugin's warnings to a result object, when there are any.
pub(crate) fn with_warnings(mut value: Value, warnings: &[String]) -> Value {
    if !warnings.is_empty() {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("warnings".into(), json!(warnings));
        }
    }
    value
}

// ---------------------------------------------------------------------------
// Helpers the tools share
// ---------------------------------------------------------------------------

impl LrGeniusMcp {
    pub(crate) fn state(&self) -> &AppState {
        &self.shared.state
    }

    pub(crate) fn api(&self) -> &Api {
        &self.shared.api
    }

    pub(crate) fn sets(&self) -> &ResultSets {
        &self.shared.sets
    }

    /// The open catalog's backend scope, when Lightroom is connected. Without
    /// it, index queries cover every catalog the backend has seen.
    pub(crate) fn catalog_id(&self) -> Option<String> {
        self.state()
            .agent_bridge
            .plugin_info()
            .map(|info| info.catalog_id)
            .filter(|id| !id.is_empty())
    }

    /// The note a tool adds when it could not scope to the open catalog.
    pub(crate) fn scope_note(&self) -> Option<&'static str> {
        if self.catalog_id().is_some() {
            None
        } else {
            Some(
                "Lightroom is not connected, so this covers every catalog the backend knows, \
                 not just the open one.",
            )
        }
    }

    /// Runs one command in Lightroom, forwarding the plugin's progress as MCP
    /// progress notifications when the client asked for them.
    pub(crate) async fn lightroom(
        &self,
        action: &str,
        args: Value,
        mut options: CallOptions,
        context: Option<&RequestContext<RoleServer>>,
    ) -> Result<CommandReply, BridgeError> {
        let forwarder = context.and_then(|ctx| {
            let token = ctx.meta.get_progress_token()?;
            let peer = ctx.peer.clone();
            let (tx, mut rx) = mpsc::unbounded_channel::<Progress>();
            options.progress = Some(tx);
            Some(tokio::spawn(async move {
                while let Some(p) = rx.recv().await {
                    let mut param = ProgressNotificationParam::new(token.clone(), p.done);
                    if let Some(total) = p.total {
                        param = param.with_total(total);
                    }
                    if let Some(message) = p.message {
                        param = param.with_message(message);
                    }
                    if peer.notify_progress(param).await.is_err() {
                        break;
                    }
                }
            }))
        });
        let result = self.state().agent_bridge.call(action, args, options).await;
        if let Some(forwarder) = forwarder {
            forwarder.abort();
        }
        result
    }

    /// Turns a [`Photos`] argument into photo IDs.
    pub(crate) async fn resolve_photos(&self, photos: &Photos) -> Result<Resolved, String> {
        let set = photos
            .set
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let ids = photos.photo_ids.as_ref().filter(|ids| !ids.is_empty());
        let resolved = match (set, ids) {
            (Some(_), Some(_)) => {
                return Err("Give either `set` or `photo_ids`, not both.".into());
            }
            (None, None) => {
                return Err(
                    "Say which photos: pass `set` (a result set such as \"s3\", or \
                     \"selection\", or \"view\") or `photo_ids`."
                        .into(),
                );
            }
            (None, Some(ids)) => Resolved {
                photo_ids: ids.clone(),
                label: format!("{} given photos", ids.len()),
            },
            (Some(scope @ ("selection" | "view")), None) => {
                let reply = self
                    .lightroom(
                        "get_photos",
                        json!({"scope": scope, "limit": MAX_PHOTOS_PER_CALL}),
                        CallOptions::read(std::time::Duration::from_secs(120)),
                        None,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let photo_ids = string_list(&reply.result["photo_ids"]);
                if photo_ids.is_empty() {
                    return Err(if scope == "selection" {
                        "No photos are selected in Lightroom.".into()
                    } else {
                        "Lightroom is not showing any photos.".into()
                    });
                }
                Resolved {
                    label: if scope == "selection" {
                        format!("the {} photos selected in Lightroom", photo_ids.len())
                    } else {
                        format!("the {} photos in Lightroom's current view", photo_ids.len())
                    },
                    photo_ids,
                }
            }
            (Some(id), None) => {
                let Some(found) = self.sets().get(id) else {
                    return Err(format!(
                        "There is no result set \"{id}\". Sets last until the backend restarts; \
                         run the search again to get a new one."
                    ));
                };
                Resolved {
                    label: format!("set {} ({})", found.id, found.label),
                    photo_ids: found.photo_ids,
                }
            }
        };
        Ok(resolved)
    }

    /// One line of facts per photo from the backend's index: what a model
    /// needs to tell photos apart without looking at them.
    pub(crate) async fn summaries(&self, photo_ids: &[String]) -> Vec<Value> {
        let rows: HashMap<String, Map<String, Value>> = match self.state().store() {
            Some(store) => store
                .get_meta(IMAGE_TABLE, photo_ids)
                .await
                .unwrap_or_default()
                .into_iter()
                .collect(),
            None => HashMap::new(),
        };
        photo_ids
            .iter()
            .map(|id| summarize(id, rows.get(id)))
            .collect()
    }

    /// The standard answer of a tool that found photos: the new set and a
    /// preview of its first members.
    pub(crate) async fn set_answer(&self, set: &ResultSet, extra: Value) -> Value {
        let preview: Vec<String> = set.photo_ids.iter().take(PREVIEW_PHOTOS).cloned().collect();
        let mut answer = json!({
            "set_id": set.id,
            "count": set.photo_ids.len(),
            "label": set.label,
            "photos": self.summaries(&preview).await,
        });
        if set.photo_ids.len() > preview.len() {
            answer["more"] = json!(set.photo_ids.len() - preview.len());
            answer["hint"] = json!(format!(
                "Page through all of them with get_set (set_id \"{}\").",
                set.id
            ));
        }
        if let (Some(obj), Some(extra)) = (answer.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
        if let Some(note) = self.scope_note() {
            answer["note"] = json!(note);
        }
        answer
    }
}

/// Compact, model-friendly facts about one photo from its index metadata.
pub(crate) fn summarize(photo_id: &str, meta: Option<&Map<String, Value>>) -> Value {
    let mut out = Map::new();
    out.insert("photo_id".into(), json!(photo_id));
    let Some(m) = meta else {
        out.insert("indexed".into(), json!(false));
        return Value::Object(out);
    };
    let text = |key: &str| {
        m.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    if let Some(name) = text("filename") {
        out.insert("file_name".into(), json!(name));
    }
    if let Some(t) = m.get("capture_time").and_then(Value::as_f64) {
        if let Some(when) = chrono::DateTime::from_timestamp(t as i64, 0) {
            out.insert(
                "captured".into(),
                json!(when.format("%Y-%m-%d %H:%M").to_string()),
            );
        }
    }
    if let Some(title) = text("title") {
        out.insert("title".into(), json!(title));
    }
    if let Some(caption) = text("caption") {
        out.insert("caption".into(), json!(truncate(caption, 240)));
    }
    if let Some(keywords) = text("flattened_keywords") {
        let list: Vec<&str> = keywords
            .split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .take(15)
            .collect();
        out.insert("keywords".into(), json!(list));
    }
    if let Some(species) = text("species_common_name").or_else(|| text("species")) {
        out.insert("species".into(), json!(species));
    }
    if let Some(score) = search::current_aesthetic(m) {
        out.insert("aesthetic".into(), json!(round2(score)));
    }
    if let Some(genre) = search::genre_of(m) {
        out.insert("genre".into(), json!(genre));
    }
    if !lrg_store::meta::has_embedding(m) {
        out.insert("searchable".into(), json!(false));
    }
    Value::Object(out)
}

/// An aesthetic score to two decimals: finer than that is noise to a reader.
pub(crate) fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{}…", cut.trim_end())
}

pub(crate) fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Server description
// ---------------------------------------------------------------------------

pub(crate) const INSTRUCTIONS: &str = "\
LrGeniusAI connects you to the user's Adobe Lightroom Classic catalog and to LrGeniusAI's AI index \
of it (semantic image search, recognised people, species, culling grades, aesthetic scores and \
genres, AI titles and keywords).

How to work with it:
- Start with get_status: it says whether Lightroom is connected, which catalog is open, how much of \
it is indexed, and whether you may change photos.
- Tools that find photos (search_photos, find_photos_of_people, find_photos_in_catalog, \
cull_photos, ...) return a result set id such as \"s3\" plus a preview. Pass `set: \"s3\"` to the \
next tool instead of copying photo IDs. `set: \"selection\"` means the photos the user has \
selected in Lightroom right now; `set: \"view\"` means everything Lightroom is showing.
- Look before you judge: view_photo returns the photo as Lightroom renders it now, including \
develop settings. get_photo_details gives metadata.
- \"The best\" photos: search_photos with sort \"aesthetic\" ranks by LrGeniusAI's aesthetic score \
(1-9, higher is better); without a query it ranks the whole catalog, and `genres` (e.g. \
[\"landscape\"]) narrows it to a kind of photo. sort \"combined\" weighs the score against how \
well a photo matches the query. The user's own stars and flags (find_photos_in_catalog) are \
their judgement; prefer them where they exist. cull_photos compares frames within a burst, not \
unrelated photos.
- Show your results with show_in_lightroom, which fills a collection and selects it, so the user \
sees exactly what you found.
- Changes (update_photos, apply_develop_settings, collections) go through Lightroom. Each call is \
one entry in Lightroom's undo history (Edit > Undo); develop changes also leave a snapshot named \
after the change. Prefer one call over a set to many calls over single photos. Never claim a \
change worked without reading the tool's answer, including its warnings.
- Only photos that LrGeniusAI has indexed are found by search, people and culling tools. \
analyze_photos indexes more; get_status says how many are missing.
- If a tool says the user stopped agent control, stop and ask the user.";

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for LrGeniusMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(
            Implementation::new("lrgeniusai", lrg_common::version::backend_version())
                .with_title("LrGeniusAI for Lightroom Classic"),
        )
        .with_instructions(INSTRUCTIONS)
    }
}

// ---------------------------------------------------------------------------
// Prompts: ready-made workflows a user can pick in their MCP client
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CullPromptArgs {
    /// Which photos to cull: "selection" (default), "view", or a description
    /// such as "yesterday's wedding shoot".
    #[serde(default)]
    pub photos: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BestOfPersonArgs {
    /// The person's name as it appears in LrGeniusAI's People.
    pub person: String,
    /// How many photos to pick (default 20).
    #[serde(default)]
    pub count: Option<u32>,
    /// Optional extra condition, e.g. "from 2025" or "outdoors".
    #[serde(default)]
    pub condition: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TestPluginArgs {
    /// What changed, so the run can focus on it (optional).
    #[serde(default)]
    pub focus: Option<String>,
}

#[prompt_router]
impl LrGeniusMcp {
    /// Cull a shoot: group bursts and duplicates, keep the best frame of each
    /// group, flag rejects, and show the picks in Lightroom.
    #[prompt(name = "cull_shoot")]
    async fn cull_shoot(&self, Parameters(args): Parameters<CullPromptArgs>) -> Vec<PromptMessage> {
        let photos = args.photos.unwrap_or_else(|| "selection".into());
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Cull these photos in Lightroom: {photos}.\n\
                 1. Call get_status and make sure Lightroom is connected and changes are allowed.\n\
                 2. Resolve the photos (set \"selection\" or \"view\", or find them with \
                 find_photos_in_catalog / search_photos).\n\
                 3. Run cull_photos on them. Look at a few winners and rejects with view_photo \
                 before trusting the grades.\n\
                 4. Flag winners as picks and reject candidates as rejected with update_photos; \
                 leave groups marked keep_all alone.\n\
                 5. Show the picks with show_in_lightroom, and summarise what you did, including \
                 any warnings."
            ),
        )]
    }

    /// Find the best photos of one person and put them in a collection.
    #[prompt(name = "best_of_person")]
    async fn best_of_person(
        &self,
        Parameters(args): Parameters<BestOfPersonArgs>,
    ) -> Vec<PromptMessage> {
        let count = args.count.unwrap_or(20);
        let condition = args
            .condition
            .map(|c| format!(" Only consider photos {c}."))
            .unwrap_or_default();
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Find the {count} best photos of {person}.{condition}\n\
                 Use find_photos_of_people, narrow down with search_photos or \
                 find_photos_in_catalog if needed, then cull_photos to rank them. Check the top \
                 candidates with view_photo (eyes open, sharp, flattering). Put the final choice \
                 in a collection with show_in_lightroom named \"Best of {person}\" and tell me why \
                 you chose them.",
                person = args.person
            ),
        )]
    }

    /// Organise a fresh import: index it, then keyword, caption and cull it.
    #[prompt(name = "organize_import")]
    async fn organize_import(&self) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            Role::User,
            "Organise my latest import in Lightroom.\n\
             1. get_status, then find the photos: ask me which folder if it is not obvious, or \
             use set \"view\" if Lightroom is showing the import.\n\
             2. Index whatever is not indexed yet with analyze_photos.\n\
             3. Cull bursts and duplicates with cull_photos and flag the results.\n\
             4. Suggest collections by subject (search_photos, find_photos_of_people) and create \
             them with show_in_lightroom after I agree.\n\
             Report everything you changed and every warning.",
        )]
    }

    /// For developers: run the plugin's in-Lightroom tests and investigate
    /// failures.
    #[prompt(name = "test_plugin")]
    async fn test_plugin(
        &self,
        Parameters(args): Parameters<TestPluginArgs>,
    ) -> Vec<PromptMessage> {
        let focus = args
            .focus
            .map(|f| format!(" Focus on: {f}."))
            .unwrap_or_default();
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Test the LrGeniusAI plugin inside Lightroom.{focus}\n\
                 1. get_status: Lightroom must be connected; note the plugin version and the \
                 open catalog.\n\
                 2. The tests run in that catalog. Unless the user has told you it is a test \
                 catalog, ask before you test in it: name the catalog and say what the tests do. \
                 Test a production catalog only with the user's approval, and only within what \
                 they approved (read-only, or with catalog writes).\n\
                 3. run_plugin_tests (all suites; include_catalog_writes only in a test catalog \
                 or with approval for writes). For each failure, read the error, check get_logs \
                 (plugin and backend) around that time, and, if developer commands are on, probe \
                 with eval_lua.\n\
                 4. After code changes, reload_plugin_modules and run the failing suite again.\n\
                 Report: the catalog you tested in, passed/failed counts, each failure with its \
                 likely cause and the file to fix."
            ),
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_carry_the_current_aesthetic_score_and_genre() {
        let mut meta = Map::new();
        meta.insert(lrg_store::meta::AESTHETIC_SCORE_FIELD.into(), json!(7.1234));
        meta.insert(
            lrg_store::meta::AESTHETIC_MODEL_FIELD.into(),
            json!(search::AESTHETIC_MODEL_ID),
        );
        meta.insert(lrg_store::meta::GENRE_FIELD.into(), json!("landscape"));
        let s = summarize("p1", Some(&meta));
        assert_eq!(s["aesthetic"], 7.12);
        assert_eq!(s["genre"], "landscape");

        // A score from another export would rank wrongly next to current
        // ones, so summaries leave it out, like search does.
        meta.insert(
            lrg_store::meta::AESTHETIC_MODEL_FIELD.into(),
            json!("older-export"),
        );
        assert!(summarize("p1", Some(&meta)).get("aesthetic").is_none());
    }

    #[test]
    fn summaries_carry_what_tells_photos_apart() {
        let meta: Map<String, Value> = serde_json::from_value(json!({
            "filename": "IMG_0042.CR3",
            "capture_time": 1_700_000_000.0,
            "title": "Dog on the beach",
            "caption": "A dog runs along the shore at sunset.",
            "flattened_keywords": "dog, beach, sunset, ",
            "has_embedding": true,
        }))
        .unwrap();
        let s = summarize("p1", Some(&meta));
        assert_eq!(s["file_name"], "IMG_0042.CR3");
        assert_eq!(s["captured"], "2023-11-14 22:13");
        assert_eq!(s["keywords"], json!(["dog", "beach", "sunset"]));
        assert!(s.get("searchable").is_none());

        let unknown = summarize("p2", None);
        assert_eq!(unknown["indexed"], false);
    }

    #[test]
    fn truncation_marks_the_cut() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("a long caption text", 6), "a long…");
    }
}
