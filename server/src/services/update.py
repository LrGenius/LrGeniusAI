import os
import hashlib
from pathlib import Path
import json
import subprocess
import sys
import time
from config import logger

_update_in_progress = False


def verify_sha256(content: bytes, expected_hash: str) -> bool:
    if not expected_hash:
        return True
    actual_hash = hashlib.sha256(content).hexdigest()
    return actual_hash.lower() == expected_hash.lower()


def perform_code_update(manifest: dict, plugin_path: str) -> tuple[bool, str]:
    """
    Spawns the external updater GUI process and returns success if it started.
    """
    global _update_in_progress
    if _update_in_progress:
        return False, "An update is already in progress"
    _update_in_progress = True

    try:
        backend_root = Path(__file__).resolve().parents[2]
        updater_script = backend_root / "src" / "scripts" / "updater.py"

        # Save manifest to a temp file for the script
        manifest_path = Path(os.path.expanduser("~/.lrgeniusai/manifest_to_apply.json"))
        manifest_path.parent.mkdir(parents=True, exist_ok=True)
        with open(manifest_path, "w") as f:
            json.dump(manifest, f)

        import threading

        # Run in a thread to not block the response
        def run_updater():
            logger.info(f"Spawning updater GUI: {updater_script}")

            # Start detached so it outlives the backend
            cmd = [
                sys.executable,
                str(updater_script),
                str(manifest_path),
                plugin_path,
                str(backend_root),
            ]
            creationflags = (
                subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0
            )
            subprocess.Popen(cmd, start_new_session=True, creationflags=creationflags)

            # Now we can shutdown the backend to free up files
            logger.info("Updater spawned. Requesting backend shutdown.")
            time.sleep(2)
            import server_lifecycle

            server_lifecycle.request_shutdown()

        threading.Thread(target=run_updater, daemon=True).start()

        return True, "Update started"

    except Exception as e:
        _update_in_progress = False
        logger.error(f"Error starting update: {e}", exc_info=True)
        return False, str(e)
