"""Tests for pure helper functions in services/search.py.

``services.search`` transitively imports ``server_lifecycle`` → ``open_clip``
→ real ``torch``.  We stub the entire chain at the ``sys.modules`` level
before importing the service module so no CUDA/ML libraries need to be loaded.
"""

import sys
from unittest.mock import MagicMock

# Stub every heavy transitive dep before services.search is imported.
for _mod in [
    "open_clip",
    "server_lifecycle",
    "huggingface_hub",
    "torch",
    "torch.nn",
    "torch.nn.functional",
    "torch.utils",
    "torch.utils.checkpoint",
    "services.vertexai",
]:
    sys.modules.setdefault(_mod, MagicMock())

from services.search import (  # noqa: E402  (import after sys.modules patch)
    DEFAULT_MAX_RESULTS,
    DEFAULT_RELEVANCE_STRICTNESS,
    _clamp_max_results,
    _clamp_strictness,
    _merge_semantic_results,
    _normalize_search_sources,
    _smart_filter_by_relevance,
    _strictness_to_k,
    _transform_and_sort_results,
    _transform_vertex_results,
)


# ---------------------------------------------------------------------------
# _clamp_max_results
# ---------------------------------------------------------------------------


class TestClampMaxResults:
    def test_none_returns_default(self):
        assert _clamp_max_results(None) == DEFAULT_MAX_RESULTS

    def test_string_int_parsed(self):
        assert _clamp_max_results("150") == 150

    def test_non_numeric_string_returns_default(self):
        assert _clamp_max_results("abc") == DEFAULT_MAX_RESULTS

    def test_below_min_clamped_to_min(self):
        assert _clamp_max_results(5) == 10

    def test_one_clamped_to_min(self):
        assert _clamp_max_results(1) == 10

    def test_negative_clamped_to_min(self):
        assert _clamp_max_results(-1) == 10

    def test_above_max_clamped_to_max(self):
        assert _clamp_max_results(3000) == 2000

    def test_exact_min_accepted(self):
        assert _clamp_max_results(10) == 10

    def test_exact_max_accepted(self):
        assert _clamp_max_results(2000) == 2000

    def test_midrange_value_unchanged(self):
        assert _clamp_max_results(500) == 500


# ---------------------------------------------------------------------------
# _clamp_strictness
# ---------------------------------------------------------------------------


class TestClampStrictness:
    def test_none_returns_default(self):
        assert _clamp_strictness(None) == DEFAULT_RELEVANCE_STRICTNESS

    def test_negative_clamped_to_zero(self):
        assert _clamp_strictness(-5) == 0

    def test_above_100_clamped_to_100(self):
        assert _clamp_strictness(150) == 100

    def test_string_int_parsed(self):
        assert _clamp_strictness("75") == 75

    def test_bad_string_returns_default(self):
        assert _clamp_strictness("bad") == DEFAULT_RELEVANCE_STRICTNESS

    def test_zero_accepted(self):
        assert _clamp_strictness(0) == 0

    def test_100_accepted(self):
        assert _clamp_strictness(100) == 100

    def test_midrange_value_unchanged(self):
        assert _clamp_strictness(42) == 42


# ---------------------------------------------------------------------------
# _strictness_to_k
# ---------------------------------------------------------------------------


class TestStrictnessToK:
    def test_zero_returns_zero(self):
        assert _strictness_to_k(0) == 0.0

    def test_25_anchor(self):
        assert _strictness_to_k(25) == 0.5

    def test_50_anchor(self):
        assert _strictness_to_k(50) == 1.0

    def test_75_anchor(self):
        assert _strictness_to_k(75) == 1.5

    def test_100_anchor(self):
        assert _strictness_to_k(100) == 2.0

    def test_below_zero_clamped(self):
        # _strictness_to_k clamps internally via max/min.
        assert _strictness_to_k(-10) == 0.0

    def test_above_100_clamped(self):
        assert _strictness_to_k(200) == 2.0


# ---------------------------------------------------------------------------
# _merge_semantic_results
# ---------------------------------------------------------------------------


