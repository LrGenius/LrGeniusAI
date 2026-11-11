//! Choosing a number of photos from a whole shoot: scenes, a target, highlights.
//!
//! Ranking answers a question inside one burst: which frame is best, which are
//! worse. A photographer delivering a gallery asks another one across the
//! shoot — "the best 300", "a fifth of it", "twenty for the sneak peek" — and
//! wants every part of the day in it, not the 300 sharpest frames of the
//! speeches. Aftershoot ("cull to target": quality, coverage, variety),
//! FilterPixel and Imagen all answer it; this is that answer.
//!
//! 1. **Scenes**: consecutive groups close in time, and alike where both have
//!    an embedding, are one scene ([`scenes`]).
//! 2. **Keep value**: a number comparable across groups ([`keep_value`]).
//!    `cull_score` is not one — it is measured against each group — so the
//!    keep value rests on the absolute signals: the aesthetic score, the moment
//!    where it was judged, and a clear defect.
//! 3. **Selection** ([`select`]): first the best pick of every scene (the best
//!    scenes, when the target is smaller than their number), then the rest
//!    by keep value, less how alike they are to what is already chosen, with a
//!    bonus for a group not yet in it and for a key person not yet in it. A
//!    reject candidate only when its scene has nothing else, and then marked.
//! 4. **Highlights**: the best of the selection, again kept apart from each
//!    other.
//!
//! The weights below are a first version. The public benchmark measures the
//! one part it can — whether the selection takes each series' best frame
//! first — and nothing about taste.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::grouping::{Group, RankedPhoto};

/// How many photos to keep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Count(usize),
    /// A share of the photos, `0..=1`.
    Fraction(f64),
}

impl Target {
    fn count(self, total: usize) -> usize {
        match self {
            Target::Count(n) => n.min(total),
            Target::Fraction(f) => {
                let n = (f.clamp(0.0, 1.0) * total as f64).round() as usize;
                if total > 0 && f > 0.0 {
                    n.max(1)
                } else {
                    n
                }
            }
        }
    }
}

/// When two consecutive groups are one scene.
#[derive(Debug, Clone, Copy)]
pub struct SceneRule {
    /// At most this many seconds from the end of one group to the start of
    /// the next.
    pub max_gap_seconds: f64,
    /// And, where both have an embedding, at most this cosine distance between
    /// their picks: the same place and light, not the next room.
    pub max_distance: f64,
}

impl SceneRule {
    pub const fn defaults() -> Self {
        SceneRule {
            max_gap_seconds: 90.0,
            max_distance: 0.35,
        }
    }
}

/// How alike photos may be before the selection prefers others, and what a
/// new group or person is worth.
#[derive(Debug, Clone, Copy)]
pub struct SelectionRule {
    /// Taken off a candidate's keep value per unit of cosine similarity to the
    /// closest photo already chosen.
    pub similarity_cost: f64,
    /// Added for a candidate from a group with nothing chosen yet.
    pub new_group_bonus: f64,
    /// Added per key person the candidate shows who is in nothing chosen yet.
    pub new_person_bonus: f64,
    /// How alike two frames of one group count at least, embedding or not:
    /// grouping put them together for being near-duplicates or one burst.
    pub same_group_similarity: f64,
    /// Taken off a candidate's keep value per place it ranks below its
    /// group's pick: the keep value compares groups, the rank knows the burst.
    pub rank_cost: f64,
}

impl SelectionRule {
    pub const fn defaults() -> Self {
        SelectionRule {
            similarity_cost: 0.5,
            new_group_bonus: 0.1,
            new_person_bonus: 0.1,
            same_group_similarity: 0.9,
            rank_cost: RANK_COST,
        }
    }
}

/// On the public benchmark, choosing two photos per series, 0.1 took the
/// best frame of its series 63.6 % of the time where 0 took it 56.9 %, and
/// fewer labelled rejects (2.9 % against 3.3 %); 0.2 took more rejects again.
const RANK_COST: f64 = 0.1;

/// What [`select`] chose.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Each group's scene, by index in the groups given; scenes are numbered
    /// in time order from 0.
    pub scene_of_group: Vec<usize>,
    pub scene_count: usize,
    /// How many the target asked for.
    pub wanted: usize,
    /// The chosen photos: `wanted` of them, or fewer when no more were worth
    /// keeping.
    pub selected: HashSet<String>,
    /// Chosen although a reject candidate: the only frame of its scene.
    pub selected_with_defect: Vec<String>,
    /// The highlights, best first.
    pub highlights: Vec<String>,
}

