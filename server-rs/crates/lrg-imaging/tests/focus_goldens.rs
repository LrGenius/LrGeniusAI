//! `lrg_imaging::focus::blur_effect` against `skimage.measure.blur_effect`, from
//! `testdata/focus_goldens.json` (`testdata/make_focus_goldens.py`).
//!
//! The synthetic patches are rebuilt here from the same formulas, so they match
//! to the last bits. The astronaut cases decode the JPEG with a different
//! decoder than Pillow and carry the looser tolerance the fixture records.

use lrg_imaging::focus::{blur_effect, luma_region, LumaPatch};
use serde_json::Value;

fn lcg_luma(w: usize, h: usize) -> Vec<f64> {
    let mut x: u64 = 42;
    (0..w * h)
        .map(|_| {
            x = (x * 1103515245 + 12345) & 0x7FFF_FFFF;
            ((x >> 16) & 0xFF) as f64 / 255.0
        })
        .collect()
}

fn sines(w: usize, h: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (xf, yf) = (x as f64, y as f64);
            let v = 0.5
                + 0.18 * (xf * 1.3).sin() * (yf * 0.9).cos()
                + 0.12 * (xf * 0.37 + yf * 0.21).sin()
                + 0.08 * ((xf - yf) * 2.1).sin();
            out.push(v.clamp(0.0, 1.0));
        }
    }
    out
}

fn checker(w: usize, h: usize, cell: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push((((x / cell) + (y / cell)) % 2) as f64);
        }
    }
    out
}

fn astronaut() -> (Vec<u8>, usize, usize) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/astronaut.jpg");
    let img = image::open(path).expect("astronaut.jpg").to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    (img.into_raw(), w, h)
}

#[test]
fn blur_effect_matches_scikit_image() {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/focus_goldens.json"
    ))
    .expect("focus_goldens.json");
    let goldens: Value = serde_json::from_str(&raw).unwrap();
    let (rgb, aw, ah) = astronaut();

    let cases = goldens["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let w = case["width"].as_u64().unwrap() as usize;
        let h = case["height"].as_u64().unwrap() as usize;
        let window = case["window"].as_u64().unwrap() as usize;
        let want = case["blur_effect"].as_f64().unwrap();
        let tolerance = case["tolerance"].as_f64().unwrap();

        let patch = match name {
            "sines" | "sines_wide" => LumaPatch {
                data: sines(w, h),
                width: w,
                height: h,
            },
            "lcg" => LumaPatch {
                data: lcg_luma(w, h),
                width: w,
                height: h,
            },
            "checker4" => LumaPatch {
                data: checker(w, h, 4),
                width: w,
                height: h,
            },
            _ => {
                let rect: Vec<f64> = case["rect"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect();
                let patch = luma_region(&rgb, aw, ah, [rect[0], rect[1], rect[2], rect[3]], 0)
                    .expect("region inside the image");
                assert_eq!((patch.width, patch.height), (w, h), "{name}: region size");
                patch
            }
        };
        let got = blur_effect(&patch, window).unwrap_or_else(|| panic!("{name}: no value"));
        assert!(
            (got - want).abs() <= tolerance,
            "{name} (window {window}): got {got}, scikit-image {want}"
        );
    }
}
