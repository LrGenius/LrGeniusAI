import unittest

from llm_provider_base import LLMProviderBase


class _StubProvider(LLMProviderBase):
    """Concrete subclass so we can exercise base-class helper methods."""

    def generate_metadata(self, request):
        raise NotImplementedError

    def is_available(self):
        return True

    def generate_edit_recipe(self, request):
        raise NotImplementedError

    def list_available_models(self):
        return []


def _provider():
    return _StubProvider({})


class NormalizeKeywordLeafTests(unittest.TestCase):
    def test_bare_string_returned_stripped(self):
        self.assertEqual(
            _provider()._normalize_keyword_leaf("  Mountain  "), "Mountain"
        )

    def test_empty_or_whitespace_string_returns_none(self):
        self.assertIsNone(_provider()._normalize_keyword_leaf(""))
        self.assertIsNone(_provider()._normalize_keyword_leaf("   "))

    def test_dict_with_name_only(self):
        self.assertEqual(
            _provider()._normalize_keyword_leaf({"name": "Mountain"}),
            {"name": "Mountain"},
        )

    def test_dict_strips_name_whitespace(self):
        self.assertEqual(
            _provider()._normalize_keyword_leaf({"name": "  Mountain  "}),
            {"name": "Mountain"},
        )

    def test_dict_with_synonyms_dedupes_against_name(self):
        result = _provider()._normalize_keyword_leaf(
            {"name": "Mountain", "synonyms": ["mountain", "Peak"]}
        )
        self.assertEqual(result, {"name": "Mountain", "synonyms": ["Peak"]})

    def test_dict_synonyms_dedupe_each_other_case_insensitive(self):
        result = _provider()._normalize_keyword_leaf(
            {"name": "Mountain", "synonyms": ["Peak", "PEAK", "peak", "Summit"]}
        )
        self.assertEqual(result, {"name": "Mountain", "synonyms": ["Peak", "Summit"]})

    def test_dict_synonyms_non_list_dropped(self):
        # synonyms field omitted entirely when not a list
        self.assertEqual(
            _provider()._normalize_keyword_leaf(
                {"name": "Mountain", "synonyms": "Peak"}
            ),
            {"name": "Mountain"},
        )

    def test_dict_with_only_empty_synonyms_omits_field(self):
        self.assertEqual(
            _provider()._normalize_keyword_leaf(
                {"name": "Mountain", "synonyms": ["", "  "]}
            ),
            {"name": "Mountain"},
        )

    def test_dict_missing_name_returns_none(self):
        self.assertIsNone(_provider()._normalize_keyword_leaf({"synonyms": ["X"]}))

    def test_dict_with_non_string_name_returns_none(self):
        self.assertIsNone(_provider()._normalize_keyword_leaf({"name": None}))
        self.assertIsNone(_provider()._normalize_keyword_leaf({"name": 42}))

    def test_dict_with_empty_name_returns_none(self):
        self.assertIsNone(_provider()._normalize_keyword_leaf({"name": "   "}))

    def test_unsupported_types_return_none(self):
        self.assertIsNone(_provider()._normalize_keyword_leaf(42))
        self.assertIsNone(_provider()._normalize_keyword_leaf(None))
        self.assertIsNone(_provider()._normalize_keyword_leaf(["not", "a", "leaf"]))


class NormalizeKeywordsStructureTests(unittest.TestCase):
    def test_flat_list_of_strings(self):
        result = _provider()._normalize_keywords_structure(["Alpha", "Beta"])
        self.assertEqual(result, ["Alpha", "Beta"])

    def test_list_drops_empty_and_invalid_leaves(self):
        result = _provider()._normalize_keywords_structure(
            ["Alpha", "", "  ", None, 42, {"name": "Beta"}]
        )
        self.assertEqual(result, ["Alpha", {"name": "Beta"}])

    def test_list_with_structured_objects(self):
        result = _provider()._normalize_keywords_structure(
            [{"name": "Alpha", "synonyms": ["A1"]}, {"name": "Beta"}]
        )
        self.assertEqual(
            result,
            [{"name": "Alpha", "synonyms": ["A1"]}, {"name": "Beta"}],
        )

    def test_dict_with_name_treated_as_leaf(self):
        result = _provider()._normalize_keywords_structure(
            {"name": "Mountain", "synonyms": ["Peak"]}
        )
        self.assertEqual(result, {"name": "Mountain", "synonyms": ["Peak"]})

    def test_nested_dict_recurses_and_preserves_keys(self):
        result = _provider()._normalize_keywords_structure(
            {
                "Nature": ["Mountain", "Lake"],
                "People": [{"name": "Family"}],
            }
        )
        self.assertEqual(
            result,
            {
                "Nature": ["Mountain", "Lake"],
                "People": [{"name": "Family"}],
            },
        )

    def test_nested_dict_drops_empty_branches(self):
        result = _provider()._normalize_keywords_structure(
            {
                "Nature": ["Mountain"],
                "Empty": [],
                "AlsoEmpty": [None, "", "  "],
                "Nope": {},
            }
        )
        self.assertEqual(result, {"Nature": ["Mountain"]})

    def test_deeply_nested_structure(self):
        result = _provider()._normalize_keywords_structure(
            {"Top": {"Mid": {"Leaf": ["Found"]}}}
        )
        self.assertEqual(result, {"Top": {"Mid": {"Leaf": ["Found"]}}})

    def test_list_of_lists_recurses(self):
        result = _provider()._normalize_keywords_structure(
            [["Alpha", "Beta"], ["Gamma"]]
        )
        self.assertEqual(result, [["Alpha", "Beta"], ["Gamma"]])

    def test_empty_inputs(self):
        self.assertEqual(_provider()._normalize_keywords_structure([]), [])
        self.assertEqual(_provider()._normalize_keywords_structure({}), {})

    def test_bare_string_normalized_as_leaf(self):
        self.assertEqual(
            _provider()._normalize_keywords_structure("  Mountain  "), "Mountain"
        )

    def test_unsupported_scalar_returns_none(self):
        self.assertIsNone(_provider()._normalize_keywords_structure(42))


if __name__ == "__main__":
    unittest.main()
