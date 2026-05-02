"""Regression tests for B2: /index_by_reference unpacked 3 values from
process_image_task while it returns 4. Pre-fix this raised ValueError on
every call.
"""

import pytest

from geniusai_server import app


@pytest.fixture
def client():
    app.config["TESTING"] = True
    with app.test_client() as c:
        yield c


@pytest.fixture
def tmp_image(tmp_path):
    p = tmp_path / "test.jpg"
    p.write_bytes(b"fake image bytes")
    return str(p)


def test_index_by_reference_returns_envelope(client, mocker, tmp_image):
    mocker.patch(
        "routes.index.process_image_task",
        return_value=(1, 0, [], []),
    )
    response = client.post(
        "/index_by_reference",
        json={"images": [{"path": tmp_image, "photo_id": "abc"}]},
    )
    assert response.status_code == 200
    payload = response.get_json()
    assert payload["status"] == "processed"
    assert payload["success_count"] == 1
    assert payload["failure_count"] == 0
    # Warnings list always present (B2 fix added it to the response).
    assert "warnings" in payload


def test_index_by_reference_unpacks_four_values_without_error(
    client, mocker, tmp_image
):
    """Pre-B2-fix this raised ValueError because the route unpacked only 3 values."""
    mocker.patch(
        "routes.index.process_image_task",
        return_value=(2, 1, ["one failure"], ["soft warning"]),
    )
    response = client.post(
        "/index_by_reference",
        json={
            "images": [
                {"path": tmp_image, "photo_id": "a"},
                {"path": tmp_image, "photo_id": "b"},
                {"path": tmp_image, "photo_id": "c"},
            ]
        },
    )
    assert response.status_code == 200
    payload = response.get_json()
    assert payload["success_count"] == 2
    assert payload["failure_count"] == 1
    assert payload["warnings"] == ["soft warning"]
    assert payload["error_messages"] == ["one failure"]


def test_index_by_reference_aggregates_read_and_processing_failures(
    client, mocker, tmp_path
):
    """File that doesn't exist → counts as a read failure, no call to
    process_image_task is made for it. With one valid image we still expect
    success_count to come from the service result.
    """
    valid = tmp_path / "ok.jpg"
    valid.write_bytes(b"x")
    missing = str(tmp_path / "does_not_exist.jpg")

    mocker.patch(
        "routes.index.process_image_task",
        return_value=(1, 0, [], []),
    )
    response = client.post(
        "/index_by_reference",
        json={
            "images": [
                {"path": str(valid), "photo_id": "a"},
                {"path": missing, "photo_id": "b"},
            ]
        },
    )
    assert response.status_code == 200
    payload = response.get_json()
    assert payload["success_count"] == 1
    # 1 read failure + 0 processing failures
    assert payload["failure_count"] == 1
