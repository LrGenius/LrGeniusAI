//! Index cleanup: what becomes of the rows of photos that left a catalog.
//!
//! The plug-in sends the complete list of photo ids in its catalog. A row that
//! catalog claimed whose id is not on the list belongs to a photo that was
//! deleted from the catalog (or removed from it), and the catalog lets go of
//! it. Other catalogs sharing the database keep theirs, so a photo in two
//! catalogs survives losing one of them.
//!
//! A row left in no catalog at all is stamped [`meta::ORPHANED_AT_FIELD`] and
//! kept for [`ORPHAN_GRACE_SECS`] before it is purged with its faces and
//! species embedding. The wait is what makes the cleanup safe to run
//! unattended: a photo removed by mistake and imported again gets the same
//! photo_id, and the next cleanup gives it its AI data back instead of the
//! photo being indexed from scratch.
//!
//! A row with no catalog and no stamp is never purged. Rows written before
//! `catalog_ids` existed look exactly like that, and belong to a catalog that
//! has not claimed its photos yet.

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use crate::{meta, Result, Store, FACE_TABLE, IMAGE_TABLE, SPECIES_TABLE};

/// How long a photo that is in no catalog keeps its data.
pub const ORPHAN_GRACE_DAYS: i64 = 30;
pub const ORPHAN_GRACE_SECS: i64 = ORPHAN_GRACE_DAYS * 24 * 60 * 60;

/// The fewest releases [`CleanupPlan::is_large_removal`] asks about. Below it
/// a mistake is small enough to leave to the grace period.
pub const LARGE_REMOVAL_MIN: usize = 50;

/// Rows per read-modify-write round trip.
const WRITE_CHUNK: usize = 200;

/// What a cleanup would do, worked out from one scan of the image table.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CleanupPlan {
    /// Rows the catalog claimed before the run.
    pub indexed: usize,
    /// Claimed by the catalog, not in its inventory: the catalog lets go.
    pub release: Vec<String>,
    /// In the inventory and in no catalog: a photo that came back, or one
    /// this catalog never claimed.
    pub readopt: Vec<String>,
    /// In no catalog since longer than the grace period, and not coming back.
    pub purge: Vec<String>,
    /// Rows in no catalog that are still within their grace period.
    pub waiting: usize,
    /// The earliest stamp among `waiting`, which is when the next purge falls due.
    pub oldest_waiting: Option<i64>,
}

impl CleanupPlan {
    /// Whether the catalog would let go of more than half of what it has
    /// indexed.
    ///
    /// That is what a broken inventory looks like — photo ids resolved
    /// another way than at indexing time, or a catalog read that came back
    /// empty — and the grace period does not help there: the ids never come
    /// back, so everything would be purged a month later. Deleting most of a
    /// catalog on purpose is rare enough to ask about.
    pub fn is_large_removal(&self) -> bool {
        self.release.len() >= LARGE_REMOVAL_MIN && self.release.len() * 2 > self.indexed
    }
}

/// What [`apply`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CleanupOutcome {
    pub released: usize,
    /// Of `released`, the rows that are now in no catalog.
    pub orphaned: usize,
    pub readopted: usize,
    pub purged: usize,
    /// Purges that did not happen. The release and re-adopt steps fail the
    /// whole call instead: they are what the user asked for.
    pub warnings: Vec<String>,
}

