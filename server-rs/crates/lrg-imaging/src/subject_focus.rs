//! Focus on a photo's subject, and on everything else.
//!
//! Frame-wide sharpness asks whether the frame is busy, and the sharpest tile
//! asks whether *anything* is in focus. Neither knows what the photo is of, so
//! neither can tell the two cases culling most needs apart when there is no
//! face to measure:
//!
//! - the subject soft on a sharp background — focus landed behind it, the frame
//!   is a reject;
//! - the subject sharp on a soft background — a shallow depth of field, or the
//!   background smeared by a pan, and the frame is the keeper.
//!
//! The object masks come from `lrg_ml::subject` (RF-DETR-Seg); this module only
//! takes them as grids of booleans, so it needs no model. [`choose_subject`]
//! decides which of the objects the photo is of, and [`measure`] tiles the
//! frame, measures each tile with [`crate::focus`]'s blur effect, and reads the
//! subject's tiles and the background's apart.

use crate::focus::{blur_effect_axes, luma_region, BLUR_WINDOW};

/// An object as `lrg_ml::subject` reports it, without the dependency.
#[derive(Debug, Clone, Copy)]
pub struct ObjectMask<'a> {
    /// `"person"`, `"animal"`, `"vehicle"`, `"object"` or `"scenery"`.
    pub kind: &'a str,
    pub score: f32,
    /// Side of the mask grid; the grid spans the whole frame.
    pub grid: usize,
    /// `grid × grid` cells, row-major.
    pub mask: &'a [bool],
}

/// What the photo is of: the main object and the others of its kind beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct Subject {
    pub kind: String,
    /// The main object's score.
    pub score: f64,
    pub grid: usize,
    pub mask: Vec<bool>,
    /// The share of the frame the subject covers.
    pub area: f64,
    /// Centre of the subject's mask, as fractions of the frame.
    pub center: (f64, f64),
}

/// A subject smaller than this share of the frame is not measured: a few
/// hundred pixels on the working copy, too few tiles to say anything.
pub const MIN_SUBJECT_AREA: f64 = 0.004;

/// An object of the main one's kind joins the subject when it is at least this
/// share of the main one's area — the second person of a couple, not a
/// passer-by in the back.
const COMPANION_AREA: f64 = 0.25;

/// ...and the model is at least this sure of it.
const COMPANION_SCORE: f32 = 0.5;

fn area_and_center(mask: &[bool], grid: usize) -> (f64, (f64, f64)) {
    let (mut n, mut sx, mut sy) = (0usize, 0.0f64, 0.0f64);
    for (i, _) in mask.iter().enumerate().filter(|(_, &m)| m) {
        n += 1;
        sx += (i % grid) as f64 + 0.5;
        sy += (i / grid) as f64 + 0.5;
    }
    if n == 0 {
        return (0.0, (0.5, 0.5));
    }
    let g = grid as f64;
    (n as f64 / (g * g), (sx / n as f64 / g, sy / n as f64 / g))
}

/// Whether two masks on the same grid touch: a cell of one inside the other,
/// or next to one of its cells.
fn touching(a: &[bool], b: &[bool], grid: usize) -> bool {
    a.iter().enumerate().filter(|(_, &on)| on).any(|(i, _)| {
        let (x, y) = ((i % grid) as isize, (i / grid) as isize);
        (-1..=1).any(|dy| {
            (-1..=1).any(|dx| {
                let (nx, ny) = (x + dx, y + dy);
                nx >= 0
                    && ny >= 0
                    && (nx as usize) < grid
                    && (ny as usize) < grid
                    && b[ny as usize * grid + nx as usize]
            })
        })
    })
}

/// Kinds that make one subject together when they touch: the rider and the
/// horse, the cyclist and the bicycle.
fn joins_across_kinds(kind: &str) -> bool {
    matches!(kind, "person" | "animal" | "vehicle")
}

