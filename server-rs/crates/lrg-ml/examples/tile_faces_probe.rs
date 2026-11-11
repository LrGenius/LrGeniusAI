//! Counts the faces the tile pass adds to the first pass, on the photos given,
//! at the size the server detects on (2048 px on the long edge).
//!
//! ```bash
//! cargo run --release -p lrg-ml --example tile_faces_probe -- [--crops DIR] a.jpg b.jpg
//! ```
//!
//! Prints one line per photo: faces without tiles, with them, and the size of
//! each added face. `--crops DIR` saves each added face, with a margin, for
//! checking by eye that it is one.

use image::imageops::FilterType;
use lrg_imaging::cull_config::FaceMetricsConfig;
use lrg_ml::faces::{FaceModel, FacePass};

const WORKING_EDGE: u32 = 2048;

fn main() {
    let cfg = FaceMetricsConfig::defaults();
    let tiled = FaceModel::new(lrg_ml::model_paths::resolve_face());
    let plain = FaceModel::new(lrg_ml::model_paths::resolve_face()).without_tiles();
    assert!(tiled.is_cached(), "face detection models not found");

    let args: Vec<String> = std::env::args().skip(1).collect();
    let crops = args
        .iter()
        .position(|a| a == "--crops")
        .and_then(|i| args.get(i + 1))
        .cloned();
    if let Some(dir) = &crops {
        std::fs::create_dir_all(dir).expect("crops directory");
    }
    let (mut before, mut after) = (0usize, 0usize);
    for path in args
        .iter()
        .filter(|a| *a != "--crops" && Some(*a) != crops.as_ref())
    {
        let img = match image::open(path) {
            Ok(img) => img,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let img = if img.width().max(img.height()) > WORKING_EDGE {
            img.resize(WORKING_EDGE, WORKING_EDGE, FilterType::Lanczos3)
        } else {
            img
        }
        .to_rgb8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let first = plain
            .detect_faces(img.as_raw(), w, h, &cfg, FacePass::QualityOnly)
            .expect("first pass");
        let all = tiled
            .detect_faces(img.as_raw(), w, h, &cfg, FacePass::QualityOnly)
            .expect("tile pass");
        before += first.len();
        after += all.len();
        let added = &all[first.len()..];
        let sizes: Vec<String> = added
            .iter()
            .map(|f| format!("{}px@{:.2}", f.bbox[2] - f.bbox[0], f.det_score))
            .collect();
        let name = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo");
        println!(
            "{name}: {} -> {} faces {}",
            first.len(),
            all.len(),
            sizes.join(" ")
        );
        if let Some(dir) = &crops {
            for (i, f) in added.iter().enumerate() {
                let [x1, y1, x2, y2] = f.bbox;
                let m = (x2 - x1) / 2;
                let (cx, cy) = ((x1 - m).max(0) as u32, (y1 - m).max(0) as u32);
                let cw = ((x2 + m) as u32).min(w as u32) - cx;
                let ch = ((y2 + m) as u32).min(h as u32) - cy;
                let crop = image::imageops::crop_imm(&img, cx, cy, cw, ch).to_image();
                let crop =
                    image::imageops::resize(&crop, 96, 96 * ch / cw.max(1), FilterType::Lanczos3);
                crop.save(format!("{dir}/{name}_{i}.png"))
                    .expect("save crop");
            }
        }
    }
    println!("total: {before} -> {after} faces");
}
