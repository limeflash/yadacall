//! `yadacall summarize` command implementation.

use std::{collections::HashMap, path::PathBuf};

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use clap::{Args, ValueEnum};
use serde_json::Value;
use yc_core::meeting::Bookmark;
use yc_llm::{
    OpenAiCompatibleProvider, ProviderConfig, SummaryInput, SummaryOptions, SummaryTemplate,
    SummaryTurn, summarize,
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProviderChoice {
    Local,
    Ollama,
    Openai,
    Anthropic,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum TemplateChoice {
    #[default]
    Standard,
    Standup,
    #[value(name = "one_on_one")]
    OneOnOne,
    Interview,
}

/// Arguments for `yadacall summarize`.
#[derive(Debug, Args)]
pub struct SummarizeArgs {
    /// Input `transcript.json` or `transcript.md`.
    input: PathBuf,
    /// Provider endpoint.
    #[arg(long, value_enum, default_value_t = ProviderChoice::Local)]
    provider: ProviderChoice,
    /// Model id. Local endpoints discover the first model when omitted.
    #[arg(long)]
    model: Option<String>,
    /// OpenAI-compatible base URL.
    #[arg(long)]
    base_url: Option<String>,
    /// Summary focus template.
    #[arg(long, value_enum, default_value_t = TemplateChoice::Standard)]
    template: TemplateChoice,
    /// Print the resulting Summary JSON.
    #[arg(long)]
    json: bool,
    /// Meeting date when the transcript has no date metadata (YYYY-MM-DD).
    #[arg(long)]
    date: Option<String>,
}

/// Run the command.
pub async fn run(args: SummarizeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let raw = tokio::fs::read_to_string(&args.input).await?;
    let input = parse_input(&raw, &args.input, args.date.as_deref(), args.template)?;
    let provider = build_provider(&args).await?;
    let summary = summarize(&*provider, &input, &SummaryOptions::default(), |progress| {
        if progress.total_windows > 0 {
            eprintln!(
                "summarizing window {}/{}",
                progress.completed_windows, progress.total_windows
            );
        }
    })
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("{}", yc_text::render_summary_markdown(&summary));
    }
    Ok(())
}

async fn build_provider(
    args: &SummarizeArgs,
) -> Result<Box<dyn yc_llm::ChatProvider>, Box<dyn std::error::Error>> {
    let provider = match args.provider {
        ProviderChoice::Local => {
            let base_url = args
                .base_url
                .as_deref()
                .unwrap_or("http://127.0.0.1:8080/v1");
            let probe = OpenAiCompatibleProvider::new(base_url, None, "local", "local")?;
            let model = match &args.model {
                Some(model) => model.clone(),
                None => probe
                    .list_models()
                    .await?
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| "local".to_owned()),
            };
            Box::new(OpenAiCompatibleProvider::new(
                base_url, None, "local", model,
            )?) as Box<dyn yc_llm::ChatProvider>
        }
        ProviderChoice::Ollama => {
            let config = ProviderConfig::Ollama {
                model: args.model.as_deref().unwrap_or("llama3.2").to_owned(),
                base_url: args
                    .base_url
                    .as_deref()
                    .unwrap_or("http://localhost:11434/v1")
                    .to_owned(),
            };
            config.build()?
        }
        ProviderChoice::Openai => {
            let config = ProviderConfig::OpenAiCompatible {
                provider_id: "openai".to_owned(),
                base_url: args
                    .base_url
                    .as_deref()
                    .unwrap_or("https://api.openai.com/v1")
                    .to_owned(),
                model: args.model.as_deref().unwrap_or("gpt-4o-mini").to_owned(),
            };
            config.build()?
        }
        ProviderChoice::Anthropic => ProviderConfig::Anthropic {
            model: args
                .model
                .as_deref()
                .unwrap_or("claude-sonnet-5-5")
                .to_owned(),
        }
        .build()?,
    };
    Ok(provider)
}

fn parse_input(
    raw: &str,
    path: &std::path::Path,
    date_override: Option<&str>,
    template: TemplateChoice,
) -> Result<SummaryInput, Box<dyn std::error::Error>> {
    let is_markdown = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
    if is_markdown {
        parse_markdown(raw, date_override, template)
    } else {
        parse_json(raw, date_override, template)
    }
}

