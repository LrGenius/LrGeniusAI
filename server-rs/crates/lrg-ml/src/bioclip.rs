//! ort-backed BioCLIP 2 (ViT-L/14) image tower plus a precomputed
//! Tree-of-Life zero-shot head.
//!
//! BioCLIP is a CLIP, not a classifier. Taxonomy comes from a matrix of text
//! embeddings — one row per taxon, each row the encoding of a full Linnean
//! path — and classification is cosine against that matrix. So this module
//! loads *two* things behind one lazy-load gate: the ONNX image tower, and
//! the head ([`TaxaHead`]).
//!
//! The head shipped with the plugin is **pruned**: upstream has 867,455 taxa
//! (2.66 GB fp32), which is larger than the model. `scripts/
//! bioclip_taxa_filter.toml` documents which taxa survive and, more
//! importantly, what pruning costs — [`classify`](BioclipModel::classify)
//! sums probability mass over surviving rows only, so a dropped clade does
//! not become an "unknown", it becomes a wrong-but-similar answer at the
//! species rank. The per-rank confidence floor is the defence: the deepest
//! rank that clears it is what gets reported, so an unrecognised beetle lands
//! on "Coleoptera" rather than on a confident wrong species.
//!
//! Follows the same lifecycle contract as [`crate::siglip`] and
//! [`crate::faces`]: lazy load, 30-minute idle unload, `/health`-style
//! status.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ort::session::Session;
use ort::value::Tensor;

use crate::bioclip_pre;

pub const EMBED_DIM: usize = 768;
const IDLE_UNLOAD_SECONDS: i64 = 30 * 60;

/// The seven Linnean ranks, coarsest first. Index into this array is the
/// `rank` used throughout this module.
pub const RANKS: [&str; 7] = [
    "kingdom", "phylum", "class", "order", "family", "genus", "species",
];
const RANK_COUNT: usize = RANKS.len();
const GENUS: usize = 5;
const SPECIES: usize = 6;

/// `bioclip2_taxa.bin` header, written by `scripts/export_bioclip2_fp16.py`.
/// Keep both sides in sync — there is no negotiation, just the magic and the
/// version.
const TAXA_MAGIC: &[u8; 8] = b"LRGTAXA\0";
const TAXA_FORMAT_VERSION: u32 = 1;
const TAXA_HEADER_BYTES: usize = 28;

#[derive(Debug, thiserror::Error)]
pub enum BioclipError {
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),
    #[error("taxonomy head: {0}")]
    Head(String),
    #[error("model not loaded")]
    NotLoaded,
}

pub struct BioclipModelPaths {
    pub image_onnx: PathBuf,
    pub taxa_bin: PathBuf,
    pub taxa_json: PathBuf,
}

impl BioclipModelPaths {
    pub fn all_exist(&self) -> bool {
        self.image_onnx.is_file() && self.taxa_bin.is_file() && self.taxa_json.is_file()
    }
}

/// How deep to report, and how many runners-up to keep.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyConfig {
    /// Aggregated probability a rank's best node must reach to be reported.
    pub min_confidence: f32,
    /// Number of alternatives to return at the reported rank.
    pub top_k: usize,
}

impl Default for ClassifyConfig {
    fn default() -> Self {
        // 0.35 is deliberately below "more likely than not": rank aggregation
        // spreads mass across near-identical congeners, so a correct genus
        // call routinely tops out around 0.4-0.6 even when the model is
        // right. Tuned against the pruned head, not the full one.
        ClassifyConfig {
            min_confidence: 0.35,
            top_k: 3,
        }
    }
}

/// One candidate at the reported rank.
#[derive(Debug, Clone)]
pub struct TaxonCandidate {
    /// Full path from kingdom down to the reported rank, `>`-joined.
    pub taxonomy: String,
    /// Binomial at species rank, single name above it.
    pub scientific_name: String,
    /// GBIF vernacular name, empty when the taxon has none.
    pub common_name: String,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct TaxonomyPrediction {
    /// Index into [`RANKS`], or `None` when nothing cleared the floor.
    pub rank: Option<usize>,
    pub best: Option<TaxonCandidate>,
    pub alternatives: Vec<TaxonCandidate>,
}

impl TaxonomyPrediction {
    /// `"species"` / `"genus"` / ... / `"none"`, for the metadata blob.
    pub fn rank_label(&self) -> &'static str {
        self.rank.map(|r| RANKS[r]).unwrap_or("none")
    }
}

