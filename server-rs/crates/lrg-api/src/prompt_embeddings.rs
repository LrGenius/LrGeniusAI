//! Text embeddings of the fixed prompts photos are scored against, kept in the
//! database so that indexing, culling and the edit routes never need the
//! SigLIP2 text tower.
//!
//! Four things score a stored image embedding against fixed text: the
//! CLIP-IQA prompt sets culling ranks with, the organism gate in front of
//! BioCLIP, the scene probes the training and style routes tag with, and the
//! genre classifier indexing names a photo's kind with. The
//! text side of each is a pure function of the model and a compile-time list
//! of strings, yet each used to run the text tower to get it — about a
//! gigabyte loaded into an indexing run that otherwise only uses the image
//! tower, and kept resident for half an hour after its one call.
//!
//! [`prepare`] makes the prompts a request needs available, cheapest source
//! first: this process's memory, then the bound database's `text_embeddings`
//! table, and only for what neither holds, the text tower — whose results are
//! written to the table, and which is dropped again straight afterwards when
//! it was this call that loaded it. So each prompt is embedded once per model
//! per database, and in normal use only a free-text search loads the tower.
//!
//! Only the prompts the request needs, not every fixed prompt at once: on a
//! CPU without fast fp16 a single text-tower pass can take many seconds, and
//! the first cull after this change must not take longer than it did before
//! it — the culling request has a five-minute timeout in the plug-in.
//!
//! A request runs [`prepare`] once, before its per-photo work, and the scoring
//! paths read [`PromptCache::current`], which is synchronous because they are.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use lrg_analysis::training::{scene_tags_from_similarities, SCENE_PROBES};
use lrg_ml::clip_iqa::{IqaPrompts, PromptSet};
use lrg_ml::genre::{GenreClassifier, GenreResult};
use lrg_ml::siglip::{l2_normalize, EMBED_DIM, MODEL_ID};
use lrg_store::TextEmbedding;

use crate::state::AppState;

/// What a missing SigLIP2 model means for these prompts, phrased as the fix.
const MODEL_MISSING: &str = "the AI search model is not downloaded yet — run \
     \"Download AI models\" in Plug-in Manager";

/// Every fixed text an image embedding is scored against, each once.
///
/// Public so the contract tests can seed the table the way an earlier run
/// would have.
pub fn fixed_prompts() -> Vec<&'static str> {
    let mut texts = texts_of(&PromptSet::ALL);
    texts.extend(scene_probe_texts());
    texts.extend(genre_texts());
    dedup(texts)
}

/// The texts of `sets`, each once: what a caller scoring against those sets
/// asks [`prepare`] for.
pub(crate) fn texts_of(sets: &[PromptSet]) -> Vec<&'static str> {
    dedup(sets.iter().flat_map(|set| set.texts()).collect())
}

/// The genres' prompt texts, what [`PromptEmbeddings::genre`] needs.
pub(crate) fn genre_texts() -> Vec<&'static str> {
    lrg_ml::genre::texts().collect()
}

/// The scene probes' texts, what [`PromptEmbeddings::scene_tags`] needs.
pub(crate) fn scene_probe_texts() -> Vec<&'static str> {
    SCENE_PROBES.iter().map(|(_, text)| *text).collect()
}

