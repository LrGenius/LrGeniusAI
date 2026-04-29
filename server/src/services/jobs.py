import threading
import time
import uuid

_lock = threading.Lock()
_jobs: dict[str, dict] = {}

_TTL_SECONDS = 600


def _purge_expired() -> None:
    now = time.monotonic()
    expired = [jid for jid, j in _jobs.items() if now - j["created_at"] > _TTL_SECONDS]
    for jid in expired:
        del _jobs[jid]


def create_job() -> str:
    job_id = uuid.uuid4().hex
    with _lock:
        _purge_expired()
        _jobs[job_id] = {
            "status": "running",
            "result": None,
            "error": None,
            "created_at": time.monotonic(),
        }
    return job_id


def complete_job(job_id: str, result: dict) -> None:
    with _lock:
        if job_id in _jobs:
            _jobs[job_id]["status"] = "done"
            _jobs[job_id]["result"] = result


def fail_job(job_id: str, error: str) -> None:
    with _lock:
        if job_id in _jobs:
            _jobs[job_id]["status"] = "error"
            _jobs[job_id]["error"] = error


def get_job(job_id: str) -> dict | None:
    with _lock:
        job = _jobs.get(job_id)
        if job is None:
            return None
        snapshot = dict(job)
        if snapshot["status"] in ("done", "error"):
            del _jobs[job_id]
        return snapshot
