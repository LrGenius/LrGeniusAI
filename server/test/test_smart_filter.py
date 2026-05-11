"""Tests for the adaptive relevance filter in services.search."""

from services.search import (
    _smart_filter_by_relevance,
    _strictness_to_k,
    _clamp_max_results,
    _clamp_strictness,
    RELEVANCE_FLOOR,
    DEFAULT_MAX_RESULTS,
    DEFAULT_RELEVANCE_STRICTNESS,
)


def _make_results(distances):
    return [{"photo_id": f"p{i}", "distance": d} for i, d in enumerate(distances)]


def test_strictness_zero_returns_unfiltered():
    results = _make_results([0.1, 0.2, 0.3, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6])
    out = _smart_filter_by_relevance(results, 0)
    assert out == results


def test_empty_input_returns_empty():
    assert _smart_filter_by_relevance([], 50) == []


def test_tiny_input_below_floor_passes_through():
    # Fewer than FLOOR entries should be returned untouched regardless of strictness.
    results = _make_results([0.1, 0.2, 0.3])
    out = _smart_filter_by_relevance(results, 100)
    assert out == results


def test_bimodal_distribution_keeps_only_relevant_cluster():
    # Many clearly-relevant matches, then more noise with a big gap between.
    # Enough relevant items so we don't bump into the FLOOR padding.
    relevant = [0.30 + 0.005 * i for i in range(15)]
    noise = [1.20 + 0.01 * i for i in range(35)]
    results = _make_results(relevant + noise)
    out = _smart_filter_by_relevance(results, DEFAULT_RELEVANCE_STRICTNESS)
    assert len(out) == len(relevant)
    for r in out:
        assert r["distance"] < 1.0


def test_bimodal_below_floor_pads_with_closest():
    # Only a few relevant items but a clear knee: filter should pad to FLOOR
    # with the next-closest items so no empty/very-small result lists ship.
    relevant = [0.30, 0.32, 0.34]
    noise = [1.20 + 0.01 * i for i in range(35)]
    results = _make_results(relevant + noise)
    out = _smart_filter_by_relevance(results, DEFAULT_RELEVANCE_STRICTNESS)
    assert len(out) == RELEVANCE_FLOOR
    # All three real matches are present.
    assert {"p0", "p1", "p2"}.issubset({r["photo_id"] for r in out})


def test_smooth_distribution_falls_back_to_floor():
    # No obvious knee, no clear bimodality: filter should still return >= FLOOR.
    distances = [0.50 + 0.005 * i for i in range(80)]
    results = _make_results(distances)
    out = _smart_filter_by_relevance(results, 90)
    assert len(out) >= RELEVANCE_FLOOR


def test_strictness_monotonic_in_kept_count():
    # As strictness rises, we should never keep MORE than at a looser setting
    # (allow equality because of the FLOOR clamp).
    relevant = [0.30, 0.32, 0.34, 0.36, 0.38, 0.40, 0.42, 0.44, 0.46, 0.48]
    noise = [1.10 + 0.01 * i for i in range(40)]
    results = _make_results(relevant + noise)
    kept_loose = len(_smart_filter_by_relevance(results, 25))
    kept_moderate = len(_smart_filter_by_relevance(results, 50))
    kept_strict = len(_smart_filter_by_relevance(results, 100))
    assert kept_loose >= kept_moderate >= kept_strict


def test_knee_detection_force_includes_leading_cluster():
    # Even with strict statistical cutoff, a clear gap should keep the leading
    # cluster in the result.
    leading = [0.30, 0.31, 0.32]
    noise = [1.50 + 0.01 * i for i in range(40)]
    results = _make_results(leading + noise)
    out = _smart_filter_by_relevance(results, 100)
    leading_ids = {f"p{i}" for i in range(len(leading))}
    out_ids = {r["photo_id"] for r in out}
    assert leading_ids.issubset(out_ids)


def test_strictness_to_k_anchors():
    assert _strictness_to_k(0) == 0.0
    assert _strictness_to_k(25) == 0.5
    assert _strictness_to_k(50) == 1.0
    assert _strictness_to_k(75) == 1.5
    assert _strictness_to_k(100) == 2.0


def test_clamp_max_results():
    assert _clamp_max_results(None) == DEFAULT_MAX_RESULTS
    assert _clamp_max_results("not a number") == DEFAULT_MAX_RESULTS
    assert _clamp_max_results(1) == 10  # below MIN_RESULTS_HARD_LIMIT
    assert _clamp_max_results(50) == 50
    assert _clamp_max_results(99999) == 2000  # above MAX_RESULTS_HARD_LIMIT


def test_clamp_strictness():
    assert _clamp_strictness(None) == DEFAULT_RELEVANCE_STRICTNESS
    assert _clamp_strictness("nope") == DEFAULT_RELEVANCE_STRICTNESS
    assert _clamp_strictness(-50) == 0
    assert _clamp_strictness(500) == 100
    assert _clamp_strictness(42) == 42
