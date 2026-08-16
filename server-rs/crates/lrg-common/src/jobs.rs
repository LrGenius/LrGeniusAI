//! Port of `services/jobs.py`: a generic in-memory async-job registry
//! (used today by keyword clustering, `/keywords/cluster/start`). Jobs
//! expire after 600s of inactivity; `get_job` on a finished job
//! returns it once and then removes it — matching Python's one-shot
//! poll-then-gone semantics exactly.
//!
//! Expiry is keyed on last activity, not creation time: a job that
//! reports progress or is being polled stays alive however long it runs.
//! Keyed on creation, a slow LLM validation pass (local models routinely
//! need more than ten minutes) could be purged while still running, and
//! the next status poll would 404 on a job that was working fine.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

const TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Running,
    Done,
    Error,
}

impl JobStatus {
    fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub status: JobStatus,
    pub result: Option<Value>,
    pub error: Option<String>,
    /// Free-form, job-specific description of what the job is doing right
    /// now. Clients use it to keep a progress UI alive while a job runs;
    /// `None` means the job has not reported anything yet.
    pub progress: Option<Value>,
}

impl JobSnapshot {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "status": self.status.as_str(),
            "result": self.result,
            "error": self.error,
            "progress": self.progress,
        })
    }
}

struct JobEntry {
    status: JobStatus,
    result: Option<Value>,
    error: Option<String>,
    progress: Option<Value>,
    last_activity: Instant,
}

#[derive(Default)]
pub struct JobRegistry {
    jobs: Mutex<HashMap<String, JobEntry>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        JobRegistry::default()
    }

    fn purge_expired(jobs: &mut HashMap<String, JobEntry>) {
        let now = Instant::now();
        jobs.retain(|_, j| now.duration_since(j.last_activity) <= TTL);
    }

    pub fn create_job(&self) -> String {
        let job_id = uuid::Uuid::new_v4().simple().to_string();
        let mut jobs = self.jobs.lock().unwrap();
        Self::purge_expired(&mut jobs);
        jobs.insert(
            job_id.clone(),
            JobEntry {
                status: JobStatus::Running,
                result: None,
                error: None,
                progress: None,
                last_activity: Instant::now(),
            },
        );
        job_id
    }

    /// Publish what the job is currently working on. Overwrites whatever
    /// was reported before — callers send a complete snapshot, not a delta.
    pub fn set_progress(&self, job_id: &str, progress: Value) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(job_id) {
            job.progress = Some(progress);
            job.last_activity = Instant::now();
        }
    }

    pub fn complete_job(&self, job_id: &str, result: Value) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = JobStatus::Done;
            job.result = Some(result);
            job.last_activity = Instant::now();
        }
    }

    pub fn fail_job(&self, job_id: &str, error: String) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = JobStatus::Error;
            job.error = Some(error);
            job.last_activity = Instant::now();
        }
    }

    /// Matches Python: a finished job (done/error) is removed from the
    /// registry the moment it's read, so it can only be polled once.
    /// Polling a running job counts as activity and keeps it alive.
    pub fn get_job(&self, job_id: &str) -> Option<JobSnapshot> {
        let mut jobs = self.jobs.lock().unwrap();
        let job = jobs.get_mut(job_id)?;
        let snapshot = JobSnapshot {
            status: job.status.clone(),
            result: job.result.clone(),
            error: job.error.clone(),
            progress: job.progress.clone(),
        };
        if matches!(snapshot.status, JobStatus::Done | JobStatus::Error) {
            jobs.remove(job_id);
        } else {
            job.last_activity = Instant::now();
        }
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_job_can_be_polled_repeatedly() {
        let reg = JobRegistry::new();
        let id = reg.create_job();
        assert_eq!(reg.get_job(&id).unwrap().status, JobStatus::Running);
        assert_eq!(reg.get_job(&id).unwrap().status, JobStatus::Running);
    }

    #[test]
    fn finished_job_is_removed_after_first_read() {
        let reg = JobRegistry::new();
        let id = reg.create_job();
        reg.complete_job(&id, serde_json::json!({"ok": true}));
        let snap = reg.get_job(&id).unwrap();
        assert_eq!(snap.status, JobStatus::Done);
        assert_eq!(snap.result, Some(serde_json::json!({"ok": true})));
        assert!(reg.get_job(&id).is_none());
    }

    #[test]
    fn failed_job_carries_error_and_is_removed() {
        let reg = JobRegistry::new();
        let id = reg.create_job();
        reg.fail_job(&id, "boom".to_string());
        let snap = reg.get_job(&id).unwrap();
        assert_eq!(snap.status, JobStatus::Error);
        assert_eq!(snap.error.as_deref(), Some("boom"));
        assert!(reg.get_job(&id).is_none());
    }

    #[test]
    fn unknown_job_returns_none() {
        let reg = JobRegistry::new();
        assert!(reg.get_job("nonexistent").is_none());
    }

    #[test]
    fn progress_is_reported_to_pollers_and_overwritten_by_later_updates() {
        let reg = JobRegistry::new();
        let id = reg.create_job();
        assert!(reg.get_job(&id).unwrap().progress.is_none());

        reg.set_progress(&id, serde_json::json!({"stage": "embedding"}));
        assert_eq!(
            reg.get_job(&id).unwrap().progress,
            Some(serde_json::json!({"stage": "embedding"}))
        );

        reg.set_progress(&id, serde_json::json!({"stage": "llm", "done": 1}));
        assert_eq!(
            reg.get_job(&id).unwrap().progress,
            Some(serde_json::json!({"stage": "llm", "done": 1}))
        );
    }

    #[test]
    fn progress_on_unknown_job_is_a_no_op() {
        let reg = JobRegistry::new();
        reg.set_progress("nonexistent", serde_json::json!({"stage": "llm"}));
        assert!(reg.get_job("nonexistent").is_none());
    }

    #[test]
    fn long_running_job_survives_a_purge_while_it_reports_progress() {
        let reg = JobRegistry::new();
        let id = reg.create_job();
        // Backdate the entry past the TTL, as a job running longer than
        // 600s would be, then have it report progress: the next purge
        // (triggered by creating another job) must not evict it.
        {
            let mut jobs = reg.jobs.lock().unwrap();
            let job = jobs.get_mut(&id).unwrap();
            job.last_activity = Instant::now() - TTL - Duration::from_secs(60);
        }
        reg.set_progress(&id, serde_json::json!({"stage": "llm"}));

        let _other = reg.create_job();
        assert_eq!(reg.get_job(&id).unwrap().status, JobStatus::Running);
    }

    #[test]
    fn stale_job_is_purged_when_a_new_job_is_created() {
        let reg = JobRegistry::new();
        let id = reg.create_job();
        {
            let mut jobs = reg.jobs.lock().unwrap();
            let job = jobs.get_mut(&id).unwrap();
            job.last_activity = Instant::now() - TTL - Duration::from_secs(60);
        }

        let _other = reg.create_job();
        assert!(reg.get_job(&id).is_none());
    }
}