/// Labels as written by `write_taxa_labels`: per-rank interned vocabularies
/// plus one index tuple per row. Interning is not just a size win — rank
/// aggregation compares `u32` ids instead of strings on every photo.
#[derive(serde::Deserialize)]
struct TaxaLabelsFile {
    version: u32,
    taxa_version: String,
    vocab: Vec<Vec<String>>,
    rows: Vec<Vec<u32>>,
    common: Vec<String>,
}

/// The precomputed zero-shot head: taxa embeddings + labels + the prefix
/// grouping that makes per-rank aggregation a flat array scan.
struct TaxaHead {
    n_rows: usize,
    logit_scale_exp: f32,
    /// Row-major `[n_rows, EMBED_DIM]`, L2-normalized, kept as raw fp16 bits.
    ///
    /// Held at the file's own precision rather than dequantized to f32 at
    /// load, which halves it: 1.5 KB per row instead of 3 KB, so ~258 MB
    /// rather than ~517 MB for the shipped 168k-row head. Scoring goes
    /// through [`f16_lut`], a 256 KB table that stays L2-resident.
    ///
    /// Measured on the shipped head (arm64, release): `classify` 44 ms
    /// against `embed_image` 110 ms, so the head is ~28% of a species pass.
    /// Bounded memory is worth that here — this project's repeated indexing
    /// failures have all been memory, never throughput.
    matrix: Vec<u16>,
    vocab: Vec<Vec<String>>,
    rows: Vec<Vec<u32>>,
    common: Vec<String>,
    taxa_version: String,
    /// `node_of[rank][row]` = dense id of the row's ancestor at `rank`. Two
    /// rows share an id exactly when their paths agree down to that rank.
    node_of: Vec<Vec<u32>>,
    /// `node_repr[rank][node]` = any row carrying that node, for label lookup.
    node_repr: Vec<Vec<u32>>,
}

fn read_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

