//! Tunable parameters for the per-image and per-face culling metrics.
//!
//! These live here, below both `lrg-ml` and `lrg-analysis`, because the code
//! that consumes them is split across those two crates: `metrics::culling_metrics`
//! reads [`ImageMetricsConfig`], while `lrg_ml::face_quality` and
//! `lrg_analysis::face_aggregate` both read [`FaceMetricsConfig`]. Neither of
//! those crates can depend on the other, and `lrg-analysis`'s `CullingConfig`
//! (which owns grouping/ranking on top of these two) sits above both.
//!
//! History: every field here previously existed *twice* — once in
//! `lrg_analysis::culling_config`, where the culling presets set it, and again
//! as a private `const` next to each algorithm, which is what actually got
//! used. The preset copies were dead, so `portrait`/`sports`/`event` could not
//! move any image or face metric no matter what they set. These are now the
//! single source of truth and the presets reach them for real.
//!
//! [`Default`] reproduces `BASE_CULLING_CONFIG` exactly, so the golden tests
//! that pin metric output stay bit-identical.

/// Parameters for [`crate::metrics::culling_metrics`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageMetricsConfig {
    pub sharpness_denominator: f64,
    pub highlight_threshold: f64,
    pub shadow_threshold: f64,
    pub highlight_clip_weight: f64,
    pub shadow_clip_weight: f64,
    pub exposure_target: f64,
    pub exposure_tolerance: f64,
    pub exposure_balance_weight: f64,
    pub exposure_clip_weight: f64,
    pub noise_denominator: f64,
    pub technical_weight_sharpness: f64,
    pub technical_weight_exposure: f64,
    pub technical_weight_noise: f64,
    pub aesthetic_contrast_weight: f64,
    pub aesthetic_colorfulness_weight: f64,
    pub aesthetic_exposure_weight: f64,
    /// Side of the square tile grid the frame is divided into for the
    /// subject-region sharpness pass. 8 gives 64 tiles of ~64px on the 512px
    /// working image — small enough to isolate a face or a bird against a
    /// smooth background, large enough that each tile's Laplacian variance is
    /// still a stable estimate.
    pub sharpness_tile_grid: usize,
    /// A tile counts as "in focus" at this fraction of the sharpest tile's
    /// variance. Drives `cull_focus_concentration` and the sharp-region
    /// centroid.
    pub sharpness_tile_focus_fraction: f64,
    /// Largest local gradient magnitude (per pixel, in `0..1` luma units,
    /// measured on the 3×3-smoothed image) at which a pixel still counts as
    /// *flat* for `cull_noise_flat`.
    ///
    /// The frame-wide noise estimate is a high-pass residual, and fine detail
    /// *is* high-pass energy: measured on two frames of one scene differing
    /// only in focus, it scored the sharp frame 0.082 and the blurred one
    /// 0.012, so it argued *against* sharpness inside every burst. Gating the
    /// residual on pixels where the smoothed image has no structure asks the
    /// question the estimator was always meant to answer — how much the
    /// pixels wobble where nothing is going on — and leaves texture out of it.
    /// The gradient is taken on the smoothed image rather than the raw one so
    /// that the noise being measured cannot talk its own pixels out of the
    /// mask.
    pub noise_flat_gradient_threshold: f64,
    /// Smallest share of the interior that must be flat before the flat-region
    /// estimate is trusted. Below this (a frame that is texture edge to edge)
    /// `cull_noise_flat` falls back to the frame-wide value so the field is
    /// always present and never manufactured from a handful of pixels.
    pub noise_flat_min_fraction: f64,
    /// Normaliser for `cull_noise_flat`. Lower than `noise_denominator`
    /// because a residual measured only where the image is smooth is a
    /// smaller number than one averaged over edges as well.
    pub noise_flat_denominator: f64,
}

impl ImageMetricsConfig {
    /// `const` so `lrg_analysis`'s `BASE` culling config can stay a `const`.
    pub const fn defaults() -> Self {
        ImageMetricsConfig {
            sharpness_denominator: 0.015,
            highlight_threshold: 0.98,
            shadow_threshold: 0.02,
            highlight_clip_weight: 2.5,
            shadow_clip_weight: 2.0,
            exposure_target: 0.5,
            exposure_tolerance: 0.35,
            exposure_balance_weight: 0.75,
            exposure_clip_weight: 0.25,
            noise_denominator: 0.08,
            technical_weight_sharpness: 0.5,
            technical_weight_exposure: 0.35,
            technical_weight_noise: 0.15,
            aesthetic_contrast_weight: 0.45,
            aesthetic_colorfulness_weight: 0.35,
            aesthetic_exposure_weight: 0.20,
            sharpness_tile_grid: 8,
            sharpness_tile_focus_fraction: 0.5,
            // ~5 levels of 255 per pixel on the smoothed image: a gentle
            // tonal ramp passes, any real edge or texture does not.
            noise_flat_gradient_threshold: 0.02,
            noise_flat_min_fraction: 0.02,
            noise_flat_denominator: 0.04,
        }
    }
}

