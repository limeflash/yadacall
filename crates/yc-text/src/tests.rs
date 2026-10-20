use chrono::{TimeZone, Utc};
use yc_core::{
    meeting::{Bookmark, Meeting, MeetingId, MeetingStatus, Participant, TrackInfo, TrackKind},
    summary::{Decision, Summary, Task},
    transcript::{SPEAKER_ME, SPEAKER_THEM, Segment, Turn, Word},
};

use crate::*;

fn meeting() -> Meeting {
    Meeting {
        id: MeetingId::new(),
        title: "Planning: окно?".to_owned(),
        title_is_auto: false,
        started_at: Utc
            .with_ymd_and_hms(2026, 9, 28, 14, 30, 0)
            .single()
            .expect("valid date"),
        duration_ms: 3_661_000,
        language: "ru".to_owned(),
        expected_speakers: None,
        expected_speakers_explicit: false,
        participant_hints: Vec::new(),
        tracks: vec![
            TrackInfo {
                kind: TrackKind::Mic,
                device: "mic".to_owned(),
                offset_ms: 200,
                samples: 0,
            },
            TrackInfo {
                kind: TrackKind::System,
                device: "loopback".to_owned(),
                offset_ms: 1_000,
                samples: 0,
            },
        ],
        bookmarks: Vec::new(),
        participants: Vec::new(),
        asr_model: Some("test-model".to_owned()),
        status: MeetingStatus::Ready,
    }
}

fn segment(start: u64, end: u64, track: TrackKind, speaker: &str, text: &str) -> Segment {
    Segment {
        start_ms: start,
        end_ms: end,
        track,
        speaker: speaker.to_owned(),
        text: text.to_owned(),
        words: vec![Word {
            start_ms: start,
            end_ms: end,
            text: text.to_owned(),
        }],
        confidence: Some(0.9),
        corrections: Vec::new(),
    }
}

#[test]
fn merges_offsets_speakers_and_word_times() {
    let meeting = meeting();
    let mic = vec![segment(0, 500, TrackKind::Mic, SPEAKER_ME, "привет")];
    let them = vec![segment(0, 500, TrackKind::System, "s1", "ответ")];
    let merged = merge_tracks(&mic, &them, &meeting);
    assert_eq!(merged[0].start_ms, 200);
    assert_eq!(merged[0].words[0].start_ms, 200);
    assert_eq!(merged[1].start_ms, 1_000);
    assert_eq!(merged[1].speaker, "s1");
}

#[test]
fn builds_turns_across_short_gaps_but_keeps_overlaps_ordered() {
    let segments = vec![
        segment(0, 500, TrackKind::Mic, SPEAKER_ME, "первое"),
        segment(700, 900, TrackKind::Mic, SPEAKER_ME, "второе"),
        segment(750, 1_200, TrackKind::System, SPEAKER_THEM, "ответ"),
        segment(2_000, 2_200, TrackKind::Mic, SPEAKER_ME, "позже"),
    ];
    let turns = build_turns(&segments);
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].text, "первое второе");
    assert_eq!(turns[1].speaker, SPEAKER_THEM);
    assert_eq!(turns[2].start_ms, 2_000);
}

#[test]
fn removes_matching_echoes_and_reports_source_indices() {
    let segments = vec![
        segment(
            1_000,
            2_000,
            TrackKind::Mic,
            SPEAKER_ME,
            "Да, конечно, открою экран.",
        ),
        segment(
            1_200,
            2_100,
            TrackKind::System,
            SPEAKER_THEM,
            "Да конечно открою экран",
        ),
        segment(5_000, 5_500, TrackKind::Mic, SPEAKER_ME, "другая мысль"),
    ];
    let result = filter_echo_duplicates(&segments, EchoFilterConfig::default());
    assert_eq!(result.dropped.len(), 1);
    assert_eq!(result.dropped[0].segment_index, 0);
    assert_eq!(result.dropped[0].echoed_by_index, 1);
    assert_eq!(result.kept.len(), 2);
}

