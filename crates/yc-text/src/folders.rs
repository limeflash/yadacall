use yc_core::meeting::Meeting;

const DEFAULT_FOLDER_LIMIT: usize = 80;

/// Build the canonical library folder name for a meeting, preserving Cyrillic characters.
pub fn meeting_folder_name(meeting: &Meeting) -> String {
    let date = meeting.started_at.format("%Y-%m-%d %H-%M").to_string();
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
