//! In-memory [`Backend`] with the design's demo data (`demo_data.json`, shared with the browser mock in
//! `ui/src/lib/api/mock.ts`). Everything is clickable: a simulated recorder (levels, live phrases), post-call
//! processing, model downloads, ask-across-meetings with citations, settings, permissions.
//!
//! The real engine implements the same trait later; nothing in the UI knows the difference.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use yc_core::Error;
use yc_core::Ms;
use yc_core::events::{DownloadState, EngineEvent, Level, RecorderState};
use yc_core::meeting::{
    Bookmark, Meeting, MeetingId, MeetingStatus, Participant, Stage, TrackInfo, TrackKind,
};
use yc_core::summary::{Decision, Summary, Task};
use yc_core::transcript::{Segment, Turn};

use crate::api::*;

const DATA: &str = include_str!("demo_data.json");

// ---- raw demo data ---------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct Raw {
    today: String,
    meetings: Vec<RawMeeting>,
    ask: RawAsk,
    models: Vec<ModelEntry>,
    hardware: HardwareProfile,
    #[serde(rename = "inputDevices")]
    input_devices: Vec<InputDevice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMeeting {
    id: String,
    title: String,
    start: String,
    duration_sec: u64,
    status: serde_json::Value,
    snippet: Option<String>,
    audio_note: Option<String>,
    has_audio: bool,
    participants: Vec<RawPerson>,
    turns: Vec<(String, u64, u64, String)>,
    bookmarks: Vec<RawBookmark>,
    summary: Option<RawSummary>,
}