class TestMergeSemanticResults:
    def test_empty_both_returns_empty(self):
        assert _merge_semantic_results([], []) == []

    def test_vertex_only_returned_first(self):
        vertex = [{"photo_id": "v1", "distance": 0.1}]
        result = _merge_semantic_results([], vertex)
        assert result[0]["photo_id"] == "v1"

    def test_siglip_only_ids_appended(self):
        siglip = [{"photo_id": "s1", "distance": 0.2}]
        result = _merge_semantic_results(siglip, [])
        assert result[0]["photo_id"] == "s1"

    def test_vertex_ids_take_priority_over_siglip(self):
        siglip = [
            {"photo_id": "shared", "distance": 0.1},
            {"photo_id": "s_only", "distance": 0.3},
        ]
        vertex = [{"photo_id": "shared", "distance": 0.05}]
        result = _merge_semantic_results(siglip, vertex)
        # "shared" should appear only once (vertex version).
        ids = [r["photo_id"] for r in result]
        assert ids.count("shared") == 1
        assert ids[0] == "shared"

    def test_siglip_only_added_after_vertex(self):
        siglip = [
            {"photo_id": "v_id", "distance": 0.1},
            {"photo_id": "s_only", "distance": 0.2},
        ]
        vertex = [{"photo_id": "v_id", "distance": 0.05}]
        result = _merge_semantic_results(siglip, vertex)
        ids = [r["photo_id"] for r in result]
        assert ids == ["v_id", "s_only"]

    def test_no_cross_model_distance_comparison(self):
        """Vertex results always precede siglip-only results regardless of distance."""
        siglip = [{"photo_id": "s1", "distance": 0.01}]
        vertex = [{"photo_id": "v1", "distance": 0.99}]
        result = _merge_semantic_results(siglip, vertex)
        assert result[0]["photo_id"] == "v1"
        assert result[1]["photo_id"] == "s1"


# ---------------------------------------------------------------------------
# _transform_and_sort_results
# ---------------------------------------------------------------------------


class TestTransformAndSortResults:
    def _make_chroma(self, ids, distances, metadatas=None):
        if metadatas is None:
            metadatas = [{} for _ in ids]
        return {"ids": [ids], "distances": [distances], "metadatas": [metadatas]}

    def test_empty_ids_returns_empty(self):
        assert (
            _transform_and_sort_results(
                {"ids": [[]], "distances": [[]], "metadatas": [[]]}, False
            )
            == []
        )

    def test_none_results_returns_empty(self):
        assert _transform_and_sort_results(None, False) == []

    def test_has_embedding_false_skipped(self):
        data = self._make_chroma(
            ["p1", "p2"],
            [0.1, 0.2],
            [{"has_embedding": False}, {"has_embedding": True}],
        )
        result = _transform_and_sort_results(data, False)
        ids = [r["photo_id"] for r in result]
        assert "p1" not in ids
        assert "p2" in ids

    def test_sorted_by_distance_ascending(self):
        data = self._make_chroma(["p1", "p2", "p3"], [0.5, 0.1, 0.3])
        result = _transform_and_sort_results(data, False)
        distances = [r["distance"] for r in result]
        assert distances == sorted(distances)

    def test_photo_id_and_uuid_both_set(self):
        data = self._make_chroma(["abc"], [0.2])
        result = _transform_and_sort_results(data, False)
        assert result[0]["photo_id"] == "abc"
        assert result[0]["uuid"] == "abc"

    def test_distance_rounded_to_4_places(self):
        data = self._make_chroma(["p1"], [0.123456789])
        result = _transform_and_sort_results(data, False)
        assert result[0]["distance"] == 0.1235


# ---------------------------------------------------------------------------
# _transform_vertex_results
# ---------------------------------------------------------------------------


class TestTransformVertexResults:
    def test_empty_returns_empty(self):
        assert _transform_vertex_results({}) == []

    def test_none_returns_empty(self):
        assert _transform_vertex_results(None) == []

    def test_empty_ids_list_returns_empty(self):
        assert _transform_vertex_results({"ids": [[]], "distances": [[]]}) == []

    def test_normal_result_sorted_by_distance(self):
        data = {"ids": [["v2", "v1"]], "distances": [[0.8, 0.2]]}
        result = _transform_vertex_results(data)
        assert result[0]["photo_id"] == "v1"
        assert result[1]["photo_id"] == "v2"

    def test_photo_id_and_uuid_set(self):
        data = {"ids": [["x"]], "distances": [[0.5]]}
        result = _transform_vertex_results(data)
        assert result[0]["photo_id"] == "x"
        assert result[0]["uuid"] == "x"


