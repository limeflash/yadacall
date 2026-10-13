//! Pure transcript transformations, formatting, chunking, and text normalization.

mod filters;
mod folders;
mod merge;
mod normalize;
mod render;
mod search_chunks;
mod terms;

pub use filters::{
    DroppedEcho, EchoFilterConfig, EchoFilterResult, FilteredTranscript, HallucinationConfig,
    HallucinationDrop, HallucinationReason, filter_echo_duplicates, filter_hallucinations,
};
pub use folders::{meeting_folder_name, sanitize_folder_name};
pub use merge::{build_turns, merge_tracks};
pub use normalize::{normalize_tokens, stem_text};
pub use render::{copy_for_claude, render_summary_markdown, render_transcript_markdown};
pub use search_chunks::{SearchChunk, chunk_turns};
pub use terms::TermDictionary;

#[cfg(test)]
mod tests;
