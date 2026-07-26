//! Port of `services/jobs.py`: a generic in-memory async-job registry
//! (used today by keyword clustering, `/keywords/cluster/start`). Jobs
//! expire after 600s if never polled; `get_job` on a finished job
//! returns it once and then removes it — matching Python's one-shot
//! poll-then-gone semantics exactly.

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
}

impl JobSnapshot {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "status": self.status.as_str(),
            "result": self.result,
            "error": self.error,
        })
    }
}

struct JobEntry {
    status: JobStatus,
    result: Option<Value>,
    error: Option<String>,
    created_at: Instant,
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
        jobs.retain(|_, j| now.duration_since(j.created_at) <= TTL);
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
                created_at: Instant::now(),
            },
        );
        job_id
    }

    pub fn complete_job(&self, job_id: &str, result: Value) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = JobStatus::Done;
            job.result = Some(result);
        }
    }

    pub fn fail_job(&self, job_id: &str, error: String) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = JobStatus::Error;
            job.error = Some(error);
        }
    }

    /// Matches Python: a finished job (done/error) is removed from the
    /// registry the moment it's read, so it can only be polled once.
    pub fn get_job(&self, job_id: &str) -> Option<JobSnapshot> {
        let mut jobs = self.jobs.lock().unwrap();
        let job = jobs.get(job_id)?;
        let snapshot = JobSnapshot {
            status: job.status.clone(),
            result: job.result.clone(),
            error: job.error.clone(),
        };
        if matches!(snapshot.status, JobStatus::Done | JobStatus::Error) {
            jobs.remove(job_id);
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
}
