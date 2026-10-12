//! Matching diarised speakers to calendar invitees.

use yc_core::meeting::ParticipantHint;

use crate::diarize::{KnownVoice, cosine, match_voices};

/// Identity and invite suggestions for one diarisation cluster.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpeakerResolution {
    pub voice_index: Option<usize>,
    pub hint_index: Option<usize>,
    pub suggestions: Vec<String>,
}

/// Uses voice evidence first, then assigns the sole remaining invitee to the sole unnamed cluster.
/// Suggestions are ordered by how well a matched voice's saved name relates to each invitee.
pub(crate) fn resolve_speakers(
    centroids: &[Vec<f32>],
    voices: &[KnownVoice],
    hints: &[ParticipantHint],
    min_voice_similarity: f32,
) -> Vec<SpeakerResolution> {
    let voice_matches = match_voices(centroids, voices, min_voice_similarity);
    let mut hint_matches = vec![None; centroids.len()];
    let mut used_hints = vec![false; hints.len()];

    let mut edges = Vec::new();
    for (cluster, voice_index) in voice_matches.iter().enumerate() {
        let Some(voice_index) = voice_index else {
            continue;
        };
        let voice = &voices[*voice_index];
        let voice_similarity = cosine(&centroids[cluster], &voice.centroid);
        for (hint_index, hint) in hints.iter().enumerate() {
            if let Some(name_score) = voice_hint_match_score(voice, hint) {
                edges.push((name_score, voice_similarity, cluster, hint_index));
            }
        }
    }
    // A stronger name relation wins first. Voice similarity breaks conflicts between saved
    // profiles that both resemble one invitee; all ties stay deterministic by cluster and invitee.
    edges.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.total_cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.cmp(&b.3))
    });
    for (_, _, cluster, hint_index) in edges {
        if hint_matches[cluster].is_none() && !used_hints[hint_index] {
            hint_matches[cluster] = Some(hint_index);
            used_hints[hint_index] = true;
        }
    }

    let remaining_clusters: Vec<usize> = (0..centroids.len())
        .filter(|&cluster| hint_matches[cluster].is_none())
        .collect();
    let remaining_hints: Vec<usize> = (0..hints.len()).filter(|&hint| !used_hints[hint]).collect();
    if let ([cluster], [hint_index]) = (remaining_clusters.as_slice(), remaining_hints.as_slice()) {
        hint_matches[*cluster] = Some(*hint_index);
        used_hints[*hint_index] = true;
    }

    centroids
        .iter()
        .enumerate()
        .map(|(cluster, _)| {
            let suggestions = if hint_matches[cluster].is_some() {
                Vec::new()
            } else {
                ranked_suggestions(voice_matches[cluster], voices, hints, &used_hints)
            };
            SpeakerResolution {
                voice_index: voice_matches[cluster],
                hint_index: hint_matches[cluster],
                suggestions,
            }
        })
        .collect()
}