/// A photo's keep value, `0..1`, comparable between groups: the aesthetic
/// score, blended with the moment where it was judged, halved by a clear
/// defect.
pub fn keep_value(photo: &RankedPhoto) -> f64 {
    let read = |key: &str| photo.metrics.get(key).and_then(Value::as_f64);
    let aesthetic = read("aesthetic").unwrap_or(0.5);
    let value = match read("moment") {
        Some(moment) => 0.7 * aesthetic + 0.3 * moment,
        None => aesthetic,
    };
    let defect = photo
        .metrics
        .get("clear_defect")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (if defect { value * 0.5 } else { value }).clamp(0.0, 1.0)
}

/// Unit vectors, for cosine similarity by dot product.
fn unit(v: &[f32]) -> Option<Vec<f32>> {
    let norm = v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    (norm > 0.0).then(|| v.iter().map(|x| (*x as f64 / norm) as f32).collect())
}

fn similarity(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum()
}

/// The scene of each group, groups in the order given; see [`SceneRule`].
pub fn scenes<'a>(
    groups: &[Group],
    embedding_of: &dyn Fn(&str) -> Option<&'a [f32]>,
    rule: &SceneRule,
) -> (Vec<usize>, usize) {
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| {
        let key = |g: &Group| g.min_capture_time.unwrap_or(f64::INFINITY);
        key(&groups[a]).total_cmp(&key(&groups[b])).then(a.cmp(&b))
    });
    let pick = |g: &Group| {
        g.winner_photo_id
            .as_deref()
            .or(g.photo_ids.first().map(String::as_str))
            .and_then(embedding_of)
            .and_then(unit)
    };
    let mut scene_of = vec![0usize; groups.len()];
    let mut scene = 0usize;
    let mut previous: Option<usize> = None;
    for &g in &order {
        if let Some(p) = previous {
            let (a, b) = (&groups[p], &groups[g]);
            let near = match (a.max_capture_time, b.min_capture_time) {
                (Some(end), Some(start)) => start - end <= rule.max_gap_seconds,
                _ => false,
            };
            let alike = match (pick(a), pick(b)) {
                (Some(x), Some(y)) => 1.0 - similarity(&x, &y) <= rule.max_distance,
                _ => true,
            };
            if !(near && alike) {
                scene += 1;
            }
        }
        scene_of[g] = scene;
        previous = Some(g);
    }
    let count = if groups.is_empty() { 0 } else { scene + 1 };
    (scene_of, count)
}

