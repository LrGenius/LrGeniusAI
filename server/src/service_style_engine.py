"""
Photographer Style Engine — LLM-free AI Edit backend.

Produces a Lightroom edit recipe that reproduces the photographer's personal
editing style by:

  1. Retrieving the top-N visually similar training examples via CLIP.
  2. Re-scoring candidates with a multi-criteria composite score that adds:
       * Exposure proximity  (luminance/contrast match)
       * Scene-type overlap  (genre match)
       * Time-of-day proximity  (light-quality cue)
  3. Interpolating the develop settings of the top-K winners weighted by
     their composite score.
  4. Applying a small RAW-adaptive compensation layer that adjusts the
     interpolated recipe to account for exposure differences between the
     training examples and the new photo.
  5. Computing a confidence score so the plugin can show quality feedback.

When confidence is below the threshold and LLM fallback is enabled, the
caller should fall back to the usual LLM path with training examples as
few-shot context.
"""

from __future__ import annotations

import json
from typing import Any, Dict, List, Optional, Tuple

from config import logger
import service_training as training_service

# ---------------------------------------------------------------------------
# Tunable weights (can be made user-configurable later via config / prefs)
# ---------------------------------------------------------------------------

WEIGHT_CLIP = 0.50  # CLIP visual similarity
WEIGHT_EXPOSURE = 0.25  # Exposure state proximity
WEIGHT_SCENE = 0.15  # Scene-type tag overlap
WEIGHT_TIME_OF_DAY = 0.10  # Time-of-day / lighting cue

# Confidence thresholds
CONFIDENCE_GOOD = 0.70  # ≥ this → style engine output direct, no warning
CONFIDENCE_LOW = 0.45  # < this → LLM fallback recommended

# Interpolation: number of top examples to blend
TOP_K_BLEND = 3

# Candidate pool: how many CLIP-similar examples to fetch before re-scoring
CANDIDATE_POOL = 20

# Minimum training examples required for style engine to activate
MIN_TRAINING_EXAMPLES = 5

# ---------------------------------------------------------------------------
# Distance → similarity conversion
# ---------------------------------------------------------------------------


def _clip_distance_to_similarity(distance: float) -> float:
    """Convert ChromaDB L2 distance (squared) to cosine similarity proxy.

    ChromaDB uses squared L2 distance for normalized vectors.
    For unit vectors: ||a - b||² = 2 - 2·cos(θ)  →  cos(θ) = 1 - d/2
    """
    return max(0.0, min(1.0, 1.0 - distance / 2.0))


# ---------------------------------------------------------------------------
# Exposure proximity scoring
# ---------------------------------------------------------------------------


def _exposure_proximity(
    query: Dict[str, float],
    candidate: Dict[str, float],
) -> float:
    """Score how closely the candidate's exposure state matches the query.

    Compares luminance mean and contrast.  Returns 0..1 where 1 = identical.
    """
    deltas: List[float] = []

    for key in ("exp_luminance_mean", "exp_contrast", "exp_warmth_proxy"):
        q_val = query.get(key)
        c_val = candidate.get(key)
        if q_val is not None and c_val is not None:
            deltas.append(abs(float(q_val) - float(c_val)))

    if not deltas:
        return 0.5  # neutral when no data available

    # Average absolute delta, scaled so delta=0.3 → score ≈ 0
    mean_delta = sum(deltas) / len(deltas)
    return max(0.0, 1.0 - mean_delta / 0.3)


# ---------------------------------------------------------------------------
# Scene-type tag overlap scoring
# ---------------------------------------------------------------------------


def _scene_overlap(
    query_tags: List[str],
    candidate_tags: List[str],
) -> float:
    """Jaccard-style overlap between two sets of scene tags.

    Returns 0..1.  When both sets are empty, returns 0.5 (neutral).
    """
    q_set = set(query_tags or [])
    c_set = set(candidate_tags or [])
    if not q_set and not c_set:
        return 0.5
    if not q_set or not c_set:
        return 0.3  # one side has no tags — mild penalise
    intersection = q_set & c_set
    union = q_set | c_set
    return len(intersection) / len(union)


# ---------------------------------------------------------------------------
# Time-of-day proximity scoring
# ---------------------------------------------------------------------------

_TOD_ORDER = ["dawn", "morning", "afternoon", "evening", "night", "unknown"]


