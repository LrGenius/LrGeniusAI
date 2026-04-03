import unittest

from src.edit_recipe import normalize_edit_recipe


class NormalizeEditRecipeTests(unittest.TestCase):
    def test_clamps_global_values_and_discards_invalid_mask(self):
        recipe = normalize_edit_recipe({
            "summary": "  Brighten portrait  ",
            "global": {
                "exposure": 9,
                "contrast": -120,
                "temperature": "5600",
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
                "lens_corrections": {
                    "enable_profile_corrections": True,
                    "remove_chromatic_aberration": False,
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
        })

        self.assertEqual(recipe["summary"], "Brighten portrait")
        self.assertEqual(recipe["global"]["exposure"], 5.0)
        self.assertEqual(recipe["global"]["contrast"], -100.0)
        self.assertEqual(recipe["global"]["temperature"], 5600.0)
        self.assertEqual(recipe["global"]["vignette"], -40.0)
        self.assertEqual(recipe["global"]["vignette_feather"], 100.0)
        self.assertEqual(recipe["global"]["sharpen_radius"], 3.0)
        self.assertEqual(recipe["global"]["noise_reduction_detail"], 85.0)
        self.assertEqual(recipe["global"]["grain_size"], 33.0)
        self.assertEqual(recipe["global"]["tone_curve"]["highlights"], 30.0)
        self.assertEqual(recipe["global"]["tone_curve"]["shadow_split"], 0.0)
        self.assertEqual(recipe["global"]["tone_curve"]["midtone_split"], 50.0)
        self.assertEqual(recipe["global"]["tone_curve"]["highlight_split"], 100.0)
        self.assertEqual(recipe["global"]["tone_curve"]["point_curve"]["master"], [0, 0, 64, 56, 128, 136, 255, 255])
        self.assertEqual(recipe["global"]["tone_curve"]["point_curve"]["red"], [0, 0, 255, 250])
        self.assertEqual(recipe["global"]["tone_curve"]["point_curve"]["green"], [0, 3, 255, 255])
        self.assertNotIn("blue", recipe["global"]["tone_curve"]["point_curve"])
        self.assertEqual(recipe["global"]["tone_curve"]["extended_point_curve"]["master"], [0, 0, 84, 68, 130, 147, 183, 193, 448, 448])
        self.assertEqual(recipe["global"]["tone_curve"]["extended_point_curve"]["red"], [0, 0, 512, 490])
        self.assertTrue(recipe["global"]["lens_corrections"]["enable_profile_corrections"])
        self.assertEqual(len(recipe["masks"]), 1)
        self.assertEqual(recipe["masks"][0]["kind"], "subject")
        self.assertEqual(recipe["masks"][0]["adjustments"]["clarity"], -100.0)
        self.assertIn("keep skin natural", recipe["warnings"])
        self.assertTrue(any("unsupported kind" in warning for warning in recipe["warnings"]))

    def test_handles_invalid_payload(self):
        recipe = normalize_edit_recipe("invalid")
        self.assertEqual(recipe["global"], {})
        self.assertEqual(recipe["masks"], [])
        self.assertTrue(recipe["warnings"])

if __name__ == "__main__":
    unittest.main()
