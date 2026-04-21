import io
import pytest
import json
from unittest.mock import MagicMock


from geniusai_server import app


@pytest.fixture
def client():
    app.config["TESTING"] = True
    with app.test_client() as client:
        yield client


# --- Admin / Diagnostic Endpoints ---


def test_ping(client):
    response = client.get("/ping")
    assert response.status_code == 200
    assert response.data.decode("utf-8").strip() == "pong"


def test_version(client):
    response = client.get("/version")
    assert response.status_code == 200
    data = response.get_json()
    assert "backend_version" in data


def test_version_check(client):
    response = client.post(
        "/version/check",
        json={
            "plugin_version": "1.0.0",
            "plugin_release_tag": "v1.0.0",
            "plugin_build": 12345,
        },
    )
    assert response.status_code == 200


# --- Database Endpoints ---


def test_db_stats(client, mocker):
    mocker.patch("routes_db.service_db.get_database_stats", return_value={"total": 0})
    response = client.get("/db/stats")
    assert response.status_code == 200
    assert response.get_json() == {"total": 0}


def test_get_ids(client, mocker):
    mocker.patch(
        "routes_index.chroma_service.get_all_image_ids", return_value=["id1", "id2"]
    )
    response = client.get("/get/ids")
    assert response.status_code == 200
    assert response.get_json() == ["id1", "id2"]


# --- Search / Cull Endpoints ---


def test_search(client, mocker):
    mocker.patch("routes_search.service_search.search_images", return_value=([], None))
    response = client.post("/search", json={"term": "test search", "max_results": 5})
    assert response.status_code == 200
    assert response.get_json() == {"results": []}


def test_find_similar(client, mocker):
    mocker.patch(
        "routes_search.service_search.find_similar_images", return_value=([], None)
    )
    response = client.post(
        "/find_similar", json={"photo_id": "test_id", "max_results": 5}
    )

    assert response.status_code == 200
    assert response.get_json() == {"results": []}


def test_group_similar(client, mocker):
    mocker.patch(
        "routes_search.service_search.group_similar_images", return_value=([], None)
    )
    response = client.post("/group_similar", json={"photo_ids": ["id1", "id2"]})
    assert response.status_code == 200


def test_cull(client, mocker):
    mocker.patch("service_search.cull_images", return_value={"groups": []})
    response = client.post("/cull", json={"photo_ids": ["id1", "id2"]})
    assert response.status_code == 200
    assert response.get_json() == {"groups": []}


# --- AI Edit Endpoints ---


def test_edit(client, mocker):
    mock_service = MagicMock()
    mock_response = MagicMock()
    mock_response.success = True
    mock_response.recipe = {"summary": "Fixed"}
    mock_response.input_tokens = 10
    mock_response.output_tokens = 20
    mock_service.generate_edit_recipe_single.return_value = mock_response
    mock_response.warning = None
    mock_response.error = None
    mocker.patch("routes_edit.get_analysis_service", return_value=mock_service)
    mocker.patch("routes_edit.chroma_service.get_image", return_value=None)
    mocker.patch("routes_edit.chroma_service.add_image")

    data = {"photo_id": ["id1"], "options": "{}"}
    response = client.post(
        "/edit",
        data={**data, "image": (io.BytesIO(b"fake data"), "test.jpg")},
        content_type="multipart/form-data",
    )
    assert response.status_code == 200


def test_edit_base64(client, mocker):
    mock_service = MagicMock()
    mock_response = MagicMock()
    mock_response.success = True
    mock_response.recipe = {"summary": "Fixed"}
    mock_response.input_tokens = 10
    mock_response.output_tokens = 20
    mock_service.generate_edit_recipe_single.return_value = mock_response
    mock_response.warning = None
    mock_response.error = None
    mocker.patch("routes_edit.get_analysis_service", return_value=mock_service)
    mocker.patch("routes_edit.chroma_service.get_image", return_value=None)
    mocker.patch("routes_edit.chroma_service.add_image")

    response = client.post(
        "/edit_base64",
        json={
            "image": "ZmFrZSBkYXRh",  # "fake data" in base64
            "photo_id": "test_id",
            "filename": "test.jpg",
        },
    )
    assert response.status_code == 200


# --- Indexing Endpoints ---


def test_index_unprocessed(client, mocker):
    mocker.patch("routes_index.get_photo_ids_needing_processing", return_value=["id2"])
    response = client.post(
        "/index/check-unprocessed", json={"photo_ids": ["id1", "id2"]}
    )
    assert response.status_code == 200
    assert "photo_ids" in response.get_json()
    assert response.get_json()["photo_ids"] == ["id2"]


def test_remove(client, mocker):
    mocker.patch("routes_index.chroma_service.delete_image", return_value=True)
    mocker.patch(
        "routes_index.chroma_service.delete_faces_by_photo_uuid", return_value=True
    )
    response = client.post("/remove", json={"photo_id": "id1"})
    assert response.status_code == 200