impl TaxaHead {
    fn load(bin: &Path, json: &Path) -> Result<TaxaHead, BioclipError> {
        let raw = std::fs::read(bin)
            .map_err(|e| BioclipError::Head(format!("{}: {e}", bin.display())))?;
        if raw.len() < TAXA_HEADER_BYTES || &raw[0..8] != TAXA_MAGIC {
            return Err(BioclipError::Head(format!(
                "{}: not a taxa head (bad magic)",
                bin.display()
            )));
        }
        let version = read_u32(&raw, 8);
        if version != TAXA_FORMAT_VERSION {
            return Err(BioclipError::Head(format!(
                "{}: format version {version}, expected {TAXA_FORMAT_VERSION}",
                bin.display()
            )));
        }
        let n_rows = read_u32(&raw, 12) as usize;
        let dim = read_u32(&raw, 16) as usize;
        let logit_scale_exp = f32::from_le_bytes([raw[20], raw[21], raw[22], raw[23]]);
        if dim != EMBED_DIM {
            return Err(BioclipError::Head(format!(
                "{}: head dim {dim}, expected {EMBED_DIM}",
                bin.display()
            )));
        }
        let expected = TAXA_HEADER_BYTES + n_rows * dim * 2;
        if raw.len() != expected {
            return Err(BioclipError::Head(format!(
                "{}: {} bytes, expected {expected} for {n_rows}x{dim} fp16",
                bin.display(),
                raw.len()
            )));
        }

        let matrix: Vec<u16> = raw[TAXA_HEADER_BYTES..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();

        let text = std::fs::read_to_string(json)
            .map_err(|e| BioclipError::Head(format!("{}: {e}", json.display())))?;
        let labels: TaxaLabelsFile = serde_json::from_str(&text)
            .map_err(|e| BioclipError::Head(format!("{}: {e}", json.display())))?;
        if labels.version != TAXA_FORMAT_VERSION {
            return Err(BioclipError::Head(format!(
                "{}: format version {}, expected {TAXA_FORMAT_VERSION}",
                json.display(),
                labels.version
            )));
        }
        if labels.rows.len() != n_rows || labels.common.len() != n_rows {
            return Err(BioclipError::Head(format!(
                "{} has {} label rows but {} has {n_rows} embedding rows",
                json.display(),
                labels.rows.len(),
                bin.display()
            )));
        }
        if labels.vocab.len() != RANK_COUNT {
            return Err(BioclipError::Head(format!(
                "{}: {} rank vocabularies, expected {RANK_COUNT}",
                json.display(),
                labels.vocab.len()
            )));
        }
        for (row_i, row) in labels.rows.iter().enumerate() {
            if row.len() != RANK_COUNT {
                return Err(BioclipError::Head(format!(
                    "{}: row {row_i} has {} ranks, expected {RANK_COUNT}",
                    json.display(),
                    row.len()
                )));
            }
            for (rank, &id) in row.iter().enumerate() {
                if id as usize >= labels.vocab[rank].len() {
                    return Err(BioclipError::Head(format!(
                        "{}: row {row_i} rank {rank} id {id} out of vocabulary",
                        json.display()
                    )));
                }
            }
        }

        let (node_of, node_repr) = build_prefix_nodes(&labels.rows);

        log::info!(
            "Loaded BioCLIP taxonomy head '{}': {n_rows} taxa, {} genera, {:.1} MB resident",
            labels.taxa_version,
            node_repr[GENUS].len(),
            (matrix.len() * 2) as f64 / 1e6
        );

        Ok(TaxaHead {
            n_rows,
            logit_scale_exp,
            matrix,
            vocab: labels.vocab,
            rows: labels.rows,
            common: labels.common,
            taxa_version: labels.taxa_version,
            node_of,
            node_repr,
        })
    }

    /// `[kingdom, ..., rank]` names for the node a row belongs to.
    ///
    /// Ranks are not always populated: 15.6% of the shipped head has at least
    /// one blank, most of it the bony fish, which carry no `class` at all in
    /// TreeOfLife-200M. Callers that render the path must skip the blanks;
    /// this returns them so [`Self::name_at`] can still ask whether the
    /// *reported* rank has a name of its own.
    fn path_of(&self, row: usize, rank: usize) -> Vec<&str> {
        (0..=rank)
            .map(|r| self.vocab[r][self.rows[row][r] as usize].as_str())
            .collect()
    }

    /// The row's own name at `rank`, empty when that rank is unpopulated.
    fn name_at(&self, row: usize, rank: usize) -> &str {
        self.vocab[rank][self.rows[row][rank] as usize].as_str()
    }

    fn candidate(&self, row: usize, rank: usize, confidence: f32) -> TaxonCandidate {
        let path = self.path_of(row, rank);
        // Blank ranks are dropped rather than rendered, so a fish reads
        // `Animalia>Chordata>Perciformes>...` instead of carrying an empty
        // segment between two separators.
        let rendered: Vec<&str> = path.iter().copied().filter(|p| !p.is_empty()).collect();
        // Upstream stores the species *epithet* only; the binomial is genus +
        // epithet, exactly as pybioclip's `create_classification_dict` builds
        // it.
        let scientific_name = if rank == SPECIES {
            format!("{} {}", path[GENUS], path[SPECIES])
        } else {
            path[rank].to_string()
        };
        TaxonCandidate {
            taxonomy: rendered.join(">"),
            scientific_name,
            // A common name identifies a species, not a family — reporting
            // "Great Tit" for a genus-rank call would overstate the answer.
            common_name: if rank == SPECIES {
                self.common[row].clone()
            } else {
                String::new()
            },
            confidence,
        }
    }
}

/// Group rows into per-rank prefix nodes.
///
/// A node at rank `r` is uniquely identified by `(its parent node, its own
/// vocabulary id)`, which is what makes this one linear pass instead of
/// hashing seven growing key slices per row.
fn build_prefix_nodes(rows: &[Vec<u32>]) -> (Vec<Vec<u32>>, Vec<Vec<u32>>) {
    let mut node_of = vec![vec![0u32; rows.len()]; RANK_COUNT];
    let mut node_repr = vec![Vec::new(); RANK_COUNT];

    for rank in 0..RANK_COUNT {
        let mut seen: HashMap<(u32, u32), u32> = HashMap::new();
        for (row_i, row) in rows.iter().enumerate() {
            let parent = if rank == 0 {
                0
            } else {
                node_of[rank - 1][row_i]
            };
            let key = (parent, row[rank]);
            let next = seen.len() as u32;
            let id = *seen.entry(key).or_insert_with(|| {
                node_repr[rank].push(row_i as u32);
                next
            });
            node_of[rank][row_i] = id;
        }
    }
    (node_of, node_repr)
}

/// Dequantization table for every one of the 65,536 fp16 bit patterns.
///
/// Built once per process, shared by every head. 256 KB, small enough to stay
/// in L2 across a scoring pass, which is what makes storing the head at fp16
/// affordable — the alternative is converting ~130M values per photo with
/// branchy arithmetic in the inner loop.
fn f16_lut() -> &'static [f32; 65536] {
    static LUT: std::sync::OnceLock<Box<[f32; 65536]>> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut table = Box::new([0.0f32; 65536]);
        for (bits, slot) in table.iter_mut().enumerate() {
            *slot = f16_to_f32(bits as u16);
        }
        table
    })
}