#[derive(Deserialize)]
struct RawPerson {
    key: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawBookmark {
    at_sec: u64,
    label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSummary {
    about: Vec<String>,
    decisions: Vec<RawDecision>,
    tasks: Vec<RawTask>,
    #[serde(default)]
    open_questions: Vec<RawDecision>,
    model: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDecision {
    text: String,
    at_sec: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTask {
    text: String,
    owner: Option<String>,
    due: Option<String>,
    at_sec: Option<u64>,
    done: bool,
}

#[derive(Deserialize)]
struct RawAsk {
    answer: String,
    citations: Vec<RawCitation>,
    excerpts: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCitation {
    n: u32,
    meeting: String,
    at_sec: u64,
    speaker: String,
    quote: String,
}

// ---- state --------------------------------------------------------------------------------------------

struct DemoMeeting {
    meeting: Meeting,
    turns: Vec<Turn>,
    summary: Option<Summary>,
    has_audio: bool,
    snippet: Option<String>,
    audio_note: Option<String>,
    /// The seeded "standup" keeps looping its progress like the design; others finish.
    looping: bool,
}

struct Rec {
    id: MeetingId,
    title: String,
    started: std::time::Instant,
    paused_at: Option<std::time::Instant>,
    paused_total: Duration,
    marks: Vec<Ms>,
    turns: Vec<Turn>,
    mic: Option<String>,
}

impl Rec {
    fn elapsed(&self) -> Ms {
        let end = self.paused_at.unwrap_or_else(std::time::Instant::now);
        end.duration_since(self.started)
            .saturating_sub(self.paused_total)
            .as_millis() as Ms
    }
}

struct Inner {
    meetings: Vec<DemoMeeting>,
    models: Vec<ModelEntry>,
    hardware: HardwareProfile,
    devices: Vec<InputDevice>,
    settings: Settings,
    perms: Permissions,
    rec: Option<Rec>,
    imports: Vec<ImportJob>,
    download_sinks: HashMap<String, EventSink>,
    ask: RawAsk,
    // simulation
    rng: u64,
    who: Who,
    who_left: u32,
    live_idx: usize,
    next_live_ms: Ms,
    tick: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum Who {
    Me,
    Them,
    None,
    Both,
}

const LIVE_SCRIPT: [(&str, TrackKind, &str); 5] = [
    (
        "them",
        TrackKind::System,
        "А на мобильной кнопка не уезжает под клавиатуру?",
    ),
    (
        "me",
        TrackKind::Mic,
        "Проверю на айфоне, но по макету она должна оставаться над клавиатурой.",
    ),
    (
        "them",
        TrackKind::System,
        "Отлично, тогда по карточкам профиля — оставляем как в прошлой версии?",
    ),
    (
        "me",
        TrackKind::Mic,
        "Да, только поправим отступы и заголовок раздела.",
    ),
    (
        "them",
        TrackKind::System,
        "Хорошо, тогда я соберу замечания и отправлю к вечеру.",
    ),
];

pub struct DemoBackend {
    inner: Mutex<Inner>,
    sink: EventSink,
}

fn ms(sec: u64) -> Ms {
    sec * 1000
}

fn local_to_utc(naive: NaiveDateTime, shift: chrono::Duration) -> DateTime<Utc> {
    let shifted = naive + shift;
    Local
        .from_local_datetime(&shifted)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&shifted))
}

impl DemoBackend {
    pub fn new(sink: EventSink) -> Arc<Self> {
        // The data is embedded; a parse failure is a programming error caught by the unit test below.
        let raw: Raw = serde_json::from_str(DATA).expect("demo_data.json is valid");
        let base = NaiveDate::parse_from_str(&raw.today, "%Y-%m-%d").expect("demo date");
        let shift = Local::now().date_naive().signed_duration_since(base);
        let meetings = raw
            .meetings
            .iter()
            .map(|m| Self::build_meeting(m, shift))
            .collect();
        let inner = Inner {
            meetings,
            models: raw.models,
            hardware: raw.hardware,
            devices: raw.input_devices,
            settings: Settings::default(),
            perms: Permissions {
                microphone: PermissionState::Granted,
                system_audio: PermissionState::NotDetermined,
            },
            rec: None,
            imports: Vec::new(),
            download_sinks: HashMap::new(),
            ask: raw.ask,
            rng: 0x2545F4914F6CDD1D,
            who: Who::Me,
            who_left: 70,
            live_idx: 0,
            next_live_ms: 4000,
            tick: 0,
        };
        Arc::new(Self {
            inner: Mutex::new(inner),
            sink,
        })
    }

    fn build_meeting(m: &RawMeeting, shift: chrono::Duration) -> DemoMeeting {
        let naive =
            NaiveDateTime::parse_from_str(&m.start, "%Y-%m-%dT%H:%M:%S").expect("demo start");
        let started_at = local_to_utc(naive, shift);
        let status = match &m.status {
            serde_json::Value::String(s) if s == "ready" => MeetingStatus::Ready,
            serde_json::Value::Object(o) => MeetingStatus::Processing {
                stage: Stage::Transcribe,
                progress: o.get("processing").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            },
            _ => MeetingStatus::Recording,
        };
        let participants = m
            .participants
            .iter()
            .filter(|p| !p.name.is_empty())
            .map(|p| Participant {
                key: p.key.clone(),
                name: p.name.clone(),
                voice_id: None,
                suggestions: Vec::new(),
            })
            .collect();
        let turns = m
            .turns
            .iter()
            .map(|(who, s, e, text)| Turn {
                speaker: who.clone(),
                start_ms: ms(*s),
                end_ms: ms(*e),
                text: text.clone(),
            })
            .collect();
        let summary = m.summary.as_ref().map(|s| Summary {
            title: None,
            about: s.about.clone(),
            decisions: s
                .decisions
                .iter()
                .map(|d| Decision {
                    text: d.text.clone(),
                    at_ms: Some(ms(d.at_sec)),
                })
                .collect(),
            tasks: s
                .tasks
                .iter()
                .map(|t| Task {
                    text: t.text.clone(),
                    owner: t.owner.clone(),
                    due: t
                        .due
                        .as_ref()
                        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                        .map(|d| (d + shift).to_string()),
                    at_ms: t.at_sec.map(ms),
                    done: t.done,
                })
                .collect(),
            open_questions: s.open_questions.iter().map(|q| q.text.clone()).collect(),
            language: "ru".into(),
            model: s.model.clone(),
        });
        let meeting = Meeting {
            id: m.id.parse().expect("demo uuid"),
            title: m.title.clone(),
            title_is_auto: false,
            started_at,
            duration_ms: ms(m.duration_sec),
            language: "ru".into(),
            expected_speakers: None,
            expected_speakers_explicit: false,
            participant_hints: Vec::new(),
            tracks: vec![
                TrackInfo {
                    kind: TrackKind::Mic,
                    device: "MacBook Pro".into(),
                    offset_ms: 0,
                    samples: 0,
                },
                TrackInfo {
                    kind: TrackKind::System,
                    device: "Zoom".into(),
                    offset_ms: 0,
                    samples: 0,
                },
            ],
            bookmarks: m
                .bookmarks
                .iter()
                .map(|b| Bookmark {
                    at_ms: ms(b.at_sec),
                    label: Some(b.label.clone()),
                })
                .collect(),
            participants,
            asr_model: Some("gigaam-v3-rnnt".into()),
            status,
        };
        let looping = matches!(meeting.status, MeetingStatus::Processing { .. });
        DemoMeeting {
            meeting,
            turns,
            summary,
            has_audio: m.has_audio,
            snippet: m.snippet.clone(),
            audio_note: m.audio_note.clone(),
            looping,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, e: EngineEvent) {
        (self.sink)(e);
    }

    /// Background simulation: recorder levels / live phrases, processing progress, downloads, imports.
    /// Spawned once by the shell; runs for the life of the app.
    pub async fn run(self: Arc<Self>) {
        let mut iv = tokio::time::interval(Duration::from_millis(40));
        loop {
            iv.tick().await;
            let events = self.step();
            for e in events {
                self.emit(e);
            }
        }
    }

    fn step(&self) -> Vec<EngineEvent> {
        let mut out: Vec<EngineEvent> = Vec::new();
        let mut sinks: Vec<(EventSink, EngineEvent)> = Vec::new();
        {
            let mut guard = self.lock();
            let g = &mut *guard;
            g.tick += 1;
            let tick = g.tick;

            // recorder
            if g.rec.as_ref().is_some_and(|r| r.paused_at.is_none()) {
                if g.who_left == 0 {
                    let r = g.next_rand();
                    g.who = if r < 0.42 {
                        Who::Me
                    } else if r < 0.86 {
                        Who::Them
                    } else if r < 0.95 {
                        Who::None
                    } else {
                        Who::Both
                    };
                    g.who_left = 12 + (g.next_rand() * 35.0) as u32;
                } else {
                    g.who_left -= 1;
                }
                let t = tick as f32 * 0.04;
                let lv = |on: bool, k: f32| {
                    let a = if on {
                        0.35 + 0.55 * (t * k).sin().abs()
                    } else {
                        0.02
                    };
                    Level {
                        rms: a * 0.6,
                        peak: a,
                    }
                };
                let (me, them) = (
                    matches!(g.who, Who::Me | Who::Both),
                    matches!(g.who, Who::Them | Who::Both),
                );
                out.push(EngineEvent::Levels {
                    mic: lv(me, 7.3),
                    system: lv(them, 6.1),
                });
            }
            if tick.is_multiple_of(12) {
                let live_idx = g.live_idx;
                let next = g.next_live_ms;
                if let Some(r) = g.rec.as_mut() {
                    let el = r.elapsed();
                    out.push(EngineEvent::Recorder {
                        meeting: r.id,
                        state: if r.paused_at.is_some() {
                            RecorderState::Paused
                        } else {
                            RecorderState::Recording
                        },
                        elapsed_ms: el,
                    });
                    if r.paused_at.is_none() && el >= next && live_idx < LIVE_SCRIPT.len() {
                        let (who, track, text) = LIVE_SCRIPT[live_idx];
                        let seg = Segment {
                            start_ms: el,
                            end_ms: el + 4000,
                            track,
                            speaker: who.into(),
                            text: text.into(),
                            words: vec![],
                            confidence: None,
                        };
                        r.turns.push(Turn {
                            speaker: who.into(),
                            start_ms: el,
                            end_ms: el + 4000,
                            text: text.into(),
                        });
                        out.push(EngineEvent::LiveSegment {
                            meeting: r.id,
                            segment: seg,
                        });
                        g.live_idx += 1;
                        g.next_live_ms = el + 6000;
                    }
                }
            }

            // post-call processing
            if tick.is_multiple_of(6) {
                for m in g.meetings.iter_mut() {
                    if let MeetingStatus::Processing { progress, .. } = m.meeting.status.clone() {
                        let step = if m.looping { 0.01 } else { 0.05 };
                        let mut next = progress + step;
                        if m.looping && next >= 0.99 {
                            next = 0.12;
                        }
                        if next >= 1.0 {
                            m.meeting.status = MeetingStatus::Ready;
                            if m.summary.is_none() {
                                m.summary = Some(demo_summary(&m.turns));
                                m.snippet =
                                    m.summary.as_ref().and_then(|s| s.about.first().cloned());
                            }
                        } else {
                            let stage = if next > 0.6 {
                                Stage::Summarize
                            } else {
                                Stage::Transcribe
                            };
                            m.meeting.status = MeetingStatus::Processing {
                                stage,
                                progress: next,
                            };
                        }
                        out.push(EngineEvent::Status {
                            meeting: m.meeting.id,
                            status: m.meeting.status.clone(),
                        });
                    }
                }
            }

            // imports
            if tick.is_multiple_of(5) {
                let mut finished: Vec<(String, String)> = Vec::new();
                let mut running = g.imports.iter().any(|j| j.state == ImportState::Running);
                for j in g.imports.iter_mut() {
                    match j.state {
                        ImportState::Queued if !running => {
                            j.state = ImportState::Running;
                            running = true;
                        }
                        ImportState::Running => {
                            j.progress = (j.progress + 0.04).min(1.0);
                            if j.progress >= 1.0 {
                                j.state = ImportState::Done;
                                finished.push((j.id.clone(), j.file_name.clone()));
                            }
                        }
                        _ => {}
                    }
                }
                for (job, name) in finished {
                    let id = MeetingId::new();
                    let turns = vec![Turn {
                        speaker: "them".into(),
                        start_ms: 2000,
                        end_ms: 9000,
                        text: format!("Файл «{name}» расшифрован на этом компьютере."),
                    }];
                    let summary = demo_summary(&turns);
                    let meeting = Meeting {
                        id,
                        title: name
                            .rsplit_once('.')
                            .map(|(s, _)| s.to_string())
                            .unwrap_or(name.clone()),
                        title_is_auto: false,
                        started_at: Utc::now(),
                        duration_ms: 60_000,
                        language: "ru".into(),
                        expected_speakers: None,
                        expected_speakers_explicit: false,
                        participant_hints: Vec::new(),
                        tracks: vec![],
                        bookmarks: vec![],
                        participants: vec![Participant {
                            key: "them".into(),
                            name: "Собеседник".into(),
                            voice_id: None,
                            suggestions: Vec::new(),
                        }],
                        asr_model: Some("gigaam-v3-rnnt".into()),
                        status: MeetingStatus::Ready,
                    };
                    g.meetings.insert(
                        0,
                        DemoMeeting {
                            meeting,
                            turns,
                            snippet: summary.about.first().cloned(),
                            summary: Some(summary),
                            has_audio: false,
                            audio_note: None,
                            looping: false,
                        },
                    );
                    if let Some(j) = g.imports.iter_mut().find(|j| j.id == job) {
                        j.meeting_id = Some(id.to_string());
                    }
                    out.push(EngineEvent::Status {
                        meeting: id,
                        status: MeetingStatus::Ready,
                    });
                }
            }

            // downloads
            if tick.is_multiple_of(6) {
                let ids: Vec<String> = g
                    .models
                    .iter()
                    .filter(|m| {
                        m.download
                            .as_ref()
                            .is_some_and(|d| matches!(d.state, DownloadState::Running))
                    })
                    .map(|m| m.id.clone())
                    .collect();
                for id in ids {
                    let sink = g.download_sinks.get(&id).cloned();
                    let Some(m) = g.models.iter_mut().find(|m| m.id == id) else {
                        continue;
                    };
                    let Some(d) = m.download.as_mut() else {
                        continue;
                    };
                    d.done_bytes = (d.done_bytes + d.bytes_per_sec / 4).min(d.total_bytes);
                    d.eta_secs = (d.bytes_per_sec > 0)
                        .then(|| (d.total_bytes - d.done_bytes) / d.bytes_per_sec);
                    let ev = if d.done_bytes >= d.total_bytes {
                        let total = d.total_bytes;
                        m.installed = true;
                        m.download = None;
                        EngineEvent::Download {
                            item: id.clone(),
                            done_bytes: total,
                            total_bytes: total,
                            bytes_per_sec: 0,
                            state: DownloadState::Done,
                        }
                    } else {
                        EngineEvent::Download {
                            item: id.clone(),
                            done_bytes: d.done_bytes,
                            total_bytes: d.total_bytes,
                            bytes_per_sec: d.bytes_per_sec,
                            state: d.state.clone(),
                        }
                    };
                    match sink {
                        Some(s) => sinks.push((s, ev)),
                        None => out.push(ev),
                    }
                }
            }
        }
        for (s, e) in sinks {
            s(e);
        }
        out
    }

    fn snapshot_of(r: Option<&Rec>, systems: &str) -> RecorderSnapshot {
        match r {
            Some(r) => RecorderSnapshot {
                phase: if r.paused_at.is_some() {
                    RecorderPhase::Paused
                } else {
                    RecorderPhase::Recording
                },
                meeting_id: Some(r.id.to_string()),
                title: Some(r.title.clone()),
                elapsed_ms: r.elapsed(),
                bookmarks: r.marks.clone(),
                mic_device: Some(r.mic.clone().unwrap_or_else(|| "MacBook Pro".into())),
                system_source: Some(systems.into()),
            },
            None => RecorderSnapshot::default(),
        }
    }
}

impl Inner {
    fn next_rand(&mut self) -> f32 {
        // xorshift64*, enough for demo animation.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        ((self.rng.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32) / (1u64 << 24) as f32
    }

    fn find(&self, id: &str) -> Result<&DemoMeeting> {
        self.meetings
            .iter()
            .find(|m| m.meeting.id.to_string() == id)
            .ok_or_else(|| Error::NotFound(format!("meeting {id}")))
    }

    fn find_mut(&mut self, id: &str) -> Result<&mut DemoMeeting> {
        self.meetings
            .iter_mut()
            .find(|m| m.meeting.id.to_string() == id)
            .ok_or_else(|| Error::NotFound(format!("meeting {id}")))
    }
}

fn demo_summary(turns: &[Turn]) -> Summary {
    Summary {
        title: None,
        about: vec![
            "Итоги собраны локально: обсудили ключевые решения и договорились о следующих шагах."
                .into(),
        ],
        decisions: turns
            .get(2)
            .map(|t| {
                vec![Decision {
                    text: "Двигаемся дальше по плану".into(),
                    at_ms: Some(t.start_ms),
                }]
            })
            .unwrap_or_default(),
        tasks: turns
            .get(1)
            .map(|t| {
                vec![Task {
                    text: "Проверить кнопку над клавиатурой на iPhone".into(),
                    owner: Some("me".into()),
                    due: None,
                    at_ms: Some(t.start_ms),
                    done: false,
                }]
            })
            .unwrap_or_default(),
        open_questions: vec![],
        language: "ru".into(),
        model: "local:gemma-4-e4b".into(),
    }
}

fn clock(ms: Ms) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn markdown_of(m: &DemoMeeting) -> String {
    let name = |k: &str| match k {
        "me" => "Я".to_string(),
        k => m
            .meeting
            .participants
            .iter()
            .find(|p| p.key == k)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Гость".into()),
    };
    let mut out = format!(
        "---\ntitle: {}\ndate: {}\nduration: {}\n---\n\n# {}\n\n",
        m.meeting.title,
        m.meeting.started_at.to_rfc3339(),
        clock(m.meeting.duration_ms),
        m.meeting.title
    );
    if let Some(s) = &m.summary {
        out.push_str("## О чём встреча\n\n");
        for p in &s.about {
            out.push_str(p);
            out.push_str("\n\n");
        }
        if !s.decisions.is_empty() {
            out.push_str("## Договорённости\n\n");
            for d in &s.decisions {
                out.push_str(&format!(
                    "- {}{}\n",
                    d.text,
                    d.at_ms
                        .map(|a| format!(" [{}]", clock(a)))
                        .unwrap_or_default()
                ));
            }
            out.push('\n');
        }
        if !s.tasks.is_empty() {
            out.push_str("## Задачи\n\n");
            for t in &s.tasks {
                out.push_str(&format!(
                    "- [{}] {}{}\n",
                    if t.done { "x" } else { " " },
                    t.text,
                    t.owner
                        .as_ref()
                        .map(|o| format!(" — {}", name(o)))
                        .unwrap_or_default()
                ));
            }
            out.push('\n');
        }
    }
    out.push_str("## Расшифровка\n\n");
    for t in &m.turns {
        out.push_str(&format!(
            "[{}] {}: {}\n",
            clock(t.start_ms),
            name(&t.speaker),
            t.text
        ));
    }
    out
}

#[async_trait]
impl Backend for DemoBackend {
    async fn list_meetings(&self) -> Result<Vec<MeetingListItem>> {
        let g = self.lock();
        let mut list: Vec<MeetingListItem> = g
            .meetings
            .iter()
            .map(|m| MeetingListItem {
                id: m.meeting.id.to_string(),
                title: m.meeting.title.clone(),
                started_at: m.meeting.started_at,
                duration_ms: m.meeting.duration_ms,
                status: m.meeting.status.clone(),
                snippet: m.snippet.clone(),
                audio_note: m.audio_note.clone(),
            })
            .collect();
        if let Some(r) = &g.rec {
            list.push(MeetingListItem {
                id: r.id.to_string(),
                title: if r.title.is_empty() {
                    "Без названия".into()
                } else {
                    r.title.clone()
                },
                started_at: Utc::now() - chrono::Duration::milliseconds(r.elapsed() as i64),
                duration_ms: r.elapsed(),
                status: MeetingStatus::Recording,
                snippet: Some("Идёт запись — текст пишется на лету".into()),
                audio_note: None,
            });
        }
        list.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(list)
    }

    async fn get_meeting(&self, id: &str) -> Result<MeetingDetail> {
        let g = self.lock();
        let m = g.find(id)?;
        Ok(MeetingDetail {
            meeting: m.meeting.clone(),
            turns: m.turns.clone(),
            summary: m.summary.clone(),
            audio_url: None,
            has_audio: m.has_audio,
        })
    }

    async fn rename_meeting(&self, id: &str, title: &str) -> Result<()> {
        self.lock().find_mut(id)?.meeting.title = title.trim().to_string();
        Ok(())
    }

    async fn delete_meeting(&self, id: &str) -> Result<()> {
        self.lock()
            .meetings
            .retain(|m| m.meeting.id.to_string() != id);
        Ok(())
    }

    async fn rename_speaker(&self, id: &str, key: &str, name: &str) -> Result<()> {
        let mut g = self.lock();
        let m = g.find_mut(id)?;
        match m.meeting.participants.iter_mut().find(|p| p.key == key) {
            Some(p) => p.name = name.to_string(),
            None => m.meeting.participants.push(Participant {
                key: key.into(),
                name: name.into(),
                voice_id: None,
                suggestions: Vec::new(),
            }),
        }
        Ok(())
    }

    async fn set_task_done(&self, id: &str, index: usize, done: bool) -> Result<()> {
        let mut g = self.lock();
        if let Some(t) = g
            .find_mut(id)?
            .summary
            .as_mut()
            .and_then(|s| s.tasks.get_mut(index))
        {
            t.done = done;
        }
        Ok(())
    }

    async fn regenerate_summary(&self, id: &str) -> Result<Summary> {
        tokio::time::sleep(Duration::from_millis(900)).await;
        let mut g = self.lock();
        let m = g.find_mut(id)?;
        if m.summary.is_none() {
            m.summary = Some(demo_summary(&m.turns));
        }
        Ok(m.summary.clone().unwrap_or_default())
    }

    async fn meeting_retranscribe(&self, _id: &str) -> Result<()> {
        Err(Error::Invalid(
            "stored audio is unavailable in demo mode".to_owned(),
        ))
    }

    async fn export_meeting(&self, id: &str, format: ExportFormat) -> Result<String> {
        let (title, body) = {
            let g = self.lock();
            let m = g.find(id)?;
            let body = match format {
                ExportFormat::Md => markdown_of(m),
                ExportFormat::Json => serde_json::to_string_pretty(&serde_json::json!({ "meeting": m.meeting, "turns": m.turns, "summary": m.summary })).map_err(|e| Error::Storage(e.to_string()))?,
            };
            (m.meeting.title.clone(), body)
        };
        let dir = std::env::temp_dir().join("YadaCall-demo-export");
        std::fs::create_dir_all(&dir)?;
        let safe: String = title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = dir.join(format!(
            "{safe}.{}",
            if format == ExportFormat::Md {
                "md"
            } else {
                "json"
            }
        ));
        std::fs::write(&path, body)?;
        Ok(path.to_string_lossy().into_owned())
    }

    async fn meeting_dir(&self, _id: &str) -> Result<Option<PathBuf>> {
        Ok(None)
    }

    async fn copy_for_claude(&self, id: &str) -> Result<String> {
        let g = self.lock();
        Ok(markdown_of(g.find(id)?))
    }

    async fn recorder_state(&self) -> Result<RecorderSnapshot> {
        Ok(Self::snapshot_of(self.lock().rec.as_ref(), "Zoom"))
    }

    async fn recorder_start(&self, opts: RecorderStartOptions) -> Result<RecorderSnapshot> {
        let mut g = self.lock();
        if g.rec.is_none() {
            let mic = opts
                .mic_device
                .as_ref()
                .and_then(|id| g.devices.iter().find(|d| &d.id == id))
                .map(|d| d.name.clone());
            let r = Rec {
                id: MeetingId::new(),
                title: opts.title.unwrap_or_default().trim().to_string(),
                started: std::time::Instant::now(),
                paused_at: None,
                paused_total: Duration::ZERO,
                marks: vec![],
                turns: vec![],
                mic,
            };
            let id = r.id;
            g.rec = Some(r);
            g.who = Who::Me;
            g.who_left = 75;
            g.live_idx = 0;
            g.next_live_ms = 4000;
            drop(g);
            self.emit(EngineEvent::Recorder {
                meeting: id,
                state: RecorderState::Recording,
                elapsed_ms: 0,
            });
            return Ok(Self::snapshot_of(self.lock().rec.as_ref(), "Zoom"));
        }
        Ok(Self::snapshot_of(g.rec.as_ref(), "Zoom"))
    }

    async fn recorder_pause(&self) -> Result<()> {
        let ev = {
            let mut g = self.lock();
            g.rec.as_mut().filter(|r| r.paused_at.is_none()).map(|r| {
                r.paused_at = Some(std::time::Instant::now());
                EngineEvent::Recorder {
                    meeting: r.id,
                    state: RecorderState::Paused,
                    elapsed_ms: r.elapsed(),
                }
            })
        };
        if let Some(e) = ev {
            self.emit(e);
        }
        Ok(())
    }

    async fn recorder_resume(&self) -> Result<()> {
        let ev = {
            let mut g = self.lock();
            g.rec.as_mut().filter(|r| r.paused_at.is_some()).map(|r| {
                if let Some(p) = r.paused_at.take() {
                    r.paused_total += p.elapsed();
                }
                EngineEvent::Recorder {
                    meeting: r.id,
                    state: RecorderState::Recording,
                    elapsed_ms: r.elapsed(),
                }
            })
        };
        if let Some(e) = ev {
            self.emit(e);
        }
        Ok(())
    }

    async fn recorder_stop(&self) -> Result<String> {
        let (id, ev) = {
            let mut g = self.lock();
            let Some(r) = g.rec.take() else {
                return Ok(String::new());
            };
            let dur = r.elapsed();
            let meeting = Meeting {
                id: r.id,
                title: if r.title.is_empty() {
                    "Без названия".into()
                } else {
                    r.title.clone()
                },
                title_is_auto: false,
                started_at: Utc::now() - chrono::Duration::milliseconds(dur as i64),
                duration_ms: dur,
                language: "ru".into(),
                expected_speakers: None,
                expected_speakers_explicit: false,
                participant_hints: Vec::new(),
                tracks: vec![],
                bookmarks: r
                    .marks
                    .iter()
                    .map(|&at_ms| Bookmark { at_ms, label: None })
                    .collect(),
                participants: vec![Participant {
                    key: "them".into(),
                    name: "Собеседник".into(),
                    voice_id: None,
                    suggestions: Vec::new(),
                }],
                asr_model: Some("gigaam-v3-rnnt".into()),
                status: MeetingStatus::Processing {
                    stage: Stage::Transcribe,
                    progress: 0.0,
                },
            };
            let id = meeting.id;
            g.meetings.insert(
                0,
                DemoMeeting {
                    meeting,
                    turns: r.turns,
                    summary: None,
                    has_audio: false,
                    snippet: Some("Собираю итоги…".into()),
                    audio_note: None,
                    looping: false,
                },
            );
            (
                id,
                EngineEvent::Recorder {
                    meeting: id,
                    state: RecorderState::Stopped,
                    elapsed_ms: dur,
                },
            )
        };
        self.emit(ev);
        Ok(id.to_string())
    }

    async fn recorder_cancel(&self) -> Result<()> {
        let ev = self.lock().rec.take().map(|r| EngineEvent::Recorder {
            meeting: r.id,
            state: RecorderState::Cancelled,
            elapsed_ms: r.elapsed(),
        });
        if let Some(e) = ev {
            self.emit(e);
        }
        Ok(())
    }

    async fn recorder_bookmark(&self, _label: Option<String>) -> Result<()> {
        if let Some(r) = self.lock().rec.as_mut() {
            let at = r.elapsed();
            r.marks.push(at);
        }
        Ok(())
    }

    async fn list_input_devices(&self) -> Result<Vec<InputDevice>> {
        Ok(self.lock().devices.clone())
    }

    async fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(vec![]);
        }
        let g = self.lock();
        Ok(g.meetings
            .iter()
            .flat_map(|m| {
                let needle = needle.clone();
                m.turns
                    .iter()
                    .filter(move |t| t.text.to_lowercase().contains(&needle))
                    .map(|t| SearchHit {
                        meeting_id: m.meeting.id.to_string(),
                        title: m.meeting.title.clone(),
                        started_at: m.meeting.started_at,
                        at_ms: t.start_ms,
                        speaker: t.speaker.clone(),
                        text: t.text.clone(),
                        score: 1.0,
                    })
            })
            .collect())
    }

    async fn ask(&self, _question: &str, sink: AskSink) -> Result<()> {
        let generated;
        let (answer, citations, excerpts, provider) = {
            let g = self.lock();
            let cites: Vec<Citation> = g
                .ask
                .citations
                .iter()
                .filter_map(|c| {
                    let m = g.find(&c.meeting).ok()?;
                    Some(Citation {
                        n: c.n,
                        meeting_id: c.meeting.clone(),
                        meeting_title: m.meeting.title.clone(),
                        started_at: m.meeting.started_at,
                        at_ms: ms(c.at_sec),
                        speaker: c.speaker.clone(),
                        quote: c.quote.clone(),
                    })
                })
                .collect();
            let provider = match &g.settings.summaries.mode {
                SummaryMode::Off | SummaryMode::Local { .. } => "local",
                SummaryMode::Ollama { .. } => "Ollama Cloud",
                SummaryMode::Api { .. } => "Claude",
            };
            generated = g.settings.summaries.mode != SummaryMode::Off;
            (g.ask.answer.clone(), cites, g.ask.excerpts, provider)
        };
        tokio::time::sleep(Duration::from_millis(500)).await;
        // stream word by word, keeping the trailing space with each token
        let answer = if generated { answer } else { String::new() };
        let mut start = 0;
        for (i, c) in answer.char_indices() {
            if c == ' ' {
                sink(AskEvent::Token {
                    text: answer[start..=i].to_string(),
                });
                start = i + 1;
                tokio::time::sleep(Duration::from_millis(35)).await;
            }
        }
        if start < answer.len() {
            sink(AskEvent::Token {
                text: answer[start..].to_string(),
            });
        }
        sink(AskEvent::Citations { items: citations });
        sink(AskEvent::Done {
            sent: SentNote {
                excerpts,
                provider: provider.into(),
                audio: false,
            },
            generated,
        });
        Ok(())
    }

    async fn list_tasks(&self, include_done: bool) -> Result<Vec<TaskItem>> {
        let g = self.lock();
        let name_of = |m: &DemoMeeting, key: &str| -> String {
            if key == "me" {
                return "me".into();
            }
            m.meeting
                .participants
                .iter()
                .find(|p| p.key == key)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| key.to_string())
        };
        Ok(g.meetings
            .iter()
            .flat_map(|m| {
                m.summary.iter().flat_map(move |s| {
                    s.tasks.iter().enumerate().map(move |(index, t)| TaskItem {
                        meeting_id: m.meeting.id.to_string(),
                        meeting_title: m.meeting.title.clone(),
                        started_at: m.meeting.started_at,
                        index,
                        text: t.text.clone(),
                        owner: t.owner.as_ref().map(|o| name_of(m, o)),
                        due: t.due.clone(),
                        at_ms: t.at_ms,
                        done: t.done,
                    })
                })
            })
            .filter(|t| include_done || !t.done)
            .collect())
    }

    async fn import_audio(&self, paths: Vec<String>) -> Result<Vec<ImportJob>> {
        let mut g = self.lock();
        let jobs: Vec<ImportJob> = paths
            .iter()
            .map(|p| ImportJob {
                id: uuid::Uuid::now_v7().to_string(),
                file_name: p.rsplit(['/', '\\']).next().unwrap_or(p).to_string(),
                state: ImportState::Queued,
                progress: 0.0,
                meeting_id: None,
                error: None,
            })
            .collect();
        for j in jobs.iter().rev() {
            g.imports.insert(0, j.clone());
        }
        Ok(jobs)
    }

    async fn list_imports(&self) -> Result<Vec<ImportJob>> {
        Ok(self.lock().imports.clone())
    }

    async fn models_overview(&self) -> Result<ModelsOverview> {
        let g = self.lock();
        let mut hardware = g.hardware.clone();
        hardware.models_bytes = g
            .models
            .iter()
            .filter(|m| m.installed)
            .map(|m| m.size_bytes)
            .sum();
        Ok(ModelsOverview {
            hardware,
            models: g.models.clone(),
            runtime: Some(crate::api::RuntimeInfo {
                id: "llama-demo".into(),
                name: "llama.cpp b8639".into(),
                size_bytes: 40_000_000,
                installed: true,
                busy: false,
            }),
        })
    }

    async fn model_download(&self, id: &str, progress: EventSink) -> Result<()> {
        if id == "llama-demo" {
            return Ok(()); // the runtime row's "reinstall": nothing to do in the demo
        }
        let mut g = self.lock();
        let m = g
            .models
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| Error::NotFound(format!("model {id}")))?;
        let done = m.download.as_ref().map(|d| d.done_bytes).unwrap_or(0);
        m.download = Some(ModelDownload {
            state: DownloadState::Running,
            done_bytes: done,
            total_bytes: m.size_bytes,
            bytes_per_sec: 6_200_000,
            eta_secs: None,
        });
        g.download_sinks.insert(id.to_string(), progress);
        Ok(())
    }

    async fn model_pause(&self, id: &str) -> Result<()> {
        if let Some(d) = self
            .lock()
            .models
            .iter_mut()
            .find(|m| m.id == id)
            .and_then(|m| m.download.as_mut())
        {
            d.state = DownloadState::Paused;
            d.bytes_per_sec = d.bytes_per_sec.max(1);
        }
        Ok(())
    }

    async fn model_cancel(&self, id: &str) -> Result<()> {
        let mut g = self.lock();
        if let Some(m) = g.models.iter_mut().find(|m| m.id == id) {
            m.download = None;
        }
        g.download_sinks.remove(id);
        Ok(())
    }

    async fn model_remove(&self, id: &str) -> Result<()> {
        if let Some(m) = self
            .lock()
            .models
            .iter_mut()
            .find(|m| m.id == id && !m.active)
        {
            m.installed = false;
        }
        Ok(())
    }

    async fn model_set_active(&self, id: &str) -> Result<()> {
        let mut g = self.lock();
        let Some((kind, installed)) = g
            .models
            .iter()
            .find(|m| m.id == id)
            .map(|m| (m.kind, m.installed))
        else {
            return Ok(());
        };
        if installed {
            for m in g.models.iter_mut().filter(|m| m.kind == kind) {
                m.active = m.id == id;
            }
        }
        Ok(())
    }

    async fn model_benchmark(&self, id: &str) -> Result<BenchmarkResult> {
        tokio::time::sleep(Duration::from_millis(1200)).await;
        Ok(BenchmarkResult {
            model_id: id.to_string(),
            realtime_factor: 42.0,
        })
    }

    async fn ollama_status(&self, base: OllamaBase) -> Result<OllamaStatus> {
        let cloud = |name: &str| yc_llm::OllamaModel {
            name: name.into(),
            size_bytes: 338,
            cloud: true,
        };
        Ok(OllamaStatus {
            running: true,
            version: Some("0.40.3".into()),
            signed_in: true,
            models: match base {
                OllamaBase::Local => vec![
                    cloud("glm-5.2:cloud"),
                    yc_llm::OllamaModel {
                        name: "gemma4:e4b".into(),
                        size_bytes: 5_000_000_000,
                        cloud: false,
                    },
                ],
                OllamaBase::Cloud => vec![cloud("gpt-oss:120b"), cloud("glm-5.2")],
            },
            error: None,
        })
    }

    async fn test_summary_provider(&self, _mode: Option<SummaryMode>) -> Result<ProviderTest> {
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(ProviderTest {
            ok: true,
            latency_ms: 700,
            reply: Some("Да, я на связи.".into()),
            error: None,
        })
    }

    async fn get_settings(&self) -> Result<Settings> {
        Ok(self.lock().settings.clone())
    }

    async fn update_settings(&self, patch: SettingsPatch) -> Result<Settings> {
        let mut g = self.lock();
        g.settings = g.settings.clone().apply(patch);
        Ok(g.settings.clone())
    }

    async fn permissions(&self) -> Result<Permissions> {
        Ok(self.lock().perms)
    }

    async fn request_permission(&self, kind: PermissionKind) -> Result<Permissions> {
        let mut g = self.lock();
        match kind {
            PermissionKind::Microphone => g.perms.microphone = PermissionState::Granted,
            PermissionKind::SystemAudio => g.perms.system_audio = PermissionState::Granted,
        }
        Ok(g.perms)
    }

    async fn open_system_settings(&self, _kind: PermissionKind) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> Arc<DemoBackend> {
        DemoBackend::new(Arc::new(|_| {}))
    }

    #[test]
    fn demo_data_parses_and_ids_are_valid() {
        let b = backend();
        assert!(b.lock().meetings.len() >= 6);
    }

    #[tokio::test]
    async fn lists_tasks_and_open_count_matches_design() {
        let b = backend();
        assert_eq!(b.list_tasks(false).await.unwrap().len(), 3);
        assert!(b.list_tasks(true).await.unwrap().len() > 3);
    }

    #[tokio::test]
    async fn recorder_round_trip_creates_a_processing_meeting() {
        let b = backend();
        b.recorder_start(RecorderStartOptions {
            title: Some("Тест".into()),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            b.recorder_state().await.unwrap().phase,
            RecorderPhase::Recording
        );
        b.recorder_bookmark(None).await.unwrap();
        let id = b.recorder_stop().await.unwrap();
        let d = b.get_meeting(&id).await.unwrap();
        assert!(matches!(d.meeting.status, MeetingStatus::Processing { .. }));
        assert_eq!(b.recorder_state().await.unwrap().phase, RecorderPhase::Idle);
    }

    #[tokio::test]
    async fn settings_patch_applies() {
        let b = backend();
        let s = b
            .update_settings(SettingsPatch {
                accent: Some("#b8acf2".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(s.accent, "#b8acf2");
    }

    #[tokio::test]
    async fn ask_streams_tokens_then_citations_then_done() {
        let b = backend();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let l2 = log.clone();
        b.ask(
            "q",
            Arc::new(move |e| {
                let tag = match e {
                    AskEvent::Token { .. } => "t",
                    AskEvent::Citations { .. } => "c",
                    AskEvent::Done { .. } => "d",
                    AskEvent::Error { .. } => "e",
                };
                l2.lock().unwrap().push(tag.into());
            }),
        )
        .await
        .unwrap();
        let log = log.lock().unwrap();
        assert_eq!(log.last().map(String::as_str), Some("d"));
        assert!(log.iter().any(|t| t == "c"));
    }

    #[tokio::test]
    async fn ask_with_summaries_off_returns_sources_without_text() {
        let b = backend();
        b.update_settings(SettingsPatch {
            summaries: Some(SummaryPatch {
                mode: Some(SummaryMode::Off),
                key: None,
            }),
            ..Default::default()
        })
        .await
        .unwrap();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let l2 = log.clone();
        b.ask(
            "q",
            Arc::new(move |e| {
                let tag = match e {
                    AskEvent::Token { .. } => "t",
                    AskEvent::Citations { .. } => "c",
                    AskEvent::Done {
                        generated: false, ..
                    } => "d",
                    AskEvent::Done { .. } => "D",
                    AskEvent::Error { .. } => "e",
                };
                l2.lock().unwrap().push(tag.into());
            }),
        )
        .await
        .unwrap();
        assert_eq!(*log.lock().unwrap(), ["c", "d"]);
    }
}
