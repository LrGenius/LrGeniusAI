"""Tests for routes/faces.py — covers all face and person endpoints."""

import base64
import pytest

from geniusai_server import app


@pytest.fixture
def client():
    app.config["TESTING"] = True
    with app.test_client() as c:
        yield c


def _b64(data: bytes = b"fakeimage") -> str:
    return base64.b64encode(data).decode("ascii")


# ---------------------------------------------------------------------------
# /faces/detect
# ---------------------------------------------------------------------------


class TestDetectFaces:
    def test_missing_body_returns_400(self, client):
        response = client.post("/faces/detect", json={})
        assert response.status_code == 400
        assert "image" in response.get_json()["error"]

    def test_invalid_base64_returns_400(self, client):
        response = client.post("/faces/detect", json={"image": "not-valid-base64!!!"})
        assert response.status_code == 400
        assert "base64" in response.get_json()["error"].lower()

    def test_no_faces_detected_returns_empty_list(self, client, mocker):
        mocker.patch("routes.faces.face_service.detect_faces", return_value=[])
        response = client.post("/faces/detect", json={"image": _b64()})
        assert response.status_code == 200
        data = response.get_json()
        assert data["status"] == "ok"
        assert data["faces"] == []

    def test_faces_detected_returns_thumbnails(self, client, mocker):
        mocker.patch(
            "routes.faces.face_service.detect_faces",
            return_value=[
                {"thumbnail": "thumb0", "embedding": [0.1]},
                {"thumbnail": "thumb1", "embedding": [0.2]},
            ],
        )
        response = client.post("/faces/detect", json={"image": _b64()})
        assert response.status_code == 200
        data = response.get_json()
        assert data["status"] == "ok"
        assert len(data["faces"]) == 2
        assert data["faces"][0]["index"] == 0
        assert data["faces"][0]["thumbnail"] == "thumb0"
        assert data["faces"][1]["index"] == 1

    def test_detection_error_returns_500(self, client, mocker):
        mocker.patch(
            "routes.faces.face_service.detect_faces",
            side_effect=RuntimeError("model unavailable"),
        )
        response = client.post("/faces/detect", json={"image": _b64()})
        assert response.status_code == 500
        assert "error" in response.get_json()


# ---------------------------------------------------------------------------
# /faces/query
# ---------------------------------------------------------------------------


class TestQueryFaces:
    def test_missing_image_returns_400(self, client):
        response = client.post("/faces/query", json={})
        assert response.status_code == 400

    def test_no_faces_in_image_returns_no_face(self, client, mocker):
        mocker.patch("routes.faces.face_service.detect_faces", return_value=[])
        response = client.post("/faces/query", json={"image": _b64()})
        assert response.status_code == 200
        data = response.get_json()
        assert data["status"] == "no_face"
        assert data["results"] == []

    def test_out_of_bounds_face_index_returns_400(self, client, mocker):
        mocker.patch(
            "routes.faces.face_service.detect_faces",
            return_value=[{"embedding": [0.1], "thumbnail": "t"}],
        )
        response = client.post("/faces/query", json={"image": _b64(), "face_index": 5})
        assert response.status_code == 400
        assert "face_index" in response.get_json()["error"]

    def test_valid_query_returns_results(self, client, mocker):
        mocker.patch(
            "routes.faces.face_service.detect_faces",
            return_value=[{"embedding": [0.1, 0.2], "thumbnail": "t"}],
        )
        mocker.patch(
            "routes.faces.chroma_service.query_faces",
            return_value={
                "ids": [["face1", "face2"]],
                "distances": [[0.1, 0.3]],
                "metadatas": [
                    [
                        {"photo_id": "p1", "thumbnail": "tb1", "person_id": "person1"},
                        {"photo_id": "p2", "thumbnail": "tb2", "person_id": "person2"},
                    ]
                ],
            },
        )
        response = client.post("/faces/query", json={"image": _b64()})
        assert response.status_code == 200
        data = response.get_json()
        assert data["status"] == "ok"
        assert len(data["results"]) == 2
        assert data["results"][0]["face_id"] == "face1"
        assert data["results"][0]["photo_id"] == "p1"


# ---------------------------------------------------------------------------
# /faces/cluster
# ---------------------------------------------------------------------------