/// IEEE 754 binary16 -> binary32.
///
/// Hand-rolled rather than pulled from `half`: this runs once per head load
/// over ~77M values, and the crate's conversion is only intrinsic-backed when
/// the target enables the `fp16` feature, which is not guaranteed across the
/// platforms this ships on.
fn f16_to_f32(bits: u16) -> f32 {
    let negative = bits & 0x8000 != 0;
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = (bits >> 10) & 0x1f;
    let mant = (bits & 0x03ff) as u32;
    match exp {
        // Zero or subnormal: the value is mant * 2^-24, and 0x33800000 is
        // 2^-24 as an f32 bit pattern. Covers +/-0.0 with mant == 0.
        0 => {
            let v = (mant as f32) * f32::from_bits(0x3380_0000);
            if negative {
                -v
            } else {
                v
            }
        }
        // Inf / NaN: keep the mantissa so a NaN payload stays a NaN.
        0x1f => f32::from_bits(sign | 0x7f80_0000 | (mant << 13)),
        // Normal: rebias the exponent from fp16's 15 to fp32's 127.
        _ => f32::from_bits(sign | ((exp as u32 + 112) << 23) | (mant << 13)),
    }
}

struct Loaded {
    image_session: Session,
    head: TaxaHead,
}

/// Lazily-loaded, idle-unloadable BioCLIP 2 handle. One instance is shared
/// behind an `Arc` in `AppState`.
pub struct BioclipModel {
    paths: BioclipModelPaths,
    loaded: Mutex<Option<Loaded>>,
    last_used_unix: AtomicI64,
    load_error: Mutex<Option<String>>,
    /// Cached from the last successful load so `species_model` can be stamped
    /// on photos without forcing the head into memory.
    taxa_version: Mutex<Option<String>>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// CPU-only, arena disabled, KleidiAI per the crate-wide policy — see
/// [`crate::faces::build_session`]'s writeup for why the arena is off.
///
/// No CoreML here even on macOS: this graph has a dynamic batch axis, which
/// is exactly the case `siglip::build_session` documents as not qualifying
/// for CoreML's MLProgram path.
fn build_session(path: &Path) -> ort::Result<Session> {
    let builder = Session::builder()?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])?;
    crate::apply_kleidiai_policy(builder)?.commit_from_file(path)
}

impl BioclipModel {
    pub fn new(paths: BioclipModelPaths) -> Self {
        BioclipModel {
            paths,
            loaded: Mutex::new(None),
            last_used_unix: AtomicI64::new(0),
            load_error: Mutex::new(None),
            taxa_version: Mutex::new(None),
        }
    }

