//! The job runner: claims queued jobs from the store one at a time, runs them, retries retryable
//! failures with backoff and keeps the meeting's status in step.
//!
//! All stages are serialised on one lane. They are model-heavy (diarise, summarise, embed) or must
//! not overtake a stage of the same meeting that is still waiting (archive deletes the audio the
//! diarise stage reads), and a single lane gives both guarantees; throughput is dominated by the
//! models, not by the scheduler.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yc_core::events::EngineEvent;
use yc_core::meeting::{MeetingId, MeetingStatus, Stage};
use yc_core::{Error, Result};
use yc_store::{Job, MeetingRecord};

use crate::engine::Engine;

/// Kinds the runner handles, in no particular order.
pub const JOB_KINDS: &[&str] = &["transcribe", "diarize", "summarize", "archive", "index"];

/// Runner tuning.
#[derive(Debug, Clone)]
pub struct JobConfig {
    /// How often an idle runner looks for work even without a wake-up.
    pub poll: Duration,
    /// A retryable failure is retried until the job was claimed this many times.
    pub max_attempts: u32,
    /// First retry delay; doubles each attempt (capped at 5 minutes).
    pub backoff_base: Duration,
}

impl Default for JobConfig {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(2),
            max_attempts: 3,
            backoff_base: Duration::from_secs(2),
        }
    }
}

/// Payload of every job row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobPayload {
    /// The kinds of the pipeline this job belongs to, in order.
    pub plan: Vec<String>,
    /// Index of this job in `plan`.
    pub step: u32,
    /// A bare re-embed after a user edit: runs silently, status untouched.
    pub reindex_only: bool,
    /// Title that was generated at recording start; a summary may replace it while unchanged.
    pub auto_title: Option<String>,
}

/// UI stage a job kind belongs to.
pub fn kind_stage(kind: &str) -> Stage {
    match kind {
        "transcribe" => Stage::Transcribe,
        "diarize" => Stage::Diarize,
        "summarize" => Stage::Summarize,
        _ => Stage::Index,
    }
}

pub(crate) fn spawn_runner(engine: &Arc<Engine>) {
    let weak = Arc::downgrade(engine);
    let notify = Arc::clone(&engine.jobs_notify);
    let token = engine.shutdown.clone();
    let poll = engine.cfg.jobs.poll;
    tokio::spawn(async move {
        loop {
            let Some(engine) = weak.upgrade() else { return };
            if token.is_cancelled() {
                return;
            }
            let worked = match engine.run_next_job().await {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("job runner: {e}");
                    false
                }
            };
            drop(engine);
            if worked {
                continue;
            }
            tokio::select! {
                _ = token.cancelled() => return,
                _ = notify.notified() => {}
                _ = tokio::time::sleep(poll) => {}
            }
        }
    });
}