/// When the row lost its last catalog, if it carries a readable stamp.
pub fn orphaned_at(metadata: &Map<String, Value>) -> Option<i64> {
    match metadata.get(meta::ORPHANED_AT_FIELD)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Works out what a cleanup of `catalog_id` against `inventory` (every photo
/// id in that catalog) would do. `rows` is the whole image table, id and
/// metadata.
pub fn plan(
    rows: impl IntoIterator<Item = (String, Map<String, Value>)>,
    catalog_id: &str,
    inventory: &HashSet<String>,
    now: i64,
) -> CleanupPlan {
    let mut plan = CleanupPlan::default();
    for (id, metadata) in rows {
        let catalogs = meta::parse_catalog_ids(&metadata);
        if catalogs.contains(catalog_id) {
            plan.indexed += 1;
            if !inventory.contains(&id) {
                plan.release.push(id);
            }
            continue;
        }
        if !catalogs.is_empty() {
            continue;
        }
        if inventory.contains(&id) {
            plan.readopt.push(id);
            continue;
        }
        if let Some(at) = orphaned_at(&metadata) {
            if now - at >= ORPHAN_GRACE_SECS {
                plan.purge.push(id);
            } else {
                plan.waiting += 1;
                plan.oldest_waiting = Some(plan.oldest_waiting.map_or(at, |o| o.min(at)));
            }
        }
    }
    plan
}

/// Carries out `plan` for `catalog_id`.
///
/// Every row is read again before it is written, so a change since the scan
/// is kept rather than overwritten, and a purge candidate that some catalog
/// claimed in the meantime is left alone.
pub async fn apply(
    store: &Store,
    catalog_id: &str,
    plan: &CleanupPlan,
    now: i64,
) -> Result<CleanupOutcome> {
    let mut outcome = CleanupOutcome::default();

    for chunk in plan.release.chunks(WRITE_CHUNK) {
        let mut records = store.get(IMAGE_TABLE, chunk).await?;
        for record in &mut records {
            let mut catalogs = meta::parse_catalog_ids(&record.metadata);
            catalogs.remove(catalog_id);
            if catalogs.is_empty() {
                record
                    .metadata
                    .insert(meta::ORPHANED_AT_FIELD.into(), json!(now));
                outcome.orphaned += 1;
            }
            record.metadata.insert(
                meta::CATALOG_IDS_FIELD.into(),
                Value::String(meta::serialize_catalog_ids(&catalogs)),
            );
            let id = record.id.clone();
            meta::ensure_photo_metadata(&id, &mut record.metadata);
        }
        store.upsert(IMAGE_TABLE, &records).await?;
        outcome.released += records.len();
    }

    for chunk in plan.readopt.chunks(WRITE_CHUNK) {
        let mut records = store.get(IMAGE_TABLE, chunk).await?;
        for record in &mut records {
            let mut catalogs = meta::parse_catalog_ids(&record.metadata);
            catalogs.insert(catalog_id.to_string());
            record.metadata.insert(
                meta::CATALOG_IDS_FIELD.into(),
                Value::String(meta::serialize_catalog_ids(&catalogs)),
            );
            record.metadata.remove(meta::ORPHANED_AT_FIELD);
            let id = record.id.clone();
            meta::ensure_photo_metadata(&id, &mut record.metadata);
        }
        store.upsert(IMAGE_TABLE, &records).await?;
        outcome.readopted += records.len();
    }

    if !plan.purge.is_empty() {
        match purge(store, &plan.purge, now).await {
            Ok(n) => outcome.purged = n,
            Err(e) => {
                log::warn!(
                    "Index cleanup: purging {} photos failed: {e}",
                    plan.purge.len()
                );
                outcome.warnings.push(format!(
                    "{} photos that have been in no catalog for more than {ORPHAN_GRACE_DAYS} days \
                     could not be deleted for good ({e}). The next cleanup tries again.",
                    plan.purge.len()
                ));
            }
        }
    }

    Ok(outcome)
}

/// Deletes the rows in `candidates` that are still due, with their faces and
/// species embedding, and returns how many went.
///
/// The faces and the species row go first: once the image row is gone nothing
/// would find them again, whereas an image row whose dependents failed to go
/// is simply purged again next time.
async fn purge(store: &Store, candidates: &[String], now: i64) -> Result<usize> {
    let due: Vec<String> = store
        .get_meta(IMAGE_TABLE, candidates)
        .await?
        .into_iter()
        .filter(|(_, m)| {
            meta::parse_catalog_ids(m).is_empty()
                && orphaned_at(m).is_some_and(|at| now - at >= ORPHAN_GRACE_SECS)
        })
        .map(|(id, _)| id)
        .collect();
    if due.is_empty() {
        return Ok(0);
    }
    // Face ids are `{photo_id}_{n}` — see `Store::delete_by_id_prefix`.
    let face_prefixes: Vec<String> = due.iter().map(|id| format!("{id}_")).collect();
    store
        .delete_by_id_prefixes(FACE_TABLE, &face_prefixes)
        .await?;
    store.delete(SPECIES_TABLE, &due).await?;
    // Training examples (TRAINING_TABLE) are left alone on purpose: they record
    // how the user edits, which stays true after the photo is gone, and the
    // user manages them on their own.
    store.delete(IMAGE_TABLE, &due).await?;
    Ok(due.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoreRecord;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 24 * 60 * 60;

    fn row(id: &str, catalogs: &[&str], orphaned: Option<Value>) -> (String, Map<String, Value>) {
        let mut m = Map::new();
        let set = catalogs.iter().map(|c| c.to_string()).collect();
        m.insert(
            meta::CATALOG_IDS_FIELD.into(),
            json!(meta::serialize_catalog_ids(&set)),
        );
        if let Some(at) = orphaned {
            m.insert(meta::ORPHANED_AT_FIELD.into(), at);
        }
        (id.to_string(), m)
    }

    fn inventory(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plan_sorts_rows_into_release_readopt_purge_and_waiting() {
        let rows = vec![
            row("kept", &["cat"], None),
            row("deleted", &["cat"], None),
            row("shared_deleted", &["cat", "other"], None),
            row("other_only", &["other"], None),
            row("came_back", &[], Some(json!(NOW - 3 * DAY))),
            row("unclaimed_in_catalog", &[], None),
            row("expired", &[], Some(json!(NOW - 31 * DAY))),
            row("recent", &[], Some(json!(NOW - 2 * DAY))),
            row("older", &[], Some(json!((NOW - 10 * DAY).to_string()))),
            row("legacy_no_catalog", &[], None),
            row("unreadable_stamp", &[], Some(json!("soon"))),
        ];
        let plan = plan(
            rows,
            "cat",
            &inventory(&["kept", "came_back", "unclaimed_in_catalog", "other_only"]),
            NOW,
        );
        assert_eq!(plan.indexed, 3);
        assert_eq!(plan.release, vec!["deleted", "shared_deleted"]);
        assert_eq!(plan.readopt, vec!["came_back", "unclaimed_in_catalog"]);
        assert_eq!(plan.purge, vec!["expired"]);
        assert_eq!(plan.waiting, 2);
        assert_eq!(plan.oldest_waiting, Some(NOW - 10 * DAY));
    }

    #[test]
    fn exactly_the_grace_period_is_due() {
        let rows = vec![
            row("due", &[], Some(json!(NOW - ORPHAN_GRACE_SECS))),
            row("not_yet", &[], Some(json!(NOW - ORPHAN_GRACE_SECS + 1))),
        ];
        let plan = plan(rows, "cat", &HashSet::new(), NOW);
        assert_eq!(plan.purge, vec!["due"]);
        assert_eq!(plan.waiting, 1);
    }

    #[test]
    fn large_removal_needs_both_the_share_and_the_count() {
        let with = |release: usize, indexed: usize| CleanupPlan {
            indexed,
            release: (0..release).map(|i| i.to_string()).collect(),
            ..Default::default()
        };
        // Every indexed photo gone: what a broken inventory looks like.
        assert!(with(LARGE_REMOVAL_MIN, LARGE_REMOVAL_MIN).is_large_removal());
        // Half is not more than half.
        assert!(!with(LARGE_REMOVAL_MIN, LARGE_REMOVAL_MIN * 2).is_large_removal());
        // Most of a small index is left to the grace period.
        assert!(!with(LARGE_REMOVAL_MIN - 1, LARGE_REMOVAL_MIN - 1).is_large_removal());
        // A large share of a large index, deleted in one go.
        assert!(with(600, 1000).is_large_removal());
        assert!(!with(400, 1000).is_large_removal());
    }

    fn record(id: &str, catalogs: &[&str], orphaned: Option<i64>, dim: usize) -> StoreRecord {
        let (_, metadata) = row(id, catalogs, orphaned.map(|at| json!(at)));
        StoreRecord {
            id: id.to_string(),
            vector: Some(vec![0.1; dim]),
            metadata,
        }
    }

    #[tokio::test]
    async fn apply_releases_readopts_and_purges_with_faces_and_species() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert(
                IMAGE_TABLE,
                &[
                    record("kept", &["cat"], None, 1152),
                    record("deleted", &["cat"], None, 1152),
                    record("shared", &["cat", "other"], None, 1152),
                    record("came_back", &[], Some(NOW - DAY), 1152),
                    record("expired", &[], Some(NOW - 40 * DAY), 1152),
                    record("expired_but_claimed_since", &[], Some(NOW - 40 * DAY), 1152),
                ],
            )
            .await
            .unwrap();
        store
            .upsert(
                FACE_TABLE,
                &[
                    record("expired_0", &[], None, 512),
                    record("expired_1", &[], None, 512),
                    record("deleted_0", &[], None, 512),
                ],
            )
            .await
            .unwrap();
        store
            .upsert(
                SPECIES_TABLE,
                &[
                    record("expired", &[], None, 768),
                    record("came_back", &[], None, 768),
                ],
            )
            .await
            .unwrap();

        let rows = store.scan_meta(IMAGE_TABLE).await.unwrap();
        let plan = plan(rows, "cat", &inventory(&["kept", "came_back"]), NOW);
        // Indexed in another catalog between the scan and the purge.
        store
            .upsert(
                IMAGE_TABLE,
                &[record("expired_but_claimed_since", &["third"], None, 1152)],
            )
            .await
            .unwrap();

        let outcome = apply(&store, "cat", &plan, NOW).await.unwrap();
        assert_eq!(
            outcome,
            CleanupOutcome {
                released: 2,
                orphaned: 1,
                readopted: 1,
                purged: 1,
                warnings: vec![],
            }
        );

        let meta_of = |rows: &[(String, Map<String, Value>)], id: &str| {
            rows.iter().find(|(i, _)| i == id).map(|(_, m)| m.clone())
        };
        let rows = store.scan_meta(IMAGE_TABLE).await.unwrap();
        let deleted = meta_of(&rows, "deleted").unwrap();
        assert!(meta::parse_catalog_ids(&deleted).is_empty());
        assert_eq!(orphaned_at(&deleted), Some(NOW));
        let shared = meta_of(&rows, "shared").unwrap();
        assert_eq!(
            meta::parse_catalog_ids(&shared)
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["other"]
        );
        assert_eq!(orphaned_at(&shared), None);
        let back = meta_of(&rows, "came_back").unwrap();
        assert!(meta::parse_catalog_ids(&back).contains("cat"));
        assert_eq!(orphaned_at(&back), None);
        assert!(meta_of(&rows, "expired").is_none());
        assert!(meta_of(&rows, "expired_but_claimed_since").is_some());
        // The vector survives a catalog change.
        let kept = store.get(IMAGE_TABLE, &["deleted".into()]).await.unwrap();
        assert!(kept[0].vector.is_some());

        let mut faces = store.scan_ids(FACE_TABLE).await.unwrap();
        faces.sort();
        assert_eq!(faces, vec!["deleted_0"]);
        assert_eq!(
            store.scan_ids(SPECIES_TABLE).await.unwrap(),
            vec!["came_back"]
        );
    }
}