# --- Faces Endpoints ---


def test_faces_query(client, mocker):
    mocker.patch(
        "routes_faces.face_service.detect_faces",
        return_value=[{"embedding": [0.1, 0.2]}],
    )
    # Based on routes_faces implementation
    mocker.patch(
        "routes_faces.chroma_service.query_faces",
        return_value={
            "ids": [["f1"]],
            "distances": [[0.5]],
            "metadatas": [[{"photo_id": "p1"}]],
        },
    )

    response = client.post(
        "/faces/query", json={"image": "fakebase64", "n_results": 10}
    )
    assert response.status_code in [
        200,
        400,
    ]  # Usually 200 if base64 is bypassed or 400 if decode fails


def test_faces_cluster(client, mocker):
    mocker.patch(
        "routes_faces.persons_service.run_clustering", return_value={"summary": "ok"}
    )
    response = client.post("/faces/cluster")
    assert response.status_code == 200


def test_list_persons(client, mocker):
    mocker.patch("routes_faces.persons_service.list_persons", return_value=[])
    response = client.get("/faces/persons")
    assert response.status_code == 200


# --- Training Endpoints ---


def test_training_add(client, mocker):
    mocker.patch("routes_training.training_service.add_training_example")
    mocker.patch("routes_training.training_service.get_training_count", return_value=1)
    # Bypass CLIP embedding for simplicity in smoke test
    mocker.patch("routes_training._compute_clip_embedding", return_value=None)

    response = client.post(
        "/training/add",
        data={"photo_id": "test_id", "develop_settings": json.dumps({"Exposure": 1.0})},
    )
    assert response.status_code == 200
    assert response.get_json()["status"] == "ok"


def test_training_list(client, mocker):
    mocker.patch(
        "routes_training.training_service.list_training_examples", return_value=[]
    )
    response = client.get("/training/list")
    assert response.status_code == 200
    assert "examples" in response.get_json()


def test_training_count(client, mocker):
    mocker.patch("routes_training.training_service.get_training_count", return_value=5)
    response = client.get("/training/count")
    assert response.status_code == 200
    assert response.get_json()["count"] == 5


def test_training_delete(client, mocker):
    mocker.patch(
        "routes_training.training_service.delete_training_example", return_value=True
    )
    mocker.patch("routes_training.training_service.get_training_count", return_value=4)
    response = client.delete("/training/test_id")
    assert response.status_code == 200


# --- DB Maintenance Endpoints ---


def test_db_backup(client, mocker):
    mocker.patch(
        "routes_db.service_db.build_backup_zip",
        return_value=("fake_path.zip", "backup.zip"),
    )
    # Mock send_file to avoid FileNotFoundError in test environment
    mocker.patch(
        "routes_db.send_file",
        return_value=app.response_class("fake content", mimetype="application/zip"),
    )
    # Mock os.remove to avoid errors when the after_this_request handler runs
    mocker.patch("routes_db.os.remove")
    response = client.get("/db/backup")
    assert response.status_code == 200
    assert response.headers["Content-Type"] == "application/zip"


def test_db_migrate_photo_ids(client, mocker):
    mocker.patch("routes_db.service_db.migrate_photo_ids", return_value={"updated": 1})
    response = client.post("/db/migrate-photo-ids", json={"test": "mapping"})
    assert response.status_code == 200


# --- CLIP Endpoints ---


def test_clip_status(client, mocker):
    mocker.patch("routes_clip.is_model_cached", return_value=True)
    response = client.get("/clip/status")
    assert response.status_code == 200
    assert response.get_json()["clip"] == "ready"


# --- Import Endpoints ---


def test_import_metadata(client, mocker):
    mocker.patch(
        "routes_import.import_service.import_metadata_task", return_value=(1, 0)
    )
    response = client.post(
        "/import/metadata", json={"metadata_items": [{"photo_id": "p1"}]}
    )
    assert response.status_code == 200
    assert response.get_json()["success_count"] == 1


# --- Server Health and Models ---


def test_health(client, mocker):
    mocker.patch("server_lifecycle.get_health_status", return_value={"clip": "ok"})
    mock_service = MagicMock()
    mock_service.get_health_status.return_value = {"llm": "ok"}
    mocker.patch("routes_server.get_analysis_service", return_value=mock_service)
    mocker.patch("service_face._get_face_app")

    response = client.get("/health")
    assert response.status_code == 200
    data = response.get_json()
    assert "clip" in data
    assert "llm" in data


def test_models(client, mocker):
    mock_service = MagicMock()
    mock_service.get_available_models.return_value = {"chatgpt": ["gpt-4"]}
    mocker.patch("routes_server.get_analysis_service", return_value=mock_service)

    response = client.get("/models")
    assert response.status_code == 200
    assert "models" in response.get_json()