impl Engine {
    /// Claims and runs one job. `Ok(false)` when the queue is empty.
    async fn run_next_job(self: &Arc<Self>) -> Result<bool> {
        let Some(job) = self.blocking(|e| e.store.claim_next(JOB_KINDS)).await? else {
            return Ok(false);
        };
        let payload: JobPayload = serde_json::from_value(job.payload.clone()).unwrap_or_default();
        let stage = kind_stage(&job.kind);
        if !payload.reindex_only {
            self.emit(EngineEvent::Status {
                meeting: job.meeting_id,
                status: MeetingStatus::Processing {
                    stage,
                    progress: progress_of(&payload, 0.0),
                },
            });
        }
        let outcome = self.execute_job(&job, &payload).await;
        let id = job.meeting_id;
        match outcome {
            Ok(()) => {
                let job_id = job.id;
                self.blocking(move |e| e.store.complete(job_id)).await?;
                self.job_finished(id);
                // A re-transcription re-runs finalisation, which set the status itself.
                if job.kind != "transcribe"
                    && let Err(e) = self.settle_status(id, &payload).await
                {
                    tracing::warn!("status update for {id} failed: {e}");
                }
            }
            Err(Error::NotFound(what)) => {
                tracing::info!("job {} for {id} dropped, not found: {what}", job.id);
                let job_id = job.id;
                self.blocking(move |e| e.store.complete(job_id)).await?;
                self.job_finished(id);
            }
            Err(e) if e.is_retryable() && job.attempts < self.cfg.jobs.max_attempts => {
                tracing::warn!("job {} ({}) failed, will retry: {e}", job.id, job.kind);
                let delay = backoff(self.cfg.jobs.backoff_base, job.attempts);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = self.shutdown.cancelled() => {}
                }
                let (job_id, msg) = (job.id, e.to_string());
                self.blocking(move |en| en.store.fail(job_id, &msg, true))
                    .await?;
                self.jobs_notify.notify_one();
            }
            Err(e) => {
                tracing::error!("job {} ({}) failed: {e}", job.id, job.kind);
                let (job_id, msg) = (job.id, e.to_string());
                self.blocking(move |en| en.store.fail(job_id, &msg, false))
                    .await?;
                self.job_finished(id);
                if !payload.reindex_only {
                    self.fail_meeting(id, stage, &e).await;
                }
            }
        }
        Ok(true)
    }

    fn job_finished(&self, id: MeetingId) {
        self.mark_outstanding(id, -1);
        if let Ok(mut set) = self.pending_reindex.lock() {
            set.remove(&id);
        }
    }

    async fn execute_job(self: &Arc<Self>, job: &Job, payload: &JobPayload) -> Result<()> {
        let (job, payload) = (job.clone(), payload.clone());
        match job.kind.as_str() {
            "summarize" => self.run_summarize(&job, &payload).await,
            "diarize" => self.blocking(move |e| e.run_diarize(&job, &payload)).await,
            "archive" => self.blocking(move |e| e.run_archive(&job, &payload)).await,
            "index" => self.blocking(move |e| e.run_index(&job, &payload)).await,
            "transcribe" => {
                self.blocking(move |e| e.run_retranscribe(&job, &payload))
                    .await
            }
            other => Err(Error::Invalid(format!("unknown job kind `{other}`"))),
        }
    }

    /// Status the meeting should have once the job described by `payload` succeeded.
    pub(crate) fn next_status(current: &MeetingStatus, payload: &JobPayload) -> MeetingStatus {
        if payload.reindex_only || matches!(current, MeetingStatus::Failed { .. }) {
            return current.clone();
        }
        let next = payload.step as usize + 1;
        match payload.plan.get(next) {
            None => MeetingStatus::Ready,
            Some(kind) => MeetingStatus::Processing {
                stage: kind_stage(kind),
                progress: next as f32 / payload.plan.len().max(1) as f32,
            },
        }
    }

    /// Persists the post-job status if the handler did not already.
    pub(crate) async fn settle_status(
        self: &Arc<Self>,
        id: MeetingId,
        payload: &JobPayload,
    ) -> Result<()> {
        let payload = payload.clone();
        self.blocking(move |e| e.settle_status_blocking(id, &payload))
            .await
    }

    pub(crate) fn settle_status_blocking(&self, id: MeetingId, payload: &JobPayload) -> Result<()> {
        self.mutate_meeting(id, |rec| {
            let next = Self::next_status(&rec.meeting.status, payload);
            let changed = next != rec.meeting.status;
            rec.meeting.status = next;
            Ok(((), changed))
        })
    }

    async fn fail_meeting(self: &Arc<Self>, id: MeetingId, stage: Stage, error: &Error) {
        let (message, retryable) = (error.to_string(), error.is_retryable());
        let r = self
            .blocking(move |e| {
                e.mutate_meeting(id, |rec| {
                    rec.meeting.status = MeetingStatus::Failed {
                        stage,
                        message,
                        retryable,
                    };
                    Ok(((), true))
                })
            })
            .await;
        if let Err(e) = r {
            tracing::warn!("could not record the failure of {id}: {e}");
        }
    }

    /// Reads a meeting, lets `f` change it and saves it if `f` says so (under the write lock, so
    /// concurrent edits cannot interleave). Emits a `Status` event when the status changed.
    pub(crate) fn mutate_meeting<T>(
        &self,
        id: MeetingId,
        f: impl FnOnce(&mut MeetingRecord) -> Result<(T, bool)>,
    ) -> Result<T> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| Error::Storage("write lock poisoned".into()))?;
        let mut rec = self.store.get_meeting(id)?;
        let before = rec.meeting.status.clone();
        let (value, changed) = f(&mut rec)?;
        if changed {
            self.store
                .save_meeting(&rec.meeting, &rec.transcript, rec.summary.as_ref())?;
            if rec.meeting.status != before {
                self.emit(EngineEvent::Status {
                    meeting: id,
                    status: rec.meeting.status.clone(),
                });
            }
        }
        Ok(value)
    }

    /// Queues a re-embed of `id` unless one is already waiting. Call after any save that rewrote
    /// the index rows (they drop the meeting's vectors).
    pub(crate) fn schedule_reindex(&self, id: MeetingId) {
        if !self.ports.text_embedder.is_available() {
            return;
        }
        let first = self
            .pending_reindex
            .lock()
            .map(|mut s| s.insert(id))
            .unwrap_or(true);
        if first {
            let payload = JobPayload {
                plan: vec!["index".into()],
                step: 0,
                reindex_only: true,
                auto_title: None,
            };
            if let Err(e) = self.enqueue_job(id, "index", &payload) {
                tracing::warn!("could not queue re-index for {id}: {e}");
            }
        }
    }

    /// Re-runs a failed stage (and the index behind it).
    pub async fn retry_failed(self: &Arc<Self>, id: MeetingId) -> Result<()> {
        let rec = self.blocking(move |e| e.store.get_meeting(id)).await?;
        let MeetingStatus::Failed { stage, .. } = rec.meeting.status else {
            return Err(Error::Invalid("the meeting has not failed".into()));
        };
        let kind = match stage {
            Stage::Transcribe => "transcribe",
            Stage::Diarize => "diarize",
            Stage::Summarize => "summarize",
            Stage::Index => "index",
        };
        self.queue_stage(id, kind).await
    }

    /// Recomputes the summary with the current provider.
    pub async fn regenerate_summary(self: &Arc<Self>, id: MeetingId) -> Result<()> {
        self.blocking(move |e| e.store.get_meeting(id)).await?;
        if !self
            .ports
            .summarizer
            .as_ref()
            .is_some_and(|s| s.is_configured())
        {
            return Err(Error::Invalid("no summary provider is configured".into()));
        }
        self.queue_stage(id, "summarize").await
    }

    async fn queue_stage(self: &Arc<Self>, id: MeetingId, kind: &'static str) -> Result<()> {
        let mut plan = vec![kind.to_owned()];
        if kind != "index" {
            plan.push("index".to_owned());
        }
        if kind == "transcribe" {
            plan.truncate(1);
        }
        let stage = kind_stage(kind);
        self.blocking(move |e| {
            e.mutate_meeting(id, |rec| {
                rec.meeting.status = MeetingStatus::Processing {
                    stage,
                    progress: 0.0,
                };
                Ok(((), true))
            })?;
            for (step, k) in plan.iter().enumerate() {
                let payload = JobPayload {
                    plan: plan.clone(),
                    step: step as u32,
                    reindex_only: false,
                    auto_title: None,
                };
                e.enqueue_job(id, k, &payload)?;
            }
            Ok(())
        })
        .await
    }
}