# ---------------------------------------------------------------------------
# _normalize_search_sources
# ---------------------------------------------------------------------------


class TestNormalizeSearchSources:
    def test_none_returns_all_defaults(self):
        result = _normalize_search_sources(None)
        assert result["semantic_siglip"] is True
        assert result["semantic_vertex"] is True
        assert result["metadata"] is True
        assert "flattened_keywords" in result["metadata_fields"]

    def test_empty_dict_returns_defaults(self):
        result = _normalize_search_sources({})
        assert result["semantic_siglip"] is True

    def test_partial_dict_fills_in_defaults(self):
        result = _normalize_search_sources({"semantic_siglip": False})
        assert result["semantic_siglip"] is False
        assert result["semantic_vertex"] is True

    def test_metadata_fields_filters_to_allowed(self):
        result = _normalize_search_sources(
            {"metadata_fields": ["flattened_keywords", "bogus_field"]}
        )
        assert "flattened_keywords" in result["metadata_fields"]
        assert "bogus_field" not in result["metadata_fields"]

    def test_empty_metadata_fields_reverts_to_defaults(self):
        result = _normalize_search_sources({"metadata_fields": []})
        assert len(result["metadata_fields"]) > 0

    def test_all_invalid_metadata_fields_reverts_to_defaults(self):
        result = _normalize_search_sources({"metadata_fields": ["bad1", "bad2"]})
        assert len(result["metadata_fields"]) > 0

    def test_none_metadata_fields_uses_defaults(self):
        result = _normalize_search_sources({"metadata_fields": None})
        assert "flattened_keywords" in result["metadata_fields"]

    def test_known_metadata_fields_all_accepted(self):
        allowed = ["flattened_keywords", "alt_text", "caption", "title"]
        result = _normalize_search_sources({"metadata_fields": allowed})
        assert set(result["metadata_fields"]) == set(allowed)


# ---------------------------------------------------------------------------
# _smart_filter_by_relevance
# ---------------------------------------------------------------------------


def _make_results(distances):
    return [{"photo_id": f"p{i}", "distance": d} for i, d in enumerate(distances)]


class TestSmartFilterByRelevance:
    def test_empty_returns_empty(self):
        assert _smart_filter_by_relevance([], 50) == []

    def test_strictness_zero_returns_unchanged(self):
        results = _make_results([0.1, 0.5, 1.0, 1.5])
        assert _smart_filter_by_relevance(results, 0) == results

    def test_small_list_at_or_below_floor_passes_through(self):
        # RELEVANCE_FLOOR == 8; list of 6 should be returned untouched.
        results = _make_results([float(i) for i in range(6)])
        out = _smart_filter_by_relevance(results, 100)
        assert out == results

    def test_exact_floor_size_passes_through(self):
        results = _make_results([float(i) for i in range(8)])
        out = _smart_filter_by_relevance(results, 100)
        assert out == results

    def test_bimodal_distribution_keeps_relevant_cluster(self):
        # 12 clearly-relevant items followed by 30 noisy ones with a big gap.
        relevant = [0.20 + 0.005 * i for i in range(12)]
        noise = [1.50 + 0.01 * i for i in range(30)]
        results = _make_results(relevant + noise)
        out = _smart_filter_by_relevance(results, 50)
        assert all(r["distance"] < 1.0 for r in out)

    def test_floor_enforced_when_cluster_too_small(self):
        # 3 relevant + 35 noise: filter pads result to RELEVANCE_FLOOR.
        relevant = [0.1, 0.11, 0.12]
        noise = [2.0 + 0.01 * i for i in range(35)]
        results = _make_results(relevant + noise)
        out = _smart_filter_by_relevance(results, 50)
        assert len(out) >= 8

    def test_smooth_distribution_never_returns_below_floor(self):
        results = _make_results([0.5 + 0.005 * i for i in range(60)])
        out = _smart_filter_by_relevance(results, 90)
        assert len(out) >= 8

    def test_higher_strictness_keeps_same_or_fewer(self):
        relevant = [0.30 + 0.005 * i for i in range(10)]
        noise = [1.20 + 0.01 * i for i in range(40)]
        results = _make_results(relevant + noise)
        kept_low = len(_smart_filter_by_relevance(results, 25))
        kept_high = len(_smart_filter_by_relevance(results, 100))
        assert kept_low >= kept_high
