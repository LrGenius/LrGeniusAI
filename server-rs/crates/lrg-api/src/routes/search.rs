//! `/search` — port of `routes/search.py::search_route` +
//! `services/search.py::search_images`. Scope for M6: SigLIP2 semantic
//! search (brute-force cosine over `image_embeddings` — small enough
//! per-catalog to skip building a LanceDB ANN index for now) + in-memory
//! metadata substring search, merged and knee-filtered exactly like
//! Python.
//!
//! On top of that, the stored AestheticSigLIP score and genre: a search can be
//! ordered by them (`sort`), restricted by them (`min_aesthetic`, `genres`), and
//! needs no `term` at all — without one it is "the best N" of the scope, ranked
//! by aesthetic score from stored metadata alone (no text tower, no vectors).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Map, Value};

use lrg_analysis::relevance_filter::{
    clamp_max_results, clamp_strictness, smart_filter_by_relevance, ScoredResult,
};
use lrg_store::{meta, IMAGE_TABLE};

use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/search", axum::routing::get(search).post(search))
        .route("/search/genres", axum::routing::get(search_genres))
}

/// The genres a search can be restricted to, from the one list the classifier
/// itself uses, so a UI never offers one the backend cannot name.
async fn search_genres() -> Json<Value> {
    let mut genres: Vec<Value> = lrg_ml::genre::GENRES
        .iter()
        .map(|g| json!({"id": g.id, "label": g.label}))
        .collect();
    genres.push(json!({"id": lrg_ml::genre::OTHER_ID, "label": lrg_ml::genre::OTHER_LABEL}));
    Json(json!({"genres": genres}))
}

const DEFAULT_METADATA_FIELDS: [&str; 4] = ["flattened_keywords", "alt_text", "caption", "title"];

#[derive(serde::Deserialize, Default)]
struct SearchQuery {
    term: Option<String>,
    catalog_id: Option<String>,
    relevance_strictness: Option<i64>,
    max_results: Option<i64>,
}

fn cosine_distance(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let na: f64 = a
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    let nb: f64 = b
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    if na <= 0.0 || nb <= 0.0 {
        1.0
    } else {
        1.0 - dot / (na * nb)
    }
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round_ties_even() / 10000.0
}

/// How the results are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortMode {
    /// By the search's own distance; semantic hits first, then metadata hits.
    Relevance,
    /// By the stored aesthetic score, best first.
    Aesthetic,
    /// A weighted mix of relevance and aesthetic score.
    Combined,
}

impl SortMode {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "relevance" => Some(SortMode::Relevance),
            "aesthetic" => Some(SortMode::Aesthetic),
            "combined" => Some(SortMode::Combined),
            _ => None,
        }
    }
}

/// One search hit: a photo and, for a semantic hit, its distance.
struct Hit {
    photo_id: String,
    distance: Option<f64>,
}

/// The photo's aesthetic score, when it is from the current export. A score
/// from another export is not comparable with this one's, so it counts as none.
fn current_aesthetic(m: &Map<String, Value>) -> Option<f64> {
    let current = m.get(meta::AESTHETIC_MODEL_FIELD).and_then(Value::as_str)
        == Some(lrg_ml::aesthetic::MODEL_ID);
    m.get(meta::AESTHETIC_SCORE_FIELD)
        .and_then(Value::as_f64)
        .filter(|_| current)
}

fn genre_of(m: &Map<String, Value>) -> Option<&str> {
    m.get(meta::GENRE_FIELD)
        .and_then(Value::as_str)
        .filter(|g| !g.is_empty())
}

/// What `min_aesthetic` and `genres` ask of a photo.
#[derive(Default)]
struct Restrictions {
    min_aesthetic: Option<f64>,
    genres: Option<HashSet<String>>,
}

impl Restrictions {
    fn is_empty(&self) -> bool {
        self.min_aesthetic.is_none() && self.genres.is_none()
    }
}

/// What dropping photos for a restriction cost, for the user: the ones that
/// could not be judged at all, as opposed to the ones judged and found short.
#[derive(Default)]
struct Dropped {
    no_score: usize,
    no_genre: usize,
}