fn progress_of(payload: &JobPayload, within: f32) -> f32 {
    (payload.step as f32 + within) / payload.plan.len().max(1) as f32
}

pub(crate) fn backoff(base: Duration, attempts: u32) -> Duration {
    let factor = 1u32 << attempts.saturating_sub(1).min(8);
    (base * factor).min(Duration::from_secs(300))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let b = Duration::from_secs(2);
        assert_eq!(backoff(b, 1), Duration::from_secs(2));
        assert_eq!(backoff(b, 2), Duration::from_secs(4));
        assert_eq!(backoff(b, 3), Duration::from_secs(8));
        assert_eq!(backoff(b, 30), Duration::from_secs(300));
    }

    #[test]
    fn next_status_walks_the_plan_and_keeps_failures() {
        let payload = |step| JobPayload {
            plan: vec!["diarize".into(), "archive".into(), "index".into()],
            step,
            ..Default::default()
        };
        let p = MeetingStatus::Processing {
            stage: Stage::Diarize,
            progress: 0.0,
        };
        assert!(matches!(
            Engine::next_status(&p, &payload(0)),
            MeetingStatus::Processing {
                stage: Stage::Index,
                ..
            }
        ));
        assert_eq!(Engine::next_status(&p, &payload(2)), MeetingStatus::Ready);
        let failed = MeetingStatus::Failed {
            stage: Stage::Diarize,
            message: "x".into(),
            retryable: false,
        };
        assert_eq!(Engine::next_status(&failed, &payload(2)), failed);
        let mut re = payload(0);
        re.reindex_only = true;
        assert_eq!(
            Engine::next_status(&MeetingStatus::Ready, &re),
            MeetingStatus::Ready
        );
    }
}
