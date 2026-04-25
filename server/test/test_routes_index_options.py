import unittest

from routes_index import _extract_options


class ExtractOptionsTests(unittest.TestCase):
    def test_all_defaults(self):
        opts = _extract_options({})
        self.assertEqual(opts["language"], "German")
        self.assertEqual(opts["temperature"], 0.2)
        self.assertTrue(opts["generate_keywords"])
        self.assertTrue(opts["generate_caption"])
        self.assertTrue(opts["generate_title"])
        self.assertTrue(opts["generate_alt_text"])
        self.assertFalse(opts["submit_keywords"])
        self.assertFalse(opts["submit_folder_names"])
        self.assertEqual(opts["existing_keywords"], None)
        self.assertEqual(opts["keyword_categories"], [])
        self.assertEqual(opts["style_strength"], 0.5)

    def test_keyword_categories_as_dict_json_string(self):
        opts = _extract_options({"keyword_categories": '{"People": ["Family"]}'})
        self.assertEqual(opts["keyword_categories"], {"People": ["Family"]})

    def test_keyword_categories_as_list_json_string(self):
        opts = _extract_options({"keyword_categories": '["A", "B"]'})
        self.assertEqual(opts["keyword_categories"], ["A", "B"])

    def test_keyword_categories_malformed_json_falls_back_to_empty_list(self):
        opts = _extract_options({"keyword_categories": "{not valid json"})
        self.assertEqual(opts["keyword_categories"], [])

    def test_keyword_categories_passthrough_when_already_parsed(self):
        # JSON requests deliver a parsed object directly (no string parsing)
        opts = _extract_options({"keyword_categories": {"X": ["Y"]}})
        self.assertEqual(opts["keyword_categories"], {"X": ["Y"]})

    def test_style_strength_clamped_below_zero(self):
        self.assertEqual(
            _extract_options({"style_strength": "-1.5"})["style_strength"], 0.0
        )

    def test_style_strength_clamped_above_one(self):
        self.assertEqual(
            _extract_options({"style_strength": "9.9"})["style_strength"], 1.0
        )

    def test_style_strength_in_range_preserved(self):
        self.assertEqual(
            _extract_options({"style_strength": "0.7"})["style_strength"], 0.7
        )

    def test_style_strength_invalid_string_defaults_to_half(self):
        self.assertEqual(
            _extract_options({"style_strength": "not a number"})["style_strength"], 0.5
        )

    def test_style_strength_none_defaults_to_half(self):
        self.assertEqual(
            _extract_options({"style_strength": None})["style_strength"], 0.5
        )

    def test_boolean_coercion_accepts_true_false_strings(self):
        opts = _extract_options(
            {
                "generate_keywords": "false",
                "generate_caption": "FALSE",
                "submit_keywords": "True",
                "submit_folder_names": "TRUE",
            }
        )
        self.assertFalse(opts["generate_keywords"])
        self.assertFalse(opts["generate_caption"])
        self.assertTrue(opts["submit_keywords"])
        self.assertTrue(opts["submit_folder_names"])

    def test_boolean_coercion_unrecognized_string_is_false(self):
        # Anything that isn't lowercased "true" coerces to False
        opts = _extract_options({"generate_keywords": "yes"})
        self.assertFalse(opts["generate_keywords"])

    def test_existing_keywords_raw_csv_string_dropped_to_none(self):
        # Pin current behavior: a raw CSV string is fed through json.loads
        # first; since "Alpha,Beta" isn't valid JSON, it falls back to None
        # and existing_keywords becomes None. Likely a latent bug — the
        # in-source comment claims CSVs are normalized to a list.
        opts = _extract_options({"existing_keywords": "Alpha,Beta,Gamma"})
        self.assertIsNone(opts["existing_keywords"])

    def test_existing_keywords_json_quoted_csv_string_split(self):
        # When the client JSON-encodes a CSV string, the inner string is
        # split on commas into a list.
        opts = _extract_options({"existing_keywords": '"  Alpha , Beta,  ,Gamma "'})
        self.assertEqual(opts["existing_keywords"], ["Alpha", "Beta", "Gamma"])

    def test_existing_keywords_list_input(self):
        opts = _extract_options({"existing_keywords": ["Alpha", "  Beta  ", ""]})
        self.assertEqual(opts["existing_keywords"], ["Alpha", "Beta"])

    def test_existing_keywords_json_string_list(self):
        # _parse_json_field will parse the JSON string into a list
        opts = _extract_options({"existing_keywords": '["Alpha", "Beta"]'})
        self.assertEqual(opts["existing_keywords"], ["Alpha", "Beta"])

    def test_existing_keywords_missing_yields_none(self):
        self.assertIsNone(_extract_options({})["existing_keywords"])

    def test_regenerate_metadata_camelcase_supported(self):
        opts = _extract_options({"regenerateMetadata": "false"})
        self.assertFalse(opts["regenerate_metadata"])

    def test_regenerate_metadata_snake_case_takes_precedence(self):
        opts = _extract_options(
            {"regenerate_metadata": "false", "regenerateMetadata": "true"}
        )
        self.assertFalse(opts["regenerate_metadata"])

    def test_regenerate_metadata_default_true(self):
        self.assertTrue(_extract_options({})["regenerate_metadata"])

    def test_keyword_secondary_language_empty_becomes_none(self):
        # Empty string `or None` evaluates to None
        self.assertIsNone(
            _extract_options({"keyword_secondary_language": ""})[
                "keyword_secondary_language"
            ]
        )

    def test_temperature_float_coerced(self):
        opts = _extract_options({"temperature": "0.7"})
        self.assertEqual(opts["temperature"], 0.7)


if __name__ == "__main__":
    unittest.main()