impl Default for ImageMetricsConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Parameters for `lrg_ml::face_quality` and `lrg_analysis::face_aggregate`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceMetricsConfig {
    pub face_sharpness_denominator: f64,
    pub eye_patch_ratio: f64,
    pub eye_patch_radius_min: i64,
    pub eye_patch_radius_max: i64,
    pub eye_openness_denominator: f64,
    pub prominence_normalizer: f64,
    pub visibility_det_weight: f64,
    pub visibility_center_weight: f64,
    pub score_weight_sharpness: f64,
    pub score_weight_prominence: f64,
    pub score_weight_visibility: f64,
    pub score_weight_eye_openness: f64,
    pub score_weight_occlusion: f64,
    /// Weight of the landmark-geometry term in `cull_occlusion`.
    ///
    /// The three fields this replaces — `occlusion_det_weight`,
    /// `occlusion_center_weight`, `occlusion_eye_weight` — combined
    /// `1 - (0.55·det_score + 0.20·center_proximity + 0.25·eye_openness)`, which
    /// never looked at the image at all. It was near-collinear with detector
    /// confidence, and `center_proximity` measures *composition*: a face near
    /// the edge of the frame is not occluded. `reject_occlusion_threshold` was
    /// therefore a second confidence gate wearing a different name.
    pub occlusion_geometry_weight: f64,
    /// Weight of the mirror-asymmetry term in `cull_occlusion`.
    pub occlusion_symmetry_weight: f64,
    /// RMS landmark-fit residual, as a fraction of the aligned crop's width,
    /// that counts as a fully implausible face geometry.
    ///
    /// 0.06 is ~6.7px of RMS error on the 112px aligned crop: comfortably above
    /// the couple of pixels a clean frontal detection produces, and reached by
    /// a face with a feature covered or turned well off-axis.
    pub occlusion_geometry_normalizer: f64,
    /// Which faces count as "prominent" when rolling per-face eye, blink and
    /// occlusion scores up to the photo, as a fraction of the largest face's
    /// area. A face below this is treated as background and cannot veto the
    /// frame.
    ///
    /// Photo-level eyes-open is the *worst* prominent face, not the best. A
    /// ten-person group shot where nine people blink and one has their eyes
    /// open is a reject, and taking the max — which is what this used to do —
    /// scored it as perfect. Without the size gate, though, any half-visible
    /// bystander at the edge of the frame would sink every frame equally and
    /// the signal would stop discriminating.
    pub prominent_face_area_fraction: f64,
    /// Half the side of the square measured around each eye for `eye_focus`,
    /// as a fraction of the distance between the eyes.
    ///
    /// An adult eye opening is about half the interocular distance wide, so
    /// 0.25 spans the opening and the lashes above and below it without
    /// reaching the brow — the detail a photographer zooms in on to check
    /// focus.
    pub eye_focus_patch_ratio: f64,
    /// Smallest interocular distance, in pixels of the image measured, at
    /// which eye focus is judged at all. Below it the eye is reported as not
    /// measurable rather than guessed at.
    ///
    /// 40 px makes a 21 px patch. Calibrated on `testdata/astronaut.jpg`: a
    /// 23 px eye patch separates sharp from a sigma-2 blur by 0.22, a 21 px
    /// one still by 0.19, but a 13 px patch only by 0.06 — the re-blur window
    /// then covers the whole eye and the measure saturates.
    pub eye_focus_min_iod_px: f64,
    /// Smallest face box side, in pixels of the image measured, for
    /// `face_focus`.
    pub face_focus_min_side_px: f64,
    /// Longest side an eye patch is measured at; a larger one is
    /// box-downsampled by a whole factor first. Bounds the cost of a tight
    /// headshot from a high-resolution body without changing which frame of a
    /// burst is sharper, and keeps the reading of a very large eye from being
    /// dominated by how soft the sensor is at 100 % — 128 px is an eye 256 px
    /// from the other, far beyond the size at which a missed focus shows.
    pub eye_focus_max_patch_side: usize,
    /// Longest side a face is measured at for `face_focus`; see
    /// [`Self::eye_focus_max_patch_side`].
    pub face_focus_max_side: usize,
    /// Length of the box filter the focus measure re-blurs with
    /// (`skimage.measure.blur_effect`'s `h_size`).
    pub focus_blur_window: usize,
    /// Smallest face box side, in pixels of the image analysed, that the face
    /// state model (`lrg_ml::face_state`) is run on. The MediaPipe model card
    /// warns that a face too small to rescale to its 256 px input loses
    /// accuracy; at 64 px the region it sees is upsampled about 2.7x.
    pub face_state_min_face_px: f64,
    /// Below this face-presence probability the mesh is not looking at a face,
    /// and the eye state is unknown (MediaPipe's own default threshold).
    pub face_state_min_presence: f64,
    /// Beyond this head turn, in degrees, the eye state is unknown. The model
    /// card puts the model's limit at 80°; landmarks degrade well before that.
    pub face_state_max_yaw_degrees: f64,
    /// From this head turn on, only the near eye is read: the far one is
    /// foreshortened or hidden, and reads as closed when it is not.
    pub face_state_near_eye_yaw_degrees: f64,
    /// Smallest interocular distance, in pixels of the image analysed, at
    /// which the eye state is judged.
    pub face_state_min_eye_px: f64,
    /// `eyeBlink` of the more closed eye at or above this is closed eyes. On
    /// real photographs, judged by hand, eyes shut read 0.54–0.78 and no open
    /// eye reached 0.35; a slit narrowed against the sun overlaps the shut
    /// range, and no coefficient separates the two. **A hypothesis until a
    /// hand-culled fixture says otherwise.**
    pub face_state_closed_threshold: f64,
    /// `eyeBlink` at or above this, but below the closed threshold, is half
    /// closed: a squint, a mid-blink, a sleepy look.
    pub face_state_half_threshold: f64,
    /// How many degrees of turn inside the head one unit of `eyeLook*` is
    /// worth, sideways and up or down. Sideways from people turned away and
    /// looking back at the lens, where the eyes' reading cancels the head's
    /// turn at about this; up and down from the portraits' pitch against
    /// `eyeLookUp/Down`, the rougher of the two.
    pub face_state_gaze_across_degrees: f64,
    pub face_state_gaze_up_degrees: f64,
    /// `eyeSquint` rises on a broad smile as well, so it only counts as a
    /// squint on a face smiling less than this.
    pub face_state_squint_max_smile: f64,
    /// Likewise an open mouth is a laugh on a face smiling more than this, and
    /// only something to mention below it.
    pub face_state_mouth_max_smile: f64,
    /// Lids reading closed on a head lowered at least this far are a gaze going
    /// down, not a blink: a child looking at the candles reads like eyes shut.
    /// Of 34 faces with the eyes really shut, mostly people asleep, this takes 10
    /// for looking down, and the cost of that is a note where a reject would have
    /// been.
    pub face_state_lowered_pitch_degrees: f64,
}

