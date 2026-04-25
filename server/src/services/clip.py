import threading
import os
import tqdm
from huggingface_hub import snapshot_download, hf_hub_download, HfApi
from config import logger, IMAGE_MODEL_ID


_download_status = {
    "status": "idle",
    "progress": 0,
    "total": 0,
    "error": None,
    "current_file": None,
}

_download_lock = threading.Lock()
_counter_lock = threading.Lock()
_download_thread = None


class DownloadProgressTracker(tqdm.tqdm):
    """
    Custom tqdm wrapper to capture byte-wise progress from hf_hub_download
    and update the global status for the Lightroom UI.
    """

    def __init__(self, *args, **kwargs):
        # Mute standard output to avoid polluting logs/console
        kwargs["file"] = open(os.devnull, "w")
        kwargs.pop("name", None)
        super().__init__(*args, **kwargs)

    def update(self, n=1):
        super().update(n)
        if n > 0:
            with _download_lock:
                _download_status["progress"] += n

                # Ensure progress doesn't exceed total due to tqdm behavior or imprecise total size
                if (
                    _download_status["total"] > 0
                    and _download_status["progress"] > _download_status["total"]
                ):
                    _download_status["progress"] = _download_status["total"]


def get_download_status():
    with _download_lock:
        return _download_status


def start_download_clip_model():
    global _download_thread
    with _download_lock:
        if _download_thread and _download_thread.is_alive():
            logger.warning("Download thread is already running.")
            return
        _download_thread = threading.Thread(
            target=_download_clip_model_thread, name="clip_model_downloader"
        )
        _download_thread.daemon = True
        _download_thread.start()


def _download_clip_model_thread():
    global _download_status
    with _download_lock:
        if _download_status["status"] == "downloading":
            logger.warning("Download already in progress.")
            return

    logger.info(f"Starting granular CLIP model download: {IMAGE_MODEL_ID}")

    try:
        api = HfApi()
        model_info = api.model_info(IMAGE_MODEL_ID, files_metadata=True)

        # Determine all files with actual sizes
        files_to_download = [f.rfilename for f in model_info.siblings if f.size]
        total_size = sum(f.size for f in model_info.siblings if f.size)

        with _download_lock:
            _download_status.update(
                {
                    "status": "downloading",
                    "progress": 0,
                    "total": total_size,
                    "error": None,
                    "current_file": None,
                }
            )

        # Download files one by one to capture smooth byte-wise progress per file.
        # Track bytes from completed files to handle cases where hf_hub_download
        # skips a file (already cached) and doesn't trigger tqdm updates.
        bytes_from_completed_files = 0

        for filename in files_to_download:
            # Find size from model_info
            file_size = next(
                (f.size for f in model_info.siblings if f.rfilename == filename), 0
            )

            with _download_lock:
                _download_status["current_file"] = filename

            logger.info(f"Downloading model file: {filename} ({file_size} bytes)")
            hf_hub_download(
                repo_id=IMAGE_MODEL_ID,
                filename=filename,
                tqdm_class=DownloadProgressTracker,
            )

            # After hf_hub_download returns, we ensure the progress for THIS file is fully counted
            bytes_from_completed_files += file_size
            with _download_lock:
                if _download_status["progress"] < bytes_from_completed_files:
                    _download_status["progress"] = bytes_from_completed_files

        # Final snapshot_download call strictly for cross-compatibility.
        # Since files are cached, this will just link the snapshot folder and return instantly.
        logger.info("Finalizing CLIP model snapshot structure...")
        path = snapshot_download(repo_id=IMAGE_MODEL_ID)

        with _download_lock:
            _download_status["status"] = "completed"
            _download_status["progress"] = total_size

        logger.info(f"CLIP model download complete. Path: {path}")
    except Exception as e:
        logger.error(f"Error downloading CLIP model: {e}", exc_info=True)
        with _download_lock:
            _download_status["status"] = "error"
            _download_status["error"] = str(e)
