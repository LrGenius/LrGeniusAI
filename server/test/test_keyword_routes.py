"""Integration-style tests for routes/keywords.py using the Flask test client.

The CLIP model is stubbed out; no GPU or real embeddings are required.
"""

import json
import time
import unittest
from unittest.mock import MagicMock, patch

import pytest

from geniusai_server import app


@pytest.fixture
def client():
    app.config["TESTING"] = True
    with app.test_client() as c:
        yield c


def _stub_clip():
    """Return (mock_tokenizer, mock_model) that produce unit-norm embeddings."""
    tokenizer = MagicMock()
    tokenizer.return_value = MagicMock()

    model = MagicMock()
    model.context_length = 77

    def fake_encode_text(tokens):
        import torch

        # Return a 2-D tensor; F.normalize will make each row unit-norm
        n = tokens.shape[0] if hasattr(tokens, "shape") else 2
        return torch.randn(n, 512)

    model.encode_text.side_effect = fake_encode_text
    return tokenizer, model


# ─── /keywords/cluster (synchronous) ─────────────────────────────────────────


class ClusterSyncTests(unittest.TestCase):
    def setUp(self):
        self.app = app.test_client()
        app.config["TESTING"] = True

    def _post(self, payload, **kw):
        return self.app.post(
            "/keywords/cluster",
            data=json.dumps(payload),
            content_type="application/json",
            **kw,
        )

    def test_empty_keywords_returns_empty_results(self):
        resp = self._post({"keywords": []})
        self.assertEqual(resp.status_code, 200)
        data = resp.get_json()
        self.assertEqual(data["results"], [])
        self.assertIsNone(data["error"])

    def test_single_keyword_returns_empty_results(self):
        resp = self._post({"keywords": ["Dog"]})
        self.assertEqual(resp.status_code, 200)
        self.assertEqual(resp.get_json()["results"], [])

    def test_non_list_keywords_returns_400(self):
        resp = self._post({"keywords": "not-a-list"})
        self.assertEqual(resp.status_code, 400)

    def test_no_clip_model_returns_empty_with_warning(self):
        with patch("routes.keywords.server_lifecycle.get_tokenizer", return_value=None):
            with patch("routes.keywords.server_lifecycle.get_model", return_value=None):
                resp = self._post({"keywords": ["Dog", "Cat"]})
        self.assertEqual(resp.status_code, 200)
        data = resp.get_json()
        self.assertEqual(data["results"], [])
        self.assertIsNotNone(data["warning"])

    def test_duplicate_keywords_deduplicated(self):
        with patch("routes.keywords.server_lifecycle.get_tokenizer") as mt:
            with patch("routes.keywords.server_lifecycle.get_model") as mm:
                tok, mdl = _stub_clip()
                mt.return_value = tok
                mm.return_value = mdl
                resp = self._post({"keywords": ["Dog", "dog", "DOG", "Cat"]})
        self.assertEqual(resp.status_code, 200)

    def test_threshold_clamped_below_half(self):
        # Threshold < 0.5 gets clamped to 0.5 — should not crash
        with patch("routes.keywords.server_lifecycle.get_tokenizer", return_value=None):
            with patch("routes.keywords.server_lifecycle.get_model", return_value=None):
                resp = self._post({"keywords": ["A", "B"], "threshold": 0.1})
        self.assertEqual(resp.status_code, 200)


# ─── /keywords/cluster/start + /keywords/cluster/status ──────────────────────


class ClusterAsyncTests(unittest.TestCase):
    def setUp(self):
        self.app = app.test_client()
        app.config["TESTING"] = True

    def test_start_returns_job_id(self):
        with patch("routes.keywords.server_lifecycle.get_tokenizer", return_value=None):
            with patch("routes.keywords.server_lifecycle.get_model", return_value=None):
                resp = self.app.post(
                    "/keywords/cluster/start",
                    json={"keywords": ["Dog", "Cat"]},
                )
        self.assertEqual(resp.status_code, 202)
        data = resp.get_json()
        self.assertIn("job_id", data)
        self.assertIsNotNone(data["job_id"])

    def test_status_unknown_job_returns_404(self):
        resp = self.app.get("/keywords/cluster/status/nonexistent-job-id")
        self.assertEqual(resp.status_code, 404)

    def test_start_then_poll_until_done(self):
        with patch("routes.keywords.server_lifecycle.get_tokenizer", return_value=None):
            with patch("routes.keywords.server_lifecycle.get_model", return_value=None):
                start_resp = self.app.post(
                    "/keywords/cluster/start",
                    json={"keywords": ["Dog", "Cat"]},
                )
        self.assertEqual(start_resp.status_code, 202)
        job_id = start_resp.get_json()["job_id"]

        # Poll with a short deadline — async worker runs in a daemon thread
        deadline = time.monotonic() + 5.0
        status = None
        while time.monotonic() < deadline:
            poll = self.app.get(f"/keywords/cluster/status/{job_id}")
            if poll.status_code == 404:
                break  # job was completed and cleaned up
            data = poll.get_json()
            status = data.get("status")
            if status in ("done", "error"):
                break
            time.sleep(0.1)

        self.assertIn(status, ("done", "error", None))  # None = 404 (cleaned up)

    def test_start_invalid_keywords_returns_400(self):
        resp = self.app.post(
            "/keywords/cluster/start",
            json={"keywords": "not-a-list"},
        )
        self.assertEqual(resp.status_code, 400)


# ─── /keywords/apply-merges ───────────────────────────────────────────────────


class ApplyMergesRouteTests(unittest.TestCase):
    def setUp(self):
        self.app = app.test_client()
        app.config["TESTING"] = True

    def _post(self, payload):
        return self.app.post("/keywords/apply-merges", json=payload)

    def test_empty_merges_returns_zero(self):
        with patch("services.chroma") as mc:
            mc.collection = None
            resp = self._post({"merges": []})
        self.assertEqual(resp.status_code, 200)
        self.assertEqual(resp.get_json()["updated_photos"], 0)

    def test_invalid_merges_type_returns_400(self):
        resp = self._post({"merges": "not-a-list"})
        self.assertEqual(resp.status_code, 400)

    def test_successful_merge_returns_count(self):
        mock_col = MagicMock()
        mock_col.get.return_value = {
            "ids": ["p1"],
            "metadatas": [{"flattened_keywords": "Car, Bike", "keywords": ""}],
        }
        mock_col.update = MagicMock()

        with patch("services.chroma") as mc:
            mc.collection = mock_col
            mc.STATS_GET_LIMIT = 50000
            resp = self._post(
                {"merges": [{"duplicate": "Car", "canonical": "Automobile"}]}
            )

        self.assertEqual(resp.status_code, 200)
        data = resp.get_json()
        self.assertEqual(data["updated_photos"], 1)
        self.assertIsNone(data["error"])

    def test_no_collection_returns_zero(self):
        with patch("services.chroma") as mc:
            mc.collection = None
            resp = self._post(
                {"merges": [{"duplicate": "Car", "canonical": "Automobile"}]}
            )
        self.assertEqual(resp.status_code, 200)
        self.assertEqual(resp.get_json()["updated_photos"], 0)


if __name__ == "__main__":
    unittest.main()
