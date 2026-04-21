import sys
import os
from unittest.mock import patch

# Add src to path
sys.path.append(os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "src")))

from service_style_engine import (
    calculate_composite_score,
    interpolate_recipes,
    generate_style_edit,
    StyleEngineResult,
)


def test_calculate_composite_score():
    query_exposure = {"exp_luminance_mean": 0.5, "exp_contrast": 0.5}
    candidate = {
        "exp_luminance_mean": 0.52,
        "exp_contrast": 0.48,
        "scene_tags": ["scene_outdoor", "scene_landscape"],
        "time_of_day_bucket": "afternoon",
    }
    query_scene_tags = ["scene_outdoor", "scene_landscape"]
    query_tod = "afternoon"
    clip_sim = 0.95

    score = calculate_composite_score(
        clip_sim=clip_sim,
        query_exposure=query_exposure,
        candidate=candidate,
        query_scene_tags=query_scene_tags,
        query_tod=query_tod,
    )

    assert score > 0.8

    # Test lower score with mismatch
    score_mismatch = calculate_composite_score(
        clip_sim=0.2,  # low sim
        query_exposure=query_exposure,
        candidate=candidate,
        query_scene_tags=["scene_indoor"],  # mismatch
        query_tod="night",  # mismatch
    )

    assert score > score_mismatch


def test_interpolate_recipes():
    # Example 1: Exposure +1
    ex1 = {"canonical_settings": {"exposure": 1.0, "contrast": 10}}
    # Example 2: Exposure -1
    ex2 = {"canonical_settings": {"exposure": -1.0, "contrast": 50}}

    winners = [(ex1, 0.5), (ex2, 0.5)]

    interpolated = interpolate_recipes(winners)

    # (1.0*0.5) + (-1.0*0.5) = 0
    assert interpolated.get("exposure") == 0.0
    # (10*0.5) + (50*0.5) = 30
    assert interpolated.get("contrast") == 30


@patch("service_style_engine.training_service")
def test_generate_style_edit_adaptive(mock_training):
    # If training photo was dark (0.2) and target is bright (0.8)
    # The engine should suggest LOWERING exposure relative to what was done to the dark photo

    # Mock training service stats
    mock_training.get_training_count.return_value = 20

    # Mock query_similar_training_examples
    # ex1 was a dark photo (0.2) and we gave it +0.5 exposure
    mock_ex = {
        "photo_id": "ex1",
        "filename": "ex1.jpg",
        "exp_luminance_mean": 0.2,
        "exp_contrast": 0.5,
        "canonical_settings": {"exposure": 0.5, "contrast": 20},
        "distance": 0.05,  # high sim
        "scene_tags": [],
        "time_of_day_bucket": "unknown",
    }
    mock_training.query_similar_training_examples.return_value = [mock_ex]

    # Mock compute_exposure_metrics (target is bright: 0.8)
    mock_training.compute_exposure_metrics.return_value = {
        "exp_luminance_mean": 0.8,
        "exp_contrast": 0.5,
    }
    mock_training.compute_scene_tags.return_value = []
    mock_training.time_of_day_bucket.return_value = "unknown"
    mock_training.focal_length_bucket.return_value = "unknown"

    # Run the engine
    result = generate_style_edit(
        photo_id="target1", image_bytes=b"fake", clip_embedding=[0.1] * 512
    )

    assert isinstance(result, StyleEngineResult)
    assert result.engine == "style"

    # target (0.8) - training (0.2) = 0.6 difference.
    # Current logic: exposure_correction = -lum_delta * 5.0
    # -0.6 * 5.0 = -3.0 (clamped to -1.5)
    # Base exposure was 0.5. Result should be 0.5 - 1.5 = -1.0

    final_exposure = result.recipe["global"].get("exposure")
    assert final_exposure < 0.5  # Should have been lowered
    assert final_exposure == -1.0  # 0.5 + (-1.5)


@patch("service_style_engine.training_service")
def test_generate_style_edit_not_enough_training(mock_training):
    mock_training.get_training_count.return_value = 2

    result = generate_style_edit(photo_id="target1", image_bytes=b"fake")

    assert result.engine == "none"
    assert "inactive" in result.warning
    assert "2" in result.warning
