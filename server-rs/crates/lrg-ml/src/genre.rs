//! Zero-shot photo genre, from the SigLIP2 embedding a photo already has.
//!
//! A fixed list of genres, each described by a prompt or two, scored against the
//! stored image embedding: the genre is the one whose prompts are closest, or
//! `other` when nothing is clearly closest. It costs a few dot products per
//! photo — the prompts' text embeddings come from the database like the other
//! fixed prompt sets (`lrg-api`'s `prompt_embeddings`), so no text tower runs.
//!
//! Like the CLIP-IQA prompt sets this is a *guess from the picture*, not a
//! statement about how it was made: a macro shot of a flower is "macro", a
//! street scene at night may be "street" or "astro". The list is deliberately
//! coarse, and [`GENRES`] is the single source for both the classifier and the
//! lists a UI offers.

use crate::siglip::l2_normalize;

/// One genre: a stable id (what is stored and filtered on), a label for people,
/// and the prompts that describe it.
#[derive(Debug, PartialEq, Eq)]
pub struct Genre {
    pub id: &'static str,
    pub label: &'static str,
    pub prompts: &'static [&'static str],
}

/// Identifies the genre list, its prompts and the thresholds below, stored with
/// every genre as `genre_version`. **Bump it whenever any of them changes**: a
/// stored genre is only comparable with the rules that produced it, and indexing
/// recomputes the ones from another version.
pub const VERSION: &str = "1";

/// What a photo gets when no genre is clearly the closest one.
pub const OTHER_ID: &str = "other";
pub const OTHER_LABEL: &str = "Other";

pub const GENRES: &[Genre] = &[
    Genre {
        id: "landscape",
        label: "Landscape",
        prompts: &[
            "a landscape photo of mountains, fields, a coast or other wide natural scenery.",
            "a photo of a scenic outdoor landscape.",
        ],
    },
    Genre {
        id: "portrait",
        label: "Portrait",
        prompts: &[
            "a portrait photo of a person.",
            "a close-up photo of a person's face.",
        ],
    },
    Genre {
        id: "street",
        label: "Street",
        prompts: &[
            "a street photo of people in a city.",
            "candid street photography in an urban scene.",
        ],
    },
    Genre {
        id: "wildlife",
        label: "Wildlife & animals",
        prompts: &[
            "a photo of a wild animal in nature.",
            "a wildlife photo of a bird or a mammal.",
        ],
    },
    Genre {
        id: "sport",
        label: "Sport",
        prompts: &[
            "a photo of a sports action scene.",
            "an athlete in motion during a game.",
        ],
    },
    Genre {
        id: "event",
        label: "Event & wedding",
        prompts: &[
            "a photo of a wedding or a party with people celebrating.",
            "an event photo of a celebration.",
        ],
    },
    Genre {
        id: "architecture",
        label: "Architecture",
        prompts: &[
            "a photo of a building or architecture.",
            "interior or exterior architecture photography.",
        ],
    },
    Genre {
        id: "macro",
        label: "Macro",
        prompts: &["a macro close-up photo of a small subject such as an insect or a flower."],
    },
    Genre {
        id: "food",
        label: "Food",
        prompts: &[
            "a photo of food on a plate.",
            "a food photography shot of a meal.",
        ],
    },
    Genre {
        id: "still_life",
        label: "Product & still life",
        prompts: &["a product photo or a still life of an object."],
    },
    Genre {
        id: "astro",
        label: "Astro & night sky",
        prompts: &[
            "a night sky photo with stars or the milky way.",
            "an astrophotography image of the night sky.",
        ],
    },
    Genre {
        id: "concert",
        label: "Concert & stage",
        prompts: &["a photo of a concert or a performer on stage under stage lights."],
    },
    Genre {
        id: "aerial",
        label: "Aerial",
        prompts: &["an aerial photo taken from a drone or an airplane looking down."],
    },
];

/// Every prompt text, each genre's in order: what a caller asks the prompt
/// store for.
pub fn texts() -> impl Iterator<Item = &'static str> {
    GENRES.iter().flat_map(|g| g.prompts.iter().copied())
}

/// The label for a stored genre id, `None` for an id this build does not know.
pub fn label_for(id: &str) -> Option<&'static str> {
    if id == OTHER_ID {
        return Some(OTHER_LABEL);
    }
    GENRES.iter().find(|g| g.id == id).map(|g| g.label)
}

/// Same scale CLIP-style two-way softmaxes use (`clip_iqa`): raw cosines of
/// different genres differ by a few hundredths, which a plain softmax would
/// flatten to a uniform distribution.
const LOGIT_SCALE: f64 = 100.0;

/// The closest genre must be at least this much more likely than the next one
/// to be named, otherwise the photo is `other`. Both are measured on the
/// softmax over genres, so they are comparable between photos.
const MIN_CONFIDENCE: f64 = 0.30;
const MIN_LEAD: f64 = 0.10;

#[derive(Debug, Clone, PartialEq)]
pub struct GenreResult {
    pub id: &'static str,
    pub label: &'static str,
    /// The winner's share of the softmax over genres, `0..1`. For `other` it is
    /// the share the best genre would have had, so a low value says "nothing
    /// fit" and a value just under the bar says "close".
    pub confidence: f64,
}

