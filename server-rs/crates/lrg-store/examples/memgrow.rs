//! Repro harness for the indexing memory growth: replays the exact
//! per-photo write pattern `index_one` performs (IMAGE_TABLE upsert +
//! FACE_TABLE prefix-delete + FACE_TABLE upsert) and prints RSS every N
//! photos. Run with:
//!   cargo run --release -p lrg-store --example memgrow -- 3000

use std::sync::Arc;

use lrg_store::{Store, StoreRecord, FACE_TABLE, IMAGE_TABLE};
use serde_json::{json, Map};

fn rss_mb() -> u64 {
    let pid = std::process::id();
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        / 1024
}

#[tokio::main]
async fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);
    let optimize_every: usize = std::env::var("OPTIMIZE_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(25);

    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).await.unwrap());
    println!("photos,rss_mb,lance_cache_mb");
    println!("0,{},0", rss_mb());

    // A face thumbnail is a base64 JPEG in the metadata blob — roughly this
    // big in the real pipeline, and the reason FACE_TABLE metadata is heavy.
    let thumb = "A".repeat(8 * 1024);

    for i in 0..n {
        let photo_id = format!("photo_{i}");

        let mut m = Map::new();
        m.insert("photo_id".into(), json!(photo_id));
        m.insert("filename".into(), json!(format!("{photo_id}.jpg")));
        m.insert("has_embedding".into(), json!(true));
        m.insert("catalog_ids".into(), json!("[\"cat_1\"]"));
        for k in [
            "cull_sharpness",
            "cull_exposure",
            "cull_noise",
            "cull_technical_score",
            "cull_aesthetic",
        ] {
            m.insert(k.into(), json!(0.5));
        }
        let image_rec = StoreRecord {
            id: photo_id.clone(),
            vector: Some(vec![0.01f32; 1152]),
            metadata: m,
        };

        store
            .delete_by_id_prefix(FACE_TABLE, &format!("{photo_id}_"))
            .await
            .unwrap();

        let faces: Vec<StoreRecord> = (0..2)
            .map(|f| {
                let mut fm = Map::new();
                fm.insert("photo_id".into(), json!(photo_id));
                fm.insert("thumbnail".into(), json!(thumb));
                fm.insert("person_id".into(), json!(""));
                StoreRecord {
                    id: format!("{photo_id}_{f}"),
                    vector: Some(vec![0.02f32; 512]),
                    metadata: fm,
                }
            })
            .collect();
        store.upsert(FACE_TABLE, &faces).await.unwrap();
        store
            .upsert(IMAGE_TABLE, std::slice::from_ref(&image_rec))
            .await
            .unwrap();

        if optimize_every > 0 && store.pending_write_ops() >= optimize_every as u64 {
            store.optimize_all().await.unwrap();
        }

        if (i + 1) % 100 == 0 {
            println!(
                "{},{},{}",
                i + 1,
                rss_mb(),
                store.session_bytes() / (1024 * 1024)
            );
        }
    }
}
