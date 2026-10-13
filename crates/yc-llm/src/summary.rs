//! Meeting summarization, including turn-preserving map/reduce for long meetings.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yc_core::{
    Error, Result,
    meeting::{Bookmark, MeetingId},
    summary::{Decision, Summary, Task},
};

use crate::{
    ChatResponse,
    types::{ChatMessage, ChatProvider, ChatRequest, JsonSchema},
};

const TEMPLATES: &str = include_str!("templates.json");
const WINDOW_TOKEN_LIMIT: usize = 6_000;

/// Small meeting metadata needed to ground a summary without depending on storage.
#[derive(Debug, Clone)]
pub struct MeetingMetadata {
    /// Meeting identifier, used by downstream storage.
    pub id: Option<MeetingId>,
    /// Recorded title.
    pub title: String,
    /// Meeting date and time, used to resolve relative due dates.
    pub started_at: DateTime<Utc>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// A display-name turn on the meeting timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryTurn {
    /// Speaker display name, such as `Я`, `me`, or a participant name.
    pub speaker: String,
    /// Turn start on the meeting timeline.
    pub start_ms: u64,
    /// Recognized text.
    pub text: String,
}

/// Input to the meeting summarizer.
#[derive(Debug, Clone)]
pub struct SummaryInput {
    /// Meeting date, title, and duration.
    pub meeting: MeetingMetadata,
    /// Speaker display names and transcript turns.
    pub turns: Vec<SummaryTurn>,
    /// User bookmarks to provide additional grounding.
    pub bookmarks: Vec<Bookmark>,
    /// Optional output language override. Defaults to the meeting language.
    pub language: Option<String>,
    /// Prompt template.
    pub template: SummaryTemplate,
    /// Meeting language hint, such as `ru` or `en`.
    pub meeting_language: String,
}

/// Data-driven prompt template selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryTemplate {
    /// Balanced general-purpose summary.
    #[default]
    Standard,
    /// Short daily stand-up.
    Standup,
    /// One-to-one conversation.
    OneOnOne,
    /// Interview.
    Interview,
}

impl SummaryTemplate {
    fn id(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Standup => "standup",
            Self::OneOnOne => "one_on_one",
            Self::Interview => "interview",
        }
    }
}

/// Long-meeting map/reduce settings.
#[derive(Debug, Clone)]
pub struct SummaryOptions {
    /// Runtime context size. Defaults to 32768 tokens.
    pub context_tokens: usize,
    /// Space left for generated JSON. Defaults to approximately 8000 tokens.
    pub output_reserve: usize,
}

impl Default for SummaryOptions {
    fn default() -> Self {
        Self {
            context_tokens: 32_768,
            output_reserve: 8_000,
        }
    }
}