    pub fn is_cached(&self) -> bool {
        self.paths.all_exist()
    }

    fn ensure_loaded(&self) -> Result<(), BioclipError> {
        let mut guard = self.loaded.lock().unwrap();
        if guard.is_some() {
            self.last_used_unix.store(now_unix(), Ordering::Relaxed);
            return Ok(());
        }
        let result = (|| -> Result<Loaded, BioclipError> {
            let head = TaxaHead::load(&self.paths.taxa_bin, &self.paths.taxa_json)?;
            let image_session = build_session(&self.paths.image_onnx)?;
            Ok(Loaded {
                image_session,
                head,
            })
        })();
        match result {
            Ok(loaded) => {
                *self.taxa_version.lock().unwrap() = Some(loaded.head.taxa_version.clone());
                *guard = Some(loaded);
                *self.load_error.lock().unwrap() = None;
                self.last_used_unix.store(now_unix(), Ordering::Relaxed);
                Ok(())
            }
            Err(e) => {
                *self.load_error.lock().unwrap() = Some(e.to_string());
                Err(e)
            }
        }
    }

    /// Drops the loaded session(s). Safe to call when nothing is loaded.
    ///
    /// The reason is a parameter because this is reached two ways — the
    /// `/unload` route and the idle sweep — and the log line used to say
    /// "due to inactivity" for both. A real `/unload` from the plugin then
    /// looked like an idle timeout in the log, which is the kind of small lie
    /// that costs an hour the next time someone reads it.
    fn unload_with_reason(&self, reason: &str) {
        let mut guard = self.loaded.lock().unwrap();
        if guard.take().is_some() {
            log::info!("Unloading BioCLIP 2 model ({reason})");
        }
    }

    pub fn unload(&self) {
        self.unload_with_reason("requested");
    }

    /// Call periodically (e.g. every 60s) from a background task.
    pub fn unload_if_idle(&self) {
        let last = self.last_used_unix.load(Ordering::Relaxed);
        if last != 0 && now_unix() - last >= IDLE_UNLOAD_SECONDS {
            self.unload_with_reason("idle");
        }
    }

    pub fn status(&self) -> (&'static str, Option<String>) {
        if self.loaded.lock().unwrap().is_some() {
            ("loaded", None)
        } else if let Some(err) = self.load_error.lock().unwrap().clone() {
            ("failed", Some(err))
        } else {
            ("not_loaded", None)
        }
    }

    /// Identifier for the head currently on disk, e.g. `bioclip-2/taxa-v2`.
    ///
    /// Stamped into each photo's `species_model` so a head swap can re-queue
    /// exactly the photos it invalidates. `None` only when the labels file is
    /// missing or unreadable.
    ///
    /// Deliberately reads the file rather than reporting the loaded model's
    /// version: this is called by `/index/check-unprocessed`, which runs
    /// *before* any indexing and must not be the thing that pulls a 608 MB
    /// session and a 258 MB head into memory. An earlier version returned
    /// `None` until the model had loaded once and treated that as "no version
    /// constraint" — so on a freshly restarted server every stored prediction
    /// looked current no matter which head produced it, and swapping the head
    /// re-queued nothing at all.
    pub fn model_id(&self) -> Option<String> {
        if let Some(v) = self.taxa_version.lock().unwrap().clone() {
            return Some(format!("bioclip-2/{v}"));
        }
        #[derive(serde::Deserialize)]
        struct VersionOnly {
            taxa_version: String,
        }
        let text = std::fs::read_to_string(&self.paths.taxa_json).ok()?;
        let parsed: VersionOnly = serde_json::from_str(&text).ok()?;
        *self.taxa_version.lock().unwrap() = Some(parsed.taxa_version.clone());
        Some(format!("bioclip-2/{}", parsed.taxa_version))
    }

