//! Culling's key people (`key_people: true` on `/v1/cull/grade`).
//!
//! Indexing sums a photo's faces up by size and by whether they are on the
//! subject, which is all it can know about one photo. Who matters only shows
//! across the selection: the people the photographer kept photographing, and
//! the ones they named in People ([`lrg_analysis::key_people`]). Where a group
//! of frames shows one of them, its faces are summed up again from their faces
//! alone — the bride's blink counts though a guest stands larger in front of
//! her, and a frame where she turned away has no face that matters.
//!
//! It reads the photos' face rows, so it needs the full face pass
//! (identities), not the fast cull pass; photos without rows are counted for
//! the response to say so.

use std::collections::{HashMap, HashSet};

use lrg_analysis::face_aggregate::{aggregate_face_culling_metrics, FaceMetricsInput};
use lrg_analysis::face_cluster::{cluster_faces, ClusterParams, FaceInput};
use lrg_analysis::grouping::{
    GroupingInput, Partition, WHO_EYES_CLOSED_KEY, WHO_EYE_FOCUS_KEY, WHO_LOOKING_AWAY_KEY,
};
use lrg_analysis::key_people::{key_persons, KeyPeopleRule};
use lrg_imaging::cull_config::FaceMetricsConfig;
use lrg_store::{Store, FACE_TABLE};
use serde_json::{Map, Value};

use crate::routes::faces::is_unassigned;
use crate::routes::index_upload::{faces_detected, write_face_aggregate};

/// One face of a photo in the selection, as its row has it.
struct Face {
    /// Its person: from People, or a cluster of this selection's faces nobody
    /// has assigned yet (`selection_N`).
    person: Option<String>,
    input: FaceMetricsInput,
}

/// The key people of a selection and their faces.
pub struct KeyPeople {
    faces: HashMap<String, Vec<Face>>,
    key: HashSet<String>,
    names: HashMap<String, String>,
    /// Photos with faces that have no face rows: never through face
    /// recognition, so nobody in them can be told apart.
    pub unrecognised: usize,
}

impl KeyPeople {
    /// The key people with a name, sorted.
    pub fn named(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .key
            .iter()
            .filter_map(|p| self.names.get(p))
            .filter(|n| !n.is_empty())
            .cloned()
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// How many photos had face rows to judge by.
    pub fn recognised(&self) -> usize {
        self.faces.len()
    }

    /// How many key people have no name.
    pub fn unnamed(&self) -> usize {
        self.key
            .iter()
            .filter(|p| self.names.get(*p).is_none_or(|n| n.is_empty()))
            .count()
    }

    fn key_faces<'a>(&'a self, photo_id: &str) -> impl Iterator<Item = &'a Face> + 'a {
        self.faces
            .get(photo_id)
            .into_iter()
            .flatten()
            .filter(|f| f.person.as_ref().is_some_and(|p| self.key.contains(p)))
    }
}

/// The face rows of `records`, and who among them is a key person. `names` is
/// People's person → name map.
pub async fn load(
    store: &Store,
    records: &[GroupingInput],
    names: HashMap<String, String>,
) -> Result<KeyPeople, String> {
    let wanted: HashSet<&str> = records.iter().map(|r| r.photo_id.as_str()).collect();
    let prefixes: Vec<String> = records.iter().map(|r| format!("{}_", r.photo_id)).collect();
    let rows = store
        .get_by_id_prefixes(FACE_TABLE, &prefixes)
        .await
        .map_err(|e| e.to_string())?;

    // A prefix also reaches `abc_def_0` from photo `abc`; the row says whose
    // face it is.
    let rows: Vec<_> = rows
        .into_iter()
        .filter_map(|r| {
            let photo = photo_of(&r.metadata)?.to_string();
            wanted.contains(photo.as_str()).then_some((photo, r))
        })
        .collect();

    // Faces nobody assigned in People are clustered among themselves, so a
    // selection indexed but never clustered still has its frequent people.
    let unassigned: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, (_, r))| person_of(&r.metadata).is_none() && r.vector.is_some())
        .map(|(i, _)| i)
        .collect();
    let mut cluster_of: HashMap<usize, String> = HashMap::new();
    if unassigned.len() >= 2 {
        let inputs: Vec<FaceInput> = unassigned
            .iter()
            .map(|&i| {
                let r = &rows[i].1;
                FaceInput {
                    id: r.id.clone(),
                    embedding: r.vector.clone().unwrap_or_default(),
                    det_score: number(&r.metadata, "face_det_score"),
                    area_ratio: number(&r.metadata, "face_area_ratio"),
                    sharpness: number(&r.metadata, "face_sharpness"),
                    occlusion: number(&r.metadata, "face_occlusion"),
                    pinned: None,
                }
            })
            .collect();
        let outcome = cluster_faces(&inputs, &ClusterParams::defaults());
        for (&i, label) in unassigned.iter().zip(outcome.labels) {
            if let Some(label) = label {
                cluster_of.insert(i, format!("selection_{label}"));
            }
        }
    }

    let mut faces: HashMap<String, Vec<Face>> = HashMap::new();
    let mut appearances: HashMap<String, HashSet<String>> = HashMap::new();
    for (i, (photo, row)) in rows.iter().enumerate() {
        let person = person_of(&row.metadata)
            .map(str::to_string)
            .or_else(|| cluster_of.get(&i).cloned());
        if let Some(p) = &person {
            appearances
                .entry(p.clone())
                .or_default()
                .insert(photo.clone());
        }
        faces.entry(photo.clone()).or_default().push(Face {
            person,
            input: row_input(&row.metadata),
        });
    }

    let named: HashSet<String> = names
        .iter()
        .filter(|(_, n)| !n.is_empty())
        .map(|(p, _)| p.clone())
        .collect();
    let key = key_persons(
        &appearances,
        &named,
        faces.len(),
        &KeyPeopleRule::defaults(),
    );
    let unrecognised = records
        .iter()
        .filter(|r| faces_detected(&r.metadata) > 0 && !faces.contains_key(&r.photo_id))
        .count();
    Ok(KeyPeople {
        faces,
        key,
        names,
        unrecognised,
    })
}