/// Progress while summarizing partial windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryProgress {
    /// Number of completed partial summaries.
    pub completed_windows: usize,
    /// Total partial summaries planned; zero means the single-pass phase.
    pub total_windows: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SummaryJson {
    title: Option<String>,
    about: Vec<String>,
    decisions: Vec<DecisionJson>,
    tasks: Vec<TaskJson>,
    open_questions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionJson {
    text: String,
    at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskJson {
    text: String,
    owner: Option<String>,
    due: Option<String>,
    at: String,
}

/// Create a grounded summary from meeting turns.
pub async fn summarize(
    provider: &dyn ChatProvider,
    input: &SummaryInput,
    options: &SummaryOptions,
    progress: impl Fn(SummaryProgress) + Send + Sync,
) -> Result<Summary> {
    if input.turns.is_empty() {
        return Err(Error::Invalid(
            "cannot summarize an empty transcript".to_owned(),
        ));
    }
    let language = input
        .language
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&input.meeting_language)
        .to_owned();
    let title_is_untitled = is_automatic_placeholder(&input.meeting.title);
    let system = system_prompt(input, &language)?;
    let transcript = render_transcript(&input.turns, &input.bookmarks);
    let full_prompt = format!(
        "Meeting: {}\nMeeting date: {}\nDuration: {}\nLanguage: {}\n\n{}",
        input.meeting.title,
        input.meeting.started_at.format("%Y-%m-%d"),
        format_duration(input.meeting.duration_ms),
        language,
        transcript
    );
    let full_tokens = token_count(provider, &format!("{system}\n{full_prompt}")).await?;
    let available = options
        .context_tokens
        .saturating_sub(options.output_reserve.saturating_add(512));
    if available < 256 {
        return Err(Error::Invalid(
            "LLM context size must leave at least 256 prompt tokens after output reserve"
                .to_owned(),
        ));
    }
    let summary_json = if full_tokens <= available {
        progress(SummaryProgress {
            completed_windows: 0,
            total_windows: 0,
        });
        let request = summary_request(system.clone(), full_prompt, options.output_reserve.max(256));
        request_valid_summary(
            provider,
            request,
            input.meeting.duration_ms,
            title_is_untitled,
        )
        .await?
    } else {
        let window_budget = WINDOW_TOKEN_LIMIT.min(available.saturating_sub(512).max(256));
        let windows = make_windows(provider, &input.turns, &input.bookmarks, window_budget).await?;
        let total = windows.len();
        let mut partials = Vec::with_capacity(total);
        for (index, window) in windows.iter().enumerate() {
            progress(SummaryProgress {
                completed_windows: index,
                total_windows: total,
            });
            let prompt = format!(
                "Meeting: {}\nMeeting date: {}\nDuration: {}\nLanguage: {}\nWindow: {} of {}\n\n{}",
                input.meeting.title,
                input.meeting.started_at.format("%Y-%m-%d"),
                format_duration(input.meeting.duration_ms),
                language,
                index + 1,
                total,
                window
            );
            let request = summary_request(
                system.clone(),
                prompt,
                options.output_reserve.clamp(256, 2_000),
            );
            let partial = request_valid_summary(
                provider,
                request,
                input.meeting.duration_ms,
                title_is_untitled,
            )
            .await?;
            partials.push(serde_json::to_string(&partial).map_err(|error| {
                Error::Llm(format!("could not encode partial summary: {error}"))
            })?);
            progress(SummaryProgress {
                completed_windows: index + 1,
                total_windows: total,
            });
        }
        let merge_system = merge_prompt(&language, title_is_untitled);
        let merge_input = format!(
            "Meeting: {}\nMeeting date: {}\nDuration: {}\nLanguage: {}\n\nPartial summaries (JSON):\n{}",
            input.meeting.title,
            input.meeting.started_at.format("%Y-%m-%d"),
            format_duration(input.meeting.duration_ms),
            language,
            partials.join("\n")
        );
        let request = summary_request(merge_system, merge_input, options.output_reserve.max(256));
        request_valid_summary(
            provider,
            request,
            input.meeting.duration_ms,
            title_is_untitled,
        )
        .await?
    };
    to_core_summary(
        summary_json,
        &language,
        &format!("{}:{}", provider.provider_id(), provider.model()),
    )
}

fn summary_request(system: String, prompt: String, max_tokens: usize) -> ChatRequest {
    ChatRequest {
        system,
        messages: vec![ChatMessage::user(prompt)],
        max_tokens: u32::try_from(max_tokens).unwrap_or(u32::MAX),
        temperature: None,
        top_p: None,
        top_k: None,
        min_p: None,
        presence_penalty: None,
        json_schema: Some(summary_schema()),
        stop: Vec::new(),
    }
}

async fn request_valid_summary(
    provider: &dyn ChatProvider,
    request: ChatRequest,
    duration_ms: u64,
    title_is_untitled: bool,
) -> Result<SummaryJson> {
    let first = provider.complete(request.clone()).await?;
    match parse_and_validate(&first, duration_ms, title_is_untitled) {
        Ok(parsed) => Ok(parsed),
        Err(error) => {
            let mut retry = request;
            let previous = retry
                .messages
                .last()
                .map(|message| message.content.clone())
                .unwrap_or_default();
            if let Some(message) = retry.messages.last_mut() {
                message.content = format!(
                    "{previous}\n\nThe prior response did not satisfy the schema: {error}. Return only corrected JSON matching the requested schema."
                );
            }
            let response = provider.complete(retry).await?;
            parse_and_validate(&response, duration_ms, title_is_untitled).map_err(|second| {
                Error::Llm(format!(
                    "summary JSON remained invalid after one retry: {second}"
                ))
            })
        }
    }
}

fn parse_and_validate(
    response: &ChatResponse,
    duration_ms: u64,
    title_is_untitled: bool,
) -> std::result::Result<SummaryJson, String> {
    let mut parsed: SummaryJson = serde_json::from_str(json_body(&response.text))
        .map_err(|error| format!("invalid JSON: {error}"))?;
    if parsed.about.is_empty() || parsed.about.len() > 3 {
        return Err("about must contain between one and three paragraphs".to_owned());
    }
    // A title is only a suggestion: small models often propose one even when told not to. Drop an
    // unwanted, empty or generic title instead of failing the whole summary over it.
    if parsed.title.as_deref().is_some_and(|title| {
        !title_is_untitled || title.trim().is_empty() || is_generic_title(title)
    }) {
        parsed.title = None;
    }
    for decision in &parsed.decisions {
        let at_ms = parse_timestamp(&decision.at)
            .ok_or_else(|| format!("invalid decision timestamp `{}`", decision.at))?;
        if at_ms > duration_ms {
            return Err(format!(
                "decision timestamp `{}` exceeds meeting duration",
                decision.at
            ));
        }
    }
    for task in &parsed.tasks {
        let at_ms = parse_timestamp(&task.at)
            .ok_or_else(|| format!("invalid task timestamp `{}`", task.at))?;
        if at_ms > duration_ms {
            return Err(format!(
                "task timestamp `{}` exceeds meeting duration",
                task.at
            ));
        }
    }
    Ok(parsed)
}

/// The JSON object inside a reply. Providers that do not enforce the schema (Ollama cloud models)
/// wrap it in a Markdown fence or a sentence.
fn json_body(text: &str) -> &str {
    let text = text.trim();
    if text.starts_with('{') {
        return text;
    }
    match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if end > start => &text[start..=end],
        _ => text,
    }
}