/// Applies `restrictions` to `hits`, counting the photos it had to drop for want
/// of a score or genre: those are a data problem the user can fix, where a photo
/// below the minimum is simply not a match.
fn restrict(
    hits: Vec<Hit>,
    restrictions: &Restrictions,
    rows: &HashMap<&str, &Map<String, Value>>,
) -> (Vec<Hit>, Dropped) {
    if restrictions.is_empty() {
        return (hits, Dropped::default());
    }
    let mut dropped = Dropped::default();
    let kept = hits
        .into_iter()
        .filter(|hit| {
            let Some(m) = rows.get(hit.photo_id.as_str()) else {
                return false;
            };
            if let Some(min) = restrictions.min_aesthetic {
                match current_aesthetic(m) {
                    Some(score) if score >= min => {}
                    Some(_) => return false,
                    None => {
                        dropped.no_score += 1;
                        return false;
                    }
                }
            }
            if let Some(wanted) = &restrictions.genres {
                match genre_of(m) {
                    Some(genre) if wanted.contains(genre) => {}
                    Some(_) => return false,
                    None => {
                        dropped.no_genre += 1;
                        return false;
                    }
                }
            }
            true
        })
        .collect();
    (kept, dropped)
}

/// Orders `hits`. Hits without a current score go last, in their earlier order:
/// they cannot be placed on a scale they are not on, and the caller says how
/// many there were.
fn order(
    mut hits: Vec<Hit>,
    sort: SortMode,
    aesthetic_weight: f64,
    rows: &HashMap<&str, &Map<String, Value>>,
) -> Vec<Hit> {
    if sort == SortMode::Relevance {
        return hits;
    }
    let score_of = |hit: &Hit| {
        rows.get(hit.photo_id.as_str())
            .and_then(|m| current_aesthetic(m))
    };

    // Relevance within this result set, 1 best: a semantic hit by where its
    // distance falls between the closest and the farthest, a metadata hit — a
    // literal match in the title, caption or keywords — as best.
    let distances: Vec<f64> = hits.iter().filter_map(|h| h.distance).collect();
    let (dmin, dmax) = distances
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(*d), hi.max(*d)));
    let relevance = |hit: &Hit| match hit.distance {
        Some(d) if dmax > dmin => 1.0 - (d - dmin) / (dmax - dmin),
        _ => 1.0,
    };
    let aesthetic_unit = |score: f64| {
        ((score - lrg_ml::aesthetic::SCORE_MIN as f64)
            / (lrg_ml::aesthetic::SCORE_MAX as f64 - lrg_ml::aesthetic::SCORE_MIN as f64))
            .clamp(0.0, 1.0)
    };

    let (mut scored, unscored): (Vec<(f64, Hit)>, Vec<Hit>) =
        hits.drain(..)
            .fold((Vec::new(), Vec::new()), |(mut s, mut u), hit| {
                match score_of(&hit) {
                    Some(score) => {
                        let key = match sort {
                            SortMode::Combined => {
                                (1.0 - aesthetic_weight) * relevance(&hit)
                                    + aesthetic_weight * aesthetic_unit(score)
                            }
                            _ => score,
                        };
                        s.push((key, hit));
                    }
                    None => u.push(hit),
                }
                (s, u)
            });
    // Stable, so equal scores keep the order the search gave them.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .map(|(_, hit)| hit)
        .chain(unscored)
        .collect()
}

fn bad_request(message: &str) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({"error": message})),
    )
        .into_response()
}

