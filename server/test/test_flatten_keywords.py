import unittest

from service_index import _flatten_keywords


class FlattenKeywordsTests(unittest.TestCase):
    def test_empty_inputs_return_empty_string(self):
        self.assertEqual(_flatten_keywords(None), "")
        self.assertEqual(_flatten_keywords(""), "")
        self.assertEqual(_flatten_keywords([]), "")
        self.assertEqual(_flatten_keywords({}), "")

    def test_string_input_returned_as_is(self):
        self.assertEqual(_flatten_keywords("Keyword1, Keyword2"), "Keyword1, Keyword2")

    def test_flat_list_of_strings(self):
        self.assertEqual(
            _flatten_keywords(["Alpha", "Beta", "Gamma"]), "Alpha, Beta, Gamma"
        )

    def test_list_strips_whitespace_and_drops_empty(self):
        self.assertEqual(
            _flatten_keywords(["  Alpha  ", "", "   ", "Beta"]), "Alpha, Beta"
        )

    def test_list_dedupes_case_insensitively(self):
        self.assertEqual(
            _flatten_keywords(["Alpha", "alpha", "ALPHA", "Beta"]), "Alpha, Beta"
        )

    def test_structured_keyword_objects(self):
        keywords = [
            {"name": "Mountain", "synonyms": ["Peak", "Summit"]},
            {"name": "Lake"},
        ]
        result = _flatten_keywords(keywords)
        self.assertEqual(result, "Mountain, Peak, Summit, Lake")

    def test_synonyms_dedupe_against_name_case_insensitive(self):
        keywords = [{"name": "Mountain", "synonyms": ["mountain", "Peak"]}]
        self.assertEqual(_flatten_keywords(keywords), "Mountain, Peak")

    def test_nested_dict_recurses(self):
        keywords = {
            "Nature": {
                "Landscape": ["Mountain", "Lake"],
                "Wildlife": ["Bear"],
            },
            "People": ["Family"],
        }
        result = _flatten_keywords(keywords)
        # Order follows dict insertion order (Python 3.7+)
        self.assertEqual(result, "Mountain, Lake, Bear, Family")

    def test_nested_dict_with_structured_leaf_object(self):
        keywords = {
            "Nature": {"name": "Mountain", "synonyms": ["Peak"]},
        }
        self.assertEqual(_flatten_keywords(keywords), "Mountain, Peak")

    def test_nested_dict_with_scalar_value(self):
        keywords = {"Category": "DirectKeyword"}
        self.assertEqual(_flatten_keywords(keywords), "DirectKeyword")

    def test_dedup_across_categories(self):
        keywords = {
            "Nature": ["Mountain"],
            "Travel": ["Mountain", "Hiking"],
        }
        self.assertEqual(_flatten_keywords(keywords), "Mountain, Hiking")

    def test_malformed_dict_entries_skipped(self):
        keywords = [
            {"name": None},
            {"name": 42},
            {"name": "Valid"},
        ]
        self.assertEqual(_flatten_keywords(keywords), "Valid")

    def test_synonyms_without_name_still_emitted(self):
        # Pins current behavior: a dict missing `name` still contributes its
        # synonyms. Surprising, but downstream code expects this today.
        keywords = [{"synonyms": ["orphan"]}, {"name": "Valid"}]
        self.assertEqual(_flatten_keywords(keywords), "orphan, Valid")

    def test_synonyms_non_list_ignored(self):
        keywords = [{"name": "Mountain", "synonyms": "Peak"}]
        self.assertEqual(_flatten_keywords(keywords), "Mountain")

    def test_unknown_input_type_returns_empty_string(self):
        self.assertEqual(_flatten_keywords(42), "")
        self.assertEqual(_flatten_keywords(3.14), "")


if __name__ == "__main__":
    unittest.main()