/// The genres' prompt embeddings, ready to classify with.
pub struct GenreClassifier {
    /// One entry per genre: its prompts' L2-normalized embeddings.
    genres: Vec<Vec<Vec<f32>>>,
}

impl GenreClassifier {
    /// Builds the classifier from embeddings the caller already has, `None`
    /// when any prompt is missing: a genre scored on half its prompts would
    /// quietly change what the answer means.
    pub fn from_lookup<'a>(lookup: impl Fn(&str) -> Option<&'a [f32]>) -> Option<Self> {
        let genres = GENRES
            .iter()
            .map(|g| {
                g.prompts
                    .iter()
                    .map(|p| {
                        let mut v = lookup(p)?.to_vec();
                        l2_normalize(&mut v);
                        Some(v)
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .collect::<Option<Vec<_>>>()?;
        Some(GenreClassifier { genres })
    }

    /// Classifies one image embedding. `None` for a dimension mismatch or an
    /// all-zero vector — the nullable-vector convention for "no embedding".
    pub fn classify(&self, image: &[f32]) -> Option<GenreResult> {
        let mut v = image.to_vec();
        l2_normalize(&mut v);
        if v.is_empty() || v.iter().all(|x| *x == 0.0) {
            return None;
        }
        let mut scores = Vec::with_capacity(self.genres.len());
        for prompts in &self.genres {
            let mut best = f64::NEG_INFINITY;
            for p in prompts {
                if p.len() != v.len() {
                    return None;
                }
                let cos: f64 = v.iter().zip(p).map(|(a, b)| *a as f64 * *b as f64).sum();
                best = best.max(cos);
            }
            scores.push(best * LOGIT_SCALE);
        }
        let max = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let exps: Vec<f64> = scores.iter().map(|s| (s - max).exp()).collect();
        let total: f64 = exps.iter().sum();
        let probs: Vec<f64> = exps.iter().map(|e| e / total).collect();

        let mut order: Vec<usize> = (0..probs.len()).collect();
        order.sort_by(|a, b| probs[*b].partial_cmp(&probs[*a]).unwrap());
        let (top, second) = (order[0], order[1]);
        let confidence = probs[top];
        if confidence >= MIN_CONFIDENCE && confidence - probs[second] >= MIN_LEAD {
            Some(GenreResult {
                id: GENRES[top].id,
                label: GENRES[top].label,
                confidence,
            })
        } else {
            Some(GenreResult {
                id: OTHER_ID,
                label: OTHER_LABEL,
                confidence,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A tiny embedding space with one axis per genre, so a unit vector along an
    /// axis is unmistakably that genre's.
    fn axis_lookup() -> HashMap<&'static str, Vec<f32>> {
        let n = GENRES.len();
        let mut map = HashMap::new();
        for (i, g) in GENRES.iter().enumerate() {
            for p in g.prompts {
                let mut v = vec![0.0f32; n];
                v[i] = 1.0;
                map.insert(*p, v);
            }
        }
        map
    }

    fn classifier() -> GenreClassifier {
        let map = axis_lookup();
        GenreClassifier::from_lookup(|t| map.get(t).map(Vec::as_slice)).unwrap()
    }

    #[test]
    fn ids_and_prompts_are_unique() {
        let mut ids: Vec<_> = GENRES.iter().map(|g| g.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), GENRES.len());
        assert!(!ids.contains(&OTHER_ID));
        let all: Vec<_> = texts().collect();
        let mut unique = all.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(all.len(), unique.len());
        assert!(GENRES.iter().all(|g| !g.prompts.is_empty()));
    }

    #[test]
    fn labels_resolve_by_id() {
        assert_eq!(label_for("macro"), Some("Macro"));
        assert_eq!(label_for(OTHER_ID), Some(OTHER_LABEL));
        assert_eq!(label_for("nonsense"), None);
    }

    #[test]
    fn a_clear_match_names_its_genre() {
        let c = classifier();
        for (i, g) in GENRES.iter().enumerate() {
            let mut image = vec![0.0f32; GENRES.len()];
            image[i] = 1.0;
            let got = c.classify(&image).unwrap();
            assert_eq!(got.id, g.id);
            assert!(got.confidence > 0.9, "{}: {}", g.id, got.confidence);
        }
    }

    #[test]
    fn an_even_spread_is_other() {
        let c = classifier();
        let image = vec![1.0f32; GENRES.len()];
        let got = c.classify(&image).unwrap();
        assert_eq!(got.id, OTHER_ID);
        assert_eq!(got.label, OTHER_LABEL);
    }

    #[test]
    fn a_tie_between_two_genres_is_other() {
        let c = classifier();
        let mut image = vec![0.0f32; GENRES.len()];
        image[0] = 1.0;
        image[1] = 1.0;
        assert_eq!(c.classify(&image).unwrap().id, OTHER_ID);
    }

    #[test]
    fn unusable_embeddings_give_no_answer() {
        let c = classifier();
        assert_eq!(c.classify(&[]), None);
        assert_eq!(c.classify(&vec![0.0; GENRES.len()]), None);
        assert_eq!(c.classify(&[1.0, 0.0]), None);
    }

    #[test]
    fn a_missing_prompt_refuses_to_build() {
        let mut map = axis_lookup();
        map.remove(GENRES[3].prompts[0]);
        assert!(GenreClassifier::from_lookup(|t| map.get(t).map(Vec::as_slice)).is_none());
    }
}
