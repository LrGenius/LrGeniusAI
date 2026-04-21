import unittest
import sys
import os

# Add src to path
sys.path.append(os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "src")))

from edit_recipe import normalize_edit_recipe, filter_edit_recipe_by_controls


class NormalizeEditRecipeTests(unittest.TestCase):
    def test_clamps_global_values_and_discards_invalid_mask(self):
        recipe = normalize_edit_recipe(
            {
                "summary": "  Brighten portrait  ",
                "global": {
                    "exposure": 9,
                    "contrast": -120,
                    "temperature": "5600",
                    "white_balance": {
                        "temperature": 6200,
                        "tint": 13,
                    },
                    "crop": {
                        "left": -0.1,
                        "right": 1.2,
                        "top": 0.05,
                        "bottom": 0.95,
                        "angle": 60,
                    },
                    "vignette": -40,
                    "vignette_feather": 120,
                    "sharpen_radius": 4.0,
                    "noise_reduction_detail": 85,
                    "grain_size": 33,
                    "tone_curve": {
                        "highlights": 30,
                        "shadow_split": -5,
                        "midtone_split": 50,
                        "highlight_split": 150,
                        "point_curve": {
                            "master": [0, 0, 64, 56, 128, 136, 255, 255],
                            "red": [{"x": 0, "y": 0}, {"x": 255, "y": 250}],
                            "green": [[0, 3], [255, 255]],
                            "blue": [0, 0, 255],  # odd length should be trimmed/invalid
                        },
                        "extended_point_curve": {
                            "master": [0, 0, 84, 68, 130, 147, 183, 193, 448, 448],
                            "red": [{"x": 0, "y": 0}, {"x": 512, "y": 490}],
                        },
                    },
                },
                "masks": [
                    {
                        "kind": "subject",
                        "adjustments": {
                            "exposure": 2.3,
                            "clarity": -150,
                        },
                    },
                    {
                        "kind": "person",
                        "adjustments": {
                            "exposure": 1,
                        },
                    },
                ],
                "warnings": ["  keep skin natural  "],
            }
        )

        self.assertEqual(recipe["summary"], "Brighten portrait")
        self.assertEqual(recipe["global"]["exposure"], 5.0)
        self.assertEqual(recipe["global"]["contrast"], -100.0)
        self.assertEqual(recipe["global"]["temperature"], 5600.0)
        self.assertEqual(recipe["global"]["crop"]["left"], 0.0)
        self.assertEqual(recipe["global"]["crop"]["right"], 1.0)
        self.assertEqual(recipe["global"]["crop"]["top"], 0.05)
        self.assertEqual(recipe["global"]["crop"]["bottom"], 0.95)
        self.assertEqual(recipe["global"]["crop"]["angle"], 45.0)
        self.assertEqual(recipe["global"]["vignette"], -40.0)
        self.assertEqual(recipe["global"]["vignette_feather"], 100.0)
        self.assertEqual(recipe["global"]["sharpen_radius"], 3.0)
        self.assertEqual(recipe["global"]["noise_reduction_detail"], 85.0)
        self.assertEqual(recipe["global"]["grain_size"], 33.0)
        self.assertEqual(recipe["global"]["tone_curve"]["highlights"], 30.0)
        self.assertEqual(recipe["global"]["tone_curve"]["shadow_split"], 0.0)
        self.assertEqual(recipe["global"]["tone_curve"]["midtone_split"], 50.0)
        self.assertEqual(recipe["global"]["tone_curve"]["highlight_split"], 100.0)
        self.assertEqual(
            recipe["global"]["tone_curve"]["point_curve"]["master"],
            [0, 0, 64, 56, 128, 136, 255, 255],
        )
        self.assertEqual(
            recipe["global"]["tone_curve"]["point_curve"]["red"], [0, 0, 255, 250]
        )
        self.assertEqual(
            recipe["global"]["tone_curve"]["point_curve"]["green"], [0, 3, 255, 255]
        )
        self.assertNotIn("blue", recipe["global"]["tone_curve"]["point_curve"])
        self.assertEqual(
            recipe["global"]["tone_curve"]["extended_point_curve"]["master"],
            [0, 0, 84, 68, 130, 147, 183, 193, 448, 448],
        )
        self.assertEqual(
            recipe["global"]["tone_curve"]["extended_point_curve"]["red"],
            [0, 0, 512, 490],
        )
        # Top-level keys take precedence over white_balance object.
        self.assertEqual(recipe["global"]["temperature"], 5600.0)
        self.assertNotIn("white_balance", recipe["global"])
        self.assertEqual(len(recipe["masks"]), 1)
        self.assertEqual(recipe["masks"][0]["kind"], "subject")
        self.assertEqual(recipe["masks"][0]["adjustments"]["clarity"], -100.0)
        self.assertIn("keep skin natural", recipe["warnings"])
        self.assertTrue(
            any("unsupported kind" in warning for warning in recipe["warnings"])
        )

    def test_handles_invalid_payload(self):
        recipe = normalize_edit_recipe("invalid")
        self.assertEqual(recipe["global"], {})
        self.assertEqual(recipe["masks"], [])
        self.assertTrue(recipe["warnings"])

    def test_white_balance_object_maps_to_temperature_and_tint(self):
        recipe = normalize_edit_recipe(
            {
                "summary": "WB tweak",
                "global": {
                    "white_balance": {
                        "temperature": 70000,
                        "tint": -200,
                    },
                },
                "masks": [],
                "warnings": [],
            }
        )
        self.assertEqual(recipe["global"]["temperature"], 50000.0)
        self.assertEqual(recipe["global"]["tint"], -150.0)

    def test_filter_recipe_by_category_controls(self):
        recipe = normalize_edit_recipe(
            {
                "summary": "Category control test",
                "global": {
                    "exposure": 0.5,
                    "temperature": 5600,
                    "clarity": 20,
                    "vibrance": 15,
                    "color_grading": {"shadows": {"hue": 220, "saturation": 20}},
                    "tone_curve": {
                        "highlights": 25,
                        "point_curve": {"master": [0, 0, 128, 140, 255, 255]},
                    },
                    "crop": {"left": 0.02, "right": 0.98, "top": 0.03, "bottom": 0.97},
                    "sharpening": 40,
                    "vignette": -25,
                    "lens_corrections": {"enable_profile_corrections": True},
                },
                "masks": [
                    {
                        "kind": "subject",
                        "adjustments": {
                            "exposure": 0.3,
                            "clarity": 10,
                            "sharpness": 25,
                        },
                    },
                ],
                "warnings": [],
            }
        )

        filtered = filter_edit_recipe_by_controls(
            recipe,
            {
                "include_masks": True,
                "adjust_white_balance": False,
                "adjust_basic_tone": True,
                "adjust_presence": False,
                "adjust_color_mix": False,
                "do_color_grading": False,
                "use_tone_curve": True,
                "use_point_curve": False,
                "adjust_detail": False,
                "adjust_effects": False,
                "adjust_lens_corrections": False,
                "allow_auto_crop": True,
                "composition_mode": "none",
            },
        )

        self.assertIn("exposure", filtered["global"])
        self.assertNotIn("temperature", filtered["global"])
        self.assertNotIn("clarity", filtered["global"])
        self.assertNotIn("vibrance", filtered["global"])
        self.assertNotIn("color_grading", filtered["global"])
        self.assertNotIn("sharpening", filtered["global"])
        self.assertNotIn("vignette", filtered["global"])
        self.assertNotIn("lens_corrections", filtered["global"])
        self.assertNotIn("crop", filtered["global"])
        self.assertIn("tone_curve", filtered["global"])
        self.assertNotIn("point_curve", filtered["global"]["tone_curve"])

        self.assertEqual(len(filtered["masks"]), 1)
        self.assertIn("exposure", filtered["masks"][0]["adjustments"])
        self.assertNotIn("clarity", filtered["masks"][0]["adjustments"])
        self.assertNotIn("sharpness", filtered["masks"][0]["adjustments"])

    def test_crop_xywh_shape_is_normalized(self):
        recipe = normalize_edit_recipe(
            {
                "summary": "xywh crop",
                "global": {
                    "crop": {
                        "x": 0.1,
                        "y": 0.2,
                        "width": 0.7,
                        "height": 0.6,
                        "rotation": 12,
                    },
                },
                "masks": [],
                "warnings": [],
            }
        )

        self.assertIn("crop", recipe["global"])
        self.assertEqual(recipe["global"]["crop"]["left"], 0.1)
        self.assertEqual(recipe["global"]["crop"]["top"], 0.2)
        self.assertEqual(recipe["global"]["crop"]["right"], 0.8)
        self.assertEqual(recipe["global"]["crop"]["bottom"], 0.8)
        self.assertEqual(recipe["global"]["crop"]["angle"], 12.0)

    def test_invalid_crop_shape_adds_warning(self):
        recipe = normalize_edit_recipe(
            {
                "summary": "bad crop",
                "global": {
                    "crop": {
                        "x": 0.3,
                        "width": 0.4,
                    },
                },
                "masks": [],
                "warnings": [],
            }
        )

        self.assertNotIn("crop", recipe["global"])
        self.assertTrue(
            any("Ignored crop" in warning for warning in recipe["warnings"])
        )


if __name__ == "__main__":
    unittest.main()