def _tod_proximity(query_tod: str, candidate_tod: str) -> float:
    """Score proximity of time-of-day buckets.  Adjacent buckets score 0.5, same = 1.0."""
    if query_tod == "unknown" or candidate_tod == "unknown":
        return 0.5
    if query_tod == candidate_tod:
        return 1.0
    try:
        q_idx = _TOD_ORDER.index(query_tod)
        c_idx = _TOD_ORDER.index(candidate_tod)
        diff = abs(q_idx - c_idx)
        # Circular distance over 5 slots (dawn → night wrap)
        diff = min(diff, 5 - diff)
        return max(0.0, 1.0 - diff * 0.35)
    except ValueError:
        return 0.5


# ---------------------------------------------------------------------------
# Composite scoring
# ---------------------------------------------------------------------------


def calculate_composite_score(
    clip_sim: float,
    query_exposure: Dict[str, float],
    candidate: Dict[str, Any],
    query_scene_tags: List[str],
    query_tod: str,
) -> float:
    """Compute weighted composite match score for one candidate."""
    exp_score = _exposure_proximity(
        query_exposure,
        {
            k: candidate.get(k, 0.0)
            for k in (
                "exp_luminance_mean",
                "exp_contrast",
                "exp_warmth_proxy",
            )
        },
    )
    scene_score = _scene_overlap(query_scene_tags, candidate.get("scene_tags", []))
    tod_score = _tod_proximity(
        query_tod, candidate.get("time_of_day_bucket", "unknown")
    )

    return (
        WEIGHT_CLIP * clip_sim
        + WEIGHT_EXPOSURE * exp_score
        + WEIGHT_SCENE * scene_score
        + WEIGHT_TIME_OF_DAY * tod_score
    )


# ---------------------------------------------------------------------------
# Recipe interpolation
# ---------------------------------------------------------------------------


def interpolate_recipes(
    winners: List[Tuple[Dict[str, Any], float]],
) -> Dict[str, Any]:
    """Weighted blend of canonical develop settings from the top-K winners.

    Args:
        winners: List of (example_dict, composite_score) pairs.

    Returns:
        Interpolated canonical recipe dict.
    """
    total_weight = sum(score for _, score in winners)
    if total_weight <= 0:
        return {}

    blended: Dict[str, float] = {}
    for example, score in winners:
        weight = score / total_weight
        canonical = example.get("canonical_settings", {})
        if not isinstance(canonical, dict):
            try:
                canonical = json.loads(canonical)
            except Exception:
                canonical = {}
        for key, value in canonical.items():
            if isinstance(value, (int, float)):
                blended[key] = blended.get(key, 0.0) + weight * float(value)

    # Round to 1 decimal place – LR sliders don't need more precision
    return {k: round(v, 1) for k, v in blended.items()}


# ---------------------------------------------------------------------------
# RAW-adaptive exposure compensation
# ---------------------------------------------------------------------------


def adaptive_compensation(
    recipe: Dict[str, Any],
    query_exposure: Dict[str, float],
    winners: List[Tuple[Dict[str, Any], float]],
) -> Dict[str, Any]:
    """Adjust the interpolated recipe to compensate for exposure differences.

    Example: if the new photo is 0.2 EV brighter than the training examples,
    reduce exposure to reach an equivalent tonal foundation.
    """
    if not winners:
        return recipe

    # Weighted average training luminance
    total_weight = sum(score for _, score in winners)
    if total_weight <= 0:
        return recipe

    avg_train_lum = sum(
        ex.get("exp_luminance_mean", 0.5) * (score / total_weight)
        for ex, score in winners
    )
    avg_train_contrast = sum(
        ex.get("exp_contrast", 0.5) * (score / total_weight) for ex, score in winners
    )

    query_lum = query_exposure.get("exp_luminance_mean", 0.5)
    query_contrast = query_exposure.get("exp_contrast", 0.5)

    # Luminance delta → small exposure correction
    lum_delta = query_lum - avg_train_lum
    # Scale: 0.1 luminance unit ≈ 0.5 EV
    exposure_correction = (
        -lum_delta * 5.0
    )  # subtract because brighter photo needs less exposure push
    exposure_correction = max(-1.5, min(1.5, exposure_correction))

    # Contrast delta → small contrast correction
    contrast_delta = query_contrast - avg_train_contrast
    contrast_correction = -contrast_delta * 20.0
    contrast_correction = max(-15.0, min(15.0, contrast_correction))

    # Apply corrections additively on top of interpolated recipe
    result = dict(recipe)
    if abs(exposure_correction) > 0.05:
        current_exp = result.get("exposure", 0.0)
        result["exposure"] = round(current_exp + exposure_correction, 2)
        logger.debug(
            "Style engine adaptive: lum_delta=%.3f → exposure correction %+.2f",
            lum_delta,
            exposure_correction,
        )
    if abs(contrast_correction) > 1.0:
        current_con = result.get("contrast", 0.0)
        result["contrast"] = round(current_con + contrast_correction, 1)
        logger.debug(
            "Style engine adaptive: contrast_delta=%.3f → contrast correction %+.1f",
            contrast_delta,
            contrast_correction,
        )

    return result


