//! Tools over the backend's own index: status, semantic search, similar
//! photos, people, culling and result-set bookkeeping. They work without
//! Lightroom (scoped to the open catalog when it is connected), because the
//! index lives in the backend.

use std::collections::HashSet;
use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use lrg_api::agent_bridge::{BridgeError, CallOptions};
use lrg_api::routes::search;

use crate::server::{
    json_result, string_list, tool_error, LrGeniusMcp, Photos, MAX_PHOTOS_PER_CALL,
};
use crate::sets::{combine, SetOperation};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// What to look for, in plain words: "dog running on a beach", "red car
    /// at night", "people laughing at a table". Matches the image content
    /// and the photos' AI titles, captions and keywords. Leave it out to rank
    /// photos by aesthetic score alone ("the best N", optionally of some
    /// genres or within a set).
    #[serde(default)]
    pub query: Option<String>,
    /// Most photos to return (default 100, between 10 and 2000).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Only search these photos.
    #[serde(default)]
    pub within: Option<Photos>,
    /// How hard to cut weak matches, 0-100 (default 50). Lower keeps more
    /// loosely related photos; 0 turns the cut off.
    #[serde(default)]
    pub strictness: Option<i64>,
    /// Order of the results: "relevance" (best match first; the default with
    /// a query), "aesthetic" (LrGeniusAI's aesthetic score, best first; the
    /// default without a query) or "combined" (a mix of both, see
    /// aesthetic_weight; needs a query). Photos without a score come last.
    #[serde(default)]
    pub sort: Option<SearchSort>,
    /// For sort "combined": how much the aesthetic score counts against the
    /// match, 0-1 (default 0.5). 0 is pure relevance, 1 pure aesthetics.
    #[serde(default)]
    pub aesthetic_weight: Option<f64>,
    /// Only photos with at least this aesthetic score, 1-9. Typical photos
    /// score about 4.5-7; the score sorts and thins out a user's own photos,
    /// it is not a verdict. Photos without a score are left out.
    #[serde(default)]
    pub min_aesthetic: Option<f64>,
    /// Only photos of these genres: landscape, portrait, street, wildlife,
    /// sport, event, architecture, macro, food, still_life, astro, concert,
    /// aerial or other.
    #[serde(default)]
    pub genres: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchSort {
    /// Best match first.
    Relevance,
    /// Highest aesthetic score first.
    Aesthetic,
    /// A weighted mix of match and aesthetic score.
    Combined,
}

impl SearchSort {
    fn name(self) -> &'static str {
        match self {
            SearchSort::Relevance => "relevance",
            SearchSort::Aesthetic => "aesthetic",
            SearchSort::Combined => "combined",
        }
    }
}

