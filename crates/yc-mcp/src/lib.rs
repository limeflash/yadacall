//! Read access to a YadaCall library over the Model Context Protocol.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rmcp::model::{
    Implementation, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    ServerConfig,
};
use rmcp::{
    Json, ServerHandler, ServiceExt, handler::server::wrapper::Parameters, schemars, tool,
    tool_handler, tool_router, transport::stdio,
};
use serde::Deserialize;
use serde_json::{Value, json};
use yc_core::{
    meeting::MeetingId,
    ports::{EmbedKind, TextEmbedder},
};
use yc_hub::{catalog, downloader::model_storage_dir};
use yc_ml::{Backend, OnnxTextEmbedder, Pooling, TextEmbeddingConfig};
use yc_store::{SearchFilters, Store};

/// Start a stdio MCP server for the configured YadaCall library.
pub async fn serve(
    data_dir: Option<PathBuf>,
    library_dir: Option<PathBuf>,
    allow_write: bool,
) -> anyhow::Result<()> {
    let data = data_dir
        .or_else(|| std::env::var_os("YC_DATA_DIR").map(PathBuf::from))
        .unwrap_or_else(default_data_dir);
    let db = data.join("index.sqlite");
    let library = library_dir
        .or_else(|| configured_library(&db))
        .unwrap_or_else(default_library_dir);
    let store = if allow_write {
        Store::open(&library, &db)?
    } else {
        Store::open_read_only(&library, &db)?
    };
    let embedder = load_query_embedder(&store, &data);
    let server = YadaCallMcp {
        store,
        allow_write,
        embedder: Mutex::new(embedder),
    };
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn default_data_dir() -> PathBuf {
    std::env::var_os("YADACALL_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            #[cfg(target_os = "macos")]
            {
                dirs_path().join("Library/Application Support/app.yadacall.desktop")
            }
            #[cfg(target_os = "windows")]
            {
                std::env::var_os("APPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| dirs_path().join("AppData/Roaming"))
                    .join("app.yadacall.desktop")
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                dirs_path().join(".local/share/app.yadacall.desktop")
            }
        })
}

fn dirs_path() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
fn default_library_dir() -> PathBuf {
    let russian =
        sys_locale::get_locale().is_some_and(|l| l.to_ascii_lowercase().starts_with("ru"));
    dirs_path().join(if russian {
        "Созвоны"
    } else {
        "YadaCall"
    })
}

fn configured_library(db: &Path) -> Option<PathBuf> {
    let c = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .ok()?;
    let raw: String = c
        .query_row(
            "SELECT value_json FROM settings WHERE key='settings'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let path = value.get("libraryDir")?.as_str()?;
    Some(expand_tilde(path))
}
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(p) = path.strip_prefix("~/") {
        dirs_path().join(p)
    } else {
        PathBuf::from(path)
    }
}

fn load_query_embedder(store: &Store, data: &Path) -> Option<OnnxTextEmbedder> {
    const MODEL_ID: &str = "multilingual-e5-small-int8";
    if store.active_embedding_model().ok().flatten()?.0 != MODEL_ID {
        return None;
    }
    let model_catalog = catalog::embedded().ok()?;
    let spec = model_catalog
        .models
        .iter()
        .find(|model| model.id == MODEL_ID)?;
    let dir = model_storage_dir(data, spec).ok()?;
    if !spec.files.iter().filter(|file| !file.optional).all(|file| {
        std::fs::metadata(dir.join(&file.path)).is_ok_and(|meta| meta.len() == file.size)
    }) {
        return None;
    }
    let config = TextEmbeddingConfig {
        onnx_path: dir.join("onnx/model_int8.onnx"),
        tokenizer_path: dir.join("tokenizer.json"),
        query_prefix: "query: ".into(),
        passage_prefix: "passage: ".into(),
        pooling: Pooling::Mean,
        max_tokens: 512,
        dims: 384,
    };
    OnnxTextEmbedder::load(config, Backend::Auto, &dir).ok()
}

struct YadaCallMcp {
    store: Store,
    allow_write: bool,
    embedder: Mutex<Option<OnnxTextEmbedder>>,
}

impl YadaCallMcp {
    fn active_router(&self) -> rmcp::handler::server::tool::ToolRouter<Self> {
        let mut router = Self::tool_router();
        if !self.allow_write {
            router.disable_route("complete_task");
        }
        router
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ListMeetings {
    since: Option<String>,
    until: Option<String>,
    folder: Option<String>,
    favorite: Option<bool>,
    query: Option<String>,
    #[serde(default = "limit20")]
    limit: usize,
}
fn limit20() -> usize {
    20
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GetMeeting {
    id: String,
    include: Option<Vec<String>>,
    #[serde(default = "max_chars")]
    max_chars: usize,
}
fn max_chars() -> usize {
    60_000
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SearchMeetings {
    query: String,
    folder: Option<String>,
    #[serde(default = "limit10")]
    limit: usize,
}
fn limit10() -> usize {
    10
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ListTasks {
    #[serde(default = "open_status")]
    status: String,
    due_before: Option<String>,
    folder: Option<String>,
    #[serde(default = "limit50")]
    limit: usize,
}
fn open_status() -> String {
    "open".into()
}
fn limit50() -> usize {
    50
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Upcoming {
    #[serde(default = "hours24")]
    hours: u32,
}
fn hours24() -> u32 {
    24
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct CompleteTask {
    task_id: String,
    done: bool,
}

#[tool_router]
impl YadaCallMcp {
    #[tool(name = "list_meetings", description = "List meetings / Список встреч")]
    fn list_meetings(&self, Parameters(p): Parameters<ListMeetings>) -> Json<Value> {
        let folders = self.store.list_folders().unwrap_or_default();
        let folder_id = p.folder.as_deref().and_then(|f| {
            folders
                .iter()
                .find(|x| x.id == f || x.name.eq_ignore_ascii_case(f))
                .map(|x| x.id.clone())
        });
        let meetings = self
            .store
            .list_meetings_filtered(10_000, 0, folder_id.as_deref(), p.favorite == Some(true))
            .unwrap_or_default();
        let rows: Vec<Value> = meetings.into_iter().filter(|m| p.favorite != Some(false) || !m.favorite)
            .filter(|m| p.since.as_ref().is_none_or(|v| m.started_at >= *v) && p.until.as_ref().is_none_or(|v| m.started_at <= *v))
            .filter(|m| p.query.as_ref().is_none_or(|q| m.title.to_lowercase().contains(&q.to_lowercase())))
            .take(p.limit.clamp(1, 200))
            .map(|m| {
                let folder_name = m.folder_id.as_deref().and_then(|id| folders.iter().find(|f| f.id == id).map(|f| f.name.clone())).unwrap_or_else(|| "Без папки".into());
                json!({"id":m.id.to_string(),"title":m.title,"startedAt":m.started_at,"durationMs":m.duration_ms,"folder":folder_name,"favorite":m.favorite})
            }).collect();
        Json(json!({"meetings":rows}))
    }
    #[tool(
        name = "get_meeting",
        description = "Get summary, transcript, bookmarks, and tasks / Получить итоги, расшифровку, закладки и задачи"
    )]
    fn get_meeting(&self, Parameters(p): Parameters<GetMeeting>) -> Json<Value> {
        let Ok(id) = p.id.parse::<MeetingId>() else {
            return Json(json!({"error":"invalid meeting id"}));
        };
        let Ok(r) = self.store.get_meeting(id) else {
            return Json(json!({"error":"meeting not found"}));
        };
        let include = p.include.unwrap_or_else(|| {
            vec![
                "summary".into(),
                "transcript".into(),
                "bookmarks".into(),
                "tasks".into(),
            ]
        });
        let mut out = json!({"id":id.to_string(),"title":r.meeting.title,"startedAt":r.meeting.started_at.to_rfc3339(),"durationMs":r.meeting.duration_ms,"folder":r.folder,"favorite":r.meeting.favorite});
        if include.iter().any(|x| x == "summary") {
            out["summary"] = serde_json::to_value(&r.summary).unwrap_or(Value::Null);
        }
        if include.iter().any(|x| x == "bookmarks") {
            out["bookmarks"] = serde_json::to_value(&r.meeting.bookmarks).unwrap_or(Value::Null);
        }
        if include.iter().any(|x| x == "tasks") {
            out["tasks"] =
                serde_json::to_value(r.summary.as_ref().map(|s| &s.tasks)).unwrap_or(Value::Null);
        }
        if include.iter().any(|x| x == "transcript") {
            let lines: Vec<String> = r
                .transcript
                .segments
                .iter()
                .map(|s| {
                    format!(
                        "{} {}: {}",
                        mmss(s.start_ms),
                        speaker(&r.meeting.participants, &s.speaker),
                        s.text
                    )
                })
                .collect();
            let transcript = lines.join("\n");
            let cap = p.max_chars.min(200_000);
            let truncated = transcript.chars().count() > cap;
            let text: String = transcript.chars().take(cap).collect();
            out["transcript"] = json!({"text":text,"truncated":truncated,"maxChars":cap});
        }
        Json(out)
    }
    #[tool(
        name = "search_meetings",
        description = "Search meeting transcripts with FTS / Поиск по расшифровкам встреч"
    )]
    fn search_meetings(&self, Parameters(p): Parameters<SearchMeetings>) -> Json<Value> {
        let folder_id = self
            .store
            .list_folders()
            .unwrap_or_default()
            .iter()
            .find(|x| {
                p.folder
                    .as_deref()
                    .is_some_and(|f| x.id == f || x.name.eq_ignore_ascii_case(f))
            })
            .map(|x| x.id.clone());
        let query_vector = self.embedder.lock().ok().and_then(|mut embedder| {
            embedder.as_mut().and_then(|model| {
                model
                    .embed(&[&p.query], EmbedKind::Query)
                    .ok()?
                    .into_iter()
                    .next()
            })
        });
        let result = self.store.search(
            &p.query,
            query_vector.as_deref(),
            &SearchFilters {
                folder_id,
                ..Default::default()
            },
            p.limit.clamp(1, 50),
        );
        Json(match result {
            Ok(hits) => {
                json!({"results":hits.into_iter().map(|h| json!({"meetingId":h.meeting_id.to_string(),"title":h.meeting_title,"startedAt":h.started_at,"atMs":h.start_ms,"snippet":h.snippet})).collect::<Vec<_>>(),"mode":if query_vector.is_some() { "hybrid" } else { "fts" }})
            }
            Err(e) => json!({"error":e.to_string()}),
        })
    }
    #[tool(
        name = "list_tasks",
        description = "List tasks across meetings / Список задач по встречам"
    )]
    fn list_tasks(&self, Parameters(p): Parameters<ListTasks>) -> Json<Value> {
        let folders = self.store.list_folders().unwrap_or_default();
        let folder_id = p.folder.as_deref().and_then(|f| {
            folders
                .iter()
                .find(|x| x.id == f || x.name.eq_ignore_ascii_case(f))
                .map(|x| x.id.clone())
        });
        let tasks = self
            .store
            .list_tasks(false)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| match p.status.as_str() {
                "open" => !t.done,
                "done" => t.done,
                _ => true,
            })
            .filter(|t| {
                folder_id
                    .as_ref()
                    .is_none_or(|id| t.meeting_folder_id.as_ref() == Some(id))
            })
            .filter(|t| {
                p.due_before
                    .as_ref()
                    .is_none_or(|d| t.due.as_ref().is_some_and(|v| v <= d))
            })
            .take(p.limit.clamp(1, 500));
        Json(json!({"tasks":tasks.map(|t| {
            let start = self.store.get_meeting(t.meeting_id).ok().map(|r| r.meeting.started_at.to_rfc3339());
            let folder = t.meeting_folder_id.as_deref().and_then(|id| folders.iter().find(|f| f.id == id).map(|f| f.name.clone())).unwrap_or_else(|| "Без папки".into());
            json!({"taskId":format!("{}:{}",t.meeting_id,t.idx),"text":t.text,"owner":t.owner,"due":t.due,"done":t.done,"meetingId":t.meeting_id.to_string(),"meetingTitle":t.meeting_title,"meetingStartedAt":start,"atMs":t.at_ms,"folder":folder,"favorite":t.meeting_favorite})
        }).collect::<Vec<_>>() }))
    }
    #[tool(
        name = "upcoming_meetings",
        description = "List upcoming calendar events / Предстоящие встречи из календаря"
    )]
    fn upcoming_meetings(&self, Parameters(p): Parameters<Upcoming>) -> Json<Value> {
        Json(
            json!({"meetings":[],"note":format!("Calendar cache is unavailable to standalone MCP server (requested {} hours). / Кэш календаря недоступен отдельному MCP-серверу.",p.hours)}),
        )
    }
    #[tool(
        name = "complete_task",
        description = "Mark a task done / Отметить задачу выполненной"
    )]
    fn complete_task(&self, Parameters(p): Parameters<CompleteTask>) -> Json<Value> {
        if !self.allow_write {
            return Json(json!({"error":"write access is disabled; restart with --allow-write"}));
        }
        let Some((meeting_id_text, task_index_text)) = p.task_id.rsplit_once(':') else {
            return Json(json!({"error":"taskId must be meetingId:index"}));
        };
        let Ok(id) = meeting_id_text.parse::<MeetingId>() else {
            return Json(json!({"error":"invalid meeting id"}));
        };
        let Ok(idx) = task_index_text.parse::<u32>() else {
            return Json(json!({"error":"invalid task index"}));
        };
        Json(match self.store.set_task_done(id, idx, p.done) {
            Ok(()) => json!({"ok":true}),
            Err(e) => json!({"error":e.to_string()}),
        })
    }
}