fn server_error(e: impl std::fmt::Display) -> Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": e.to_string()})),
    )
        .into_response()
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
    body: Option<Json<Value>>,
) -> Response {
    log::info!("Search request received");
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);

    // Optional now: without one this is "the best N", ordered by the stored
    // aesthetic score.
    let term = q
        .term
        .clone()
        .or_else(|| body.get("term").and_then(Value::as_str).map(str::to_string))
        .filter(|t| !t.trim().is_empty());

    let sort = match body.get("sort").and_then(Value::as_str) {
        Some(raw) => match SortMode::parse(raw) {
            Some(mode) => Some(mode),
            None => {
                return bad_request(&format!(
                    "Unknown sort {raw:?}; use \"relevance\", \"aesthetic\" or \"combined\""
                ))
            }
        },
        None => None,
    };
    // Relevance is how close a photo is to the term, so without one there is
    // nothing to be relevant to. Said rather than quietly reinterpreted.
    if term.is_none() && matches!(sort, Some(SortMode::Relevance | SortMode::Combined)) {
        return bad_request("sort \"relevance\" and \"combined\" need a search term");
    }
    let sort = sort.unwrap_or(if term.is_some() {
        SortMode::Relevance
    } else {
        SortMode::Aesthetic
    });
    let aesthetic_weight = body
        .get("aesthetic_weight")
        .and_then(Value::as_f64)
        .unwrap_or(0.5)
        .clamp(0.0, 1.0);
    let restrictions = Restrictions {
        min_aesthetic: body
            .get("min_aesthetic")
            .and_then(Value::as_f64)
            .filter(|v| *v > 0.0),
        genres: body
            .get("genres")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<HashSet<_>>()
            })
            .filter(|set| !set.is_empty()),
    };

    let photo_ids_to_search: Option<HashSet<String>> = body
        .get("photo_ids")
        .or_else(|| body.get("uuids"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        });

    let catalog_id = body
        .get("catalog_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(q.catalog_id)
        .filter(|s| !s.is_empty());

    let relevance_strictness = body
        .get("relevance_strictness")
        .and_then(Value::as_i64)
        .or(q.relevance_strictness);
    let max_results = body
        .get("max_results")
        .and_then(Value::as_i64)
        .or(q.max_results);
    let n_results = clamp_max_results(max_results) as usize;
    let strictness = clamp_strictness(relevance_strictness);

    let sources = body.get("search_sources");
    let semantic_siglip = sources
        .and_then(|s| s.get("semantic_siglip"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let metadata_enabled = sources
        .and_then(|s| s.get("metadata"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let metadata_fields: Vec<String> = sources
        .and_then(|s| s.get("metadata_fields"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter(|f| DEFAULT_METADATA_FIELDS.contains(f))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            DEFAULT_METADATA_FIELDS
                .iter()
                .map(|s| s.to_string())
                .collect()
        });

    let mut warnings: Vec<String> = Vec::new();
    let Some(store) = state.store() else {
        return Json(json!({"results": Value::Array(vec![])})).into_response();
    };

    // Both filters, always both. The metadata pass used to apply the catalog
    // filter only when no `photo_ids` list was given, so a request carrying
    // both leaked photos from other catalogs.
    let in_scope = |photo_id: &str, m: &Map<String, Value>| -> bool {
        if let Some(ids) = &photo_ids_to_search {
            if !ids.contains(photo_id) {
                return false;
            }
        }
        if let Some(cid) = &catalog_id {
            if !meta::parse_catalog_ids(m).contains(cid.as_str()) {
                return false;
            }
        }
        true
    };

    // The rows every later step reads metadata from. With a term the semantic
    // pass has already pulled them, vectors included (`scan_all` is around
    // 230 MB at 50k photos, so it is done once); otherwise a vector-free scan.
    let mut scanned_rows: Option<Vec<lrg_store::StoreRecord>> = None;
    let mut meta_rows: Option<Vec<(String, Map<String, Value>)>> = None;

    let mut hits: Vec<Hit> = Vec::new();

    if let Some(term) = &term {
        // 1. Semantic search (SigLIP2).
        let mut semantic_results: Vec<ScoredResult> = Vec::new();
        let mut semantic_ids: HashSet<String> = HashSet::new();
        if semantic_siglip {
            match state.siglip.embed_text(std::slice::from_ref(term)) {
                Ok(mut batch) => {
                    let mut query_vec = batch.remove(0);
                    lrg_ml::siglip::l2_normalize(&mut query_vec);

                    let records = match store.scan_all(IMAGE_TABLE).await {
                        Ok(r) => r,
                        Err(e) => return server_error(e),
                    };
                    let mut scored: Vec<ScoredResult> = records
                        .iter()
                        .filter(|r| meta::has_embedding(&r.metadata))
                        .filter(|r| in_scope(&r.id, &r.metadata))
                        .filter_map(|r| {
                            let v = r.vector.as_ref()?;
                            Some(ScoredResult {
                                photo_id: r.id.clone(),
                                distance: round4(cosine_distance(&query_vec, v)),
                            })
                        })
                        .collect();
                    scored.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
                    scored.truncate(n_results);

                    semantic_results = smart_filter_by_relevance(scored, Some(strictness));
                    semantic_ids = semantic_results
                        .iter()
                        .map(|r| r.photo_id.clone())
                        .collect();
                    scanned_rows = Some(records);
                }
                Err(e) => {
                    let msg = format!("SigLIP model not available: {e}");
                    log::info!("{msg}");
                    warnings.push(msg);
                }
            }
        }

        // 2. Metadata substring search (in-memory over metadata only, not vectors).
        let mut metadata_results: Vec<Hit> = Vec::new();
        // The "Max results" slider used to bind the semantic half only, so a
        // common substring returned the whole catalog however low it was set.
        let metadata_budget = n_results.saturating_sub(semantic_results.len());
        if metadata_enabled && metadata_budget > 0 {
            let term_lower = term.to_lowercase();
            // Only pay for a scan when the semantic pass did not already do one.
            if scanned_rows.is_none() {
                meta_rows = match store.scan_meta(IMAGE_TABLE).await {
                    Ok(r) => Some(r),
                    Err(e) => return server_error(e),
                };
            }
            let rows: Vec<(&str, &Map<String, Value>)> = match (&scanned_rows, &meta_rows) {
                (Some(records), _) => records
                    .iter()
                    .map(|r| (r.id.as_str(), &r.metadata))
                    .collect(),
                (None, Some(rows)) => rows.iter().map(|(id, m)| (id.as_str(), m)).collect(),
                (None, None) => Vec::new(),
            };
            for (photo_id, m) in rows {
                if metadata_results.len() >= metadata_budget {
                    break;
                }
                if !in_scope(photo_id, m) || semantic_ids.contains(photo_id) {
                    continue;
                }
                let matched = metadata_fields.iter().any(|field| {
                    m.get(field)
                        .and_then(Value::as_str)
                        .is_some_and(|v| v.to_lowercase().contains(&term_lower))
                });
                if matched {
                    metadata_results.push(Hit {
                        photo_id: photo_id.to_string(),
                        distance: None,
                    });
                }
            }
        }
        hits = semantic_results
            .into_iter()
            .map(|r| Hit {
                photo_id: r.photo_id,
                distance: Some(r.distance),
            })
            .collect();
        hits.extend(metadata_results);
    } else {
        // "The best N": every photo in scope is a candidate, ranked below by
        // its stored score. Only metadata is read.
        meta_rows = match store.scan_meta(IMAGE_TABLE).await {
            Ok(r) => Some(r),
            Err(e) => return server_error(e),
        };
        let mut unscored = 0usize;
        for (photo_id, m) in meta_rows.as_deref().unwrap_or_default() {
            if !in_scope(photo_id, m) {
                continue;
            }
            if current_aesthetic(m).is_some() {
                hits.push(Hit {
                    photo_id: photo_id.clone(),
                    distance: None,
                });
            } else {
                unscored += 1;
            }
        }
        if unscored > 0 {
            warnings.push(format!(
                "{unscored} photo(s) in scope have no aesthetic score from the current model, so \
                 they are not in these results. Run 'Analyze & Index Photos' on them to score \
                 them."
            ));
        }
    }

    // The metadata every later step reads, by photo id.
    let rows: HashMap<&str, &Map<String, Value>> = match (&scanned_rows, &meta_rows) {
        (Some(records), _) => records
            .iter()
            .map(|r| (r.id.as_str(), &r.metadata))
            .collect(),
        (None, Some(rows)) => rows.iter().map(|(id, m)| (id.as_str(), m)).collect(),
        (None, None) => HashMap::new(),
    };
    // A semantic-only search never scanned metadata; ordering or restricting by
    // aesthetic or genre needs it.
    let needs_rows = sort != SortMode::Relevance || !restrictions.is_empty();
    let fallback_rows: Option<Vec<(String, Map<String, Value>)>> =
        if rows.is_empty() && needs_rows && !hits.is_empty() {
            match store.scan_meta(IMAGE_TABLE).await {
                Ok(r) => Some(r),
                Err(e) => return server_error(e),
            }
        } else {
            None
        };
    let fallback_map: HashMap<&str, &Map<String, Value>> = fallback_rows
        .iter()
        .flatten()
        .map(|(id, m)| (id.as_str(), m))
        .collect();
    let rows = if rows.is_empty() { fallback_map } else { rows };

    let (hits, dropped) = restrict(hits, &restrictions, &rows);
    if dropped.no_score > 0 {
        warnings.push(format!(
            "{} matching photo(s) have no aesthetic score from the current model and were left \
             out by the minimum aesthetic score. Run 'Analyze & Index Photos' on them to score \
             them.",
            dropped.no_score
        ));
    }
    if dropped.no_genre > 0 {
        warnings.push(format!(
            "{} matching photo(s) have no genre yet and were left out by the genre filter. Run \
             'Analyze & Index Photos' with smart photo search on them.",
            dropped.no_genre
        ));
    }

    let mut hits = order(hits, sort, aesthetic_weight, &rows);
    if sort != SortMode::Relevance && term.is_some() {
        let unscored = hits
            .iter()
            .filter(|h| {
                rows.get(h.photo_id.as_str())
                    .and_then(|m| current_aesthetic(m))
                    .is_none()
            })
            .count();
        if unscored > 0 {
            warnings.push(format!(
                "{unscored} matching photo(s) have no aesthetic score from the current model, so \
                 they come last. Run 'Analyze & Index Photos' on them to score them."
            ));
        }
    }
    hits.truncate(n_results);

    let results: Vec<Value> = hits
        .into_iter()
        .map(|hit| {
            let m = rows.get(hit.photo_id.as_str());
            let genre = m.and_then(|m| genre_of(m));
            json!({
                "photo_id": hit.photo_id,
                "uuid": hit.photo_id,
                "distance": hit.distance,
                "aesthetic_score": m.and_then(|m| current_aesthetic(m)),
                "genre": genre,
                "genre_label": genre.and_then(lrg_ml::genre::label_for),
            })
        })
        .collect();

    let mut response = json!({"results": results});
    if !warnings.is_empty() {
        response["warnings"] = json!(warnings);
        response["warning"] = json!(warnings.join("\n\n"));
    }
    Json(response).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(score: Option<f64>, model: &str, genre: Option<&str>) -> Map<String, Value> {
        let mut m = Map::new();
        if let Some(score) = score {
            m.insert(meta::AESTHETIC_SCORE_FIELD.into(), json!(score));
            m.insert(meta::AESTHETIC_MODEL_FIELD.into(), json!(model));
        }
        if let Some(genre) = genre {
            m.insert(meta::GENRE_FIELD.into(), json!(genre));
        }
        m
    }

    fn hit(id: &str, distance: Option<f64>) -> Hit {
        Hit {
            photo_id: id.into(),
            distance,
        }
    }

    fn ids(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.photo_id.as_str()).collect()
    }

    fn rows_of<'a>(
        data: &'a [(&'static str, Map<String, Value>)],
    ) -> HashMap<&'a str, &'a Map<String, Value>> {
        data.iter().map(|(id, m)| (*id, m)).collect()
    }

    #[test]
    fn a_score_from_another_export_counts_as_none() {
        let current = lrg_ml::aesthetic::MODEL_ID;
        assert_eq!(current_aesthetic(&row(Some(6.5), current, None)), Some(6.5));
        assert_eq!(current_aesthetic(&row(Some(6.5), "another", None)), None);
        assert_eq!(current_aesthetic(&row(None, current, None)), None);
    }

    #[test]
    fn aesthetic_order_puts_the_best_first_and_unscored_last_in_their_order() {
        let c = lrg_ml::aesthetic::MODEL_ID;
        let data = [
            ("a", row(Some(5.0), c, None)),
            ("none1", row(None, c, None)),
            ("b", row(Some(7.0), c, None)),
            ("stale", row(Some(9.0), "another", None)),
            ("c", row(Some(6.0), c, None)),
        ];
        let rows = rows_of(&data);
        let hits = ["a", "none1", "b", "stale", "c"]
            .iter()
            .map(|id| hit(id, None))
            .collect();
        let ordered = order(hits, SortMode::Aesthetic, 0.5, &rows);
        // The other export's 9.0 does not buy "stale" a place at the top.
        assert_eq!(ids(&ordered), ["b", "c", "a", "none1", "stale"]);
    }

    #[test]
    fn relevance_order_is_left_alone() {
        let data = [("a", row(Some(9.0), lrg_ml::aesthetic::MODEL_ID, None))];
        let rows = rows_of(&data);
        let hits = vec![hit("x", Some(0.9)), hit("a", Some(0.95))];
        let ordered = order(hits, SortMode::Relevance, 0.5, &rows);
        assert_eq!(ids(&ordered), ["x", "a"]);
    }

    /// A mix of the two: the weight decides which one wins, and at the extremes
    /// it is exactly one of them.
    #[test]
    fn combined_order_follows_the_weight() {
        let c = lrg_ml::aesthetic::MODEL_ID;
        // `near` matches the term best but is plain; `pretty` matches worst and
        // scores highest.
        let data = [
            ("near", row(Some(4.0), c, None)),
            ("mid", row(Some(6.0), c, None)),
            ("pretty", row(Some(8.0), c, None)),
        ];
        let rows = rows_of(&data);
        let make = || {
            vec![
                hit("near", Some(0.80)),
                hit("mid", Some(0.90)),
                hit("pretty", Some(1.00)),
            ]
        };
        assert_eq!(
            ids(&order(make(), SortMode::Combined, 0.0, &rows)),
            ["near", "mid", "pretty"]
        );
        assert_eq!(
            ids(&order(make(), SortMode::Combined, 1.0, &rows)),
            ["pretty", "mid", "near"]
        );
        // In between the weight decides, by plain arithmetic. Relevance is the
        // distance's position in the result set (near 1, mid 0.5, pretty 0) and
        // aesthetic is (score - 1) / 8 (0.375, 0.625, 0.875):
        //   0.5: near 0.6875, mid 0.5625, pretty 0.4375
        //   0.7: near 0.5625, mid 0.5875, pretty 0.6125
        assert_eq!(
            ids(&order(make(), SortMode::Combined, 0.5, &rows)),
            ["near", "mid", "pretty"]
        );
        assert_eq!(
            ids(&order(make(), SortMode::Combined, 0.7, &rows)),
            ["pretty", "mid", "near"]
        );
    }

    /// A literal match in the title or keywords has no distance and counts as
    /// the most relevant a hit can be.
    #[test]
    fn a_metadata_hit_counts_as_fully_relevant_when_combining() {
        let c = lrg_ml::aesthetic::MODEL_ID;
        let data = [
            ("near", row(Some(6.0), c, None)),
            ("far", row(Some(6.0), c, None)),
            ("literal", row(Some(6.0), c, None)),
        ];
        let rows = rows_of(&data);
        // Equal scores, so relevance decides: the literal match ties with the
        // closest semantic hit (both 1.0, in their search order) and beats the
        // farthest (0.0).
        let hits = vec![
            hit("near", Some(0.80)),
            hit("far", Some(0.90)),
            hit("literal", None),
        ];
        let ordered = order(hits, SortMode::Combined, 0.5, &rows);
        assert_eq!(ids(&ordered), ["near", "literal", "far"]);
    }

    #[test]
    fn restrictions_drop_low_scores_and_other_genres_and_count_the_unjudgeable() {
        let c = lrg_ml::aesthetic::MODEL_ID;
        let data = [
            ("ok", row(Some(7.0), c, Some("macro"))),
            ("low", row(Some(5.0), c, Some("macro"))),
            ("wrong", row(Some(8.0), c, Some("street"))),
            ("noscore", row(None, c, Some("macro"))),
            ("nogenre", row(Some(8.0), c, None)),
        ];
        let rows = rows_of(&data);
        let hits = ["ok", "low", "wrong", "noscore", "nogenre"]
            .iter()
            .map(|id| hit(id, None))
            .collect();
        let restrictions = Restrictions {
            min_aesthetic: Some(6.0),
            genres: Some(["macro".to_string()].into_iter().collect()),
        };
        let (kept, dropped) = restrict(hits, &restrictions, &rows);
        assert_eq!(ids(&kept), ["ok"]);
        // `noscore` could not be judged against the minimum; `nogenre` was
        // judged on score, passed, and could not be judged on genre.
        assert_eq!(dropped.no_score, 1);
        assert_eq!(dropped.no_genre, 1);
    }

    #[test]
    fn no_restrictions_keep_everything() {
        let rows = HashMap::new();
        let (kept, dropped) = restrict(vec![hit("a", None)], &Restrictions::default(), &rows);
        assert_eq!(ids(&kept), ["a"]);
        assert_eq!((dropped.no_score, dropped.no_genre), (0, 0));
    }

    #[test]
    fn sort_names_parse_and_unknown_ones_do_not() {
        assert_eq!(SortMode::parse(" Aesthetic "), Some(SortMode::Aesthetic));
        assert_eq!(SortMode::parse("combined"), Some(SortMode::Combined));
        assert_eq!(SortMode::parse("relevance"), Some(SortMode::Relevance));
        assert_eq!(SortMode::parse("best"), None);
    }
}