    /// Embed one RGB8 image; NOT L2-normalized (callers normalize, matching
    /// [`crate::siglip::SiglipModel::embed_image`]).
    pub fn embed_image(
        &self,
        pixels: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, BioclipError> {
        self.ensure_loaded()?;
        let mut guard = self.loaded.lock().unwrap();
        let loaded = guard.as_mut().ok_or(BioclipError::NotLoaded)?;

        let data = bioclip_pre::preprocess(pixels, width, height);
        let input = Tensor::from_array((
            [1, 3, bioclip_pre::IMAGE_SIZE, bioclip_pre::IMAGE_SIZE],
            data,
        ))?;
        let outputs = loaded
            .image_session
            .run(ort::inputs!["pixel_values" => input])?;
        let (_, embedding) = outputs[0].try_extract_tensor::<f32>()?;
        Ok(embedding.to_vec())
    }

    /// Score an L2-normalized image embedding against the taxonomy head.
    ///
    /// Softmax over every taxon, then the mass is summed per taxonomic
    /// prefix at each rank. The answer is the *deepest* rank whose best node
    /// clears `min_confidence`, which keeps the result internally consistent:
    /// a node at rank `r` encodes its whole ancestry, so the reported chain
    /// is always a real lineage rather than a per-rank argmax that could
    /// disagree with itself.
    pub fn classify(
        &self,
        embedding: &[f32],
        cfg: &ClassifyConfig,
    ) -> Result<TaxonomyPrediction, BioclipError> {
        self.ensure_loaded()?;
        let guard = self.loaded.lock().unwrap();
        let head = &guard.as_ref().ok_or(BioclipError::NotLoaded)?.head;
        if embedding.len() != EMBED_DIM {
            return Err(BioclipError::Head(format!(
                "embedding has {} dims, expected {EMBED_DIM}",
                embedding.len()
            )));
        }

        // Cosine (both sides unit-norm) -> logits -> softmax.
        let lut = f16_lut();
        let mut probs = Vec::with_capacity(head.n_rows);
        let mut max_logit = f32::NEG_INFINITY;
        for row in 0..head.n_rows {
            let base = row * EMBED_DIM;
            // Four independent accumulators rather than one. A single `sum()`
            // serializes on the accumulator's latency, and with a table lookup
            // in the loop body that dependency chain — not the memory traffic —
            // is what bounds this. Four lanes let the multiply-adds pipeline.
            // The reassociation this implies is worth ~1e-7 on a 768-element
            // sum of unit-vector components, well under fp16's own precision.
            let row = &head.matrix[base..base + EMBED_DIM];
            let mut acc = [0.0f32; 4];
            for (q, e) in row.chunks_exact(4).zip(embedding.chunks_exact(4)) {
                acc[0] += lut[q[0] as usize] * e[0];
                acc[1] += lut[q[1] as usize] * e[1];
                acc[2] += lut[q[2] as usize] * e[2];
                acc[3] += lut[q[3] as usize] * e[3];
            }
            let dot = acc[0] + acc[1] + acc[2] + acc[3];
            let logit = dot * head.logit_scale_exp;
            max_logit = max_logit.max(logit);
            probs.push(logit);
        }
        let mut total = 0.0f32;
        for p in probs.iter_mut() {
            *p = (*p - max_logit).exp();
            total += *p;
        }
        if total > 0.0 {
            for p in probs.iter_mut() {
                *p /= total;
            }
        }

        for rank in (0..RANK_COUNT).rev() {
            let mut agg = vec![0.0f32; head.node_repr[rank].len()];
            for (row, p) in probs.iter().enumerate() {
                agg[head.node_of[rank][row] as usize] += p;
            }

            let mut ranked: Vec<usize> = (0..agg.len()).collect();
            ranked.sort_unstable_by(|&a, &b| {
                agg[b]
                    .partial_cmp(&agg[a])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let Some(&best_node) = ranked.first() else {
                continue;
            };
            if agg[best_node] < cfg.min_confidence {
                continue;
            }
            // A node can win its rank while having no name — the bony fish all
            // share one "Chordata, no class assigned" node. Reporting it would
            // mean an empty `species_scientific_name`, so fall through to the
            // next coarser rank, which is always populated further up. The
            // probability mass genuinely sits on that unnamed group, so
            // substituting some other node at this rank would be a lie; going
            // coarser is the honest answer.
            if head
                .name_at(head.node_repr[rank][best_node] as usize, rank)
                .is_empty()
            {
                continue;
            }

            let to_candidate =
                |node: usize| head.candidate(head.node_repr[rank][node] as usize, rank, agg[node]);
            return Ok(TaxonomyPrediction {
                rank: Some(rank),
                best: Some(to_candidate(best_node)),
                alternatives: ranked
                    .iter()
                    .skip(1)
                    .take(cfg.top_k.saturating_sub(1))
                    .map(|&n| to_candidate(n))
                    .collect(),
            });
        }

        // Nothing cleared the floor at any rank — not even "Animalia". That
        // is the honest answer for a photo with no organism in it.
        Ok(TaxonomyPrediction {
            rank: None,
            best: None,
            alternatives: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These are the *exact* decimal expansions of the fp16 bit patterns being
    // tested — writing them truncated would hide what is actually being
    // asserted, which is the whole point of the table.
    #[allow(clippy::excessive_precision)]
    #[test]
    fn f16_roundtrip_covers_specials_and_subnormals() {
        // Every value is fp16-representable, so the comparison can be exact.
        for (bits, want) in [
            (0x0000u16, 0.0f32),
            (0x8000, -0.0),
            (0x3c00, 1.0),
            (0xbc00, -1.0),
            (0x4000, 2.0),
            (0x3555, 0.333251953125),
            (0x0001, 5.960464477539063e-8), // smallest subnormal
            (0x03ff, 6.097555160522461e-5), // largest subnormal
            (0x0400, 6.103515625e-5),       // smallest normal
            (0x7bff, 65504.0),              // largest finite
        ] {
            let got = f16_to_f32(bits);
            assert_eq!(got, want, "bits {bits:#06x}");
        }
        assert!(f16_to_f32(0x7c00).is_infinite());
        assert!(f16_to_f32(0xfc00).is_infinite() && f16_to_f32(0xfc00) < 0.0);
        assert!(f16_to_f32(0x7e00).is_nan());
    }

    #[test]
    fn prefix_nodes_group_by_shared_ancestry() {
        // Three taxa: two share everything down to genus, the third diverges
        // at class. Ids are (kingdom, phylum, class, order, family, genus,
        // species).
        let rows = vec![
            vec![0, 0, 0, 0, 0, 0, 0],
            vec![0, 0, 0, 0, 0, 0, 1],
            vec![0, 0, 1, 1, 1, 1, 2],
        ];
        let (node_of, node_repr) = build_prefix_nodes(&rows);

        // One kingdom, one phylum, two classes.
        assert_eq!(node_repr[0].len(), 1);
        assert_eq!(node_repr[1].len(), 1);
        assert_eq!(node_repr[2].len(), 2);
        // Rows 0 and 1 stay together through genus, then split at species.
        assert_eq!(node_of[GENUS][0], node_of[GENUS][1]);
        assert_ne!(node_of[SPECIES][0], node_of[SPECIES][1]);
        // Row 2 is separate from class downwards.
        assert_ne!(node_of[2][0], node_of[2][2]);
        assert_eq!(node_repr[SPECIES].len(), 3);
    }

    #[test]
    fn prefix_nodes_disambiguate_reused_vocabulary_ids() {
        // Two different genera that happen to hold the same vocabulary id for
        // their species epithet (e.g. two "alba"). They must not collapse.
        let rows = vec![vec![0, 0, 0, 0, 0, 0, 7], vec![0, 0, 0, 0, 0, 1, 7]];
        let (node_of, node_repr) = build_prefix_nodes(&rows);
        assert_ne!(node_of[SPECIES][0], node_of[SPECIES][1]);
        assert_eq!(node_repr[SPECIES].len(), 2);
    }

    #[test]
    fn rank_label_reports_none_without_a_confident_rank() {
        let pred = TaxonomyPrediction {
            rank: None,
            best: None,
            alternatives: Vec::new(),
        };
        assert_eq!(pred.rank_label(), "none");
        let pred = TaxonomyPrediction {
            rank: Some(GENUS),
            best: None,
            alternatives: Vec::new(),
        };
        assert_eq!(pred.rank_label(), "genus");
    }
}