fn to_core_summary(value: SummaryJson, language: &str, model: &str) -> Result<Summary> {
    Ok(Summary {
        title: value.title,
        about: value.about,
        decisions: value
            .decisions
            .into_iter()
            .map(|decision| {
                Ok(Decision {
                    text: decision.text,
                    at_ms: Some(parse_timestamp(&decision.at).ok_or_else(|| {
                        Error::Llm(format!("invalid decision timestamp `{}`", decision.at))
                    })?),
                })
            })
            .collect::<Result<Vec<_>>>()?,
        tasks: value
            .tasks
            .into_iter()
            .map(|task| {
                Ok(Task {
                    text: task.text,
                    owner: task.owner,
                    due: task.due,
                    at_ms: Some(parse_timestamp(&task.at).ok_or_else(|| {
                        Error::Llm(format!("invalid task timestamp `{}`", task.at))
                    })?),
                    done: false,
                })
            })
            .collect::<Result<Vec<_>>>()?,
        open_questions: value.open_questions,
        language: language.to_owned(),
        model: model.to_owned(),
    })
}

fn summary_schema() -> JsonSchema {
    JsonSchema {
        name: "meeting_summary".to_owned(),
        schema: json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["title", "about", "decisions", "tasks", "open_questions"],
            "properties": {
                "title": { "type": ["string", "null"] },
                "about": { "type": "array", "minItems": 1, "maxItems": 3, "items": { "type": "string" } },
                "decisions": { "type": "array", "items": {
                    "type": "object", "additionalProperties": false, "required": ["text", "at"],
                    "properties": { "text": { "type": "string" }, "at": { "type": "string", "pattern": "^(?:[0-9]+:)?[0-5][0-9]:[0-5][0-9]$" } }
                } },
                "tasks": { "type": "array", "items": {
                    "type": "object", "additionalProperties": false, "required": ["text", "owner", "due", "at"],
                    "properties": {
                        "text": { "type": "string" },
                        "owner": { "type": ["string", "null"] },
                        "due": { "type": ["string", "null"] },
                        "at": { "type": "string", "pattern": "^(?:[0-9]+:)?[0-5][0-9]:[0-5][0-9]$" }
                    }
                } },
                "open_questions": { "type": "array", "items": { "type": "string" } }
            }
        }),
    }
}

