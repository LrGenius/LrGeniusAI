"""Tests for routes/db.py."""

import pytest

from geniusai_server import app


@pytest.fixture
def client():
    app.config["TESTING"] = True
    with app.test_client() as c:
        yield c


def test_db_stats_returns_raw_stats(client, mocker):
    stats = {
        "photos": {"total": 3, "with_embedding": 2},
        "faces": {"total": 5},
        "persons": {"total": 1},
    }
    mocker.patch("routes.db.service_db.get_database_stats", return_value=stats)

    response = client.get("/db/stats")
    assert response.status_code == 200
    assert response.get_json() == stats


def test_db_stats_payload_is_json_serializable(client, mocker):
    mocker.patch(
        "routes.db.service_db.get_database_stats",
        return_value={"photos": {"total": 0}},
    )
    response = client.get("/db/stats")
    assert response.status_code == 200
    payload = response.get_json()
    assert isinstance(payload, dict)


def test_db_stats_service_exception_returns_error(client, mocker):
    mocker.patch(
        "routes.db.service_db.get_database_stats",
        side_effect=RuntimeError("chroma unavailable"),
    )
    response = client.get("/db/stats")
    assert response.status_code == 500
    payload = response.get_json()
    assert "error" in payload
    assert "chroma unavailable" in payload["error"]
