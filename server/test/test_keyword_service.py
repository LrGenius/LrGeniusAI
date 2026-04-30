"""Unit tests for services/keywords.py.

These tests cover _replace_in_keyword_structure, validate_clusters_with_llm
(with a stubbed LLM), and apply_keyword_merges (with a mocked Chroma collection).
No real CLIP model or database is required.
"""

import json
import unittest
from unittest.mock import MagicMock, patch

from services.keywords import (
    _replace_in_keyword_structure,
    apply_keyword_merges,
    validate_clusters_with_llm,
)


# ─── _replace_in_keyword_structure ───────────────────────────────────────────


class ReplaceInKeywordStructureTests(unittest.TestCase):
    def _merge_map(self, pairs):
        return {dup.lower(): can for dup, can in pairs}

    def test_string_replaced(self):
        result, changed = _replace_in_keyword_structure("Car", {"car": "Automobile"})
        self.assertEqual(result, "Automobile")
        self.assertTrue(changed)

    def test_string_not_in_map_unchanged(self):
        result, changed = _replace_in_keyword_structure("Dog", {"car": "Automobile"})
        self.assertEqual(result, "Dog")
        self.assertFalse(changed)

    def test_list_replaces_members(self):
        result, changed = _replace_in_keyword_structure(
            ["Car", "Bike", "Automobile"], {"car": "Automobile"}
        )
        self.assertIn("Automobile", result)
        self.assertNotIn("Car", result)
        self.assertTrue(changed)

    def test_list_deduplicates_after_merge(self):
        result, changed = _replace_in_keyword_structure(
            ["Car", "Automobile"], {"car": "Automobile"}
        )
        self.assertEqual(result.count("Automobile"), 1)
        self.assertTrue(changed)

    def test_empty_list_unchanged(self):
        result, changed = _replace_in_keyword_structure([], {})
        self.assertEqual(result, [])
        self.assertFalse(changed)

    def test_dict_values_replaced(self):
        result, changed = _replace_in_keyword_structure(
            {"k": "Car"}, {"car": "Automobile"}
        )
        self.assertEqual(result, {"k": "Automobile"})
        self.assertTrue(changed)

    def test_nested_structure(self):
        struct = ["Car", {"tags": ["Car", "Bike"]}]
        result, changed = _replace_in_keyword_structure(struct, {"car": "Automobile"})
        self.assertTrue(changed)
        self.assertIn("Automobile", result)
        self.assertNotIn("Car", result[1]["tags"])

    def test_non_string_passthrough(self):
        result, changed = _replace_in_keyword_structure(42, {"car": "Automobile"})
        self.assertEqual(result, 42)
        self.assertFalse(changed)

    def test_empty_merge_map_no_change(self):
        result, changed = _replace_in_keyword_structure(["Cat", "Dog"], {})
        self.assertEqual(result, ["Cat", "Dog"])
        self.assertFalse(changed)

    def test_case_insensitive_match(self):
        result, changed = _replace_in_keyword_structure("CAR", {"car": "Automobile"})
        self.assertEqual(result, "Automobile")
        self.assertTrue(changed)


# ─── validate_clusters_with_llm ──────────────────────────────────────────────


def _make_llm_response(clusters):
    """Serialize clusters as a raw JSON array string (LLM-like response)."""
    return json.dumps(clusters)


class ValidateClustersWithLLMTests(unittest.TestCase):
    def _call(self, candidates, llm_return):
        with patch(
            "services.keywords._call_llm_text", return_value=llm_return
        ) as mock_llm:
            result = validate_clusters_with_llm(
                candidates, "chatgpt", None, None, None, None
            )
            return result, mock_llm

    def test_empty_input_returns_empty(self):
        result, _ = self._call([], None)
        self.assertEqual(result, [])

    def test_valid_merge_returned(self):
        candidates = [["Car", "Automobile"]]
        llm_resp = _make_llm_response([["Automobile", "Car"]])
        result, _ = self._call(candidates, llm_resp)
        self.assertEqual(len(result), 1)
        self.assertIn("Automobile", result[0])

    def test_no_merge_empty_list_filtered(self):
        candidates = [["Cat", "Kitten"]]
        llm_resp = _make_llm_response([[]])  # LLM says: don't merge
        result, _ = self._call(candidates, llm_resp)
        self.assertEqual(result, [])

    def test_singleton_cluster_filtered(self):
        candidates = [["Car", "Automobile"]]
        llm_resp = _make_llm_response([["Automobile"]])  # only 1 item
        result, _ = self._call(candidates, llm_resp)
        self.assertEqual(result, [])

    def test_llm_failure_falls_back_to_clip_candidates(self):
        candidates = [["Car", "Automobile"], ["Dog", "Hound"]]
        result, _ = self._call(candidates, None)  # LLM returns None
        self.assertEqual(len(result), 2)

    def test_malformed_json_falls_back_to_clip_candidates(self):
        candidates = [["Car", "Automobile"]]
        result, _ = self._call(candidates, "not valid JSON [[[")
        self.assertEqual(len(result), 1)

    def test_markdown_fenced_json_parsed(self):
        candidates = [["Car", "Automobile"]]
        llm_resp = "```json\n" + _make_llm_response([["Automobile", "Car"]]) + "\n```"
        result, _ = self._call(candidates, llm_resp)
        self.assertEqual(len(result), 1)

    def test_mismatched_count_padded_with_empty(self):
        # LLM returns fewer elements than candidates — should pad, not crash
        candidates = [["Car", "Automobile"], ["Dog", "Hound"]]
        llm_resp = _make_llm_response([["Automobile", "Car"]])  # only 1 for 2 groups
        result, _ = self._call(candidates, llm_resp)
        self.assertEqual(len(result), 1)

    def test_all_identical_input(self):
        candidates = [["Bike", "Bicycle", "Cycle"]]
        llm_resp = _make_llm_response([["Bike", "Bicycle", "Cycle"]])
        result, _ = self._call(candidates, llm_resp)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0][0], "Bike")