fn system_prompt(input: &SummaryInput, language: &str) -> Result<String> {
    let templates: Value = serde_json::from_str(TEMPLATES)
        .map_err(|error| Error::Llm(format!("invalid embedded summary templates: {error}")))?;
    let language_key = language
        .split('-')
        .next()
        .unwrap_or(language)
        .to_ascii_lowercase();
    let template = templates[input.template.id()][&language_key]
        .as_str()
        .or_else(|| templates[input.template.id()]["en"].as_str())
        .unwrap_or_default();
    let rules = if language_key == "ru" {
        "Пиши резюме по-русски, если пользователь не задал другой язык. Используй только факты, которые прямо прозвучали; не выдумывай участников и контекст. Решения указывай только если участники явно согласовали их. Задачи добавляй только когда кто-то явно обязался выполнить действие; владельцем укажи названного говорящего или Я/me, если обязательство взял пользователь. Разрешай относительные сроки относительно даты встречи. Каждое решение и каждую задачу привязывай к времени, когда это было сказано. Если подтверждений нет, оставь список пустым. «Я» — это пользователь, который записывает встречу. Приписывай действие человеку только если в реплике прямо названо, кто его сделал: фраза без подлежащего («Запустил, точнее, распознал…») может говорить о программе, другом человеке или о самом пользователе — не превращай её в «Дима запустил». Передавай мнения и наблюдения как слова говорящего («Дима заметил, что…»), а не как совершённые им действия. Если не уверен, кто что сделал, лучше опусти этот пункт."
    } else {
        "Write in the meeting language unless the user requested another language. Use only facts stated aloud; do not invent attendees or context. Include a decision only when participants explicitly agreed to it. Include a task only when someone explicitly committed to doing it; use the named speaker or Me/me as owner when the user made the commitment. Resolve relative due dates against the meeting date. Anchor every decision and task to the time it was said. Leave lists empty when the transcript does not support an item. 'Me' is the user who records the meeting. Attribute an action to a person only when the line explicitly names who did it: a subjectless phrase ('Launched, I mean recognised…') may refer to the software, someone else or the user — never turn it into 'Dima launched'. Report opinions and observations as what the speaker said ('Dima noticed that…'), not as actions they performed. If unsure who did something, leave it out."
    };
    let title_rules = match (
        language_key == "ru",
        is_automatic_placeholder(&input.meeting.title),
    ) {
        (true, true) => {
            "Текущее название создано автоматически: предложи короткое и конкретное название только по теме, явно подтверждённой разговором; если темы недостаточно, верни null. Не используй общие названия «Резюме встречи …» или «Итоги встречи»."
        }
        (true, false) => {
            "Название встречи уже задано пользователем или календарём: не предлагай его менять и верни null в поле title."
        }
        (false, true) => {
            "The current title is an automatic placeholder: suggest a short, specific title only when the topic is directly supported by the conversation; otherwise return null. Do not use generic titles such as “Meeting summary” or “Summary of meeting”."
        }
        (false, false) => {
            "The meeting already has a user or calendar title: do not suggest a replacement and return null for title."
        }
    };
    Ok(format!(
        "{template}\n\n{rules}\n\n{title_rules}\n\nReturn only JSON matching the supplied schema. `at` must use MM:SS, or HH:MM:SS for times over one hour. Keep `about` to one through three concise paragraphs."
    ))
}

fn merge_prompt(language: &str, title_is_untitled: bool) -> String {
    let title_rule = if title_is_untitled {
        "For title, keep a concise, specific topic title only when supported; otherwise use null. Reject generic summary titles."
    } else {
        "The meeting already has a title, so title must be null."
    };
    if language
        .split('-')
        .next()
        .unwrap_or(language)
        .eq_ignore_ascii_case("ru")
    {
        format!(
            "Объедини частичные резюме в одно. Удали дубликаты, сохрани подтверждённые формулировки и временные метки, не добавляй фактов. При противоречиях предпочитай конкретные исходные факты. {title_rule} Возвращай только JSON по заданной схеме."
        )
    } else {
        format!(
            "Merge the partial summaries into one. Deduplicate while preserving supported wording and timestamps. Add no facts. When details conflict, prefer concrete source facts. {title_rule} Return only JSON matching the requested schema."
        )
    }
}

fn is_generic_title(title: &str) -> bool {
    let normalized = title
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect::<String>();
    [
        "резюмевстречи",
        "итогивстречи",
        "meetingsummary",
        "summaryofmeeting",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix))
}

fn is_automatic_placeholder(title: &str) -> bool {
    let Some((prefix, clock)) = title.trim().rsplit_once(' ') else {
        return false;
    };
    let prefix = prefix.to_lowercase();
    if !matches!(prefix.as_str(), "meeting" | "встреча") {
        return false;
    }
    let Some((hour, minute)) = clock.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.chars().all(|character| character.is_ascii_digit())
        && minute.chars().all(|character| character.is_ascii_digit())
}