/// Chooses `target` photos from `groups`, and `highlights` of them; see the
/// module. `people_of` gives the key people a photo shows, where known.
pub fn select<'a>(
    groups: &[Group],
    embedding_of: &dyn Fn(&str) -> Option<&'a [f32]>,
    people_of: &dyn Fn(&str) -> Vec<String>,
    target: Target,
    highlights: usize,
    scene_rule: &SceneRule,
    rule: &SelectionRule,
) -> Selection {
    let (scene_of_group, scene_count) = scenes(groups, embedding_of, scene_rule);

    struct Candidate<'g> {
        photo: &'g RankedPhoto,
        group: usize,
        value: f64,
        embedding: Option<Vec<f32>>,
        people: Vec<String>,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        for photo in &group.photos {
            // A bracket, stack or panorama is one picture: its representative
            // stands for it, and the plugin keeps every frame anyway.
            if group.keep_all() && !photo.winner {
                continue;
            }
            candidates.push(Candidate {
                photo,
                group: g,
                value: keep_value(photo),
                embedding: embedding_of(&photo.photo_id).and_then(unit),
                people: people_of(&photo.photo_id),
            });
        }
    }
    let total: usize = groups.iter().map(|g| g.photos.len()).sum();
    let asked = target.count(total);
    let wanted = asked.min(candidates.len());

    let mut out = Selection {
        scene_of_group: scene_of_group.clone(),
        scene_count,
        wanted: asked,
        ..Default::default()
    };
    let mut chosen: Vec<usize> = Vec::new();
    let mut taken = vec![false; candidates.len()];
    let mut max_sim = vec![0.0f64; candidates.len()];
    let mut groups_in = HashSet::new();
    let mut people_in: HashSet<String> = HashSet::new();
    let choose = |i: usize,
                  chosen: &mut Vec<usize>,
                  taken: &mut Vec<bool>,
                  max_sim: &mut Vec<f64>,
                  groups_in: &mut HashSet<usize>,
                  people_in: &mut HashSet<String>| {
        taken[i] = true;
        chosen.push(i);
        groups_in.insert(candidates[i].group);
        people_in.extend(candidates[i].people.iter().cloned());
        for (j, c) in candidates.iter().enumerate() {
            let mut sim = match (&candidates[i].embedding, &c.embedding) {
                (Some(e), Some(f)) => similarity(e, f),
                _ => 0.0,
            };
            if c.group == candidates[i].group {
                sim = sim.max(rule.same_group_similarity);
            }
            max_sim[j] = max_sim[j].max(sim);
        }
    };

    // 1. The best of every scene: its best pick, or — where every frame of it
    //    is a reject candidate — its best frame, marked.
    let mut scene_best: HashMap<usize, usize> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        let scene = scene_of_group[c.group];
        let better = |j: usize| {
            let (a, b) = (&candidates[i], &candidates[j]);
            let rank = |c: &Candidate| (!c.photo.reject_candidate, c.photo.winner);
            rank(a)
                .cmp(&rank(b))
                .then(a.value.total_cmp(&b.value))
                .is_gt()
        };
        match scene_best.get(&scene) {
            Some(&j) if !better(j) => {}
            _ => {
                scene_best.insert(scene, i);
            }
        }
    }
    let mut firsts: Vec<usize> = scene_best.into_values().collect();
    firsts.sort_by(|&a, &b| candidates[b].value.total_cmp(&candidates[a].value));
    for i in firsts.into_iter().take(wanted) {
        if candidates[i].photo.reject_candidate {
            out.selected_with_defect
                .push(candidates[i].photo.photo_id.clone());
        }
        choose(
            i,
            &mut chosen,
            &mut taken,
            &mut max_sim,
            &mut groups_in,
            &mut people_in,
        );
    }

    // 2. The rest: worth keeping, unlike what is chosen, from a group or of a
    //    person not in it yet.
    while chosen.len() < wanted {
        let score = |i: usize| {
            let c = &candidates[i];
            let new_people = c.people.iter().filter(|p| !people_in.contains(*p)).count();
            c.value
                - rule.similarity_cost * max_sim[i]
                - rule.rank_cost * c.photo.rank.saturating_sub(1) as f64
                + if groups_in.contains(&c.group) {
                    0.0
                } else {
                    rule.new_group_bonus
                }
                + rule.new_person_bonus * new_people as f64
        };
        let best = (0..candidates.len())
            .filter(|&i| !taken[i] && !candidates[i].photo.reject_candidate)
            .max_by(|&a, &b| score(a).total_cmp(&score(b)));
        let Some(i) = best else { break };
        choose(
            i,
            &mut chosen,
            &mut taken,
            &mut max_sim,
            &mut groups_in,
            &mut people_in,
        );
    }
    out.selected = chosen
        .iter()
        .map(|&i| candidates[i].photo.photo_id.clone())
        .collect();

    // 3. Highlights: the best of the selection, kept apart from each other,
    //    never a frame with a defect, and of each group only its best-ranked
    //    chosen frame — not the grainier twin of a frame already in.
    let mut best_in_group: HashMap<usize, usize> = HashMap::new();
    for &i in &chosen {
        let g = candidates[i].group;
        match best_in_group.get(&g) {
            Some(&j) if candidates[j].photo.rank <= candidates[i].photo.rank => {}
            _ => {
                best_in_group.insert(g, i);
            }
        }
    }
    let mut picked: Vec<usize> = Vec::new();
    let mut hl_sim = vec![0.0f64; candidates.len()];
    while picked.len() < highlights {
        let best = chosen
            .iter()
            .copied()
            .filter(|i| best_in_group.get(&candidates[*i].group) == Some(i))
            .filter(|i| !picked.contains(i) && !candidates[*i].photo.reject_candidate)
            .filter(|i| {
                !candidates[*i]
                    .photo
                    .metrics
                    .get("clear_defect")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .max_by(|&a, &b| {
                let s = |i: usize| candidates[i].value - rule.similarity_cost * hl_sim[i];
                s(a).total_cmp(&s(b))
            });
        let Some(i) = best else { break };
        picked.push(i);
        for (j, c) in candidates.iter().enumerate() {
            let mut sim = match (&candidates[i].embedding, &c.embedding) {
                (Some(e), Some(f)) => similarity(e, f),
                _ => 0.0,
            };
            if c.group == candidates[i].group {
                sim = sim.max(rule.same_group_similarity);
            }
            hl_sim[j] = hl_sim[j].max(sim);
        }
    }
    out.highlights = picked
        .into_iter()
        .map(|i| candidates[i].photo.photo_id.clone())
        .collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grouping::{group_and_sort_images, GroupingInput};
    use serde_json::Map;

    /// A frame: its burst (same pHash, a second apart), when, how sharp, and
    /// how good the aesthetic model finds it.
    fn frame(id: &str, t: f64, hash: u64, sharpness: f64, aesthetic: f64) -> GroupingInput {
        let mut metadata = Map::new();
        for (k, v) in [
            ("cull_sharpness", sharpness),
            ("cull_sharpness_peak", sharpness),
            ("cull_exposure", 0.7),
            ("cull_noise", 0.1),
            ("cull_aesthetic", aesthetic),
            ("cull_aesthetic_model", aesthetic),
        ] {
            metadata.insert(k.into(), Value::from(v));
        }
        GroupingInput {
            photo_id: id.into(),
            filename: format!("{id}.jpg"),
            capture_time: Some(t),
            embedding: None,
            phash: Some(hash),
            metadata,
        }
    }

    fn pick(groups: &[Group], target: Target, highlights: usize) -> Selection {
        select(
            groups,
            &|_| None,
            &|_| Vec::new(),
            target,
            highlights,
            &SceneRule::defaults(),
            &SelectionRule::defaults(),
        )
    }

    /// Three scenes an hour apart, two bursts in the first. A target of two
    /// takes the best two scenes' picks; four adds the second burst of the
    /// first scene before another frame of a burst already in.
    #[test]
    fn scenes_first_then_new_groups() {
        let a = 0xffff_0000_ffff_0000;
        let b = 0x0000_ffff_0000_ffff;
        let c = 0x00ff_00ff_00ff_00ff;
        let d = 0xff00_ff00_ff00_ff00;
        let groups = group_and_sort_images(
            vec![
                frame("s1a0", 0.0, a, 0.9, 0.8),
                frame("s1a1", 1.0, a, 0.8, 0.78),
                frame("s1b0", 30.0, b, 0.9, 0.6),
                frame("s2", 3600.0, c, 0.9, 0.7),
                frame("s3", 7200.0, d, 0.9, 0.4),
            ],
            None,
            None,
            Some(1),
            "default",
        );
        let two = pick(&groups, Target::Count(2), 0);
        assert_eq!(two.scene_count, 3);
        assert_eq!(two.selected, ["s1a0".to_string(), "s2".to_string()].into());
        let four = pick(&groups, Target::Count(4), 1);
        assert!(four.selected.contains("s1b0"), "{:?}", four.selected);
        assert!(!four.selected.contains("s1a1"), "{:?}", four.selected);
        assert_eq!(four.highlights, ["s1a0"]);
        // A fifth of five is one.
        assert_eq!(pick(&groups, Target::Fraction(0.2), 0).selected.len(), 1);
    }

    /// A scene whose every frame is a reject candidate still has its best
    /// frame chosen — and said to be flawed. Not as a highlight.
    #[test]
    fn a_scene_with_only_flawed_frames_is_kept_marked() {
        let groups = group_and_sort_images(
            vec![
                frame("good", 0.0, 0xffff_0000_ffff_0000, 0.9, 0.8),
                // A blurred single: a reject candidate on its own.
                frame("blurred", 3600.0, 0x0000_ffff_0000_ffff, 0.02, 0.6),
            ],
            None,
            None,
            Some(1),
            "default",
        );
        let blurred = groups
            .iter()
            .flat_map(|g| &g.photos)
            .find(|p| p.photo_id == "blurred")
            .unwrap();
        assert!(blurred.reject_candidate, "the fixture needs a reject");
        let s = pick(&groups, Target::Count(2), 2);
        assert!(s.selected.contains("blurred"));
        assert_eq!(s.selected_with_defect, ["blurred"]);
        assert_eq!(s.highlights, ["good"]);
    }
}
