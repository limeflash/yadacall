use yc_core::{
    meeting::{Meeting, MeetingId, MeetingStatus},
    summary::Summary,
    transcript::Transcript,
};

/// Full meeting material read from the source-of-truth files.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetingRecord {
    /// Meeting metadata from `meeting.json`.
    pub meeting: Meeting,
    /// Transcript from `transcript.json`, or an empty transcript when absent.
    pub transcript: Transcript,
    /// Structured summary from `summary.json`, if present.
    pub summary: Option<Summary>,
    /// Folder component under the library root.
    pub folder: String,
}

/// Indexed meeting metadata for a library list.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetingListItem {
    /// Meeting identifier.
    pub id: MeetingId,
    /// User-facing title.
    pub title: String,
    /// RFC 3339 start timestamp.
    pub started_at: String,
    /// Meeting duration in milliseconds.
    pub duration_ms: u64,
    /// BCP-47-ish language hint.
    pub language: String,
    /// Folder component under the library root.
    pub folder: String,
    /// Indexed processing status.
    pub status: MeetingStatus,
    /// ASR model id, if present.
    pub asr_model: Option<String>,
    /// Last index update timestamp.
    pub updated_at: String,
}

/// Optional search filters. Date bounds are RFC 3339 timestamp strings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchFilters {
    /// Restrict results to this meeting.
    pub meeting_id: Option<MeetingId>,
    /// Restrict results to this language.
    pub language: Option<String>,
    /// Restrict results to a speaker key or display name.
    pub speaker: Option<String>,
    /// Inclusive lower bound for meeting start time as RFC 3339.
    pub started_after: Option<String>,
    /// Inclusive upper bound for meeting start time as RFC 3339.
    pub started_before: Option<String>,
}

/// Search result fused from a segment BM25 match or a semantic chunk match.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Meeting containing the result.
    pub meeting_id: MeetingId,
    /// Meeting title at index time.
    pub meeting_title: String,
    /// Meeting start timestamp as RFC 3339.
    pub started_at: String,
    /// Start of the matching segment or chunk.
    pub start_ms: u64,
    /// End of the matching segment or chunk.
    pub end_ms: u64,
    /// Speakers represented in the result span.
    pub speakers: Vec<String>,
    /// Original transcript text with query terms highlighted when possible.
    pub snippet: String,
    /// Reciprocal-rank fusion score; higher scores rank first.
    pub score: f64,
    /// Matching semantic chunk row id, if vector search contributed.
    pub chunk_id: Option<i64>,
    /// Matching transcript segment index, if BM25 search contributed.
    pub segment_idx: Option<u32>,
}

/// State of a queued background processing job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Waiting to be claimed.
    Queued,
    /// Claimed by a worker.
    Running,
    /// Successfully completed.
    Done,
    /// Failed without retry.
    Failed,
}

impl JobState {
    pub(crate) fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            _ => Err(rusqlite::Error::InvalidColumnType(
                0,
                "state".to_owned(),
                rusqlite::types::Type::Text,
            )),
        }
    }
}

/// A claimed or queued background job.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// Database job id.
    pub id: i64,
    /// Meeting associated with the job.
    pub meeting_id: MeetingId,
    /// Worker job kind.
    pub kind: String,
    /// Current queue state.
    pub state: JobState,
    /// Number of times the job has been claimed.
    pub attempts: u32,
    /// Most recent failure message, if any.
    pub error: Option<String>,
    /// JSON payload passed to the worker.
    pub payload: serde_json::Value,
    /// Job creation timestamp.
    pub created_at: String,
    /// Last state update timestamp.
    pub updated_at: String,
}

/// A task copied from a meeting's structured summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    /// Meeting that owns the task.
    pub meeting_id: MeetingId,
    /// Meeting title at index time.
    pub meeting_title: String,
    /// Zero-based task index in summary.json.
    pub idx: u32,
    /// Task description.
    pub text: String,
    /// Owner speaker key or free-form name.
    pub owner: Option<String>,
    /// Due date text, if present.
    pub due: Option<String>,
    /// Timestamp anchor in milliseconds, if present.
    pub at_ms: Option<u64>,
    /// Whether the user marked the task complete.
    pub done: bool,
}

/// Stored known-voice profile. `centroid` contains the model's packed float bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreVoice {
    /// Stable voice profile identifier.
    pub id: String,
    /// Display name shown in participant lists.
    pub name: String,
    /// Model centroid stored as packed float bytes.
    pub centroid: Vec<u8>,
    /// Number of samples represented by the centroid.
    pub samples: u64,
    /// Last profile update timestamp.
    pub updated_at: String,
}