/// The object the photo is of, joined by others of its kind of a similar size.
///
/// The main object is the one with the most weight: how sure the model is,
/// times the square root of its area (a large subject counts for more, but not
/// in proportion — a wall-sized background object would win otherwise), times
/// how central it is (a subject half-way to an edge counts three quarters).
/// Furniture and street fixtures are never the subject. `None` when nothing
/// covers [`MIN_SUBJECT_AREA`].
///
/// A person, an animal or a vehicle touching the main object joins it whatever
/// its kind: on the public benchmark the horse alone was the subject of a show
/// jump, and its rider "the background".
pub fn choose_subject(objects: &[ObjectMask<'_>]) -> Option<Subject> {
    let candidates: Vec<(usize, f64, f64, (f64, f64))> = objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.kind != "scenery" && o.grid > 0 && o.mask.len() == o.grid * o.grid)
        .map(|(i, o)| {
            let (area, center) = area_and_center(o.mask, o.grid);
            let off_centre = ((center.0 - 0.5).powi(2) + (center.1 - 0.5).powi(2)).sqrt();
            let centrality = 1.0 - 0.5 * (off_centre / 0.5).min(1.0);
            let weight = o.score as f64 * area.sqrt() * centrality;
            (i, weight, area, center)
        })
        .filter(|&(_, _, area, _)| area >= MIN_SUBJECT_AREA)
        .collect();
    let &(main, _, main_area, _) = candidates
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))?;
    let main_obj = &objects[main];
    let grid = main_obj.grid;
    let mut mask = main_obj.mask.to_vec();
    for &(i, _, area, _) in &candidates {
        let o = &objects[i];
        let alike = o.kind == main_obj.kind
            || (joins_across_kinds(o.kind)
                && joins_across_kinds(main_obj.kind)
                && touching(main_obj.mask, o.mask, grid));
        if i != main
            && alike
            && o.grid == grid
            && o.score >= COMPANION_SCORE
            && area >= COMPANION_AREA * main_area
        {
            for (cell, &m) in mask.iter_mut().zip(o.mask) {
                *cell |= m;
            }
        }
    }
    let (area, center) = area_and_center(&mask, grid);
    Some(Subject {
        kind: main_obj.kind.to_string(),
        score: main_obj.score as f64,
        grid,
        mask,
        area,
        center,
    })
}

/// Side of the grid a subject's mask is stored on: a third of the model's, so
/// a photo carries 170 characters of it, and still a cell of 4 % of the frame
/// across — finer than any face that has a say in culling.
pub const STORED_GRID: usize = 26;

impl Subject {
    /// The mask on [`STORED_GRID`], a cell set when any cell of the mask under
    /// it is, as hex, row-major, eight cells to a byte.
    pub fn stored_mask(&self) -> String {
        let mut coarse = vec![false; STORED_GRID * STORED_GRID];
        for (i, _) in self.mask.iter().enumerate().filter(|(_, &on)| on) {
            let (x, y) = (i % self.grid, i / self.grid);
            coarse[(y * STORED_GRID / self.grid) * STORED_GRID + x * STORED_GRID / self.grid] =
                true;
        }
        coarse
            .chunks(8)
            .map(|byte| {
                let bits = byte
                    .iter()
                    .enumerate()
                    .fold(0u8, |acc, (b, &on)| acc | (u8::from(on) << (7 - b)));
                format!("{bits:02x}")
            })
            .collect()
    }
}

/// A mask [`Subject::stored_mask`] wrote, back as cells; `None` when the text
/// is not one.
pub fn read_stored_mask(text: &str) -> Option<Vec<bool>> {
    let cells = STORED_GRID * STORED_GRID;
    if text.len() != cells.div_ceil(8) * 2 {
        return None;
    }
    let mut mask = Vec::with_capacity(cells);
    for pair in text.as_bytes().chunks(2) {
        let byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        mask.extend((0..8).map(|b| byte & (1 << (7 - b)) != 0));
    }
    mask.truncate(cells);
    Some(mask)
}