/// Sums up again, from their key people's faces only, the faces of every photo
/// in a group where any frame shows one of them — all or nothing per group,
/// like every other signal, so a frame where the key person turned away reads
/// as having no face that matters rather than keeping its bystanders. Names
/// the key person a frame's eyes, focus or gaze are about, where they have a
/// name. Returns how many groups it changed.
pub fn apply(partition: &mut Partition, key: &KeyPeople) -> usize {
    if key.key.is_empty() {
        return 0;
    }
    let mut keyed = vec![false; partition.group_count()];
    for (record, &g) in partition.records().iter().zip(partition.group_of()) {
        if key.key_faces(&record.photo_id).next().is_some() {
            keyed[g] = true;
        }
    }
    let group_of = partition.group_of().to_vec();
    // Every key face counts, however small: the point is that the bride's
    // blink matters though a guest stands larger in front of her.
    let cfg = FaceMetricsConfig {
        prominent_face_area_fraction: 0.0,
        ..FaceMetricsConfig::defaults()
    };
    for (record, g) in partition.records_mut().iter_mut().zip(group_of) {
        if !keyed[g] {
            continue;
        }
        let faces: Vec<&Face> = key.key_faces(&record.photo_id).collect();
        let inputs: Vec<FaceMetricsInput> = faces.iter().map(|f| f.input).collect();
        write_face_aggregate(
            &mut record.metadata,
            &aggregate_face_culling_metrics(&inputs, &cfg),
        );
        name_who(&mut record.metadata, &faces, key);
    }
    keyed.iter().filter(|k| **k).count()
}

/// Who the frame's worst eyes, focus and gaze belong to, by name.
fn name_who(metadata: &mut Map<String, Value>, faces: &[&Face], key: &KeyPeople) {
    let name = |f: &&Face| {
        f.person
            .as_ref()
            .and_then(|p| key.names.get(p))
            .filter(|n| !n.is_empty())
            .cloned()
    };
    let worst = |pick: fn(&FaceMetricsInput) -> Option<f64>, highest: bool| {
        faces
            .iter()
            .filter_map(|f| pick(&f.input).map(|v| (v, f)))
            .max_by(|a, b| {
                let order = a.0.total_cmp(&b.0);
                if highest {
                    order
                } else {
                    order.reverse()
                }
            })
            .and_then(|(_, f)| name(f))
    };
    for (field, who) in [
        (WHO_EYES_CLOSED_KEY, worst(|i| i.eye_closed, true)),
        (
            WHO_EYE_FOCUS_KEY,
            worst(|i| i.eye_focus.or(i.face_focus), false),
        ),
        (WHO_LOOKING_AWAY_KEY, worst(|i| i.gaze_off_lens, true)),
    ] {
        match who {
            Some(n) => {
                metadata.insert(field.into(), Value::from(n));
            }
            None => {
                metadata.remove(field);
            }
        }
    }
}

fn number(m: &Map<String, Value>, key: &str) -> Option<f64> {
    m.get(key).and_then(Value::as_f64)
}

fn photo_of(m: &Map<String, Value>) -> Option<&str> {
    m.get("photo_id")
        .or_else(|| m.get("photo_uuid"))
        .and_then(Value::as_str)
}

fn person_of(m: &Map<String, Value>) -> Option<&str> {
    m.get("person_id")
        .and_then(Value::as_str)
        .filter(|p| !is_unassigned(p))
}

/// A face row as the aggregate reads it — the same readings indexing passes
/// for a fresh face (`index_upload::metrics_input`): the face state only where
/// it judged the eyes, and lowered eyes as looking down, not as a blink.
fn row_input(m: &Map<String, Value>) -> FaceMetricsInput {
    let state = m.get("face_eye_state").and_then(Value::as_str);
    let judged = state.is_some_and(|s| s != "unknown");
    let down = state == Some("down");
    let read = |key: &str| number(m, key).filter(|_| judged);
    FaceMetricsInput {
        sharpness: number(m, "face_sharpness").unwrap_or(0.0),
        area_ratio: number(m, "face_area_ratio").unwrap_or(0.0),
        det_score: number(m, "face_det_score").unwrap_or(0.0),
        center_proximity: number(m, "face_center_proximity").unwrap_or(0.0),
        eye_openness: number(m, "face_eye_openness").unwrap_or(0.0),
        blink_penalty: number(m, "face_blink_penalty").unwrap_or(0.0),
        occlusion: number(m, "face_occlusion"),
        eye_focus: number(m, "face_eye_focus"),
        face_focus: number(m, "face_focus"),
        eye_closed: read("face_eye_closed").filter(|_| !down),
        looking_down: judged && down,
        smile: read("face_smile"),
        eye_squint: read("face_eye_squint"),
        gaze_off_lens: read("face_gaze_off_lens"),
        mouth_open: read("face_mouth_open"),
        luminance: read("face_luminance"),
        across_face_stops: read("face_across_stops"),
        blown: read("face_blown"),
    }
}
