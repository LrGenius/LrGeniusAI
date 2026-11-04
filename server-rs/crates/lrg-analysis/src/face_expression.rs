//! What the stored face state says about a face or a photo, for the workflows
//! that ask a person's question of it: "her with the eyes open", "the best
//! portrait of him", "everyone smiling at the camera".
//!
//! Culling compares frames of one burst and can lean on the group. These
//! questions are asked of a whole catalog, so there is no group to lean on and
//! each answer has to hold alone. That is why a reading the model did not make
//! is [`Verdict::Unknown`] and not a no: a photo indexed before the model was on
//! disk is a data problem the caller reports, where a frown is simply not a
//! smile.

use serde_json::{Map, Value};

use crate::grouping::face_state_is_current;

/// `mouthSmile` from here up reads as smiling. In portraits judged by eye, 0.35 to
/// 0.5 is at most the hint of one, and from 0.5 most faces show a smile, closed
/// mouth included. A snarl can reach it too.
pub const SMILING_FROM: f64 = 0.5;

/// Eyes this near the lens read as looking at the camera, head turn included.
/// Portraits have a median of 9°; a high-angle selfie reads 15–20° for looking
/// straight at the lens, because `eyeLookUp` barely moves. `looking_away` starts
/// further out (25°), so a face between the two is claimed by neither.
pub const LOOKING_AT_CAMERA_WITHIN_DEGREES: f64 = 20.0;

/// What [`portrait_score`] charges a row with no reading.
const UNREAD_COST: f64 = 0.3;

/// Answer to one question about a face or a photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Yes,
    No,
    /// The model made no such reading, so the question cannot be answered.
    Unknown,
}

impl Verdict {
    fn and(self, other: Verdict) -> Verdict {
        match (self, other) {
            (Verdict::No, _) | (_, Verdict::No) => Verdict::No,
            (Verdict::Unknown, _) | (_, Verdict::Unknown) => Verdict::Unknown,
            _ => Verdict::Yes,
        }
    }

    fn of(reading: Option<bool>) -> Verdict {
        match reading {
            Some(true) => Verdict::Yes,
            Some(false) => Verdict::No,
            None => Verdict::Unknown,
        }
    }
}

/// What a caller asks of a face or a photo. Empty asks nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExpressionFilter {
    pub eyes_open: bool,
    pub smiling: bool,
    pub looking_at_camera: bool,
}

impl ExpressionFilter {
    pub fn is_empty(&self) -> bool {
        !(self.eyes_open || self.smiling || self.looking_at_camera)
    }

    /// In plain words, for a set's label or a message: "eyes open and smiling".
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.eyes_open {
            parts.push("eyes open");
        }
        if self.smiling {
            parts.push("smiling");
        }
        if self.looking_at_camera {
            parts.push("looking at the camera");
        }
        parts.join(" and ")
    }

    fn judge(
        &self,
        eye_state: Option<&str>,
        smile: Option<f64>,
        gaze_off_lens: Option<f64>,
    ) -> Verdict {
        // "unknown" is the model saying it cannot tell, the same as no reading.
        let eye_state = eye_state.filter(|s| *s != "unknown");
        let mut verdict = Verdict::Yes;
        if self.eyes_open {
            verdict = verdict.and(Verdict::of(eye_state.map(|s| s == "open")));
        }
        if self.smiling {
            verdict = verdict.and(Verdict::of(smile.map(|s| s >= SMILING_FROM)));
        }
        if self.looking_at_camera {
            // Shut eyes look nowhere, whatever `eyeLook*` says, and that is an
            // answer rather than a missing reading.
            let looking = if eye_state == Some("closed") {
                Some(false)
            } else {
                gaze_off_lens.map(|g| g <= LOOKING_AT_CAMERA_WITHIN_DEGREES)
            };
            verdict = verdict.and(Verdict::of(looking));
        }
        verdict
    }

    /// Judges one row of the face table.
    pub fn judge_face(&self, face: &Map<String, Value>) -> Verdict {
        self.judge(
            face.get("face_eye_state").and_then(Value::as_str),
            face.get("face_smile").and_then(Value::as_f64),
            face.get("face_gaze_off_lens").and_then(Value::as_f64),
        )
    }

    /// Judges a photo by its stored aggregates, which speak for the people who
    /// matter in the frame together: everyone's eyes open, everyone smiling.
    pub fn judge_photo(&self, photo: &Map<String, Value>) -> Verdict {
        // Measured, and no faces: not a smile, and nothing the user could fix.
        if photo.get("cull_face_count").and_then(Value::as_i64) == Some(0) {
            return Verdict::No;
        }
        if !face_state_is_current(photo) {
            return Verdict::Unknown;
        }
        self.judge(
            photo.get("cull_eye_state").and_then(Value::as_str),
            photo.get("cull_smile").and_then(Value::as_f64),
            photo.get("cull_gaze_off_lens").and_then(Value::as_f64),
        )
    }
}

