import service_chroma as chroma_service
import service_persons as persons_service
from config import logger, DB_PATH

import os
import json
import shutil
import tempfile
import zipfile
from datetime import datetime


# Ordner für serverseitig aufgehobene Backups: Docker /data/db/backups, Standalone <db-path>/backups
def _get_backups_dir():
    from config import DB_PATH

    if not DB_PATH:
        return None
    return os.path.join(DB_PATH, "backups")


def get_database_stats(catalog_id=None) -> dict:
    """Return database statistics for photos, faces, and persons.
    If catalog_id is provided, photo stats are limited to that catalog (soft state).
    """
    image_stats = chroma_service.get_image_metadata_stats(catalog_id=catalog_id)
    face_count = chroma_service.get_face_count()
    persons = persons_service.list_persons()
    person_count = len(persons)

    return {
        "photos": {
            "total": image_stats["total"],
            "with_embedding": image_stats["with_embedding"],
            "with_title": image_stats["with_title"],
            "with_caption": image_stats["with_caption"],
            "with_keywords": image_stats["with_keywords"],
            "with_vertexai": image_stats["with_vertexai"],
        },
        "faces": {"total": face_count},
        "persons": {"total": person_count},
    }


def build_backup_zip() -> tuple[str, str]:
    """Create a temporary ZIP containing all persistent DB files."""
    if not DB_PATH or not os.path.isdir(DB_PATH):
        raise FileNotFoundError(
            f"Database path does not exist or is not a directory: {DB_PATH}"
        )

    backup_name = (
        f"lrgeniusai-backend-backup-{datetime.utcnow().strftime('%Y%m%d-%H%M%S')}.zip"
    )
    fd, zip_path = tempfile.mkstemp(prefix="lrgeniusai-backup-", suffix=".zip")
    os.close(fd)

    root_parent = os.path.dirname(DB_PATH)
    included_files = 0
    with zipfile.ZipFile(
        zip_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6
    ) as archive:
        for current_root, dirs, files in os.walk(DB_PATH):
            # Do not include the backups directory in the backup (avoid self-embedding and runaway size)
            dirs[:] = [
                d for d in dirs if not (current_root == DB_PATH and d == "backups")
            ]
            files.sort()
            for filename in files:
                full_path = os.path.join(current_root, filename)
                if not os.path.isfile(full_path):
                    continue
                archive_name = os.path.relpath(full_path, root_parent)
                archive.write(full_path, arcname=archive_name)
                included_files += 1

    logger.info(
        "Created DB backup zip at %s with %s files from %s",
        zip_path,
        included_files,
        DB_PATH,
    )

    # Kopie serverseitig aufbewahren (Docker: /data/db/backups, Standalone: <db-path>/backups)
    backups_dir = _get_backups_dir()
    if backups_dir:
        try:
            os.makedirs(backups_dir, exist_ok=True)
            persistent_path = os.path.join(backups_dir, backup_name)
            shutil.copy2(zip_path, persistent_path)
            logger.info("DB backup saved server-side to %s", persistent_path)
        except Exception as e:
            logger.warning("Could not save backup to %s: %s", backups_dir, e)

    return zip_path, backup_name


def prune_old_backups(max_keep: int = 10) -> int:
    """
    Remove old backup ZIPs from BACKUPS_DIR, keeping only the newest max_keep files.

    Args:
        max_keep: Number of most recent backup files to retain.

    Returns:
        Number of backup files that were deleted.
    """
    if max_keep <= 0:
        max_keep = 1

    backups_dir = _get_backups_dir()
    if not backups_dir or not os.path.isdir(backups_dir):
        return 0

    try:
        entries = [
            os.path.join(backups_dir, name)
            for name in os.listdir(backups_dir)
            if name.lower().endswith(".zip")
            and os.path.isfile(os.path.join(backups_dir, name))
        ]
    except Exception as e:
        logger.warning("Could not list backups in %s: %s", backups_dir, e)
        return 0

    if len(entries) <= max_keep:
        return 0

    entries.sort(key=lambda p: os.path.getmtime(p), reverse=True)
    to_delete = entries[max_keep:]
    deleted = 0
    for path in to_delete:
        try:
            os.remove(path)
            deleted += 1
            logger.info("Pruned old DB backup: %s", path)
        except FileNotFoundError:
            continue
        except Exception as e:
            logger.warning("Could not remove old backup %s: %s", path, e)
    if deleted > 0:
        logger.info(
            "Pruned %s old backups in %s, kept %s newest.",
            deleted,
            backups_dir,
            max_keep,
        )
    return deleted


def migrate_photo_ids(data: dict) -> dict:
    """Migrate existing Chroma IDs from legacy uuid to new photo_id values."""
    mappings = data.get("mappings")

    if mappings is None and data.get("mapping_file"):
        file_path = data["mapping_file"]
        if not os.path.isabs(file_path):
            file_path = os.path.join(DB_PATH, file_path)
        try:
            with open(file_path, "r", encoding="utf-8") as f:
                payload = json.load(f)
            mappings = payload.get("mappings", payload)
        except Exception as e:
            raise ValueError(f"Could not read mapping_file: {e}") from e

    if not isinstance(mappings, list):
        raise ValueError("mappings must be a list")

    logger.info(
        "Received photo_id migration request: mappings=%s overwrite=%s dry_run=%s update_faces=%s update_vertex=%s mapping_file=%s",
        len(mappings),
        bool(data.get("overwrite", False)),
        bool(data.get("dry_run", False)),
        bool(data.get("update_faces", True)),
        bool(data.get("update_vertex", True)),
        data.get("mapping_file"),
    )

    summary = chroma_service.migrate_photo_ids(
        mappings,
        update_faces=bool(data.get("update_faces", True)),
        update_vertex=bool(data.get("update_vertex", True)),
        overwrite=bool(data.get("overwrite", False)),
        dry_run=bool(data.get("dry_run", False)),
    )
    logger.info("Completed photo_id migration request: %s", summary)
    return summary
