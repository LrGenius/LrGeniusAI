import sys
import os
import unittest
from unittest.mock import patch

# Add src to path
sys.path.append(os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "src")))

from service_style_engine import (
    calculate_composite_score,
    interpolate_recipes,
    generate_style_edit,
)


class TestStyleEngineLogic(unittest.TestCase):
    def test_composite_score(self):
        print("\nTesting composite score...")
        query_exposure = {"exp_luminance_mean": 0.5, "exp_contrast": 0.5}
        query_scene_tags = ["scene_outdoor"]
        query_tod = "afternoon"

        # ex1 matches everything
        ex1 = {
            "exp_luminance_mean": 0.5,
            "exp_contrast": 0.5,
            "scene_tags": ["scene_outdoor"],
            "time_of_day_bucket": "afternoon",
        }

        # ex2 matches nothing well
        ex2 = {
            "exp_luminance_mean": 0.1,
            "exp_contrast": 0.2,
            "scene_tags": ["scene_dark"],
            "time_of_day_bucket": "night",
        }

        score1 = calculate_composite_score(
            1.0, query_exposure, ex1, query_scene_tags, query_tod
        )
        score2 = calculate_composite_score(
            0.0, query_exposure, ex2, query_scene_tags, query_tod
        )

        print(f"  Example 1 Score: {score1}")
        print(f"  Example 2 Score: {score2}")

        self.assertGreater(score1, score2)

    def test_interpolation(self):
        print("Testing recipe interpolation...")
        ex1 = {"canonical_settings": {"exposure": 1.0, "contrast": 0}}
        ex2 = {"canonical_settings": {"exposure": 0.0, "contrast": 40}}

        weighted = [(ex1, 0.75), (ex2, 0.25)]

        interp = interpolate_recipes(weighted)

        # 1.0 * 0.75 + 0.0 * 0.25 = 0.75
        self.assertEqual(interp.get("exposure"), 0.75)
        # 0 * 0.75 + 40 * 0.25 = 10
        self.assertEqual(interp.get("contrast"), 10)

    @patch("service_style_engine.training_service")
    def test_adaptive_compensation(self, mock_training):
        print("Testing adaptive RAW compensation...")
        # Training was high key (0.8), Target is low key (0.3).

        mock_training.get_training_count.return_value = 10
        mock_training.compute_exposure_metrics.return_value = {
            "exp_luminance_mean": 0.3,  # Darker target
            "exp_contrast": 0.5,
        }
        mock_training.compute_scene_tags.return_value = []
        mock_training.time_of_day_bucket.return_value = "unknown"
        mock_training.focal_length_bucket.return_value = "unknown"

        # ex1 was a bright photo (0.8) and we did nothing to it (exposure 0.0)
        mock_ex = {
            "photo_id": "ex1",
            "filename": "ex1.jpg",
            "exp_luminance_mean": 0.8,
            "exp_contrast": 0.5,
            "canonical_settings": {"exposure": 0.0},
            "distance": 0.05,
            "scene_tags": [],
            "time_of_day_bucket": "unknown",
        }
        mock_training.query_similar_training_examples.return_value = [mock_ex]

        result = generate_style_edit("target1", b"fake", clip_embedding=[0.1] * 512)

        final_exp = result.recipe["global"].get("exposure")
        print(f"  Final Exposure: {final_exp}")

        # Target (0.3) is darker than training (0.8).
        # lum_delta = 0.3 - 0.8 = -0.5
        # exposure_correction = -(-0.5) * 5.0 = 2.5 (clamped to 1.5)
        self.assertGreater(final_exp, 0)
        self.assertEqual(final_exp, 1.5)


if __name__ == "__main__":
    unittest.main()
