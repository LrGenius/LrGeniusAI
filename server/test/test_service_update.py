"""Tests for services/update.py — sha256 verification and the spawn-updater
control flow. The actual subprocess and shutdown call are mocked.
"""

import hashlib

from services import update as service_update


class TestVerifySha256:
    def test_empty_expected_hash_passes(self):
        assert service_update.verify_sha256(b"any content", "") is True

    def test_correct_hash_passes(self):
        content = b"hello world"
        expected = hashlib.sha256(content).hexdigest()
        assert service_update.verify_sha256(content, expected) is True

    def test_uppercase_hash_matches(self):
        content = b"hello world"
        expected = hashlib.sha256(content).hexdigest().upper()
        assert service_update.verify_sha256(content, expected) is True

    def test_wrong_hash_fails(self):
        assert service_update.verify_sha256(b"abc", "0" * 64) is False


class TestPerformCodeUpdate:
    def setup_method(self):
        # Reset the module-level guard between tests.
        service_update._update_in_progress = False

    def test_starts_thread_and_writes_manifest(self, mocker, tmp_path):
        manifest_path = tmp_path / ".lrgeniusai" / "manifest_to_apply.json"
        mocker.patch(
            "services.update.os.path.expanduser",
            return_value=str(manifest_path),
        )
        # threading is imported inside the function, so patch the stdlib module.
        thread_mock = mocker.patch("threading.Thread")

        success, message = service_update.perform_code_update(
            {"version": "1.2.3"}, "/abs/plugin/path"
        )

        assert success is True
        assert "started" in message.lower()
        thread_mock.assert_called_once()
        # Manifest was written to disk
        assert manifest_path.exists()
        import json

        assert json.loads(manifest_path.read_text())["version"] == "1.2.3"

    def test_rejects_concurrent_update(self, mocker):
        service_update._update_in_progress = True
        success, message = service_update.perform_code_update({}, "/p")
        assert success is False
        assert "already in progress" in message

    def test_resets_progress_flag_on_failure(self, mocker):
        # Force the manifest write to blow up.
        mocker.patch(
            "services.update.os.path.expanduser",
            side_effect=RuntimeError("disk full"),
        )
        success, message = service_update.perform_code_update({}, "/p")
        assert success is False
        assert "disk full" in message
        # Guard must be reset so the next attempt isn't blocked forever.
        assert service_update._update_in_progress is False