/// The `/v1/search` body for `args`, validated, and a label describing it.
/// Separate from the tool so the checks are unit-testable without a backend.
fn search_body(args: &SearchArgs) -> Result<(Value, String), String> {
    let query = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let sort = args.sort.unwrap_or(if query.is_some() {
        SearchSort::Relevance
    } else {
        SearchSort::Aesthetic
    });
    if query.is_none() && sort != SearchSort::Aesthetic {
        return Err(format!(
            "sort \"{}\" ranks photos by how well they match a query, so it needs one. Without a \
             query, use sort \"aesthetic\" (the best photos), optionally with genres.",
            sort.name()
        ));
    }

    let mut body = json!({
        "max_results": args.limit.unwrap_or(100),
        "sort": sort.name(),
    });
    let mut parts: Vec<String> = Vec::new();
    match query {
        Some(q) => {
            body["term"] = json!(q);
            parts.push(format!("search \"{q}\""));
        }
        None => parts.push("best photos by aesthetic score".into()),
    }
    if query.is_some() && sort != SearchSort::Relevance {
        parts.push(format!("sorted by {}", sort.name()));
    }
    if let Some(strictness) = args.strictness {
        body["relevance_strictness"] = json!(strictness);
    }
    if let Some(weight) = args.aesthetic_weight {
        if sort != SearchSort::Combined {
            return Err("aesthetic_weight only applies to sort \"combined\".".into());
        }
        if !(0.0..=1.0).contains(&weight) {
            return Err("aesthetic_weight must be between 0 and 1.".into());
        }
        body["aesthetic_weight"] = json!(weight);
    }
    if let Some(min) = args.min_aesthetic {
        let (lo, hi) = search::AESTHETIC_RANGE;
        if !(lo as f64..=hi as f64).contains(&min) {
            return Err(format!("min_aesthetic must be between {lo} and {hi}."));
        }
        body["min_aesthetic"] = json!(min);
        parts.push(format!("aesthetic ≥ {min}"));
    }
    if let Some(genres) = &args.genres {
        let known = search::genre_ids();
        let mut wanted: Vec<String> = Vec::new();
        for g in genres {
            let id = g.trim().to_lowercase().replace([' ', '-'], "_");
            if id.is_empty() {
                continue;
            }
            if !known.contains(&id.as_str()) {
                return Err(format!(
                    "Unknown genre \"{g}\". Use one of: {}.",
                    known.join(", ")
                ));
            }
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        if !wanted.is_empty() {
            parts.push(format!("genre {}", wanted.join("/")));
            body["genres"] = json!(wanted);
        }
    }
    Ok((body, parts.join(", ")))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SimilarMode {
    /// Photos that look alike: same kind of subject, scene or style.
    Similar,
    /// Near-identical frames: the same shot, burst neighbours, exports.
    Duplicates,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SimilarArgs {
    /// The photo to compare against.
    pub photo_id: String,
    /// "similar" (default) or "duplicates".
    #[serde(default)]
    pub mode: Option<SimilarMode>,
    /// Most photos to return (default 50, at most 2000).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Only look among these photos.
    #[serde(default)]
    pub within: Option<Photos>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPeopleArgs {
    /// Also list face clusters nobody has named yet (default false).
    #[serde(default)]
    pub include_unnamed: Option<bool>,
    /// Most people to list (default 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PeopleMatch {
    /// Photos with at least one of the people.
    Any,
    /// Photos with all of them together.
    All,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeopleArgs {
    /// Names as shown by list_people (case does not matter), or person IDs.
    pub people: Vec<String>,
    /// "any" (default) or "all".
    #[serde(default, rename = "match")]
    pub match_mode: Option<PeopleMatch>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CullArgs {
    #[serde(flatten)]
    pub photos: Photos,
    /// What kind of shoot it is, which tunes grouping and grading:
    /// "default", "event", "portrait", "sports" or "street".
    #[serde(default)]
    pub preset: Option<String>,
    /// Only group, without grading the frames inside each group. Faster.
    #[serde(default)]
    pub group_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CombineOperation {
    Union,
    Intersection,
    Difference,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CombineArgs {
    /// First set (e.g. "s3", or "selection").
    pub a: String,
    /// Second set.
    pub b: String,
    /// "union" (in either), "intersection" (in both) or "difference" (in a
    /// but not in b).
    pub operation: CombineOperation,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSetArgs {
    pub set_id: String,
    /// Index of the first photo to list (default 0).
    #[serde(default)]
    pub offset: Option<usize>,
    /// How many to list (default 50, at most 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MakeSetArgs {
    #[serde(flatten)]
    pub photos: Photos,
    /// A short description to remember the set by.
    #[serde(default)]
    pub label: Option<String>,
}

/// Percent-encodes a query-string value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[tool_router(router = index_tools, vis = "pub(crate)")]
impl LrGeniusMcp {
    /// Start here. Whether Lightroom is connected, which catalog is open,
    /// what is selected, how much of the catalog LrGeniusAI has indexed, which
    /// AI models are ready, and whether you may change photos.
    #[tool(
        name = "get_status",
        annotations(title = "Status", read_only_hint = true)
    )]
    async fn get_status(&self) -> Result<CallToolResult, McpError> {
        let bridge = self.state().agent_bridge.status();
        let catalog_id = self.catalog_id();
        let stats_path = match &catalog_id {
            Some(id) => format!("/v1/db/stats?catalog_id={}", encode(id)),
            None => "/v1/db/stats".to_string(),
        };
        let index = self.api().get(&stats_path).await;
        let health = self.api().get("/v1/server/health").await;

        let mut status = json!({
            "backend": {
                "version": lrg_common::version::backend_version(),
                "catalog_database_open": self.state().db_path().is_some(),
            },
        });

        match index {
            Ok(stats) => status["index"] = stats,
            Err(e) => status["index"] = json!({"error": e.message}),
        }
        if let Ok(h) = health {
            status["models"] = json!({
                "image_search": h.get("clip_model"),
                "faces": h.get("face_model"),
                "species": h.get("species_model"),
            });
        }

        if bridge.connected {
            let mut lightroom = json!({
                "connected": true,
                "allow_changes": bridge.plugin.as_ref().is_some_and(|p| p.allow_changes),
                "developer_commands": bridge.plugin.as_ref().is_some_and(|p| p.dev_tools),
                "socket": bridge.socket,
            });
            match self
                .lightroom(
                    "status",
                    json!({}),
                    CallOptions::read(Duration::from_secs(20)),
                    None,
                )
                .await
            {
                Ok(reply) => {
                    if let (Some(obj), Some(extra)) =
                        (lightroom.as_object_mut(), reply.result.as_object())
                    {
                        for (k, v) in extra {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                    if !reply.warnings.is_empty() {
                        lightroom["warnings"] = json!(reply.warnings);
                    }
                }
                Err(e) => lightroom["error"] = json!(e.to_string()),
            }
            // How much of the open catalog search can actually see. The
            // index keeps photos that left the catalog (the backend never
            // deletes photo data), so its count can exceed the catalog's.
            let in_catalog = lightroom["catalog"]["photo_count"].as_u64();
            let indexed = status["index"]["photos"]["with_embedding"].as_u64();
            if let (Some(total), Some(indexed)) = (in_catalog, indexed) {
                status["coverage"] = json!(if indexed < total {
                    format!(
                        "At most {indexed} of the {total} photos in this catalog are indexed for \
                         search; analyze_photos indexes the rest."
                    )
                } else {
                    format!(
                        "The index holds {indexed} photos for this catalog, which has {total} now; \
                         photos removed from the catalog stay in the index. Photos added since the \
                         last indexing are not searchable until analyze_photos runs on them \
                         (get_photo_details shows \"indexed\": false for those)."
                    )
                });
            }
            status["lightroom"] = lightroom;
        } else {
            status["lightroom"] = json!({
                "connected": false,
                "how_to_connect": BridgeError::NotConnected.to_string(),
            });
        }
        Ok(json_result(status))
    }

    /// Semantic search over the photos LrGeniusAI has indexed: describe what
    /// is in the picture. Can also rank by LrGeniusAI's aesthetic score and
    /// keep only some genres; without a query it returns the best photos.
    /// Returns a result set (e.g. "s3") and a preview that shows each
    /// photo's aesthetic score and genre.
    #[tool(
        name = "search_photos",
        annotations(title = "Search photos", read_only_hint = true)
    )]
    async fn search_photos(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (mut body, label) = match search_body(&args) {
            Ok(b) => b,
            Err(e) => return Ok(tool_error(e)),
        };
        if let Some(cid) = self.catalog_id() {
            body["catalog_id"] = json!(cid);
        }
        let mut scope_label = String::new();
        if let Some(within) = &args.within {
            match self.resolve_photos(within).await {
                Ok(r) => {
                    scope_label = format!(" within {}", r.label);
                    body["photo_ids"] = json!(r.photo_ids);
                }
                Err(e) => return Ok(tool_error(e)),
            }
        }
        let response = match self.api().post("/v1/search", body).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(format!("Search failed: {e}"))),
        };
        let results = response["results"].as_array().cloned().unwrap_or_default();
        let ids: Vec<String> = results
            .iter()
            .filter_map(|r| r["photo_id"].as_str().map(str::to_string))
            .collect();
        let set = self.sets().create(format!("{label}{scope_label}"), ids);
        let mut extra = json!({});
        let warnings = string_list(&response["warnings"]);
        if !warnings.is_empty() {
            extra["warnings"] = json!(warnings);
        } else if let Some(w) = response["warning"].as_str() {
            extra["warnings"] = json!([w]);
        }
        if set.photo_ids.is_empty() {
            extra["hint"] = json!(
                "Nothing matched. Try other words, a lower strictness or min_aesthetic, fewer \
                 genres, or check get_status: photos that are not indexed cannot be found, and \
                 photos without an aesthetic score or genre are left out by those filters."
            );
        }
        Ok(json_result(self.set_answer(&set, extra).await))
    }

    /// Photos that look like a given photo ("similar"), or near-identical
    /// frames of it ("duplicates"). Returns a result set.
    #[tool(
        name = "find_similar_photos",
        annotations(title = "Find similar photos", read_only_hint = true)
    )]
    async fn find_similar_photos(
        &self,
        Parameters(args): Parameters<SimilarArgs>,
    ) -> Result<CallToolResult, McpError> {
        let duplicates = matches!(args.mode, Some(SimilarMode::Duplicates));
        let mut body = json!({
            "photo_id": args.photo_id,
            "max_results": args.limit.unwrap_or(50),
            "similarity_mode": if duplicates { "phash" } else { "clip" },
        });
        if let Some(cid) = self.catalog_id() {
            body["catalog_id"] = json!(cid);
        }
        if let Some(within) = &args.within {
            match self.resolve_photos(within).await {
                Ok(r) => body["scope_photo_ids"] = json!(r.photo_ids),
                Err(e) => return Ok(tool_error(e)),
            }
        }
        let response = match self.api().post("/v1/search/similar", body).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(format!("Finding similar photos failed: {e}"))),
        };
        let ids: Vec<String> = response["results"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|r| r["photo_id"].as_str().map(str::to_string))
                    .filter(|id| *id != args.photo_id)
                    .collect()
            })
            .unwrap_or_default();
        let label = if duplicates {
            format!("near-duplicates of {}", args.photo_id)
        } else {
            format!("photos similar to {}", args.photo_id)
        };
        let set = self.sets().create(label, ids);
        let mut extra = json!({});
        if let Some(w) = response["warning"].as_str() {
            extra["warnings"] = json!([w]);
        }
        Ok(json_result(self.set_answer(&set, extra).await))
    }

    /// The people LrGeniusAI recognises from face detection, with photo
    /// counts. Names come from the user's People page.
    #[tool(
        name = "list_people",
        annotations(title = "List people", read_only_hint = true)
    )]
    async fn list_people(
        &self,
        Parameters(args): Parameters<ListPeopleArgs>,
    ) -> Result<CallToolResult, McpError> {
        let response = match self.api().get("/v1/faces/persons").await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(format!("Could not list people: {e}"))),
        };
        let include_unnamed = args.include_unnamed.unwrap_or(false);
        let limit = args.limit.unwrap_or(100).max(1);
        let all = response["persons"].as_array().cloned().unwrap_or_default();
        let mut unnamed = 0usize;
        let people: Vec<Value> = all
            .iter()
            .filter(|p| !p["person_id"].as_str().unwrap_or("").is_empty())
            .filter(|p| {
                let named = !p["name"].as_str().unwrap_or("").is_empty();
                if !named {
                    unnamed += 1;
                }
                named || include_unnamed
            })
            .take(limit)
            .map(|p| {
                json!({
                    "person_id": p["person_id"],
                    "name": p["name"],
                    "photo_count": p["photo_count"],
                })
            })
            .collect();
        let mut answer = json!({"people": people});
        if !include_unnamed && unnamed > 0 {
            answer["unnamed_clusters"] = json!(unnamed);
            answer["hint"] = json!(
                "Unnamed face clusters are hidden; pass include_unnamed to list them. The user \
                 names people on LrGeniusAI's People page."
            );
        }
        if people.is_empty() && all.is_empty() {
            answer["hint"] = json!(
                "No faces are indexed yet. Analyze photos with face detection on (analyze_photos \
                 with tasks [\"faces\"]), then group them on the People page."
            );
        }
        Ok(json_result(answer))
    }

    /// Photos showing one or more people, by name or person ID. Returns a
    /// result set, limited to the open catalog when Lightroom is connected.
    #[tool(
        name = "find_photos_of_people",
        annotations(title = "Find photos of people", read_only_hint = true)
    )]
    async fn find_photos_of_people(
        &self,
        Parameters(args): Parameters<PeopleArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.people.is_empty() {
            return Ok(tool_error("Name at least one person."));
        }
        let persons = match self.api().get("/v1/faces/persons").await {
            Ok(r) => r["persons"].as_array().cloned().unwrap_or_default(),
            Err(e) => return Ok(tool_error(format!("Could not list people: {e}"))),
        };
        let mut resolved = Vec::new();
        let mut unknown = Vec::new();
        for wanted in &args.people {
            let w = wanted.trim();
            let hit = persons.iter().find(|p| {
                p["person_id"].as_str() == Some(w)
                    || p["name"]
                        .as_str()
                        .is_some_and(|n| !n.is_empty() && n.eq_ignore_ascii_case(w))
            });
            match hit {
                Some(p) => resolved.push((
                    p["person_id"].as_str().unwrap_or_default().to_string(),
                    p["name"].as_str().unwrap_or(w).to_string(),
                )),
                None => unknown.push(w.to_string()),
            }
        }
        if !unknown.is_empty() {
            return Ok(tool_error(format!(
                "No person named {}. Call list_people to see the names LrGeniusAI knows.",
                unknown
                    .iter()
                    .map(|u| format!("\"{u}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        let all = matches!(args.match_mode, Some(PeopleMatch::All));
        let mut combined: Option<Vec<String>> = None;
        for (person_id, _) in &resolved {
            let path = format!("/v1/faces/persons/{}/photos", encode(person_id));
            let ids = match self.api().get(&path).await {
                Ok(r) => string_list(&r["photo_ids"]),
                Err(e) => return Ok(tool_error(format!("Could not load photos: {e}"))),
            };
            combined = Some(match combined {
                None => ids,
                Some(prev) => combine(
                    &prev,
                    &ids,
                    if all {
                        SetOperation::Intersection
                    } else {
                        SetOperation::Union
                    },
                ),
            });
        }
        let mut ids = combined.unwrap_or_default();

        // Faces are not stored per catalog; the photos are.
        if let (Some(cid), Some(store)) = (self.catalog_id(), self.state().store()) {
            let rows = store
                .get_meta(lrg_store::IMAGE_TABLE, &ids)
                .await
                .unwrap_or_default();
            let in_catalog: HashSet<String> = rows
                .into_iter()
                .filter(|(_, m)| lrg_store::meta::parse_catalog_ids(m).contains(cid.as_str()))
                .map(|(id, _)| id)
                .collect();
            ids.retain(|id| in_catalog.contains(id));
        }

        let names: Vec<&str> = resolved.iter().map(|(_, n)| n.as_str()).collect();
        let label = format!(
            "photos of {}",
            names.join(if all { " and " } else { " or " })
        );
        let set = self.sets().create(label, ids);
        Ok(json_result(self.set_answer(&set, json!({})).await))
    }

    /// Culling: groups bursts, duplicates and near-identical frames, ranks
    /// each group (sharpness, eyes, expression, exposure) and names a winner,
    /// alternates and reject candidates with reasons. Returns sets for picks,
    /// alternates and rejects. Changes nothing — flag the results with
    /// update_photos.
    #[tool(
        name = "cull_photos",
        annotations(title = "Cull photos", read_only_hint = true)
    )]
    async fn cull_photos(
        &self,
        Parameters(args): Parameters<CullArgs>,
    ) -> Result<CallToolResult, McpError> {
        let resolved = match self.resolve_photos(&args.photos).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        if resolved.photo_ids.len() > MAX_PHOTOS_PER_CALL {
            return Ok(tool_error(format!(
                "That is {} photos; cull at most {MAX_PHOTOS_PER_CALL} at a time (one shoot or \
                 day at a time works best).",
                resolved.photo_ids.len()
            )));
        }
        let group_only = args.group_only.unwrap_or(false);
        let mut body = json!({"photo_ids": resolved.photo_ids});
        if let Some(preset) = &args.preset {
            body["culling_preset"] = json!(preset);
        }
        if let Some(cid) = self.catalog_id() {
            body["catalog_id"] = json!(cid);
        }
        let path = if group_only {
            "/v1/cull/groups"
        } else {
            "/v1/cull/grade"
        };
        let response = match self.api().post(path, body).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(format!("Culling failed: {e}"))),
        };

        let groups = response["groups"].as_array().cloned().unwrap_or_default();
        let mut singles = 0usize;
        let mut picks = Vec::new();
        let mut alternates = Vec::new();
        let mut rejects = Vec::new();
        let mut keep_all = Vec::new();
        let mut group_summaries = Vec::new();
        for g in &groups {
            let ids = string_list(&g["photo_ids"]);
            if g["keep_all"].as_bool().unwrap_or(false) {
                keep_all.extend(ids.iter().cloned());
            }
            if let Some(w) = g["winner_photo_id"].as_str() {
                picks.push(w.to_string());
            }
            alternates.extend(string_list(&g["alternate_photo_ids"]));
            rejects.extend(string_list(&g["reject_candidate_photo_ids"]));
            // A photo alone in its "group" is a pick by definition; listing
            // each one would bury the groups that need a decision.
            if g["group_size"].as_u64() == Some(1) {
                singles += 1;
                continue;
            }
            let reasons: Vec<Value> = g["photos"]
                .as_array()
                .map(|photos| {
                    photos
                        .iter()
                        .filter(|p| {
                            p["winner"].as_bool() == Some(true)
                                || p["reject_candidate"].as_bool() == Some(true)
                        })
                        .map(|p| {
                            json!({
                                "photo_id": p["photo_id"],
                                "verdict": if p["winner"].as_bool() == Some(true) { "pick" } else { "reject" },
                                "why": p["explanation"],
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            group_summaries.push(json!({
                "group_id": g["group_id"],
                "type": g["group_type"],
                "size": g["group_size"],
                "keep_all": g["keep_all"],
                "winner": g["winner_photo_id"],
                "verdicts": reasons,
            }));
        }
        let label = resolved.label.clone();
        let picks_set = self.sets().create(format!("cull picks of {label}"), picks);
        let alternates_set = self
            .sets()
            .create(format!("cull alternates of {label}"), alternates);
        let rejects_set = self
            .sets()
            .create(format!("cull reject candidates of {label}"), rejects);
        let mut answer = json!({
            "summary": response["summary"],
            "sets": {
                "picks": {"set_id": picks_set.id, "count": picks_set.photo_ids.len()},
                "alternates": {"set_id": alternates_set.id, "count": alternates_set.photo_ids.len()},
                "reject_candidates": {"set_id": rejects_set.id, "count": rejects_set.photo_ids.len()},
            },
            "groups": group_summaries.iter().take(60).cloned().collect::<Vec<_>>(),
            "single_photos": singles,
        });
        if !keep_all.is_empty() {
            let set = self.sets().create(
                format!("intentional sets (brackets, panoramas) in {label}"),
                keep_all,
            );
            answer["sets"]["keep_all"] = json!({"set_id": set.id, "count": set.photo_ids.len()});
            answer["note"] = json!(
                "Groups with keep_all are brackets, focus stacks or panoramas: every frame is \
                 wanted, so do not reject any of them."
            );
        }
        if group_summaries.len() > 60 {
            answer["more_groups"] = json!(group_summaries.len() - 60);
        }
        if let Some(warnings) = response["warnings"].as_array().filter(|w| !w.is_empty()) {
            answer["warnings"] = Value::Array(warnings.clone());
        }
        Ok(json_result(answer))
    }

    /// Combines two result sets into a new one.
    #[tool(
        name = "combine_sets",
        annotations(title = "Combine sets", read_only_hint = true)
    )]
    async fn combine_sets(
        &self,
        Parameters(args): Parameters<CombineArgs>,
    ) -> Result<CallToolResult, McpError> {
        let a = match self
            .resolve_photos(&Photos {
                set: Some(args.a.clone()),
                photo_ids: None,
            })
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        let b = match self
            .resolve_photos(&Photos {
                set: Some(args.b.clone()),
                photo_ids: None,
            })
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        let (op, word) = match args.operation {
            CombineOperation::Union => (SetOperation::Union, "or"),
            CombineOperation::Intersection => (SetOperation::Intersection, "and"),
            CombineOperation::Difference => (SetOperation::Difference, "but not"),
        };
        let ids = combine(&a.photo_ids, &b.photo_ids, op);
        let set = self
            .sets()
            .create(format!("({}) {word} ({})", a.label, b.label), ids);
        Ok(json_result(self.set_answer(&set, json!({})).await))
    }

    /// Lists the photos of a result set, a page at a time, with what the
    /// index knows about each.
    #[tool(
        name = "get_set",
        annotations(title = "List a result set", read_only_hint = true)
    )]
    async fn get_set(
        &self,
        Parameters(args): Parameters<GetSetArgs>,
    ) -> Result<CallToolResult, McpError> {
        let Some(set) = self.sets().get(&args.set_id) else {
            return Ok(tool_error(format!(
                "There is no result set \"{}\". Run the search again to get a new one.",
                args.set_id
            )));
        };
        let offset = args.offset.unwrap_or(0).min(set.photo_ids.len());
        let limit = args.limit.unwrap_or(50).clamp(1, 200);
        let page: Vec<String> = set
            .photo_ids
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        let next = offset + page.len();
        let mut answer = json!({
            "set_id": set.id,
            "label": set.label,
            "count": set.photo_ids.len(),
            "offset": offset,
            "photos": self.summaries(&page).await,
        });
        if next < set.photo_ids.len() {
            answer["next_offset"] = json!(next);
        }
        Ok(json_result(answer))
    }

    /// Saves photos as a result set, e.g. to freeze the current Lightroom
    /// selection ("selection") before working on it, or to name a hand-made
    /// list of photo IDs.
    #[tool(
        name = "make_set",
        annotations(title = "Make a result set", read_only_hint = true)
    )]
    async fn make_set(
        &self,
        Parameters(args): Parameters<MakeSetArgs>,
    ) -> Result<CallToolResult, McpError> {
        let resolved = match self.resolve_photos(&args.photos).await {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        let label = args.label.unwrap_or(resolved.label);
        let set = self.sets().create(label, resolved.photo_ids);
        Ok(json_result(self.set_answer(&set, json!({})).await))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(encode("abc-1_2.3~"), "abc-1_2.3~");
        assert_eq!(encode("a b/c&d"), "a%20b%2Fc%26d");
    }

    fn args(v: Value) -> SearchArgs {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_query_alone_searches_by_relevance_as_before() {
        let (body, label) = search_body(&args(json!({"query": " lake at dawn "}))).unwrap();
        assert_eq!(body["term"], "lake at dawn");
        assert_eq!(body["sort"], "relevance");
        assert_eq!(body["max_results"], 100);
        assert!(body.get("genres").is_none());
        assert_eq!(label, "search \"lake at dawn\"");
    }

    #[test]
    fn without_a_query_it_ranks_by_aesthetic_score() {
        let (body, label) =
            search_body(&args(json!({"genres": ["Landscape"], "limit": 10}))).unwrap();
        assert!(body.get("term").is_none());
        assert_eq!(body["sort"], "aesthetic");
        assert_eq!(body["genres"], json!(["landscape"]));
        assert_eq!(body["max_results"], 10);
        assert_eq!(label, "best photos by aesthetic score, genre landscape");

        // An empty query is no query.
        let (body, _) = search_body(&args(json!({"query": "  "}))).unwrap();
        assert_eq!(body["sort"], "aesthetic");
    }

    #[test]
    fn relevance_and_combined_need_a_query() {
        for sort in ["relevance", "combined"] {
            let err = search_body(&args(json!({"sort": sort}))).unwrap_err();
            assert!(err.contains("needs one"), "{sort}: {err}");
        }
    }

    #[test]
    fn combined_takes_a_weight_and_only_combined_does() {
        let (body, label) = search_body(&args(
            json!({"query": "fog", "sort": "combined", "aesthetic_weight": 0.7}),
        ))
        .unwrap();
        assert_eq!(body["aesthetic_weight"], 0.7);
        assert_eq!(label, "search \"fog\", sorted by combined");
        assert!(search_body(&args(json!({"query": "fog", "aesthetic_weight": 0.7}))).is_err());
        assert!(search_body(&args(
            json!({"query": "fog", "sort": "combined", "aesthetic_weight": 1.5})
        ))
        .is_err());
    }

    #[test]
    fn min_aesthetic_is_checked_against_the_score_range() {
        let (body, label) = search_body(&args(json!({"min_aesthetic": 6.5}))).unwrap();
        assert_eq!(body["min_aesthetic"], 6.5);
        assert!(label.contains("aesthetic ≥ 6.5"));
        assert!(search_body(&args(json!({"min_aesthetic": 0.5}))).is_err());
        assert!(search_body(&args(json!({"min_aesthetic": 10}))).is_err());
    }

    #[test]
    fn genres_are_normalised_deduplicated_and_unknown_ones_named() {
        let (body, _) = search_body(&args(
            json!({"genres": ["still life", "Still-Life", "astro", ""]}),
        ))
        .unwrap();
        assert_eq!(body["genres"], json!(["still_life", "astro"]));
        let err = search_body(&args(json!({"genres": ["landscapes"]}))).unwrap_err();
        assert!(
            err.contains("landscapes") && err.contains("landscape,"),
            "{err}"
        );
    }

    /// The genre list in `genres`' description is written out by hand; it
    /// must name exactly the genres the backend knows.
    #[test]
    fn the_documented_genres_are_the_real_ones() {
        let schema = serde_json::to_value(schemars::schema_for!(SearchArgs)).unwrap();
        let described = schema["properties"]["genres"]["description"]
            .as_str()
            .unwrap()
            .to_string();
        let listed: Vec<String> = described
            .split_once("genres:")
            .unwrap()
            .1
            .replace(" or ", ", ")
            .trim_end_matches('.')
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let known: Vec<String> = search::genre_ids().iter().map(|s| s.to_string()).collect();
        assert_eq!(listed, known);
    }
}