fn render_transcript(turns: &[SummaryTurn], bookmarks: &[Bookmark]) -> String {
    let mut lines = turns
        .iter()
        .map(|turn| {
            (
                turn.start_ms,
                format!(
                    "[{}] {}: {}",
                    timestamp(turn.start_ms),
                    turn.speaker,
                    turn.text.trim()
                ),
            )
        })
        .collect::<Vec<_>>();
    lines.extend(bookmarks.iter().map(|bookmark| {
        (
            bookmark.at_ms,
            format!(
                "[{}] BOOKMARK: {}",
                timestamp(bookmark.at_ms),
                bookmark.label.as_deref().unwrap_or("")
            ),
        )
    }));
    lines.sort_by_key(|line| line.0);
    lines
        .into_iter()
        .map(|line| line.1)
        .collect::<Vec<_>>()
        .join("\n")
}

async fn make_windows(
    provider: &dyn ChatProvider,
    turns: &[SummaryTurn],
    bookmarks: &[Bookmark],
    cap: usize,
) -> Result<Vec<String>> {
    let mut windows = Vec::new();
    let mut start = 0;
    while start < turns.len() {
        let mut end = start;
        let mut tokens = 0_usize;
        while end < turns.len() {
            let line = format!(
                "[{}] {}: {}",
                timestamp(turns[end].start_ms),
                turns[end].speaker,
                turns[end].text
            );
            let line_tokens = token_count(provider, &line).await?;
            if end > start && tokens.saturating_add(line_tokens) > cap {
                break;
            }
            tokens = tokens.saturating_add(line_tokens);
            end += 1;
            if tokens >= cap {
                break;
            }
        }
        if end == start {
            end += 1;
        }
        let mut lines = turns[start..end]
            .iter()
            .map(|turn| {
                (
                    turn.start_ms,
                    format!(
                        "[{}] {}: {}",
                        timestamp(turn.start_ms),
                        turn.speaker,
                        turn.text.trim()
                    ),
                )
            })
            .collect::<Vec<_>>();
        let window_start = turns[start].start_ms;
        let window_end = turns[end - 1].start_ms;
        lines.extend(
            bookmarks
                .iter()
                .filter(|bookmark| bookmark.at_ms >= window_start && bookmark.at_ms <= window_end)
                .map(|bookmark| {
                    (
                        bookmark.at_ms,
                        format!(
                            "[{}] BOOKMARK: {}",
                            timestamp(bookmark.at_ms),
                            bookmark.label.as_deref().unwrap_or("")
                        ),
                    )
                }),
        );
        lines.sort_by_key(|line| line.0);
        windows.push(
            lines
                .into_iter()
                .map(|line| line.1)
                .collect::<Vec<_>>()
                .join("\n"),
        );
        if end >= turns.len() {
            break;
        }
        start = if end - start > 1 { end - 1 } else { end };
    }
    Ok(windows)
}

async fn token_count(provider: &dyn ChatProvider, text: &str) -> Result<usize> {
    if let Some(tokens) = provider.count_tokens(text).await? {
        return Ok(tokens);
    }
    let cyrillic = text
        .chars()
        .filter(|ch| ('\u{0400}'..='\u{04ff}').contains(ch))
        .count();
    let divisor = if cyrillic.saturating_mul(2) >= text.chars().count() {
        3.2
    } else {
        4.0
    };
    Ok((text.chars().count() as f64 / divisor).ceil() as usize)
}

