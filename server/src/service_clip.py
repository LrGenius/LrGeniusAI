import threading
import os
import tqdm
from huggingface_hub import snapshot_download, HfApi
from config import logger, IMAGE_MODEL_ID


_download_status = {
    "status": "idle",
    "progress": 0,
    "total": 0,
    "error": None
}


class DownloadProgressTracker(tqdm.tqdm):
    # Statische Zähler für den gesamten Prozess
    global_bytes_downloaded = 0
    global_total_size = 0
    
    def __init__(self, *args, **kwargs):
        # 1. Mute: Die Ausgabe geht ins Nichts (devnull)
        kwargs['file'] = open(os.devnull, 'w')
        kwargs.pop('name', None)
        
        # Original init aufrufen
        super().__init__(*args, **kwargs)

    def update(self, n=1):
        # Interne Logik von tqdm weiterlaufen lassen (für Timings etc.)
        super().update(n)
        
        # 2. Daten abgreifen
        # Wir filtern auf Bytes, um nicht Dateianzahl-Balken mitzuzählen
        if n > 0 and self.unit in ['B', 'b', None]:
             with _counter_lock:
                DownloadProgressTracker.global_bytes_downloaded += n
                
                # GUI Status Update
                with _download_lock:
                    current = DownloadProgressTracker.global_bytes_downloaded
                    total = DownloadProgressTracker.global_total_size
                    
                    # Cap auf 100% (falls tqdm etwas überschießt)
                    if total > 0 and current > total:
                        current = total
                    
                    _download_status["progress"] = current


def get_download_status():
    with _download_lock:
        return _download_status

_download_thread = None
_download_lock = threading.Lock()


def _download_clip_model_thread():
    global _download_status
    with _download_lock:
        if _download_status["status"] == "downloading":
            logger.warning("Download already in progress.")
            return

    logger.info("Starting CLIP model download in background thread.")

    try:
        api = HfApi()
        model_info = api.model_info(IMAGE_MODEL_ID, files_metadata=True)
        total_size = sum(f.size for f in model_info.siblings if f.size)

        DownloadProgressTracker.total_size = total_size
        DownloadProgressTracker.bytes_downloaded = 0

        with _download_lock:
            _download_status = {
                "status": "downloading",
                "progress": 0,
                "total": total_size,
                "error": None,
                "unit": "B"
            }

        path = snapshot_download(
            repo_id=IMAGE_MODEL_ID,
            # tqdm_class=DownloadProgressTracker
        )

        with _download_lock:
            _download_status["status"] = "completed"
        logger.info(f"CLIP model downloaded to {path}")
    except Exception as e:
        logger.error(f"Error downloading CLIP model in background: {e}", exc_info=True)
        with _download_lock:
            _download_status["status"] = "error"
            _download_status["error"] = str(e)


def start_download_clip_model():
    global _download_thread
    with _download_lock:
        if _download_thread and _download_thread.is_alive():
            logger.warning("Download thread is already running.")
            return
        _download_thread = threading.Thread(target=_download_clip_model_thread)
        _download_thread.daemon = True
        _download_thread.start()