"""Tests for services/db.py — backup directory handling, pruning, and the
stats aggregation that backs /db/stats. The module wraps Chroma + filesystem,
so chroma calls are mocked.
"""

import os
import time

import pytest

from services import db as service_db


class TestBackupsDir:
    def test_returns_none_when_db_path_missing(self, monkeypatch):
        monkeypatch.setattr("config.DB_PATH", "")
        assert service_db._get_backups_dir() is None

    def test_appends_backups_subdir(self, monkeypatch, tmp_path):
        monkeypatch.setattr("config.DB_PATH", str(tmp_path))
        assert service_db._get_backups_dir() == os.path.join(str(tmp_path), "backups")


class TestGetDatabaseStats:
    def test_aggregates_chroma_results(self, mocker):
        mocker.patch.object(
            service_db.chroma_service,
            "get_image_metadata_stats",
            return_value={
                "total": 7,
                "with_embedding": 6,
                "with_title": 5,
                "with_caption": 4,
                "with_keywords": 3,
                "with_vertexai": 2,
            },
        )
        mocker.patch.object(
            service_db.chroma_service, "get_face_count", return_value=11
        )
        mocker.patch.object(
            service_db.persons_service,
            "list_persons",
            return_value=[{"id": "p1"}, {"id": "p2"}],
        )

        stats = service_db.get_database_stats()
        assert stats["photos"]["total"] == 7
        assert stats["photos"]["with_embedding"] == 6
        assert stats["faces"]["total"] == 11
        assert stats["persons"]["total"] == 2

    def test_passes_catalog_id_to_chroma(self, mocker):
        spy = mocker.patch.object(
            service_db.chroma_service,
            "get_image_metadata_stats",
            return_value={
                "total": 0,
                "with_embedding": 0,
                "with_title": 0,
                "with_caption": 0,
                "with_keywords": 0,
                "with_vertexai": 0,
            },
        )
        mocker.patch.object(service_db.chroma_service, "get_face_count", return_value=0)
        mocker.patch.object(service_db.persons_service, "list_persons", return_value=[])

        service_db.get_database_stats(catalog_id="cat-42")
        spy.assert_called_once_with(catalog_id="cat-42")


class TestPruneOldBackups:
    def _make_zip(self, dirpath, name, mtime):
        path = os.path.join(dirpath, name)
        with open(path, "w") as f:
            f.write("fake zip")
        os.utime(path, (mtime, mtime))
        return path

    def test_returns_zero_when_dir_missing(self, monkeypatch, tmp_path):
        monkeypatch.setattr("config.DB_PATH", str(tmp_path))
        # No backups subdir created
        assert service_db.prune_old_backups(max_keep=5) == 0

    def test_keeps_only_max_keep_newest(self, monkeypatch, tmp_path):
        monkeypatch.setattr("config.DB_PATH", str(tmp_path))
        backups = tmp_path / "backups"
        backups.mkdir()

        now = time.time()
        # Oldest first; we expect only the 2 newest to survive max_keep=2.
        self._make_zip(str(backups), "old.zip", now - 300)
        self._make_zip(str(backups), "mid.zip", now - 200)
        self._make_zip(str(backups), "newer.zip", now - 100)
        self._make_zip(str(backups), "newest.zip", now)

        deleted = service_db.prune_old_backups(max_keep=2)
        assert deleted == 2
        remaining = sorted(os.listdir(str(backups)))
        assert remaining == ["newer.zip", "newest.zip"]

    def test_zero_max_keep_clamped_to_one(self, monkeypatch, tmp_path):
        monkeypatch.setattr("config.DB_PATH", str(tmp_path))
        backups = tmp_path / "backups"
        backups.mkdir()
        now = time.time()
        self._make_zip(str(backups), "a.zip", now - 100)
        self._make_zip(str(backups), "b.zip", now)

        deleted = service_db.prune_old_backups(max_keep=0)
        # max_keep=0 is clamped to 1, so 1 file should remain
        assert deleted == 1
        assert os.listdir(str(backups)) == ["b.zip"]

    def test_ignores_non_zip_files(self, monkeypatch, tmp_path):
        monkeypatch.setattr("config.DB_PATH", str(tmp_path))
        backups = tmp_path / "backups"
        backups.mkdir()
        # Stray .txt file should not be considered for pruning.
        (backups / "readme.txt").write_text("hi")
        now = time.time()
        self._make_zip(str(backups), "a.zip", now - 100)
        self._make_zip(str(backups), "b.zip", now)

        deleted = service_db.prune_old_backups(max_keep=1)
        assert deleted == 1
        # readme.txt survives even though it's "older"
        assert "readme.txt" in os.listdir(str(backups))


class TestMigratePhotoIds:
    def test_rejects_non_list_mappings(self):
        with pytest.raises(ValueError, match="mappings must be a list"):
            service_db.migrate_photo_ids({"mappings": "not-a-list"})

    def test_calls_chroma_migrate_with_defaults(self, mocker):
        spy = mocker.patch.object(
            service_db.chroma_service,
            "migrate_photo_ids",
            return_value={"updated": 0},
        )
        service_db.migrate_photo_ids({"mappings": []})
        spy.assert_called_once()
        kwargs = spy.call_args.kwargs
        assert kwargs["update_faces"] is True
        assert kwargs["update_vertex"] is True
        assert kwargs["overwrite"] is False
        assert kwargs["dry_run"] is False