fn dedup(texts: Vec<&'static str>) -> Vec<&'static str> {
    let mut seen = HashSet::new();
    texts
        .into_iter()
        .filter(|text| seen.insert(*text))
        .collect()
}

/// The fixed prompts' embeddings, ready to score with.
///
/// Immutable: [`prepare`] replaces the whole value rather than editing it, so
/// a reader never sees one half-filled.
#[derive(Default)]
pub struct PromptEmbeddings {
    /// L2-normalized, keyed by the prompt text.
    by_text: HashMap<&'static str, Vec<f32>>,
    /// Every set whose prompts are all present, built once here rather than
    /// per photo.
    sets: HashMap<PromptSet, IqaPrompts>,
    /// The genre classifier, when every genre prompt is present.
    genre: Option<GenreClassifier>,
    /// Why some prompts have no embedding, phrased for the user.
    problem: Option<String>,
}

impl PromptEmbeddings {
    fn new(by_text: HashMap<&'static str, Vec<f32>>, problem: Option<String>) -> Self {
        let sets = PromptSet::ALL
            .iter()
            .filter_map(|&set| {
                IqaPrompts::from_lookup(set, |text| by_text.get(text).map(Vec::as_slice))
                    .map(|prompts| (set, prompts))
            })
            .collect();
        let genre = GenreClassifier::from_lookup(|text| by_text.get(text).map(Vec::as_slice));
        PromptEmbeddings {
            by_text,
            sets,
            genre,
            problem,
        }
    }

    fn has_all(&self, texts: &[&'static str]) -> bool {
        texts.iter().all(|text| self.by_text.contains_key(text))
    }

    /// One prompt set, or `None` when any of its prompts has no embedding.
    pub(crate) fn prompt_set(&self, set: PromptSet) -> Option<&IqaPrompts> {
        self.sets.get(&set)
    }

    /// [`IqaPrompts::score`] for one set; `None` when the set is unavailable
    /// or the embedding cannot be scored.
    pub(crate) fn score(&self, set: PromptSet, embedding: &[f32]) -> Option<f64> {
        self.prompt_set(set)?.score(embedding)
    }

    /// The photo's genre, or `None` when the genre prompts are not all
    /// available or the embedding cannot be scored.
    pub(crate) fn genre(&self, embedding: &[f32]) -> Option<GenreResult> {
        self.genre.as_ref()?.classify(embedding)
    }

    /// Why prompts are missing, when they are, phrased for the user.
    pub(crate) fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Port of `compute_scene_tags`: the scene probes whose similarity to
    /// `image_embedding` clears the threshold, in probe order.
    ///
    /// Empty when any probe has no embedding, as it was when the text tower
    /// failed: tags from a partial probe set would read as "not that kind of
    /// scene" for the probes that are missing.
    pub(crate) fn scene_tags(&self, image_embedding: &[f32]) -> Vec<String> {
        let mut similarities = Vec::with_capacity(SCENE_PROBES.len());
        for &(name, text) in SCENE_PROBES {
            let Some(text_embedding) = self.by_text.get(text) else {
                return Vec::new();
            };
            let similarity: f64 = image_embedding
                .iter()
                .zip(text_embedding)
                .map(|(a, b)| *a as f64 * *b as f64)
                .sum();
            similarities.push((name.to_string(), similarity));
        }
        scene_tags_from_similarities(&similarities)
    }
}

/// This process's [`PromptEmbeddings`], plus what [`prepare`] needs in order
/// to skip work it has already done.
#[derive(Default)]
pub struct PromptCache {
    current: RwLock<Arc<PromptEmbeddings>>,
    /// Held across a whole [`prepare`], so two requests starting together do
    /// not both load the text tower for the same prompts.
    stored: tokio::sync::Mutex<Stored>,
}

/// Which prompts the bound database is known to hold already.
#[derive(Default)]
struct Stored {
    /// The lance root `texts` describes. Binding another catalog's database
    /// changes it and `texts` starts over, so that database's table is filled
    /// from memory rather than from the text tower.
    root: Option<PathBuf>,
    texts: HashSet<&'static str>,
}

impl PromptCache {
    /// The embeddings the last [`prepare`] left, possibly empty.
    pub(crate) fn current(&self) -> Arc<PromptEmbeddings> {
        Arc::clone(&self.current.read().unwrap())
    }
}

/// Makes the embeddings of `wanted` available, and returns everything this
/// process holds.
///
/// Never fails as such: what could not be found or made is missing from the
/// result, and [`PromptEmbeddings::problem`] says why. Each caller has a
/// fallback for a missing set, and whether that fallback is worth telling the
/// user about is the caller's decision.
pub(crate) async fn prepare(state: &AppState, wanted: &[&'static str]) -> Arc<PromptEmbeddings> {
    let cache = &state.prompt_embeddings;
    let mut stored = cache.stored.lock().await;
    let store = state.store();
    let root = store.as_ref().and(state.lance_root());
    if stored.root != root {
        stored.root = root;
        stored.texts.clear();
    }

    let current = cache.current();
    if current.has_all(wanted)
        && (store.is_none() || wanted.iter().all(|text| stored.texts.contains(text)))
    {
        return current;
    }

    let mut by_text = current.by_text.clone();

    if let Some(store) = &store {
        let unknown: Vec<String> = wanted
            .iter()
            .filter(|text| !stored.texts.contains(*text))
            .map(|text| text.to_string())
            .collect();
        if !unknown.is_empty() {
            match store.get_text_embeddings(MODEL_ID, &unknown).await {
                Ok(rows) => {
                    for row in rows {
                        let Some(&text) = wanted.iter().find(|text| **text == row.text) else {
                            continue;
                        };
                        // Same model, wrong size, would be a bug in whatever
                        // wrote it; scoring with it would quietly produce
                        // nothing, so it is recomputed and overwritten instead.
                        if row.vector.len() != EMBED_DIM {
                            continue;
                        }
                        stored.texts.insert(text);
                        by_text.entry(text).or_insert_with(|| {
                            let mut vector = row.vector;
                            l2_normalize(&mut vector);
                            vector
                        });
                    }
                }
                // The text tower below can still make them.
                Err(e) => log::warn!("Could not read stored prompt embeddings: {e}"),
            }
        }
    }

    let missing: Vec<&'static str> = wanted
        .iter()
        .copied()
        .filter(|text| !by_text.contains_key(text))
        .collect();
    let mut problem = None;
    if !missing.is_empty() {
        match embed_with_text_tower(state, &missing).await {
            Ok(vectors) => {
                log::info!(
                    "Embedded {} fixed prompt(s) with the SigLIP2 text tower",
                    missing.len()
                );
                by_text.extend(missing.iter().copied().zip(vectors));
            }
            Err(e) => {
                log::warn!("Fixed prompt embeddings unavailable: {e}");
                problem = Some(e);
            }
        }
    }

    if let Some(store) = &store {
        let to_store: Vec<(&'static str, TextEmbedding)> = wanted
            .iter()
            .filter(|text| !stored.texts.contains(*text))
            .filter_map(|&text| {
                by_text.get(text).map(|vector| {
                    let row = TextEmbedding {
                        model_id: MODEL_ID.to_string(),
                        text: text.to_string(),
                        vector: vector.clone(),
                    };
                    (text, row)
                })
            })
            .collect();
        let rows: Vec<TextEmbedding> = to_store.iter().map(|(_, row)| row.clone()).collect();
        match store.put_text_embeddings(&rows).await {
            Ok(()) => {
                if !rows.is_empty() {
                    log::info!("Stored {} prompt embedding(s) for {MODEL_ID}", rows.len());
                }
                stored.texts.extend(to_store.iter().map(|(text, _)| *text));
            }
            // Not reported to the user: nothing they asked for is any worse.
            // The vectors are in memory for this process and the next
            // `prepare` retries the write; only a restart before then costs a
            // text-tower run.
            Err(e) => log::warn!("Could not store prompt embeddings: {e}"),
        }
    }

    let prepared = Arc::new(PromptEmbeddings::new(by_text, problem));
    *cache.current.write().unwrap() = Arc::clone(&prepared);
    prepared
}

/// Runs the text tower over `texts` and returns the vectors normalized.
///
/// Drops the tower again afterwards unless it was already resident: the run
/// that asked has no further use for it, and a search that had it loaded
/// keeps it.
async fn embed_with_text_tower(
    state: &AppState,
    texts: &[&'static str],
) -> Result<Vec<Vec<f32>>, String> {
    if !state.siglip.is_cached() {
        return Err(MODEL_MISSING.to_string());
    }
    let siglip = Arc::clone(&state.siglip);
    let texts: Vec<String> = texts.iter().map(|text| text.to_string()).collect();
    let result = tokio::task::spawn_blocking(move || {
        let was_loaded = siglip.text_status().0 == "loaded";
        let result = siglip.embed_text(&texts);
        if !was_loaded {
            siglip.unload_text();
        }
        result
    })
    .await
    .map_err(|e| format!("the prompt embedding task failed ({e})"))?;
    let mut vectors = result.map_err(|e| {
        format!(
            "the AI search model's text encoder could not be loaded ({e}) — running \
             \"Download AI models\" in Plug-in Manager again replaces damaged files"
        )
    })?;
    for vector in &mut vectors {
        l2_normalize(vector);
    }
    Ok(vectors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_prompts_cover_every_set_and_probe_once() {
        let prompts = fixed_prompts();
        let unique: HashSet<&str> = prompts.iter().copied().collect();
        assert_eq!(unique.len(), prompts.len(), "a prompt is listed twice");
        for set in PromptSet::ALL {
            for text in set.texts() {
                assert!(unique.contains(text), "{set:?} prompt {text:?} missing");
            }
        }
        for (_, text) in SCENE_PROBES {
            assert!(unique.contains(text), "scene probe {text:?} missing");
        }
        for text in lrg_ml::genre::texts() {
            assert!(unique.contains(text), "genre prompt {text:?} missing");
        }
    }

    /// A request asks for its own sets' prompts and nothing else: on a slow
    /// CPU every extra text is seconds of text-tower time.
    #[test]
    fn texts_of_lists_exactly_the_sets_asked_for() {
        let texts = texts_of(&[PromptSet::Organism]);
        assert_eq!(texts, PromptSet::Organism.texts().collect::<Vec<_>>());
        let both = texts_of(&[PromptSet::Quality, PromptSet::Action]);
        assert_eq!(
            both.len(),
            2 * (PromptSet::Quality.pairs().len() + PromptSet::Action.pairs().len())
        );
        assert!(!both.contains(&PromptSet::Organism.texts().next().unwrap()));
    }

    fn embeddings_for(texts: &[&'static str]) -> HashMap<&'static str, Vec<f32>> {
        texts
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let mut v = vec![0.0f32; 4];
                v[i % 4] = 1.0;
                (*text, v)
            })
            .collect()
    }

    #[test]
    fn a_complete_cache_builds_every_set() {
        let all = PromptEmbeddings::new(embeddings_for(&fixed_prompts()), None);
        assert!(all.has_all(&fixed_prompts()));
        for set in PromptSet::ALL {
            assert!(all.prompt_set(set).is_some(), "{set:?}");
        }
    }

    /// A set with any prompt missing is unavailable, and the others are not
    /// held back by it.
    #[test]
    fn a_missing_prompt_takes_out_only_its_own_set() {
        let dropped = PromptSet::Organism.texts().next().unwrap();
        let texts: Vec<&'static str> = fixed_prompts()
            .into_iter()
            .filter(|text| *text != dropped)
            .collect();
        let partial = PromptEmbeddings::new(embeddings_for(&texts), Some("why".into()));
        assert!(!partial.has_all(&fixed_prompts()));
        assert!(partial.prompt_set(PromptSet::Organism).is_none());
        assert!(partial.prompt_set(PromptSet::Quality).is_some());
        assert_eq!(partial.problem(), Some("why"));
    }

    /// The genre classifier exists exactly when every genre prompt does.
    #[test]
    fn genre_needs_every_genre_prompt() {
        let all = PromptEmbeddings::new(embeddings_for(&fixed_prompts()), None);
        let image = vec![1.0f32, 0.0, 0.0, 0.0];
        assert!(all.genre(&image).is_some());

        let dropped = lrg_ml::genre::texts().next().unwrap();
        let texts: Vec<&'static str> = fixed_prompts()
            .into_iter()
            .filter(|text| *text != dropped)
            .collect();
        let partial = PromptEmbeddings::new(embeddings_for(&texts), None);
        assert!(partial.genre(&image).is_none());
    }

    #[test]
    fn scene_tags_need_every_probe() {
        let probes: Vec<&'static str> = SCENE_PROBES.iter().map(|(_, t)| *t).collect();
        let mut by_text: HashMap<&'static str, Vec<f32>> = HashMap::new();
        // Probe 0 points the same way as the image; every other is orthogonal.
        for (i, text) in probes.iter().enumerate() {
            by_text.insert(
                *text,
                if i == 0 {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                },
            );
        }
        let image = [1.0f32, 0.0];

        let full = PromptEmbeddings::new(by_text.clone(), None);
        assert_eq!(full.scene_tags(&image), vec![SCENE_PROBES[0].0.to_string()]);

        by_text.remove(probes[5]);
        let partial = PromptEmbeddings::new(by_text, None);
        assert!(partial.scene_tags(&image).is_empty());
    }
}
