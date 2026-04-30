import unittest

from services.jobs import complete_job, create_job, fail_job, get_job, _jobs, _lock


def _drain():
    with _lock:
        _jobs.clear()


class JobLifecycleTests(unittest.TestCase):
    def setUp(self):
        _drain()

    def tearDown(self):
        _drain()

    def test_create_job_returns_hex_id(self):
        job_id = create_job()
        self.assertIsInstance(job_id, str)
        self.assertEqual(len(job_id), 32)

    def test_new_job_status_is_running(self):
        job_id = create_job()
        with _lock:
            self.assertEqual(_jobs[job_id]["status"], "running")

    def test_complete_job_marks_done(self):
        job_id = create_job()
        complete_job(job_id, {"results": [1, 2], "warning": None})
        job = get_job(job_id)
        self.assertIsNotNone(job)
        self.assertEqual(job["status"], "done")
        self.assertEqual(job["result"]["results"], [1, 2])

    def test_fail_job_marks_error(self):
        job_id = create_job()
        fail_job(job_id, "something went wrong")
        job = get_job(job_id)
        self.assertIsNotNone(job)
        self.assertEqual(job["status"], "error")
        self.assertEqual(job["error"], "something went wrong")

    def test_get_job_returns_none_for_unknown_id(self):
        self.assertIsNone(get_job("does-not-exist"))

    def test_completed_job_removed_from_store_after_get(self):
        job_id = create_job()
        complete_job(job_id, {})
        get_job(job_id)
        with _lock:
            self.assertNotIn(job_id, _jobs)

    def test_failed_job_removed_from_store_after_get(self):
        job_id = create_job()
        fail_job(job_id, "err")
        get_job(job_id)
        with _lock:
            self.assertNotIn(job_id, _jobs)

    def test_running_job_stays_in_store_after_get(self):
        job_id = create_job()
        job = get_job(job_id)
        self.assertIsNotNone(job)
        with _lock:
            self.assertIn(job_id, _jobs)

    def test_expired_jobs_purged_on_next_create(self):
        job_id = create_job()
        with _lock:
            _jobs[job_id]["created_at"] -= 700  # older than 600 s TTL
        create_job()  # triggers _purge_expired
        with _lock:
            self.assertNotIn(job_id, _jobs)

    def test_complete_unknown_job_is_noop(self):
        complete_job("nonexistent", {})  # must not raise

    def test_fail_unknown_job_is_noop(self):
        fail_job("nonexistent", "err")  # must not raise

    def test_multiple_jobs_independent(self):
        id1 = create_job()
        id2 = create_job()
        complete_job(id1, {"x": 1})
        fail_job(id2, "boom")
        j1 = get_job(id1)
        j2 = get_job(id2)
        self.assertEqual(j1["status"], "done")
        self.assertEqual(j2["status"], "error")


if __name__ == "__main__":
    unittest.main()