/// Whether a point, as fractions of the frame, lies on a stored mask. No
/// margin around it: storing already widens the mask by up to a cell, and on
/// the public benchmark one more cell took in the spectators right behind a
/// player.
pub fn on_stored_mask(mask: &[bool], x: f64, y: f64) -> bool {
    let cell = |v: f64| ((v.clamp(0.0, 1.0) * STORED_GRID as f64) as usize).min(STORED_GRID - 1);
    mask.get(cell(y) * STORED_GRID + cell(x))
        .copied()
        .unwrap_or(false)
}

/// Focus on the subject and on the rest of the frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubjectFocus {
    /// `1 - blur effect` over the subject's sharper tiles; `None` when too few
    /// of them carry any structure to judge.
    pub subject: Option<f64>,
    /// The same over the background's sharper tiles.
    pub background: Option<f64>,
    /// How much sharper the background reads along one axis than across it
    /// (median over its tiles, `0..1`): high for a background smeared sideways
    /// by a pan, low for one that is out of focus all round or sharp.
    pub background_streak: Option<f64>,
    pub subject_tiles: usize,
    pub background_tiles: usize,
}

/// Tiles of the subject must lie this far inside its mask, background tiles
/// entirely outside it — and outside a margin around it, since the mask is
/// coarse (78 cells across the frame) and a tile on the subject's edge reads
/// half one, half the other.
const SUBJECT_COVERAGE: f64 = 0.8;
const BACKGROUND_MARGIN: f64 = 0.25;

/// At most this many tiles of each are measured, spread evenly: enough for a
/// stable percentile, and a bound on the cost of a large subject.
const MAX_TILES: usize = 64;

/// Which share of the tiles' focus readings, from the bottom, is reported. The
/// sharper part of a subject decides — a bird's head can be sharp while its
/// wings blur — but not the single sharpest tile, which may be a stray
/// highlight.
const PERCENTILE: f64 = 0.8;

fn coverage(subject: &Subject, w: usize, h: usize, rect: [f64; 4]) -> f64 {
    let g = subject.grid as f64;
    let mut inside = 0usize;
    const SAMPLES: usize = 5;
    for sy in 0..SAMPLES {
        for sx in 0..SAMPLES {
            let x = rect[0] + (rect[2] - rect[0]) * (sx as f64 + 0.5) / SAMPLES as f64;
            let y = rect[1] + (rect[3] - rect[1]) * (sy as f64 + 0.5) / SAMPLES as f64;
            let cx = ((x / w as f64 * g) as usize).min(subject.grid - 1);
            let cy = ((y / h as f64 * g) as usize).min(subject.grid - 1);
            inside += usize::from(subject.mask[cy * subject.grid + cx]);
        }
    }
    inside as f64 / (SAMPLES * SAMPLES) as f64
}

fn percentile(mut values: Vec<f64>, p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let rank = ((p * values.len() as f64).ceil() as usize).clamp(1, values.len());
    Some(values[rank - 1])
}

/// Every `n`-th element, so that at most `max` remain, spread over the list.
fn spread<T: Copy>(items: &[T], max: usize) -> Vec<T> {
    if items.len() <= max {
        return items.to_vec();
    }
    (0..max).map(|i| items[i * items.len() / max]).collect()
}