# ---------------------------------------------------------------------------
# Canonical recipe → LLM-compatible edit recipe dict
# ---------------------------------------------------------------------------

# Reverse mapping from canonical key → edit_recipe global fields
_CANONICAL_TO_RECIPE_FIELDS = {
    "exposure": "exposure",
    "contrast": "contrast",
    "highlights": "highlights",
    "shadows": "shadows",
    "whites": "whites",
    "blacks": "blacks",
    "temperature": "temperature",
    "tint": "tint",
    "texture": "texture",
    "clarity": "clarity",
    "dehaze": "dehaze",
    "vibrance": "vibrance",
    "saturation": "saturation",
    "sharpening": "sharpening",
    "noise_reduction": "noise_reduction",
    "color_noise_reduction": "color_noise_reduction",
    "vignette": "vignette",
    "grain": "grain",
}

# Parametric tone curve keys extracted from training
_TONE_CURVE_KEYS = {
    "tone_curve_highlights": "highlights",
    "tone_curve_lights": "lights",
    "tone_curve_darks": "darks",
    "tone_curve_shadows": "shadows",
}


def _canonical_to_edit_recipe(
    canonical: Dict[str, Any], summary: str = ""
) -> Dict[str, Any]:
    """Convert canonical key/value dict to the edit recipe format used by the plugin."""
    global_settings: Dict[str, Any] = {}

    for canon_key, recipe_key in _CANONICAL_TO_RECIPE_FIELDS.items():
        if canon_key in canonical:
            global_settings[recipe_key] = canonical[canon_key]

    # Build parametric tone_curve if any tone-curve keys are present
    tone_curve: Dict[str, Any] = {}
    for canon_key, tc_key in _TONE_CURVE_KEYS.items():
        if canon_key in canonical:
            tone_curve[tc_key] = canonical[canon_key]
    if tone_curve:
        global_settings["tone_curve"] = tone_curve

    return {
        "summary": summary or "Style-matched edit by LrGeniusAI Style Engine",
        "global": global_settings,
        "masks": [],
        "warnings": [],
    }


# ---------------------------------------------------------------------------
# Main entry point
# ---------------------------------------------------------------------------


class StyleEngineResult:
    """Result from the style engine."""

    def __init__(
        self,
        recipe: Dict[str, Any],
        confidence: float,
        matched_count: int,
        engine: str = "style",
        warning: Optional[str] = None,
        matched_filenames: Optional[List[str]] = None,
    ) -> None:
        self.recipe = recipe
        self.confidence = confidence
        self.matched_count = matched_count
        self.engine = engine
        self.warning = warning
        self.matched_filenames = matched_filenames or []