# ─── apply_keyword_merges ────────────────────────────────────────────────────


def _make_meta(flat, kw_struct):
    return {
        "flattened_keywords": flat,
        "keywords": json.dumps(kw_struct) if kw_struct is not None else "",
    }


class ApplyKeywordMergesTests(unittest.TestCase):
    def _run(self, merges, metas):
        ids = [str(i) for i in range(len(metas))]
        mock_col = MagicMock()
        mock_col.get.return_value = {"ids": ids, "metadatas": metas}
        mock_col.update = MagicMock()

        # apply_keyword_merges does a deferred `from services import chroma`,
        # so we patch the module itself, not a keyword-module attribute.
        with patch("services.chroma") as mock_chroma:
            mock_chroma.collection = mock_col
            mock_chroma.STATS_GET_LIMIT = 50000
            result = apply_keyword_merges(merges)

        return result, mock_col

    def test_empty_merges_returns_zero(self):
        result, col = self._run([], [])
        self.assertEqual(result["updated_photos"], 0)
        col.update.assert_not_called()

    def test_no_collection_returns_zero(self):
        with patch("services.chroma") as mc:
            mc.collection = None
            result = apply_keyword_merges(
                [{"duplicate": "Car", "canonical": "Automobile"}]
            )
        self.assertEqual(result["updated_photos"], 0)

    def test_flattened_keywords_replaced(self):
        merges = [{"duplicate": "Car", "canonical": "Automobile"}]
        metas = [_make_meta("Car, Bike, Car", None)]
        result, col = self._run(merges, metas)
        self.assertEqual(result["updated_photos"], 1)
        updated_meta = col.update.call_args[1]["metadatas"][0]
        self.assertIn("Automobile", updated_meta["flattened_keywords"])
        self.assertNotIn("Car", updated_meta["flattened_keywords"])

    def test_flattened_keywords_deduplicated_after_merge(self):
        merges = [{"duplicate": "Car", "canonical": "Automobile"}]
        metas = [_make_meta("Car, Automobile", None)]
        result, col = self._run(merges, metas)
        self.assertEqual(result["updated_photos"], 1)
        flat = col.update.call_args[1]["metadatas"][0]["flattened_keywords"]
        self.assertEqual(flat.count("Automobile"), 1)

    def test_json_keywords_replaced(self):
        merges = [{"duplicate": "Car", "canonical": "Automobile"}]
        metas = [_make_meta("", ["Car", "Bike"])]
        result, col = self._run(merges, metas)
        self.assertEqual(result["updated_photos"], 1)
        updated_kw = json.loads(col.update.call_args[1]["metadatas"][0]["keywords"])
        self.assertIn("Automobile", updated_kw)
        self.assertNotIn("Car", updated_kw)

    def test_photo_with_no_match_not_updated(self):
        merges = [{"duplicate": "Car", "canonical": "Automobile"}]
        metas = [_make_meta("Dog, Cat", ["Dog", "Cat"])]
        result, _ = self._run(merges, metas)
        self.assertEqual(result["updated_photos"], 0)

    def test_case_insensitive_merge(self):
        merges = [{"duplicate": "car", "canonical": "Automobile"}]
        metas = [_make_meta("Car", None)]
        result, col = self._run(merges, metas)
        self.assertEqual(result["updated_photos"], 1)

    def test_duplicate_equals_canonical_ignored(self):
        merges = [{"duplicate": "Car", "canonical": "Car"}]
        metas = [_make_meta("Car", None)]
        result, _ = self._run(merges, metas)
        self.assertEqual(result["updated_photos"], 0)

    def test_invalid_json_keywords_skipped_gracefully(self):
        merges = [{"duplicate": "Car", "canonical": "Automobile"}]
        metas = [{"flattened_keywords": "Car", "keywords": "{not: valid}"}]
        result, col = self._run(merges, metas)
        # flattened keywords still updated, json keywords left unchanged
        self.assertEqual(result["updated_photos"], 1)

    def test_multiple_merges_applied(self):
        merges = [
            {"duplicate": "Car", "canonical": "Automobile"},
            {"duplicate": "Dog", "canonical": "Canine"},
        ]
        metas = [_make_meta("Car, Dog, Bike", None)]
        result, col = self._run(merges, metas)
        flat = col.update.call_args[1]["metadatas"][0]["flattened_keywords"]
        self.assertIn("Automobile", flat)
        self.assertIn("Canine", flat)
        self.assertNotIn("Car", flat)
        self.assertNotIn("Dog", flat)


if __name__ == "__main__":
    unittest.main()
