use serde::{Deserialize, Serialize};

use crate::ProviderSessionRef;

/// Most recent terminal lines kept for one captured transcript. This matches
/// the largest live terminal-output read Yard allows.
pub const MAX_TRANSCRIPT_LINES: usize = 10_000;
/// Byte cap for one captured transcript, applied after the line cap.
pub const MAX_TRANSCRIPT_BYTES: usize = 1_048_576;

/// Whether a retained transcript is available for an assignment or worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptStatus {
    /// A capture job is queued or retrying (for example while Herdr is
    /// unreachable).
    Pending,
    /// The terminal text was captured and is stored read-only.
    Captured,
    /// No text could be captured; see the reason and provider session.
    Unavailable,
}

/// Why no retained transcript text exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptUnavailableReason {
    /// No capture was ever queued (older work, or no runtime was bound).
    NotCaptured,
    /// The runtime was observed absent before its output could be read.
    RuntimeClosed,
    /// The captured runtime identity was reused or changed, so reading it
    /// could return another agent's output.
    RuntimeReused,
    /// The worker took new work before the capture ran, so its scrollback
    /// would include the next assignment.
    WorkerReallocated,
}

/// A read-only terminal transcript retained after an assignment ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerTranscript {
    pub worker_id: String,
    pub assignment_id: Option<String>,
    pub status: TranscriptStatus,
    pub unavailable_reason: Option<TranscriptUnavailableReason>,
    pub terminal_id: Option<String>,
    /// The provider's own session reference (for example a Codex rollout or
    /// Claude transcript), the durable fallback when no text was captured.
    pub provider_session: Option<ProviderSessionRef>,
    pub source: Option<String>,
    pub format: Option<String>,
    pub text: Option<String>,
    pub line_count: u32,
    pub byte_count: u64,
    pub truncated: bool,
    pub captured_at_unix_ms: Option<u64>,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub next_attempt_at_unix_ms: Option<u64>,
}

/// Terminal text reduced to the retained bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedTranscript {
    pub text: String,
    pub line_count: usize,
    pub byte_count: usize,
    pub truncated: bool,
}

/// Keep the most recent [`MAX_TRANSCRIPT_LINES`] lines of `text`, then drop
/// leading lines until it fits in [`MAX_TRANSCRIPT_BYTES`]. A single line that
/// is still too long keeps only its tail, cut on a character boundary.
#[must_use]
pub fn bound_transcript_text(text: &str) -> BoundedTranscript {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let mut start = lines.len().saturating_sub(MAX_TRANSCRIPT_LINES);
    let mut truncated = start > 0;
    let mut bytes = lines[start..].iter().map(|line| line.len()).sum::<usize>();
    while bytes > MAX_TRANSCRIPT_BYTES && start + 1 < lines.len() {
        bytes -= lines[start].len();
        start += 1;
        truncated = true;
    }
    let mut kept = lines[start..].concat();
    if kept.len() > MAX_TRANSCRIPT_BYTES {
        let mut cut = kept.len() - MAX_TRANSCRIPT_BYTES;
        while !kept.is_char_boundary(cut) {
            cut += 1;
        }
        kept = kept[cut..].to_owned();
        truncated = true;
    }
    BoundedTranscript {
        line_count: kept.split_inclusive('\n').count(),
        byte_count: kept.len(),
        text: kept,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_TRANSCRIPT_BYTES, MAX_TRANSCRIPT_LINES, bound_transcript_text};

    #[test]
    fn keeps_short_transcripts_unchanged() {
        let bounded = bound_transcript_text("one\ntwo\nthree");
        assert_eq!(bounded.text, "one\ntwo\nthree");
        assert_eq!(bounded.line_count, 3);
        assert_eq!(bounded.byte_count, 13);
        assert!(!bounded.truncated);

        let empty = bound_transcript_text("");
        assert_eq!(empty.line_count, 0);
        assert!(!empty.truncated);
    }

    #[test]
    fn keeps_only_the_most_recent_lines() {
        let text = (0..MAX_TRANSCRIPT_LINES + 5)
            .map(|index| format!("line {index}\n"))
            .collect::<Vec<_>>()
            .concat();
        let bounded = bound_transcript_text(&text);
        assert!(bounded.truncated);
        assert_eq!(bounded.line_count, MAX_TRANSCRIPT_LINES);
        assert!(bounded.text.starts_with("line 5\n"));
        assert!(
            bounded
                .text
                .ends_with(&format!("line {}\n", MAX_TRANSCRIPT_LINES + 4))
        );
    }

    #[test]
    fn applies_the_byte_cap_on_line_and_character_boundaries() {
        let wide = format!("{}\n", "x".repeat(400_000));
        let text = format!("{wide}{wide}{wide}tail\n");
        let bounded = bound_transcript_text(&text);
        assert!(bounded.truncated);
        assert!(bounded.byte_count <= MAX_TRANSCRIPT_BYTES);
        assert!(bounded.text.ends_with("tail\n"));
        assert_eq!(bounded.line_count, 3);

        let single = "é".repeat(MAX_TRANSCRIPT_BYTES);
        let bounded = bound_transcript_text(&single);
        assert!(bounded.truncated);
        assert!(bounded.byte_count <= MAX_TRANSCRIPT_BYTES);
        assert!(bounded.text.chars().all(|character| character == 'é'));
    }
}