fn ranked_suggestions(
    voice_index: Option<usize>,
    voices: &[KnownVoice],
    hints: &[ParticipantHint],
    used_hints: &[bool],
) -> Vec<String> {
    let voice = voice_index.map(|index| &voices[index]);
    let mut ranked: Vec<(usize, u8)> = hints
        .iter()
        .enumerate()
        .filter(|(index, _)| !used_hints[*index])
        .map(|(index, hint)| {
            (
                index,
                voice.map_or(0, |voice| suggestion_score(voice, hint)),
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut names = Vec::new();
    for (index, _) in ranked {
        let name = hints[index].name.trim();
        if !name.is_empty()
            && !names
                .iter()
                .any(|existing: &String| same_name(existing, name))
        {
            names.push(name.to_owned());
        }
    }
    names
}

fn voice_hint_match_score(voice: &KnownVoice, hint: &ParticipantHint) -> Option<u8> {
    let hint_email = hint.email.as_deref().map(normalize_email);
    if hint_email.as_ref().is_some_and(|email| {
        voice
            .emails
            .iter()
            .any(|saved| normalize_email(saved) == email.as_str())
    }) {
        return Some(120);
    }

    let mut score = strong_name_score(&voice.name, &hint.name);
    if let Some(local) = hint.email.as_deref().and_then(email_local_part) {
        score = score.max(strong_name_score(&voice.name, &local));
    }
    score
}

fn suggestion_score(voice: &KnownVoice, hint: &ParticipantHint) -> u8 {
    if hint.email.as_deref().is_some_and(|email| {
        voice
            .emails
            .iter()
            .any(|saved| normalize_email(saved) == normalize_email(email))
    }) {
        return 120;
    }
    let mut score = fuzzy_name_score(&voice.name, &hint.name);
    if let Some(local) = hint.email.as_deref().and_then(email_local_part) {
        score = score.max(fuzzy_name_score(&voice.name, &local));
    }
    score
}

fn strong_name_score(left: &str, right: &str) -> Option<u8> {
    let left_forms = name_forms(left);
    let right_forms = name_forms(right);
    if left_forms.iter().any(|left| right_forms.contains(left)) {
        return Some(110);
    }
    let mut best = None;
    for left in &left_forms {
        for right in &right_forms {
            let left_tokens: Vec<&str> = left.split_whitespace().collect();
            let right_tokens: Vec<&str> = right.split_whitespace().collect();
            if initials_match(&left_tokens, &right_tokens) {
                best = best.max(Some(100));
            }
            if token_names_match(&left_tokens, &right_tokens) {
                best = best.max(Some(88));
            }
            if nickname_name_match(&left_tokens, &right_tokens) {
                best = best.max(Some(82));
            }
        }
    }
    best
}

fn fuzzy_name_score(left: &str, right: &str) -> u8 {
    if let Some(score) = strong_name_score(left, right) {
        return score;
    }
    let left_forms = name_forms(left);
    let right_forms = name_forms(right);
    let mut best = 0;
    for left in &left_forms {
        for right in &right_forms {
            let left_tokens: Vec<&str> = left.split_whitespace().collect();
            let right_tokens: Vec<&str> = right.split_whitespace().collect();
            for a in &left_tokens {
                for b in &right_tokens {
                    let aliases = aliases(a);
                    if aliases.contains(b) {
                        best = best.max(68);
                    }
                    let similarity = token_similarity(a, b);
                    best = best.max(similarity);
                }
            }
        }
    }
    best
}

fn token_names_match(left: &[&str], right: &[&str]) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let (short, long) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    if short.len() == 1 {
        return long.iter().any(|token| equivalent_token(short[0], token));
    }
    let mut cursor = 0;
    for token in short {
        let Some(offset) = long[cursor..]
            .iter()
            .position(|candidate| equivalent_token(token, candidate))
        else {
            return false;
        };
        cursor += offset + 1;
    }
    true
}

fn nickname_name_match(left: &[&str], right: &[&str]) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let (short, long) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    if short.len() == 1 {
        return long
            .first()
            .is_some_and(|candidate| nickname_equivalent(short[0], candidate));
    }
    let mut cursor = 0;
    for token in short {
        let Some(offset) = long[cursor..]
            .iter()
            .position(|candidate| nickname_equivalent(token, candidate))
        else {
            return false;
        };
        cursor += offset + 1;
    }
    true
}

fn initials_match(left: &[&str], right: &[&str]) -> bool {
    if left.len() < 2 || right.len() < 2 {
        return false;
    }
    let (short, long) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    let mut cursor = 0;
    for token in short {
        let Some(offset) = long[cursor..].iter().position(|candidate| {
            equivalent_token(token, candidate)
                || (token.chars().count() == 1 && candidate.starts_with(token))
        }) else {
            return false;
        };
        cursor += offset + 1;
    }
    true
}

fn equivalent_token(left: &str, right: &str) -> bool {
    name_forms(left)
        .iter()
        .any(|left| name_forms(right).contains(left))
}

fn nickname_equivalent(left: &str, right: &str) -> bool {
    equivalent_token(left, right)
        || aliases(left)
            .iter()
            .any(|alias| equivalent_token(alias, right))
}

fn aliases(token: &str) -> Vec<&'static str> {
    match token {
        "аня" | "anya" => vec!["анна", "anna"],
        "маша" | "masha" => vec!["мария", "maria"],
        "катя" | "katya" => vec!["екатерина", "ekaterina"],
        "дима" | "dima" => vec!["дмитрий", "dmitry"],
        "леша" | "lesha" => vec!["алексей", "alexey"],
        _ => Vec::new(),
    }
}