fn timestamp(ms: u64) -> String {
    let seconds = ms / 1_000;
    if seconds >= 3_600 {
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3_600,
            (seconds / 60) % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let values = value
        .split(':')
        .map(str::parse::<u64>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()?;
    let seconds = match values.as_slice() {
        [minutes, seconds] if *seconds < 60 => minutes.checked_mul(60)?.checked_add(*seconds)?,
        [hours, minutes, seconds] if *minutes < 60 && *seconds < 60 => hours
            .checked_mul(3_600)?
            .checked_add(minutes.checked_mul(60)?)?
            .checked_add(*seconds)?,
        _ => return None,
    };
    seconds.checked_mul(1_000)
}

fn format_duration(ms: u64) -> String {
    timestamp(ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatStream, Timings, TokenUsage};

    #[test]
    fn json_is_found_inside_a_markdown_fence() {
        assert_eq!(json_body("```json\n{\"a\": 1}\n```"), "{\"a\": 1}");
        assert_eq!(json_body("  {\"a\": 1} "), "{\"a\": 1}");
        assert_eq!(json_body("no json"), "no json");
    }

    #[test]
    fn parses_minute_and_hour_timestamps() {
        assert_eq!(parse_timestamp("12:34"), Some(754_000));
        assert_eq!(parse_timestamp("01:02:03"), Some(3_723_000));
        assert_eq!(parse_timestamp("01:99:00"), None);
    }

    #[test]
    fn timestamps_switch_to_hours_after_one_hour() {
        assert_eq!(timestamp(3_600_000), "01:00:00");
        assert_eq!(timestamp(59_000), "00:59");
    }

    #[test]
    fn strict_schema_contains_summary_fields_and_anchors() {
        let schema = summary_schema().schema;
        assert_eq!(schema["properties"]["about"]["minItems"], 1);
        assert_eq!(
            schema["properties"]["tasks"]["items"]["required"],
            json!(["text", "owner", "due", "at"])
        );
    }

    #[test]
    fn validation_rejects_missing_anchor_or_extra_field() {
        let missing: ChatResponse = serde_json::from_value(
            json!({"text":"{}", "usage":{}, "timings":{}, "stop_reason":null}),
        )
        .expect("response");
        assert!(parse_and_validate(&missing, 99_000, false).is_err());
        let extra = ChatResponse { text: r#"{"title":null,"about":["ok"],"decisions":[],"tasks":[],"open_questions":[],"extra":true}"#.to_owned(), usage: TokenUsage::default(), timings: Timings::default(), stop_reason: None };
        assert!(parse_and_validate(&extra, 99_000, false).is_err());
    }

    #[test]
    fn title_suggestions_are_kept_only_when_specific_and_wanted() {
        let response = |title: &str| ChatResponse {
            text: format!(
                r#"{{"title":{title},"about":["ok"],"decisions":[],"tasks":[],"open_questions":[]}}"#
            ),
            usage: TokenUsage::default(),
            timings: Timings::default(),
            stop_reason: None,
        };
        let title = |text: &str, untitled: bool| {
            parse_and_validate(&response(text), 99_000, untitled)
                .expect("a title problem must not fail the summary")
                .title
        };
        assert_eq!(title("null", false), None);
        assert_eq!(title(r#""NB Week""#, true).as_deref(), Some("NB Week"));
        // The meeting already has a title: the suggestion is dropped, the summary survives.
        assert_eq!(title(r#""Specific title""#, false), None);
        for generic in [
            r#""Резюме встречи NB Week""#,
            r#""Meeting summary""#,
            r#""Итоги встречи""#,
            r#""  ""#,
        ] {
            assert_eq!(title(generic, true), None);
        }
    }

    #[test]
    fn summary_prompt_respects_the_title_policy() {
        let mut input = SummaryInput {
            meeting: MeetingMetadata {
                id: None,
                title: "NB Week".to_owned(),
                started_at: Utc::now(),
                duration_ms: 60_000,
            },
            turns: Vec::new(),
            bookmarks: Vec::new(),
            language: None,
            template: SummaryTemplate::Standard,
            meeting_language: "ru".to_owned(),
        };
        let named_prompt = system_prompt(&input, "ru").expect("prompt");
        assert!(named_prompt.contains("пользователем или календарём"));
        assert!(named_prompt.contains("верни null в поле title"));

        input.meeting.title = "Встреча 11:16".to_owned();
        assert!(is_automatic_placeholder(&input.meeting.title));
        let untitled_prompt = system_prompt(&input, "ru").expect("prompt");
        assert!(untitled_prompt.contains("Резюме встречи"));
        assert!(untitled_prompt.contains("верни null"));
    }

    #[tokio::test]
    async fn windowing_keeps_turns_whole_and_overlaps_one_turn() {
        struct Counter;
        #[async_trait::async_trait]
        impl ChatProvider for Counter {
            async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse> {
                Err(Error::Llm("unused".to_owned()))
            }
            async fn stream(&self, _request: ChatRequest) -> Result<ChatStream> {
                Err(Error::Llm("unused".to_owned()))
            }
            fn provider_id(&self) -> &str {
                "test"
            }
            fn model(&self) -> &str {
                "test"
            }
            async fn count_tokens(&self, text: &str) -> Result<Option<usize>> {
                Ok(Some(text.len()))
            }
        }
        let turns = (0..6)
            .map(|i| SummaryTurn {
                speaker: "A".to_owned(),
                start_ms: i * 1_000,
                text: format!("turn-{i}"),
            })
            .collect::<Vec<_>>();
        let windows = make_windows(&Counter, &turns, &[], 40)
            .await
            .expect("windows");
        assert!(windows.len() > 1);
        let first_last = windows[0].lines().last().expect("line");
        let second_first = windows[1].lines().next().expect("line");
        assert_eq!(first_last, second_first);
    }
}