#[tool_handler(router = self.active_router())]
impl ServerHandler for YadaCallMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("yadacall", env!("CARGO_PKG_VERSION")))
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, rmcp::ErrorData>>
    + rmcp::service::MaybeSendFuture
    + '_ {
        let resources = self
            .store
            .list_meetings(20, 0)
            .unwrap_or_default()
            .into_iter()
            .map(|m| {
                Resource::new(format!("yadacall://meeting/{}", m.id), m.title.clone())
                    .with_title(m.title)
                    .with_mime_type("text/markdown")
            })
            .collect();
        std::future::ready(Ok(ListResourcesResult::with_all_items(resources)))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResponse, rmcp::ErrorData>>
    + rmcp::service::MaybeSendFuture
    + '_ {
        let answer = request
            .uri
            .strip_prefix("yadacall://meeting/")
            .and_then(|raw| raw.parse::<MeetingId>().ok())
            .and_then(|id| self.store.get_meeting(id).ok().map(|r| (id, r)))
            .map(|(id, r)| {
                let summary = r
                    .summary
                    .as_ref()
                    .map(|s| {
                        let about = s
                            .about
                            .iter()
                            .map(|x| format!("- {x}"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let decisions = s
                            .decisions
                            .iter()
                            .map(|x| format!("- {}", x.text))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let tasks = s
                            .tasks
                            .iter()
                            .map(|x| {
                                format!(
                                    "- [{}] {}{}",
                                    if x.done { "x" } else { " " },
                                    x.text,
                                    x.due
                                        .as_ref()
                                        .map(|d| format!(" (due {d})"))
                                        .unwrap_or_default()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        format!("{about}\n\n### Decisions\n{decisions}\n\n### Tasks\n{tasks}")
                    })
                    .unwrap_or_default();
                let lines = r
                    .transcript
                    .segments
                    .iter()
                    .map(|s| {
                        format!(
                            "{} **{}:** {}",
                            mmss(s.start_ms),
                            speaker(&r.meeting.participants, &s.speaker),
                            s.text
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = format!(
                    "# {}\n\nStarted: {}\n\n## Summary\n{}\n\n## Transcript\n{}",
                    r.meeting.title,
                    r.meeting.started_at.to_rfc3339(),
                    summary,
                    lines
                );
                ReadResourceResult::new(vec![ResourceContents::text(
                    text,
                    format!("yadacall://meeting/{id}"),
                )])
                .into()
            })
            .ok_or_else(|| rmcp::ErrorData::invalid_params("unknown meeting resource", None));
        std::future::ready(answer)
    }
}

fn mmss(ms: u64) -> String {
    format!("{:02}:{:02}", ms / 60_000, (ms / 1_000) % 60)
}
fn speaker<'a>(parts: &'a [yc_core::meeting::Participant], key: &'a str) -> &'a str {
    parts
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.name.as_str())
        .unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use yc_core::{meeting::Meeting, summary::Summary, transcript::Transcript};

    #[test]
    fn timestamp_format_is_minutes_seconds() {
        assert_eq!(mmss(65_000), "01:05");
    }

    #[test]
    fn all_tools_read_and_update_a_temporary_library_fixture() {
        let temp = tempfile::tempdir().expect("temp directory");
        let library = temp.path().join("library");
        let store = Store::open_in_library(&library).expect("store");
        let folder = store.create_folder("Work".into(), None).expect("folder");
        let id = MeetingId::new();
        let started = Utc::now() - Duration::days(1);
        let meeting: Meeting = serde_json::from_value(json!({
            "id": id.to_string(), "title": "Alpha planning", "startedAt": started.to_rfc3339(),
            "durationMs": 125000, "language":"en", "tracks":[], "favorite":true,
            "folderId":folder.id,"participants":[],"status":{"state":"ready"}
        }))
        .expect("meeting fixture");
        let transcript: Transcript = serde_json::from_value(json!({"segments":[
            {"startMs":65000,"endMs":70000,"track":"mic","speaker":"me","text":"alpha milestone decision"}
        ]})).expect("transcript fixture");
        let summary: Summary = serde_json::from_value(json!({
            "about":["Plan the alpha milestone"],"decisions":[{"text":"Ship alpha"}],
            "tasks":[{"text":"Prepare release","owner":"me","due":"2026-10-21","atMs":65000,"done":false}],
            "language":"en","model":"fixture"
        })).expect("summary fixture");
        store
            .save_meeting(&meeting, &transcript, Some(&summary))
            .expect("save meeting");
        let mut second_meeting = meeting.clone();
        second_meeting.id = MeetingId::new();
        second_meeting.title = "Beta follow-up".into();
        second_meeting.started_at = started - Duration::days(1);
        second_meeting.favorite = false;
        second_meeting.folder_id = None;
        let second_transcript: Transcript = serde_json::from_value(json!({"segments":[
            {"startMs":15000,"endMs":18000,"track":"system","speaker":"them","text":"beta follow-up discussion"}
        ]})).expect("second transcript fixture");
        store
            .save_meeting(&second_meeting, &second_transcript, Some(&summary))
            .expect("save second meeting");
        let server = YadaCallMcp {
            store,
            allow_write: true,
            embedder: Mutex::new(None),
        };

        let listed = server.list_meetings(Parameters(ListMeetings {
            since: None,
            until: None,
            folder: Some("Work".into()),
            favorite: Some(true),
            query: Some("alpha".into()),
            limit: 20,
        }));
        assert_eq!(listed.0["meetings"][0]["id"], id.to_string());
        assert_eq!(listed.0["meetings"][0]["folder"], "Work");
        assert_eq!(
            server
                .store
                .list_meetings(10, 0)
                .expect("two meetings")
                .len(),
            2
        );

        let detail = server.get_meeting(Parameters(GetMeeting {
            id: id.to_string(),
            include: None,
            max_chars: 8,
        }));
        assert_eq!(detail.0["transcript"]["truncated"], true);
        assert_eq!(detail.0["tasks"][0]["text"], "Prepare release");

        let search = server.search_meetings(Parameters(SearchMeetings {
            query: "milestone".into(),
            folder: Some("Work".into()),
            limit: 10,
        }));
        assert_eq!(search.0["results"][0]["meetingId"], id.to_string());
        assert_eq!(search.0["results"][0]["atMs"], 65000);

        let tasks = server.list_tasks(Parameters(ListTasks {
            status: "open".into(),
            due_before: Some("2026-10-22".into()),
            folder: Some("Work".into()),
            limit: 50,
        }));
        assert_eq!(tasks.0["tasks"][0]["owner"], "me");
        assert_eq!(tasks.0["tasks"][0]["favorite"], true);

        let upcoming = server.upcoming_meetings(Parameters(Upcoming { hours: 12 }));
        assert_eq!(upcoming.0["meetings"].as_array().map(Vec::len), Some(0));
        let completed = server.complete_task(Parameters(CompleteTask {
            task_id: format!("{id}:0"),
            done: true,
        }));
        assert_eq!(completed.0["ok"], true);
        assert!(
            server
                .store
                .list_tasks(false)
                .expect("updated tasks")
                .iter()
                .any(|task| task.meeting_id == id && task.idx == 0 && task.done)
        );
    }

    #[test]
    fn default_store_mode_opens_the_index_read_only() {
        let temp = tempfile::tempdir().expect("temp directory");
        let library = temp.path().join("library");
        let db = library.join("index.sqlite");
        drop(Store::open(&library, &db).expect("create fixture index"));
        let store = Store::open_read_only(&library, &db).expect("read-only open");
        assert!(store.set_setting("mcp-test", &json!(true)).is_err());
        let disabled = YadaCallMcp {
            store,
            allow_write: false,
            embedder: Mutex::new(None),
        };
        assert!(
            !disabled
                .active_router()
                .list_all()
                .iter()
                .any(|tool| tool.name == "complete_task")
        );
    }
}