fn token_similarity(left: &str, right: &str) -> u8 {
    let left_len = left.chars().count();
    let right_len = right.chars().count();
    let max_len = left_len.max(right_len);
    if max_len == 0 {
        return 0;
    }
    let distance = levenshtein(left, right);
    if distance > max_len / 2 {
        return 0;
    }
    (55 + 35 * (max_len - distance) / max_len) as u8
}

fn levenshtein(left: &str, right: &str) -> usize {
    let right_chars: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right_chars.len()).collect();
    for (i, left_char) in left.chars().enumerate() {
        let mut current = vec![i + 1; right_chars.len() + 1];
        for (j, right_char) in right_chars.iter().enumerate() {
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + usize::from(left_char != *right_char));
        }
        previous = current;
    }
    previous[right_chars.len()]
}

fn name_forms(name: &str) -> Vec<String> {
    let mut forms = Vec::new();
    for yo_as in ["е", "yo"] {
        let mut form = String::new();
        let mut space = true;
        for c in name.chars().flat_map(char::to_lowercase) {
            if c == 'ё' {
                form.push_str(yo_as);
                space = false;
            } else if c.is_alphanumeric() {
                form.push(c);
                space = false;
            } else if !space {
                form.push(' ');
                space = true;
            }
        }
        if form.ends_with(' ') {
            form.pop();
        }
        if !form.is_empty() && !forms.contains(&form) {
            forms.push(form.clone());
        }
        let transliterated = transliterate(&form);
        if !transliterated.is_empty() && !forms.contains(&transliterated) {
            forms.push(transliterated);
        }
    }
    forms
}

fn transliterate(text: &str) -> String {
    let mut result = String::new();
    for c in text.chars() {
        result.push_str(match c {
            'а' => "a",
            'б' => "b",
            'в' => "v",
            'г' => "g",
            'д' => "d",
            'е' | 'э' => "e",
            'ё' => "yo",
            'ж' => "zh",
            'з' => "z",
            'и' => "i",
            'й' => "y",
            'к' => "k",
            'л' => "l",
            'м' => "m",
            'н' => "n",
            'о' => "o",
            'п' => "p",
            'р' => "r",
            'с' => "s",
            'т' => "t",
            'у' => "u",
            'ф' => "f",
            'х' => "kh",
            'ц' => "ts",
            'ч' => "ch",
            'ш' => "sh",
            'щ' => "shch",
            'ъ' | 'ь' => "",
            'ы' => "y",
            'ю' => "yu",
            'я' => "ya",
            _ => {
                result.push(c);
                continue;
            }
        });
    }
    result
}