impl FaceMetricsConfig {
    /// `const` so `lrg_analysis`'s `BASE` culling config can stay a `const`.
    pub const fn defaults() -> Self {
        FaceMetricsConfig {
            face_sharpness_denominator: 0.02,
            eye_patch_ratio: 0.08,
            eye_patch_radius_min: 2,
            eye_patch_radius_max: 8,
            eye_openness_denominator: 0.07,
            prominence_normalizer: 0.12,
            visibility_det_weight: 0.5,
            visibility_center_weight: 0.5,
            score_weight_sharpness: 0.35,
            score_weight_prominence: 0.25,
            score_weight_visibility: 0.20,
            score_weight_eye_openness: 0.20,
            score_weight_occlusion: 0.15,
            // Geometry leads: it is the term that actually distinguishes a
            // covered feature, while symmetry also fires on hard side light.
            occlusion_geometry_weight: 0.65,
            occlusion_symmetry_weight: 0.35,
            occlusion_geometry_normalizer: 0.06,
            prominent_face_area_fraction: 0.25,
            eye_focus_patch_ratio: 0.25,
            eye_focus_min_iod_px: 40.0,
            face_focus_min_side_px: 32.0,
            eye_focus_max_patch_side: 128,
            face_focus_max_side: 768,
            focus_blur_window: crate::focus::BLUR_WINDOW,
            face_state_min_face_px: 64.0,
            face_state_min_presence: 0.5,
            face_state_max_yaw_degrees: 60.0,
            face_state_near_eye_yaw_degrees: 35.0,
            face_state_min_eye_px: 24.0,
            face_state_closed_threshold: 0.55,
            face_state_half_threshold: 0.35,
            face_state_gaze_across_degrees: 28.0,
            face_state_gaze_up_degrees: 20.0,
            face_state_squint_max_smile: 0.3,
            face_state_mouth_max_smile: 0.4,
            face_state_lowered_pitch_degrees: -10.0,
        }
    }
}

impl Default for FaceMetricsConfig {
    fn default() -> Self {
        Self::defaults()
    }
}
