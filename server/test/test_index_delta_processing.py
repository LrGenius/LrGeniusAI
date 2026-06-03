"""Regression tests for delta-mode indexing (``get_uuids_needing_processing``).

These run against a *real* (temporary) ChromaDB rather than mocking the service
layer. The earlier API-level test mocked ``get_photo_ids_needing_processing``,
so a regression in the real chroma path (#203 batch optimization) went
unnoticed: a single ``collection.get(ids=[...])`` over a large catalog exceeds
SQLite's host-variable limit, raised, was swallowed, and every photo was
re-flagged for processing. The large-catalog test below reproduces exactly that
condition and fails if the batch queries stop chunking.
"""

import os
import tempfile

import numpy as np
import pytest


@pytest.fixture
def chroma(monkeypatch):
    """Fresh ChromaDB backed by a throwaway directory, re-initialized per test."""
    import config
    from services import chroma as chroma_service

    tmp = tempfile.mkdtemp()
    monkeypatch.setattr(config, "DB_PATH", os.path.join(tmp, "db"))
    # Force lazy re-initialization against the new path.
    monkeypatch.setattr(chroma_service, "chroma_client", None, raising=False)
    monkeypatch.setattr(chroma_service, "collection", None, raising=False)
    monkeypatch.setattr(chroma_service, "face_collection", None, raising=False)
    monkeypatch.setattr(chroma_service, "vertex_collection", None, raising=False)
    chroma_service._ensure_initialized()
    return chroma_service


def _add(chroma, pid, *, catalog_id=None, **meta):
    meta.setdefault("photo_id", pid)
    chroma.add_image(pid, list(np.random.rand(64)), meta, catalog_id=catalog_id)


def _add_bulk(chroma, ids, **meta):
    """Add many photos, respecting Chroma's add-batch ceiling."""
    step = 5000
    for start in range(0, len(ids), step):
        chunk = ids[start : start + step]
        chroma.collection.add(
            ids=chunk,
            embeddings=[[0.0] * 64 for _ in chunk],
            metadatas=[{"photo_id": pid, **meta} for pid in chunk],
        )


# --------------------------------------------------------------------------- #
# Behavioural correctness (fast)                                              #
# --------------------------------------------------------------------------- #


def test_delta_skips_fully_processed_photos(chroma):
    from services.index import get_uuids_needing_processing

    _add(chroma, "done", has_embedding=True, title="t", cull_phash="ph")
    needing = get_uuids_needing_processing(
        ["done"],
        {"regenerate_metadata": False, "compute_embeddings": True},
    )
    assert needing == []


def test_delta_flags_new_and_incomplete_photos(chroma):
    from services.index import get_uuids_needing_processing

    _add(chroma, "done", has_embedding=True, title="t", cull_phash="ph")
    _add(chroma, "noemb", has_embedding=False, title="t", cull_phash="ph")
    needing = get_uuids_needing_processing(
        ["done", "noemb", "missing"],
        {"regenerate_metadata": False, "compute_embeddings": True},
    )
    assert set(needing) == {"noemb", "missing"}


def test_regenerate_flags_everything(chroma):
    from services.index import get_uuids_needing_processing

    _add(chroma, "done", has_embedding=True, title="t", cull_phash="ph")
    needing = get_uuids_needing_processing(
        ["done"],
        {"regenerate_metadata": True, "compute_embeddings": True},
    )
    assert needing == ["done"]


def test_get_images_batch_merges_across_chunks(chroma, monkeypatch):
    """Force a tiny chunk size so multi-chunk merging is exercised with few rows.

    Guards the chunk-merge logic independently of the SQLite limit.
    """
    monkeypatch.setattr(chroma, "GET_IDS_CHUNK_SIZE", 2)
    ids = [f"p{i}" for i in range(7)]
    for pid in ids:
        _add(chroma, pid, has_embedding=True)

    found = chroma.get_images_batch(ids)
    assert set(found.keys()) == set(ids)


# --------------------------------------------------------------------------- #
# Large-catalog regression (#203): a single batched get must not exceed        #
# SQLite's host-variable limit. Seeds above the limit so removing the          #
# chunking in chroma.py makes the underlying get raise -> records lost ->      #
# every photo wrongly re-flagged, failing this test.                           #
# --------------------------------------------------------------------------- #


def test_large_catalog_delta_does_not_reflag_everything(chroma):
    from services.index import get_uuids_needing_processing

    # SQLITE_MAX_VARIABLE_NUMBER defaults to 32766 on modern SQLite; go above it.
    n = 35000
    ids = [f"p{i}" for i in range(n)]
    _add_bulk(chroma, ids, has_embedding=True, cull_phash="ph")

    # The batch fetch must find every record (would be empty if a single
    # oversized get() raised and was swallowed).
    assert len(chroma.get_images_batch(ids)) == n

    needing = get_uuids_needing_processing(
        ids, {"regenerate_metadata": False, "compute_embeddings": True}
    )
    assert needing == []