#[test]
fn removes_stoplist_and_low_confidence_and_cuts_loops() {
    let segments = vec![
        segment(0, 100, TrackKind::Mic, SPEAKER_ME, "Продолжение следует..."),
        {
            let mut low = segment(100, 200, TrackKind::Mic, SPEAKER_ME, "проверка данных");
            low.confidence = Some(0.01);
            low
        },
        segment(
            200,
            300,
            TrackKind::Mic,
            SPEAKER_ME,
            "проверяем экран проверяем экран проверяем экран готово",
        ),
    ];
    let result = filter_hallucinations(&segments, HallucinationConfig::default());
    assert_eq!(result.dropped.len(), 2);
    assert_eq!(result.kept[0].text, "Проверяем экран готово");
}

#[test]
fn cleans_asr_repetitions_and_standalone_hesitations_without_mangling_words() {
    let cases = [
        ("Да-да, тоже можно.", "Да-да, тоже можно."),
        ("А-а, выше я уже сказал.", "Выше я уже сказал."),
        ("А-а-а, выше я уже сказал.", "Выше я уже сказал."),
        ("Э-э, да.", "Да."),
        ("эээ, да.", "Да."),
        ("ммм, да.", "Да."),
        ("м-м, да.", "Да."),
        ("хм, да.", "Да."),
        ("мы а-а пойдём", "мы пойдём"),
        ("которые а- планируются эээ", "которые планируются"),
        ("Как же? Как же без них?", "Как же? Как же без них?"),
        ("и и и и и", "И"),
        ("спасибо спасибо спасибо спасибо", "Спасибо"),
        (
            "проверяем экран проверяем экран проверяем экран готово.",
            "Проверяем экран готово.",
        ),
        (
            "ну-ну, ха-ха-ха, так-так, кое-как, что-то, по-моему",
            "ну-ну, ха-ха-ха, так-так, кое-как, что-то, по-моему",
        ),
    ];
    for (input, expected) in cases {
        let result = filter_hallucinations(
            &[segment(0, 1_000, TrackKind::Mic, SPEAKER_ME, input)],
            HallucinationConfig::default(),
        );
        assert_eq!(result.kept[0].text, expected, "{input}");
    }
}

#[test]
fn hesitation_and_loop_cleanup_also_removes_dropped_word_timings() {
    let text = "А-а, спасибо спасибо спасибо спасибо.";
    let mut transcript_segment = segment(0, 1_000, TrackKind::Mic, SPEAKER_ME, text);
    transcript_segment.words = ["А-а,", "спасибо", "спасибо", "спасибо", "спасибо."]
        .iter()
        .enumerate()
        .map(|(index, text)| Word {
            start_ms: index as u64 * 100,
            end_ms: index as u64 * 100 + 80,
            text: (*text).to_owned(),
        })
        .collect();
    let result = filter_hallucinations(&[transcript_segment], HallucinationConfig::default());
    assert_eq!(result.kept[0].text, "Спасибо.");
    assert_eq!(result.kept[0].words.len(), 1);
    assert_eq!(result.kept[0].words[0].text, "спасибо.");
    assert_eq!(result.kept[0].words[0].start_ms, 400);
}

#[test]
fn collapses_identical_adjacent_hallucinated_phrases_across_segments() {
    let repeated = (0..3)
        .map(|index| {
            segment(
                index * 1_000,
                index * 1_000 + 700,
                TrackKind::Mic,
                SPEAKER_ME,
                "проверить список задач",
            )
        })
        .collect::<Vec<_>>();
    let result = filter_hallucinations(&repeated, HallucinationConfig::default());
    assert_eq!(result.kept.len(), 1);
    assert_eq!(result.dropped.len(), 2);
    assert!(
        result
            .dropped
            .iter()
            .all(|drop| drop.reason == HallucinationReason::RepeatedPhrase)
    );
}

#[test]
fn stemmer_matches_russian_word_forms_and_normalizes_yo() {
    assert_eq!(
        stem_text("экрана разрешений"),
        stem_text("экран разрешение")
    );
    assert_eq!(normalize_tokens("Ёлки, SCREEN!!!"), vec!["елки", "screen"]);
}