fn parse_json(
    raw: &str,
    date_override: Option<&str>,
    template: TemplateChoice,
) -> Result<SummaryInput, Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(raw)?;
    let meeting = value.get("meeting").unwrap_or(&value);
    let transcript = value.get("transcript").unwrap_or(&value);
    let participants = meeting
        .get("participants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|participant| {
            Some((
                participant.get("key")?.as_str()?.to_owned(),
                participant.get("name")?.as_str()?.to_owned(),
            ))
        })
        .collect::<HashMap<_, _>>();
    let language = meeting
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("ru")
        .to_owned();
    let segments = transcript
        .get("segments")
        .and_then(Value::as_array)
        .or_else(|| value.get("segments").and_then(Value::as_array))
        .ok_or("transcript JSON must contain a `segments` array")?;
    let turns = segments
        .iter()
        .filter_map(|segment| {
            let start_ms = segment
                .get("startMs")
                .or_else(|| segment.get("start_ms"))?
                .as_u64()?;
            let speaker = segment.get("speaker")?.as_str()?;
            let display = participants
                .get(speaker)
                .cloned()
                .unwrap_or_else(|| match speaker {
                    "me" if language.starts_with("en") => "Me".to_owned(),
                    "me" => "Я".to_owned(),
                    "them" if language.starts_with("en") => "Them".to_owned(),
                    "them" => "Собеседник".to_owned(),
                    other => other.to_owned(),
                });
            Some(SummaryTurn {
                speaker: display,
                start_ms,
                text: segment.get("text")?.as_str()?.to_owned(),
            })
        })
        .collect::<Vec<_>>();
    let bookmarks = meeting
        .get("bookmarks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|bookmark| {
            Some(Bookmark {
                at_ms: bookmark
                    .get("atMs")
                    .or_else(|| bookmark.get("at_ms"))?
                    .as_u64()?,
                label: bookmark
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect::<Vec<_>>();
    let duration_ms = meeting
        .get("durationMs")
        .or_else(|| meeting.get("duration_ms"))
        .and_then(Value::as_u64)
        .or_else(|| turns.iter().map(|turn| turn.start_ms).max())
        .unwrap_or_default();
    let title = meeting
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Imported transcript")
        .to_owned();
    let started_at = date_override
        .or_else(|| {
            meeting
                .get("startedAt")
                .or_else(|| meeting.get("started_at"))
                .and_then(Value::as_str)
        })
        .and_then(parse_date)
        .unwrap_or_else(Utc::now);
    Ok(SummaryInput {
        meeting: yc_llm::MeetingMetadata {
            id: None,
            title,
            started_at,
            duration_ms,
        },
        turns,
        bookmarks,
        language: None,
        template: template.into(),
        meeting_language: language,
    })
}

fn parse_markdown(
    raw: &str,
    date_override: Option<&str>,
    template: TemplateChoice,
) -> Result<SummaryInput, Box<dyn std::error::Error>> {
    let mut title = "Imported transcript".to_owned();
    let mut date = None;
    let mut language = "ru".to_owned();
    let mut duration_ms = 0_u64;
    let mut turns = Vec::new();
    let mut bookmarks = Vec::new();
    let mut in_front_matter = false;
    for line in raw.lines() {
        if line.trim() == "---" {
            in_front_matter = !in_front_matter;
            continue;
        }
        if in_front_matter {
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                match key.trim() {
                    "title" => title = value.to_owned(),
                    "date" => date = Some(value.to_owned()),
                    "language" => language = value.to_owned(),
                    "duration" => duration_ms = parse_clock(value).unwrap_or_default(),
                    _ => {}
                }
            }
            continue;
        }
        if let Some(rest) = line.trim().strip_prefix("**[") {
            if let Some((time, rest)) = rest.split_once("] ")
                && let Some((speaker, text)) = rest.split_once(":** ")
                && let Some(start_ms) = parse_clock(time)
            {
                duration_ms = duration_ms.max(start_ms);
                turns.push(SummaryTurn {
                    speaker: speaker.to_owned(),
                    start_ms,
                    text: text.to_owned(),
                });
            }
        } else if let Some(rest) = line.trim().strip_prefix("> 🔖 ")
            && let Some((time, label)) = rest.split_once(" — ")
            && let Some(at_ms) = parse_clock(time)
        {
            bookmarks.push(Bookmark {
                at_ms,
                label: Some(label.to_owned()),
            });
        }
    }
    let started_at = date_override
        .or(date.as_deref())
        .and_then(parse_date)
        .unwrap_or_else(Utc::now);
    Ok(SummaryInput {
        meeting: yc_llm::MeetingMetadata {
            id: None,
            title,
            started_at,
            duration_ms,
        },
        turns,
        bookmarks,
        language: None,
        template: template.into(),
        meeting_language: language,
    })
}

fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        return Some(datetime.with_timezone(&Utc));
    }
    if let Ok(datetime) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M") {
        return Some(datetime.and_utc());
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?));
    }
    None
}

fn parse_clock(value: &str) -> Option<u64> {
    let components = value
        .split(':')
        .map(str::parse::<u64>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()?;
    let seconds = match components.as_slice() {
        [minutes, seconds] if *seconds < 60 => minutes.checked_mul(60)?.checked_add(*seconds)?,
        [hours, minutes, seconds] if *minutes < 60 && *seconds < 60 => hours
            .checked_mul(3_600)?
            .checked_add(minutes.checked_mul(60)?)?
            .checked_add(*seconds)?,
        _ => return None,
    };
    seconds.checked_mul(1_000)
}

impl From<TemplateChoice> for SummaryTemplate {
    fn from(value: TemplateChoice) -> Self {
        match value {
            TemplateChoice::Standard => Self::Standard,
            TemplateChoice::Standup => Self::Standup,
            TemplateChoice::OneOnOne => Self::OneOnOne,
            TemplateChoice::Interview => Self::Interview,
        }
    }
}