def generate_style_edit(
    photo_id: str,
    image_bytes: bytes,
    *,
    focal_length: Optional[float] = None,
    capture_time_unix: Optional[float] = None,
    clip_embedding: Optional[List[float]] = None,
    min_confidence: float = CONFIDENCE_LOW,
) -> StyleEngineResult:
    """Generate a style-matched edit recipe without an LLM.

    Args:
        photo_id:           Stable photo identifier.
        image_bytes:        JPEG/PNG preview for exposure metric extraction.
        focal_length:       Focal length in mm from EXIF (optional).
        capture_time_unix:  Capture Unix timestamp (optional).
        clip_embedding:     Pre-computed CLIP embedding (re-used from index if available).
        min_confidence:     Below this confidence the caller should fall back to LLM.

    Returns:
        StyleEngineResult with recipe, confidence score, and metadata.
    """
    training_count = training_service.get_training_count()
    if training_count < MIN_TRAINING_EXAMPLES:
        return StyleEngineResult(
            recipe={},
            confidence=0.0,
            matched_count=0,
            engine="none",
            warning=(
                f"Style engine inactive: only {training_count} training example(s) available "
                f"(minimum {MIN_TRAINING_EXAMPLES} required). Please save more AI training examples."
            ),
        )

    # -----------------------------------------------------------------------
    # Step 1: Compute query-side features
    # -----------------------------------------------------------------------
    query_exposure = training_service.compute_exposure_metrics(image_bytes)
    query_scene_tags = training_service.compute_scene_tags(clip_embedding)
    query_tod = training_service.time_of_day_bucket(capture_time_unix)
    focal_bucket = training_service.focal_length_bucket(focal_length)

    logger.info(
        "Style engine query: photo_id=%s lum=%.3f contrast=%.3f tags=%s tod=%s focal=%s",
        photo_id,
        query_exposure.get("exp_luminance_mean", -1),
        query_exposure.get("exp_contrast", -1),
        query_scene_tags,
        query_tod,
        focal_bucket,
    )

    # -----------------------------------------------------------------------
    # Step 2: Candidate retrieval via CLIP similarity
    # -----------------------------------------------------------------------
    if clip_embedding is not None:
        candidates = training_service.query_similar_training_examples(
            clip_embedding,
            n_results=min(CANDIDATE_POOL, training_count),
        )
    else:
        # No embedding available – fetch recent examples as fallback
        all_examples = training_service.list_training_examples()
        candidates = all_examples[:CANDIDATE_POOL]
        for c in candidates:
            c["distance"] = 0.5  # neutral distance when embedding unavailable

    if not candidates:
        return StyleEngineResult(
            recipe={},
            confidence=0.0,
            matched_count=0,
            engine="none",
            warning="No training examples could be retrieved from the database.",
        )

    # -----------------------------------------------------------------------
    # Step 3: Re-score candidates with composite criteria
    # -----------------------------------------------------------------------
    scored: List[Tuple[Dict[str, Any], float]] = []
    for candidate in candidates:
        clip_sim = _clip_distance_to_similarity(candidate.get("distance", 1.0))
        score = calculate_composite_score(
            clip_sim=clip_sim,
            query_exposure=query_exposure,
            candidate=candidate,
            query_scene_tags=query_scene_tags,
            query_tod=query_tod,
        )
        scored.append((candidate, score))

    # Sort descending by composite score
    scored.sort(key=lambda x: x[1], reverse=True)

    # -----------------------------------------------------------------------
    # Step 4: Compute confidence from best candidate scores
    # -----------------------------------------------------------------------
    # top_scores = [s for _, s in scored[:TOP_K_BLEND]]

    best_score = scored[0][1] if scored else 0.0
    confidence = round(best_score, 3)

    # -----------------------------------------------------------------------
    # Step 5: Interpolate the top-K winners
    # -----------------------------------------------------------------------
    winners = scored[:TOP_K_BLEND]
    matched_filenames = [
        ex.get("filename") or ex.get("label") or ex.get("photo_id", "")
        for ex, _ in winners
    ]

    blended = interpolate_recipes(winners)

    # -----------------------------------------------------------------------
    # Step 6: RAW-adaptive compensation
    # -----------------------------------------------------------------------
    blended = adaptive_compensation(blended, query_exposure, winners)

    # -----------------------------------------------------------------------
    # Step 7: Build summary from top example labels
    # -----------------------------------------------------------------------
    labels = list(
        set(
            ex.get("label") or ex.get("summary") or ""
            for ex, _ in winners
            if (ex.get("label") or ex.get("summary"))
        )
    )
    summary_parts = []
    if labels:
        summary_parts.append("Style: " + " / ".join(labels[:2]))
    summary_parts.append(
        f"Matched {len(winners)} of {training_count} examples (confidence {confidence:.0%})"
    )
    summary = " — ".join(summary_parts)

    recipe = _canonical_to_edit_recipe(blended, summary=summary)

    # -----------------------------------------------------------------------
    # Step 8: Attach appropriate warning for low confidence
    # -----------------------------------------------------------------------
    warning: Optional[str] = None
    if confidence < CONFIDENCE_LOW:
        warning = (
            f"Low style match confidence ({confidence:.0%}). "
            "Results may not match your editing style precisely. "
            "Consider adding more training examples for this type of photo."
        )
    elif confidence < CONFIDENCE_GOOD:
        warning = (
            f"Moderate style match confidence ({confidence:.0%}). "
            "Review the result before applying."
        )

    logger.info(
        "Style engine result: photo_id=%s confidence=%.3f matched=%d winners=%s",
        photo_id,
        confidence,
        len(winners),
        [f.get("filename", "?") for f, _ in winners],
    )

    return StyleEngineResult(
        recipe=recipe,
        confidence=confidence,
        matched_count=len(winners),
        engine="style",
        warning=warning,
        matched_filenames=matched_filenames,
    )