/// Focus on `subject` and on the background of an RGB8 image.
///
/// The tiles are a fixed fraction of the subject's size, a sixth of the side of
/// a square of its area, between 32 and 96 px: large enough to hold structure,
/// small enough that a subject has several. Background and subject are tiled
/// alike, so the two readings are comparable within the frame — and frames of
/// one burst, whose subject is the same size, with each other.
pub fn measure(rgb: &[u8], width: usize, height: usize, subject: &Subject) -> SubjectFocus {
    if width == 0
        || height == 0
        || rgb.len() < width * height * 3
        || subject.grid == 0
        || subject.mask.len() != subject.grid * subject.grid
    {
        return SubjectFocus::default();
    }
    let subject_px = subject.area * (width * height) as f64;
    let tile = ((subject_px.sqrt() / 6.0).round() as usize).clamp(32, 96);
    let (mut inner, mut outer) = (Vec::new(), Vec::new());
    let margin = tile as f64 * BACKGROUND_MARGIN;
    for ty in 0..height / tile {
        for tx in 0..width / tile {
            let rect = [
                (tx * tile) as f64,
                (ty * tile) as f64,
                ((tx + 1) * tile) as f64,
                ((ty + 1) * tile) as f64,
            ];
            if coverage(subject, width, height, rect) >= SUBJECT_COVERAGE {
                inner.push(rect);
            } else {
                let grown = [
                    (rect[0] - margin).max(0.0),
                    (rect[1] - margin).max(0.0),
                    (rect[2] + margin).min(width as f64),
                    (rect[3] + margin).min(height as f64),
                ];
                if coverage(subject, width, height, grown) == 0.0 {
                    outer.push(rect);
                }
            }
        }
    }

    let read = |rect: [f64; 4]| -> Option<[f64; 2]> {
        let patch = luma_region(rgb, width, height, rect, 0)?;
        blur_effect_axes(&patch, BLUR_WINDOW)
    };
    let subject_axes: Vec<[f64; 2]> = spread(&inner, MAX_TILES)
        .into_iter()
        .filter_map(read)
        .collect();
    let background_axes: Vec<[f64; 2]> = spread(&outer, MAX_TILES)
        .into_iter()
        .filter_map(read)
        .collect();
    let focus_of = |axes: &[f64; 2]| (1.0 - axes[0].max(axes[1])).clamp(0.0, 1.0);

    let subject_focus = (subject_axes.len() >= 2)
        .then(|| percentile(subject_axes.iter().map(focus_of).collect(), PERCENTILE))
        .flatten();
    let background_focus = (background_axes.len() >= 3)
        .then(|| percentile(background_axes.iter().map(focus_of).collect(), PERCENTILE))
        .flatten();
    let background_streak = (background_axes.len() >= 3)
        .then(|| {
            percentile(
                background_axes
                    .iter()
                    .map(|a| (a[0] - a[1]).abs())
                    .collect(),
                0.5,
            )
        })
        .flatten();
    SubjectFocus {
        subject: subject_focus,
        background: background_focus,
        background_streak,
        subject_tiles: subject_axes.len(),
        background_tiles: background_axes.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 640;
    const H: usize = 480;
    const GRID: usize = 78;

    /// Detail at every scale, as a photograph has, so both subject and
    /// background have structure a blur removes: deterministic noise, softened
    /// by one pixel as a lens would. (A few sines will not do: their fine
    /// components vanish under a blur and the coarse one left over still reads
    /// as sharp.)
    fn textured() -> Vec<f64> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let noise: Vec<f64> = (0..W * H)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                0.2 + 0.6 * (state >> 11) as f64 / (1u64 << 53) as f64
            })
            .collect();
        blurred(&noise, 1, false)
    }

    /// A box blur of `radius` along x only (`horizontal`) or along both axes.
    fn blurred(luma: &[f64], radius: usize, horizontal_only: bool) -> Vec<f64> {
        let pass = |src: &[f64], along_x: bool| -> Vec<f64> {
            let mut out = vec![0.0; W * H];
            for y in 0..H {
                for x in 0..W {
                    let (mut sum, mut n) = (0.0, 0.0);
                    for k in -(radius as isize)..=radius as isize {
                        let (xx, yy) = if along_x {
                            (x as isize + k, y as isize)
                        } else {
                            (x as isize, y as isize + k)
                        };
                        if xx >= 0 && yy >= 0 && (xx as usize) < W && (yy as usize) < H {
                            sum += src[yy as usize * W + xx as usize];
                            n += 1.0;
                        }
                    }
                    out[y * W + x] = sum / n;
                }
            }
            out
        };
        let x = pass(luma, true);
        if horizontal_only {
            x
        } else {
            pass(&x, false)
        }
    }

    /// A centred rectangle, a third of the frame each way.
    fn subject_box() -> Subject {
        let mut mask = vec![false; GRID * GRID];
        for y in GRID / 3..2 * GRID / 3 {
            for x in GRID / 3..2 * GRID / 3 {
                mask[y * GRID + x] = true;
            }
        }
        let (area, center) = area_and_center(&mask, GRID);
        Subject {
            kind: "animal".into(),
            score: 0.9,
            grid: GRID,
            mask,
            area,
            center,
        }
    }

    /// Composes `inside` within the subject's box and `outside` beyond it.
    fn compose(inside: &[f64], outside: &[f64], subject: &Subject) -> Vec<u8> {
        let mut rgb = vec![0u8; W * H * 3];
        for y in 0..H {
            for x in 0..W {
                let cx = (x * GRID / W).min(GRID - 1);
                let cy = (y * GRID / H).min(GRID - 1);
                let v = if subject.mask[cy * GRID + cx] {
                    inside[y * W + x]
                } else {
                    outside[y * W + x]
                };
                let b = (v * 255.0).round() as u8;
                rgb[(y * W + x) * 3..(y * W + x) * 3 + 3].fill(b);
            }
        }
        rgb
    }

    #[test]
    fn a_soft_subject_on_a_sharp_background_reads_soft() {
        let sharp = textured();
        let soft = blurred(&sharp, 3, false);
        let subject = subject_box();
        let focus = measure(&compose(&soft, &sharp, &subject), W, H, &subject);
        let (s, b) = (focus.subject.unwrap(), focus.background.unwrap());
        assert!(b > s + 0.15, "subject {s:.3}, background {b:.3}");
        assert!(focus.subject_tiles >= 2 && focus.background_tiles >= 3);
    }

    #[test]
    fn a_sharp_subject_on_a_soft_background_reads_sharp() {
        let sharp = textured();
        let soft = blurred(&sharp, 6, false);
        let subject = subject_box();
        let focus = measure(&compose(&sharp, &soft, &subject), W, H, &subject);
        let (s, b) = (focus.subject.unwrap(), focus.background.unwrap());
        assert!(s > b + 0.15, "subject {s:.3}, background {b:.3}");
        // Out of focus all round: no streak.
        assert!(focus.background_streak.unwrap() < 0.15);
    }

    #[test]
    fn a_pan_streaks_the_background_and_leaves_the_subject() {
        let sharp = textured();
        let panned = blurred(&sharp, 12, true);
        let subject = subject_box();
        let focus = measure(&compose(&sharp, &panned, &subject), W, H, &subject);
        let still = measure(&compose(&sharp, &sharp, &subject), W, H, &subject);
        assert!(focus.subject.unwrap() > focus.background.unwrap());
        let (pan, none) = (
            focus.background_streak.unwrap(),
            still.background_streak.unwrap(),
        );
        assert!(pan > none + 0.2, "pan {pan:.3}, still {none:.3}");
    }

    #[test]
    fn the_subject_is_the_weightiest_object_with_its_companions() {
        let cell = |x0: usize, y0: usize, x1: usize, y1: usize| {
            let mut m = vec![false; GRID * GRID];
            for y in y0..y1 {
                for x in x0..x1 {
                    m[y * GRID + x] = true;
                }
            }
            m
        };
        let table = cell(0, 40, 78, 78); // large, but scenery
        let bride = cell(20, 10, 36, 60);
        let groom = cell(40, 10, 56, 60);
        let passer_by = cell(70, 30, 73, 36); // a person, but tiny
        let dog = cell(5, 60, 12, 70);
        let objects = [
            ObjectMask {
                kind: "scenery",
                score: 0.95,
                grid: GRID,
                mask: &table,
            },
            ObjectMask {
                kind: "person",
                score: 0.9,
                grid: GRID,
                mask: &bride,
            },
            ObjectMask {
                kind: "person",
                score: 0.85,
                grid: GRID,
                mask: &groom,
            },
            ObjectMask {
                kind: "person",
                score: 0.9,
                grid: GRID,
                mask: &passer_by,
            },
            ObjectMask {
                kind: "animal",
                score: 0.9,
                grid: GRID,
                mask: &dog,
            },
        ];
        let subject = choose_subject(&objects).unwrap();
        assert_eq!(subject.kind, "person");
        let (bride_area, _) = area_and_center(&bride, GRID);
        let (groom_area, _) = area_and_center(&groom, GRID);
        assert!((subject.area - (bride_area + groom_area)).abs() < 1e-9);
        assert!((subject.center.0 - 0.5).abs() < 0.05);
    }

    #[test]
    fn a_rider_and_the_horse_are_one_subject_but_a_distant_car_is_not() {
        let cell = |x0: usize, y0: usize, x1: usize, y1: usize| {
            let mut m = vec![false; GRID * GRID];
            for y in y0..y1 {
                for x in x0..x1 {
                    m[y * GRID + x] = true;
                }
            }
            m
        };
        let horse = cell(25, 35, 55, 60);
        let rider = cell(32, 19, 46, 35); // sits on it: the masks touch
        let car = cell(0, 0, 12, 10); // a vehicle, but apart
        let objects = [
            ObjectMask {
                kind: "animal",
                score: 0.93,
                grid: GRID,
                mask: &horse,
            },
            ObjectMask {
                kind: "person",
                score: 0.9,
                grid: GRID,
                mask: &rider,
            },
            ObjectMask {
                kind: "vehicle",
                score: 0.9,
                grid: GRID,
                mask: &car,
            },
        ];
        let subject = choose_subject(&objects).unwrap();
        let (horse_area, _) = area_and_center(&horse, GRID);
        let (rider_area, _) = area_and_center(&rider, GRID);
        assert!((subject.area - (horse_area + rider_area)).abs() < 1e-9);
    }

    #[test]
    fn a_stored_mask_reads_back_and_places_a_face_on_its_person() {
        let mut mask = vec![false; GRID * GRID];
        // A person standing in the left third, head at the top.
        for y in 10..70 {
            for x in 8..24 {
                mask[y * GRID + x] = true;
            }
        }
        let subject = Subject {
            kind: "person".into(),
            score: 0.9,
            grid: GRID,
            mask,
            area: 0.0,
            center: (0.2, 0.5),
        };
        let text = subject.stored_mask();
        assert_eq!(text.len(), 170);
        let stored = read_stored_mask(&text).unwrap();
        // The face, at the top of the person, is on it; a face in the right
        // half is not, nor one a cell above the head.
        assert!(on_stored_mask(&stored, 0.2, 0.15));
        assert!(!on_stored_mask(&stored, 0.75, 0.2));
        assert!(!on_stored_mask(&stored, 0.2, 0.06));
        assert!(read_stored_mask("zz").is_none());
        assert!(read_stored_mask(&"g".repeat(170)).is_none());
    }

    #[test]
    fn nothing_large_enough_is_no_subject() {
        let mut speck = vec![false; GRID * GRID];
        speck[GRID * 40 + 40] = true;
        let objects = [ObjectMask {
            kind: "animal",
            score: 0.99,
            grid: GRID,
            mask: &speck,
        }];
        assert_eq!(choose_subject(&objects), None);
        assert_eq!(choose_subject(&[]), None);
    }

    #[test]
    fn a_featureless_frame_measures_nothing() {
        let rgb = vec![128u8; W * H * 3];
        let focus = measure(&rgb, W, H, &subject_box());
        assert_eq!(focus.subject, None);
        assert_eq!(focus.background, None);
    }
}
