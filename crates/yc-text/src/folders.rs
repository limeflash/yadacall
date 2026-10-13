use chrono::{DateTime, Local, TimeZone};
use yc_core::meeting::Meeting;

const DEFAULT_FOLDER_LIMIT: usize = 80;

/// Build the canonical library folder name for a meeting, preserving Cyrillic characters.
pub fn meeting_folder_name(meeting: &Meeting) -> String {
    let local_started_at = meeting.started_at.with_timezone(&Local);
    meeting_folder_name_at(meeting, &local_started_at)
}

fn meeting_folder_name_at<Tz: TimeZone>(meeting: &Meeting, started_at: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let date = started_at.format("%Y-%m-%d %H-%M").to_string();
    let title = sanitize_folder_name(&meeting.title, 60);
    let base = if title.is_empty() { "Meeting" } else { &title };
    sanitize_folder_name(&format!("{date} {base}"), DEFAULT_FOLDER_LIMIT)
}

/// Remove platform-invalid characters and trim a folder component to a character limit.
pub fn sanitize_folder_name(input: &str, max_chars: usize) -> String {
    let mut cleaned = input
        .chars()
        .filter_map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => Some(' '),
            character if character.is_control() => None,
            character => Some(character),
        })
        .collect::<String>();
    cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned = cleaned.trim_end_matches(['.', ' ']).to_owned();
    let limit = max_chars.max(1);
    if cleaned.chars().count() > limit {
        cleaned = cleaned.chars().take(limit).collect::<String>();
        cleaned = cleaned.trim_end_matches(['.', ' ']).to_owned();
    }
    if is_reserved_name(&cleaned) {
        cleaned.insert(0, '_');
    }
    cleaned
}

fn is_reserved_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|number| {
                matches!(number, "¹" | "²" | "³")
                    || number.len() == 1
                        && number
                            .chars()
                            .next()
                            .is_some_and(|digit| matches!(digit, '1'..='9'))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone, Utc};
    use yc_core::meeting::{MeetingId, MeetingStatus};

    #[test]
    fn folder_name_uses_local_time_with_a_fixed_timezone() {
        let meeting = Meeting {
            id: MeetingId::new(),
            title: "NB Week".to_owned(),
            title_is_auto: false,
            started_at: Utc
                .with_ymd_and_hms(2026, 10, 13, 8, 16, 0)
                .single()
                .expect("valid time"),
            duration_ms: 0,
            language: "en".to_owned(),
            expected_speakers: None,
            expected_speakers_explicit: false,
            tracks: Vec::new(),
            bookmarks: Vec::new(),
            participants: Vec::new(),
            participant_hints: Vec::new(),
            asr_model: None,
            status: MeetingStatus::Ready,
        };
        let istanbul = FixedOffset::east_opt(3 * 60 * 60).expect("valid offset");
        let local_started_at = meeting.started_at.with_timezone(&istanbul);

        assert_eq!(
            meeting_folder_name_at(&meeting, &local_started_at),
            "2026-10-13 11-16 NB Week"
        );
    }
}