fn same_name(left: &str, right: &str) -> bool {
    name_forms(left)
        .iter()
        .any(|left| name_forms(right).contains(left))
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

pub(crate) fn hint_email_for_name(name: &str, hints: &[ParticipantHint]) -> Option<String> {
    let mut emails: Vec<String> = hints
        .iter()
        .filter(|hint| same_name(name, &hint.name))
        .filter_map(|hint| hint.email.as_deref())
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(str::to_owned)
        .collect();
    emails.sort_by_key(|email| normalize_email(email));
    emails.dedup_by(|left, right| normalize_email(left) == normalize_email(right));
    (emails.len() == 1).then(|| emails.remove(0))
}

fn email_local_part(email: &str) -> Option<String> {
    let local = email.trim().split_once('@')?.0.split('+').next()?.trim();
    let normalized = name_forms(local).into_iter().next()?;
    (!normalized.is_empty()).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(name: &str, email: Option<&str>) -> ParticipantHint {
        ParticipantHint {
            name: name.to_owned(),
            email: email.map(str::to_owned),
        }
    }

    fn voice(name: &str, email: Option<&str>, centroid: &[f32]) -> KnownVoice {
        KnownVoice {
            id: name.to_owned(),
            name: name.to_owned(),
            emails: email.into_iter().map(str::to_owned).collect(),
            centroid: centroid.to_vec(),
        }
    }

    #[test]
    fn one_cluster_and_one_hint_are_assigned() {
        let resolved = resolve_speakers(&[vec![1.0, 0.0]], &[], &[hint("Alice", None)], 0.7);

        assert_eq!(resolved[0].hint_index, Some(0));
        assert!(resolved[0].suggestions.is_empty());
    }

    #[test]
    fn three_clusters_match_two_known_voices_then_assign_the_last_hint() {
        let centroids = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![-1.0, 0.0]];
        let voices = vec![
            voice("Anna", None, &[1.0, 0.0]),
            voice("Boris", None, &[0.0, 1.0]),
        ];
        let hints = [
            hint("Анна Ковалёва", Some("anna.k@example.com")),
            hint("Boris B.", None),
            hint("Carol", None),
        ];

        let resolved = resolve_speakers(&centroids, &voices, &hints, 0.7);

        assert_eq!(resolved[0].hint_index, Some(0));
        assert_eq!(resolved[1].hint_index, Some(1));
        assert_eq!(resolved[2].hint_index, Some(2));
        assert_eq!(resolved[0].voice_index, Some(0));
        assert_eq!(resolved[1].voice_index, Some(1));
    }

    #[test]
    fn more_clusters_than_hints_only_adds_suggestions() {
        let centroids = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![-1.0, 0.0]];
        let hints = [hint("Анна Ковалёва", None)];

        let resolved = resolve_speakers(&centroids, &[], &hints, 0.7);

        assert!(resolved.iter().all(|speaker| speaker.hint_index.is_none()));
        assert!(resolved.iter().all(|speaker| {
            speaker.suggestions == vec!["Анна Ковалёва".to_owned()]
        }));
    }

    #[test]
    fn conflicting_voice_matches_never_reuse_one_hint() {
        let centroids = vec![vec![1.0, 0.05], vec![0.8, 0.6]];
        let voices = vec![
            voice("Anna", None, &[1.0, 0.0]),
            voice("Anya", None, &[0.8, 0.6]),
        ];
        let hints = [hint("Анна Ковалёва", None)];

        let resolved = resolve_speakers(&centroids, &voices, &hints, 0.7);

        assert_eq!(
            resolved.iter().filter(|s| s.hint_index.is_some()).count(),
            1
        );
        assert_eq!(resolved[0].hint_index, Some(0));
        assert_eq!(resolved[1].hint_index, None);
    }

    #[test]
    fn russian_english_case_yo_initials_and_email_are_fuzzy_voice_matches() {
        let hint = hint("Анна Ковалёва", Some("anna.k@example.com"));
        assert!(strong_name_score("ANNA KOVALEVA", &hint.name).is_some());
        assert!(strong_name_score("А. Ковалёва", &hint.name).is_some());
        assert!(
            strong_name_score("Anna K", &email_local_part("anna.k@example.com").unwrap()).is_some()
        );

        let voices = [voice("Anna", None, &[1.0, 0.0])];
        let resolved = resolve_speakers(&[vec![1.0, 0.0]], &voices, &[hint], 0.7);
        assert_eq!(resolved[0].hint_index, Some(0));
    }

    #[test]
    fn a_saved_hint_email_matches_a_changed_calendar_name() {
        let voices = [voice(
            "Old display name",
            Some("anna@example.com"),
            &[1.0, 0.0],
        )];
        let hints = [hint("Коллега", Some("ANNA@example.com"))];

        let resolved = resolve_speakers(&[vec![1.0, 0.0]], &voices, &hints, 0.7);

        assert_eq!(resolved[0].voice_index, Some(0));
        assert_eq!(resolved[0].hint_index, Some(0));
    }

    #[test]
    fn similar_nickname_is_only_a_suggestion_without_voice_evidence() {
        let hints = [hint("Анна Ковалёва", None), hint("Борис Иванов", None)];

        let resolved = resolve_speakers(&[vec![1.0, 0.0]], &[], &hints, 0.7);

        assert_eq!(resolved[0].hint_index, None);
        assert_eq!(resolved[0].suggestions[0], "Анна Ковалёва");
    }
}
