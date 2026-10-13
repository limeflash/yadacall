use std::collections::HashSet;

use chrono::{DateTime, Local, TimeZone};
use yc_core::{
    meeting::{Meeting, Participant},
    summary::Summary,
    transcript::{SPEAKER_ME, SPEAKER_THEM, Segment},
};

/// Render transcript JSON data into the stable, human-readable `transcript.md` format.
pub fn render_transcript_markdown(meeting: &Meeting, segments: &[Segment]) -> String {
    let local_started_at = meeting.started_at.with_timezone(&Local);
    let names = speaker_names(meeting, segments);
    let mut used = Vec::new();
    let mut seen = HashSet::new();
    for segment in segments {
        let name = speaker_name(meeting, &segment.speaker);
        if seen.insert(name.clone()) {
            used.push(name);
        }
    }
    if used.is_empty() {
        used.extend(
            meeting
                .participants
                .iter()
                .map(|participant| participant.name.clone()),
        );
    }
    let speaker_section = if used.is_empty() {
        "speakers: []\n".to_owned()
    } else {
        format!(
            "speakers:\n{}",
            used.iter()
                .map(|name| format!("  - {}\n", yaml_quote(name)))
                .collect::<String>()
        )
    };
    let mut output = format!(
        "---\ntitle: {}\ndate: {}\nduration: {}\n{}language: {}\nmodel: {}\n---\n\n",
        yaml_quote(&meeting.title),
        yaml_quote(&format_meeting_date(&local_started_at)),
        yaml_quote(&format_duration(meeting.duration_ms)),
        speaker_section,
        yaml_quote(&meeting.language),
        yaml_quote(meeting.asr_model.as_deref().unwrap_or("unknown")),
    );

    enum Entry<'a> {
        Segment(&'a Segment),
        Bookmark(&'a yc_core::meeting::Bookmark),
    }
    let mut entries = segments
        .iter()
        .map(Entry::Segment)
        .chain(meeting.bookmarks.iter().map(Entry::Bookmark))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| match entry {
        Entry::Segment(segment) => (segment.start_ms, 1_u8),
        Entry::Bookmark(bookmark) => (bookmark.at_ms, 0_u8),
    });
    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            Entry::Segment(segment) => lines.push(format!(
                "**[{}] {}:** {}",
                timestamp(segment.start_ms),
                names
                    .get(&segment.speaker)
                    .map(String::as_str)
                    .unwrap_or("Собеседник"),
                segment.text.trim(),
            )),
            Entry::Bookmark(bookmark) => lines.push(format!(
                "> 🔖 {} — {}",
                minute_timestamp(bookmark.at_ms),
                bookmark.label.as_deref().unwrap_or_default(),
            )),
        }
    }
    output.push_str(&lines.join("\n\n"));
    output.push('\n');
    output
}

fn format_meeting_date<Tz: TimeZone>(started_at: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    started_at.format("%Y-%m-%d %H:%M").to_string()
}

/// Render a structured summary in Russian with anchored timestamps.
pub fn render_summary_markdown(summary: &Summary) -> String {
    let mut output = String::new();
    output.push_str("# О чём встреча\n\n");
    if summary.about.is_empty() {
        output.push_str("_Нет данных._\n");
    } else {
        output.push_str(&summary.about.join("\n\n"));
        output.push('\n');
    }
    output.push_str("\n## Договорённости\n\n");
    if summary.decisions.is_empty() {
        output.push_str("_Нет данных._\n");
    } else {
        for decision in &summary.decisions {
            output.push_str(&format!(
                "- {}{}\n",
                decision
                    .at_ms
                    .map(|time| format!("[{}] ", short_timestamp(time)))
                    .unwrap_or_default(),
                decision.text
            ));
        }
    }
    output.push_str("\n## Задачи\n\n");
    if summary.tasks.is_empty() {
        output.push_str("_Нет данных._\n");
    } else {
        for task in &summary.tasks {
            let mut details = Vec::new();
            if let Some(owner) = &task.owner {
                details.push(format!("ответственный: {owner}"));
            }
            if let Some(due) = &task.due {
                details.push(format!("срок: {due}"));
            }
            let suffix = if details.is_empty() {
                String::new()
            } else {
                format!(" — {}", details.join(", "))
            };
            output.push_str(&format!(
                "- {}[{}] {}{}\n",
                task.at_ms
                    .map(|time| format!("[{}] ", short_timestamp(time)))
                    .unwrap_or_default(),
                if task.done { 'x' } else { ' ' },
                task.text,
                suffix,
            ));
        }
    }
    output.push_str("\n## Открытые вопросы\n\n");
    if summary.open_questions.is_empty() {
        output.push_str("_Нет данных._\n");
    } else {
        for question in &summary.open_questions {
            output.push_str(&format!("- {question}\n"));
        }
    }
    output
}

/// Add a concise instruction before the transcript for pasting into Claude.
pub fn copy_for_claude(meeting: &Meeting, segments: &[Segment]) -> String {
    format!(
        "Проанализируй эту расшифровку встречи: выдели итоги, договорённости и задачи.\n\n{}",
        render_transcript_markdown(meeting, segments)
    )
}

fn speaker_names(
    meeting: &Meeting,
    segments: &[Segment],
) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    for participant in &meeting.participants {
        names.insert(participant.key.clone(), participant.name.clone());
    }
    for segment in segments {
        names
            .entry(segment.speaker.clone())
            .or_insert_with(|| speaker_name(meeting, &segment.speaker));
    }
    names
}

fn speaker_name(meeting: &Meeting, key: &str) -> String {
    let english_ui = meeting
        .language
        .split('-')
        .next()
        .is_some_and(|language| language.eq_ignore_ascii_case("en"));
    meeting
        .participants
        .iter()
        .find(|participant: &&Participant| participant.key == key)
        .map(|participant| participant.name.clone())
        .unwrap_or_else(|| match (key, english_ui) {
            (SPEAKER_ME, true) => "Me".to_owned(),
            (SPEAKER_THEM, true) => "Them".to_owned(),
            (SPEAKER_ME, false) => "Я".to_owned(),
            (SPEAKER_THEM, false) => "Собеседник".to_owned(),
            _ => key.to_owned(),
        })
}

fn yaml_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

fn format_duration(duration_ms: u64) -> String {
    let seconds = duration_ms / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

fn timestamp(at_ms: u64) -> String {
    let seconds = at_ms / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

fn short_timestamp(at_ms: u64) -> String {
    let seconds = at_ms / 1_000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn minute_timestamp(at_ms: u64) -> String {
    short_timestamp(at_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone, Utc};

    #[test]
    fn transcript_date_formats_the_supplied_local_timezone() {
        let started_at = Utc
            .with_ymd_and_hms(2026, 10, 13, 8, 16, 0)
            .single()
            .expect("valid time");
        let istanbul = FixedOffset::east_opt(3 * 60 * 60).expect("valid offset");
        let local_started_at = started_at.with_timezone(&istanbul);

        assert_eq!(format_meeting_date(&local_started_at), "2026-10-13 11:16");
    }
}