#[test]
fn renders_transcript_summary_and_claude_copy() {
    let mut meeting = meeting();
    meeting.participants.push(Participant {
        key: "them".to_owned(),
        name: "Аня".to_owned(),
        voice_id: None,
        suggestions: Vec::new(),
    });
    meeting.bookmarks.push(Bookmark {
        at_ms: 12_400,
        label: Some("важное".to_owned()),
    });
    let transcript = render_transcript_markdown(
        &meeting,
        &[
            segment(4_000, 5_000, TrackKind::Mic, SPEAKER_ME, "Привет"),
            segment(
                5_000,
                6_000,
                TrackKind::System,
                SPEAKER_THEM,
                "Здравствуйте",
            ),
        ],
    );
    let local_date = meeting
        .started_at
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    assert!(transcript.starts_with(&format!(
        "---\ntitle: \"Planning: окно?\"\ndate: \"{local_date}\"\nduration: \"01:01:01\""
    )));
    assert!(transcript.contains("**[00:00:04] Я:** Привет"));
    assert!(transcript.contains("**[00:00:05] Аня:** Здравствуйте"));
    assert!(transcript.contains("> 🔖 00:12 — важное"));
    assert!(copy_for_claude(&meeting, &[]).starts_with("Проанализируй эту расшифровку встречи:"));

    let mut english = meeting.clone();
    english.language = "en-US".to_owned();
    english.participants.clear();
    let english_transcript = render_transcript_markdown(
        &english,
        &[
            segment(0, 1_000, TrackKind::Mic, SPEAKER_ME, "Hi"),
            segment(1_000, 2_000, TrackKind::System, SPEAKER_THEM, "Hello"),
        ],
    );
    assert!(english_transcript.contains("**[00:00:00] Me:** Hi"));
    assert!(english_transcript.contains("**[00:00:01] Them:** Hello"));

    let summary = Summary {
        title: None,
        about: vec!["Обсудили запуск.".to_owned()],
        decisions: vec![Decision {
            text: "Запустить в пятницу".to_owned(),
            at_ms: Some(312_000),
        }],
        tasks: vec![Task {
            text: "Отправить план".to_owned(),
            owner: Some("me".to_owned()),
            due: None,
            at_ms: None,
            done: false,
        }],
        open_questions: vec!["Кто проверит?".to_owned()],
        language: "ru".to_owned(),
        model: "test".to_owned(),
    };
    let rendered = render_summary_markdown(&summary);
    for heading in [
        "О чём встреча",
        "Договорённости",
        "Задачи",
        "Открытые вопросы",
    ] {
        assert!(rendered.contains(heading));
    }
    assert!(rendered.contains("[05:12] Запустить в пятницу"));
}

#[test]
fn folder_names_are_portable_and_keep_cyrillic() {
    assert_eq!(sanitize_folder_name("CON", 80), "_CON");
    assert_eq!(sanitize_folder_name("com1.txt", 80), "_com1.txt");
    assert_eq!(sanitize_folder_name("Call: <>?* \"x\". ", 80), "Call x");
    let name = meeting_folder_name(&meeting());
    let local_date = meeting()
        .started_at
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H-%M")
        .to_string();
    assert!(name.starts_with(&format!("{local_date} Planning окно")));
    assert!(name.chars().count() <= 80);
}

#[test]
fn chunking_uses_speaker_names_and_one_turn_overlap() {
    let speaker_a = "me".to_owned();
    let speaker_b = "them".to_owned();
    let turns = (0..8)
        .map(|index| Turn {
            corrections: Vec::new(),
            speaker: if index % 2 == 0 {
                speaker_a.clone()
            } else {
                speaker_b.clone()
            },
            start_ms: index * 1_000,
            end_ms: index * 1_000 + 900,
            text: "слово ".repeat(40),
        })
        .collect::<Vec<_>>();
    let chunks = chunk_turns(
        MeetingId::new(),
        &turns,
        &[
            (speaker_a, "Аня".to_owned()),
            (speaker_b, "Борис".to_owned()),
        ],
    );
    assert!(chunks.len() > 1);
    assert!(chunks[0].text.starts_with("Аня: "));
    assert!(chunks[0].end_ms >= chunks[1].start_ms);
}