/// How good a portrait of its person a face row makes, higher better: for the
/// picture on a person's card, and for "the best photos of her".
///
/// Starts from what the detector and the sharpness measure say, which is all a
/// row from before the face state has. Then a shut eye costs more than anything
/// else can earn: a person's card showing them blinking is the failure this
/// exists for. A row with no reading at all pays a fixed price: nobody has seen
/// its eyes, so it should not outrank a portrait known to be good, and among such
/// rows the order stays what it always was.
pub fn portrait_score(face: &Map<String, Value>) -> f64 {
    let number = |key: &str| face.get(key).and_then(Value::as_f64);
    let mut score =
        number("face_det_score").unwrap_or(0.0) + number("face_sharpness").unwrap_or(0.0);
    match face.get("face_eye_state").and_then(Value::as_str) {
        Some("closed") => score -= 1.0,
        Some("half") => score -= 0.4,
        Some(_) => {}
        None => score -= UNREAD_COST,
    }
    if let Some(yaw) = number("face_yaw") {
        score -= 0.5 * (yaw.abs() / 60.0).clamp(0.0, 1.0);
    }
    if let Some(gaze) = number("face_gaze_off_lens") {
        score -= 0.3 * (gaze / 45.0).clamp(0.0, 1.0);
    }
    if let Some(smile) = number("face_smile") {
        score += 0.2 * smile.clamp(0.0, 1.0);
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grouping::{FACE_STATE_VERSION, FACE_STATE_VERSION_KEY};
    use serde_json::json;

    fn row(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn filter(eyes_open: bool, smiling: bool, looking_at_camera: bool) -> ExpressionFilter {
        ExpressionFilter {
            eyes_open,
            smiling,
            looking_at_camera,
        }
    }

    #[test]
    fn an_empty_filter_asks_nothing_and_passes_anything() {
        assert!(ExpressionFilter::default().is_empty());
        assert_eq!(
            ExpressionFilter::default().judge_face(&row(json!({}))),
            Verdict::Yes
        );
    }

    #[test]
    fn a_face_answers_each_question_from_its_own_reading() {
        let face = row(json!({
            "face_eye_state": "open",
            "face_smile": 0.8,
            "face_gaze_off_lens": 6.0,
        }));
        assert_eq!(filter(true, true, true).judge_face(&face), Verdict::Yes);

        let frowning = row(json!({"face_eye_state": "open", "face_smile": 0.1}));
        assert_eq!(
            filter(false, true, false).judge_face(&frowning),
            Verdict::No
        );

        let blinking = row(json!({"face_eye_state": "closed"}));
        assert_eq!(
            filter(true, false, false).judge_face(&blinking),
            Verdict::No
        );

        let away = row(json!({"face_gaze_off_lens": 38.0}));
        assert_eq!(filter(false, false, true).judge_face(&away), Verdict::No);
    }

    /// A photo that could not be judged is a data problem the caller reports;
    /// one judged and found wanting is simply not a match.
    #[test]
    fn a_missing_reading_is_unknown_and_a_no_outranks_it() {
        let empty = row(json!({}));
        assert_eq!(
            filter(true, false, false).judge_face(&empty),
            Verdict::Unknown
        );
        let unsure = row(json!({"face_eye_state": "unknown"}));
        assert_eq!(
            filter(true, false, false).judge_face(&unsure),
            Verdict::Unknown
        );
        // Shut eyes settle it, whatever else is missing.
        let shut = row(json!({"face_eye_state": "closed"}));
        assert_eq!(filter(true, true, false).judge_face(&shut), Verdict::No);
    }

    #[test]
    fn a_photo_speaks_for_everyone_in_it() {
        let current = |extra: Value| {
            let mut m = row(extra);
            m.insert(FACE_STATE_VERSION_KEY.into(), json!(FACE_STATE_VERSION));
            m.insert("cull_face_count".into(), json!(2));
            m
        };
        let group = current(json!({
            "cull_eye_state": "open",
            "cull_smile": 0.7,
            "cull_gaze_off_lens": 12.0,
        }));
        assert_eq!(filter(true, true, true).judge_photo(&group), Verdict::Yes);
        let one_blink = current(json!({"cull_eye_state": "closed"}));
        assert_eq!(
            filter(true, false, false).judge_photo(&one_blink),
            Verdict::No
        );
    }

    #[test]
    fn a_photo_without_faces_is_no_and_one_not_analysed_is_unknown() {
        let landscape = row(json!({"cull_face_count": 0}));
        assert_eq!(
            filter(false, true, false).judge_photo(&landscape),
            Verdict::No
        );

        let never_prepared = row(json!({}));
        assert_eq!(
            filter(false, true, false).judge_photo(&never_prepared),
            Verdict::Unknown
        );

        // An older analysis means something else by the same fields.
        let stale = row(json!({
            "cull_face_count": 1,
            "cull_smile": 0.9,
            FACE_STATE_VERSION_KEY: FACE_STATE_VERSION - 1,
        }));
        assert_eq!(
            filter(false, true, false).judge_photo(&stale),
            Verdict::Unknown
        );
    }

    /// Shut eyes look nowhere: a plain no, not a reading to go and fetch, and not
    /// a yes when `eyeLook*` happens to read near the lens.
    #[test]
    fn shut_eyes_are_not_looking_at_the_camera() {
        let face = row(json!({"face_eye_state": "closed", "face_gaze_off_lens": 4.0}));
        assert_eq!(filter(false, false, true).judge_face(&face), Verdict::No);
        let photo = row(json!({
            "cull_face_count": 1,
            "cull_eye_state": "closed",
            FACE_STATE_VERSION_KEY: FACE_STATE_VERSION,
        }));
        assert_eq!(filter(false, false, true).judge_photo(&photo), Verdict::No);
    }

    #[test]
    fn the_filter_says_what_it_asks() {
        assert_eq!(
            filter(true, true, false).describe(),
            "eyes open and smiling"
        );
        assert_eq!(
            filter(false, false, true).describe(),
            "looking at the camera"
        );
    }

    /// The card showing someone blinking is what this exists to prevent: a
    /// sharper, better detected frame with the eyes shut still loses.
    #[test]
    fn a_sharp_blink_loses_to_a_softer_frame_with_the_eyes_open() {
        let blink = row(json!({
            "face_det_score": 0.98,
            "face_sharpness": 0.9,
            "face_eye_state": "closed",
            "face_yaw": 2.0,
            "face_gaze_off_lens": 5.0,
        }));
        let open = row(json!({
            "face_det_score": 0.9,
            "face_sharpness": 0.5,
            "face_eye_state": "open",
            "face_yaw": 8.0,
            "face_gaze_off_lens": 8.0,
        }));
        assert!(portrait_score(&open) > portrait_score(&blink));
    }

    #[test]
    fn a_frontal_face_beats_a_turned_one_and_a_smile_breaks_the_tie() {
        let base = json!({
            "face_det_score": 0.9,
            "face_sharpness": 0.5,
            "face_eye_state": "open",
            "face_gaze_off_lens": 8.0,
        });
        let at = |key: &str, value: f64| {
            let mut m = row(base.clone());
            m.insert(key.into(), json!(value));
            m
        };
        assert!(portrait_score(&at("face_yaw", 3.0)) > portrait_score(&at("face_yaw", 45.0)));
        assert!(portrait_score(&at("face_smile", 0.9)) > portrait_score(&at("face_smile", 0.1)));
    }

    /// Nobody has seen its eyes: it yields to a portrait known to be good, and
    /// such rows keep the order among themselves that they always had.
    #[test]
    fn a_row_without_a_reading_yields_to_a_good_portrait_and_keeps_its_order() {
        let old = |sharpness: f64| row(json!({"face_det_score": 0.9, "face_sharpness": sharpness}));
        assert!(portrait_score(&old(0.8)) > portrait_score(&old(0.5)));
        let known_good = row(json!({
            "face_det_score": 0.9,
            "face_sharpness": 0.5,
            "face_eye_state": "open",
            "face_yaw": 5.0,
            "face_gaze_off_lens": 8.0,
        }));
        assert!(portrait_score(&known_good) > portrait_score(&old(0.5)));
    }
}