class TestClusterFaces:
    def test_cluster_success(self, client, mocker):
        mocker.patch(
            "routes.faces.persons_service.run_clustering",
            return_value={"persons_created": 3, "faces_assigned": 12},
        )
        response = client.post("/faces/cluster", json={})
        assert response.status_code == 200
        data = response.get_json()
        assert data["status"] == "ok"
        assert data["persons_created"] == 3

    def test_invalid_linkage_defaults_to_complete(self, client, mocker):
        mock_cluster = mocker.patch(
            "routes.faces.persons_service.run_clustering",
            return_value={"persons_created": 0, "faces_assigned": 0},
        )
        client.post("/faces/cluster", json={"linkage": "invalid_value"})
        call_kwargs = mock_cluster.call_args[1]
        assert call_kwargs["linkage"] == "complete"

    def test_cluster_exception_returns_500(self, client, mocker):
        mocker.patch(
            "routes.faces.persons_service.run_clustering",
            side_effect=RuntimeError("clustering failed"),
        )
        response = client.post("/faces/cluster", json={})
        assert response.status_code == 500


# ---------------------------------------------------------------------------
# /faces/persons
# ---------------------------------------------------------------------------


class TestListPersons:
    def test_list_persons_success(self, client, mocker):
        mocker.patch(
            "routes.faces.persons_service.list_persons",
            return_value=[{"person_id": "p1", "name": "Alice", "face_count": 5}],
        )
        response = client.get("/faces/persons")
        assert response.status_code == 200
        data = response.get_json()
        assert data["status"] == "ok"
        assert len(data["persons"]) == 1
        assert data["persons"][0]["name"] == "Alice"

    def test_list_persons_exception_returns_500(self, client, mocker):
        mocker.patch(
            "routes.faces.persons_service.list_persons",
            side_effect=RuntimeError("db error"),
        )
        response = client.get("/faces/persons")
        assert response.status_code == 500


# ---------------------------------------------------------------------------
# /faces/persons/<person_id>/thumbnail
# ---------------------------------------------------------------------------


class TestGetPersonThumbnail:
    def test_returns_thumbnail(self, client, mocker):
        mocker.patch(
            "routes.faces.persons_service.get_person_thumbnail_b64",
            return_value="base64thumb",
        )
        response = client.get("/faces/persons/person-123/thumbnail")
        assert response.status_code == 200
        data = response.get_json()
        assert data["status"] == "ok"
        assert data["person_id"] == "person-123"
        assert data["thumbnail"] == "base64thumb"

    def test_exception_returns_500(self, client, mocker):
        mocker.patch(
            "routes.faces.persons_service.get_person_thumbnail_b64",
            side_effect=RuntimeError("not found"),
        )
        response = client.get("/faces/persons/bad-id/thumbnail")
        assert response.status_code == 500


# ---------------------------------------------------------------------------
# PUT /faces/persons/<person_id>
# ---------------------------------------------------------------------------


class TestSetPersonName:
    def test_set_name_success(self, client, mocker):
        mock_set = mocker.patch("routes.faces.persons_service.set_person_name")
        response = client.put("/faces/persons/person-abc", json={"name": "  Bob  "})
        assert response.status_code == 200
        data = response.get_json()
        assert data["status"] == "ok"
        assert data["name"] == "Bob"
        mock_set.assert_called_once_with("person-abc", "  Bob  ")

    def test_non_string_name_coerced(self, client, mocker):
        mocker.patch("routes.faces.persons_service.set_person_name")
        response = client.put("/faces/persons/p1", json={"name": 42})
        assert response.status_code == 200

    def test_exception_returns_500(self, client, mocker):
        mocker.patch(
            "routes.faces.persons_service.set_person_name",
            side_effect=RuntimeError("write error"),
        )
        response = client.put("/faces/persons/p1", json={"name": "Alice"})
        assert response.status_code == 500


# ---------------------------------------------------------------------------
# /faces/persons/<person_id>/photos
# ---------------------------------------------------------------------------


class TestGetPhotosForPerson:
    def test_returns_photo_ids(self, client, mocker):
        mocker.patch(
            "routes.faces.persons_service.get_photo_ids_for_person",
            return_value=["photo1", "photo2"],
        )
        response = client.get("/faces/persons/p1/photos")
        assert response.status_code == 200
        data = response.get_json()
        assert data["status"] == "ok"
        assert data["photo_ids"] == ["photo1", "photo2"]
        assert data["photo_uuids"] == ["photo1", "photo2"]

    def test_exception_returns_500(self, client, mocker):
        mocker.patch(
            "routes.faces.persons_service.get_photo_ids_for_person",
            side_effect=RuntimeError("not found"),
        )
        response = client.get("/faces/persons/bad/photos")
        assert response.status_code == 500
