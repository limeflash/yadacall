//! `yadacall google login|accounts|logout|agenda` and `yadacall ics add|agenda`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use chrono::{Duration, Local};
use clap::{Args, Subcommand};
use tokio_util::sync::CancellationToken;
use yc_connect::google::KeychainTokenStore;
use yc_connect::{
    AccountKind, AccountState, AgendaEvent, ConnectConfig, ConnectError, ConnectService,
    SystemClock, VideoProvider, browser::open_in_browser,
};

/// `google` command options.
#[derive(Debug, Args)]
pub struct GoogleArgs {
    #[command(subcommand)]
    command: GoogleCommand,
}

#[derive(Debug, Subcommand)]
enum GoogleCommand {
    /// Sign in with a Google account in the browser (read-only calendar access).
    Login,
    /// List signed-in accounts and ICS subscriptions.
    Accounts,
    /// Sign out: revokes the token at Google and removes it from the keychain.
    Logout {
        /// Account e-mail (or `ics:<id>` for an ICS subscription).
        email: String,
    },
    /// Show the merged agenda of all sources.
    Agenda(AgendaArgs),
}

/// `ics` command options.
#[derive(Debug, Args)]
pub struct IcsArgs {
    #[command(subcommand)]
    command: IcsCommand,
}

#[derive(Debug, Subcommand)]
enum IcsCommand {
    /// Subscribe to a calendar's secret ICS URL (Google, Outlook, Yandex, iCloud).
    Add {
        url: String,
        /// Display name.
        #[arg(long, default_value = "ICS")]
        label: String,
    },
    /// Show the agenda of ICS subscriptions only.
    Agenda(AgendaArgs),
}

#[derive(Debug, Args)]
struct AgendaArgs {
    /// Days ahead to show.
    #[arg(long, default_value_t = 7)]
    days: i64,
    /// Print JSON instead of a table.
    #[arg(long)]
    json: bool,
}

/// Runs a `google` subcommand.
pub async fn run(args: GoogleArgs) -> anyhow::Result<()> {
    let svc = service()?;
    match args.command {
        GoogleCommand::Login => login(&svc).await,
        GoogleCommand::Accounts => accounts(&svc),
        GoogleCommand::Logout { email } => {
            let outcome = svc.remove_account(&email).await.map_err(friendly)?;
            println!(
                "signed out {email}{}",
                if outcome.revoked {
                    " (token revoked at Google)"
                } else if email.contains('@') {
                    " (removed locally; revoking at Google was not confirmed)"
                } else {
                    " (removed)"
                }
            );
            Ok(())
        }
        GoogleCommand::Agenda(a) => agenda(&svc, a, None).await,
    }
}

/// Runs an `ics` subcommand.
pub async fn run_ics(args: IcsArgs) -> anyhow::Result<()> {
    let svc = service()?;
    match args.command {
        IcsCommand::Add { url, label } => {
            let meta = svc.add_ics(&url, &label).await.map_err(friendly)?;
            println!(
                "added {} as {} (URL stored in the keychain)",
                meta.label, meta.id
            );
            Ok(())
        }
        IcsCommand::Agenda(a) => agenda(&svc, a, Some(AccountKind::Ics)).await,
    }
}

fn service() -> anyhow::Result<ConnectService> {
    let dir = app_data_dir()?;
    let cfg = ConnectConfig::new(dir.join("connect.json"));
    ConnectService::new(cfg, Arc::new(KeychainTokenStore), Arc::new(SystemClock)).map_err(friendly)
}

fn app_data_dir() -> anyhow::Result<PathBuf> {
    let path = match std::env::var_os("YC_DATA_DIR") {
        Some(p) => PathBuf::from(p),
        None => dirs::data_local_dir()
            .context("cannot resolve the local app data directory")?
            .join("YadaCall"),
    };
    std::fs::create_dir_all(&path).context("create YadaCall app data directory")?;
    Ok(path)
}

fn friendly(error: ConnectError) -> anyhow::Error {
    anyhow::anyhow!("{error}")
}

async fn login(svc: &ConnectService) -> anyhow::Result<()> {
    if !svc.google_configured() {
        bail!(
            "Google sign-in is not configured in this build.\n\
             Create a Desktop-app OAuth client and set YADACALL_GOOGLE_CLIENT_ID (and\n\
             YADACALL_GOOGLE_CLIENT_SECRET): see docs/GOOGLE_OAUTH.md.\n\
             Meanwhile `yadacall ics add <secret-ICS-URL>` works without any sign-in."
        );
    }
    let cancel = CancellationToken::new();
    let on_ctrl_c = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        on_ctrl_c.cancel();
    });
    println!("Opening the browser to sign in with Google (read-only calendar access)...");
    let meta = svc
        .add_google_account(
            |url| {
                println!("If the browser does not open, visit:\n  {url}");
                open_in_browser(url);
            },
            &cancel,
        )
        .await
        .map_err(friendly)?;
    println!(
        "signed in as {}{}",
        meta.email,
        if meta.is_workspace() {
            " (Workspace)"
        } else {
            ""
        }
    );
    Ok(())
}

fn accounts(svc: &ConnectService) -> anyhow::Result<()> {
    let list = svc.accounts().list();
    if list.is_empty() {
        println!("no accounts yet: `yadacall google login` or `yadacall ics add <url>`");
        return Ok(());
    }
    for a in list {
        let kind = match a.kind {
            AccountKind::Google if a.is_workspace() => "google·workspace",
            AccountKind::Google => "google",
            AccountKind::Ics => "ics",
        };
        let state = match a.state {
            AccountState::Ok => "",
            AccountState::AuthExpired => "  вход истёк · Войти снова",
        };
        let id = if a.email.is_empty() { &a.id } else { &a.email };
        println!(
            "{id:32} {kind:17} {:?}{}  [{}]{state}",
            a.policy,
            if a.visible { "" } else { " hidden" },
            a.label
        );
    }
    Ok(())
}

async fn agenda(
    svc: &ConnectService,
    args: AgendaArgs,
    only: Option<AccountKind>,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let report = svc
        .refresh_window((now - Duration::days(1), now + Duration::days(args.days)))
        .await;
    for (account, error) in &report.errors {
        eprintln!("warning: {account}: {error}");
    }
    let kinds: std::collections::HashMap<_, _> = svc
        .accounts()
        .list()
        .into_iter()
        .map(|a| (a.id.clone(), (a.kind, a.label)))
        .collect();
    let events: Vec<&AgendaEvent> = report
        .agenda
        .iter()
        .filter(|e| only.is_none_or(|k| kinds.get(&e.account).is_some_and(|(kind, _)| *kind == k)))
        .collect();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&events)?);
        return Ok(());
    }
    if events.is_empty() {
        println!("no events in the window");
        return Ok(());
    }
    let mut day = None;
    for e in events {
        let start = e.start.with_timezone(&Local);
        if day != Some(start.date_naive()) {
            day = Some(start.date_naive());
            println!("\n{}", start.format("%a %d %b"));
        }
        let label = kinds.get(&e.account).map_or("?", |(_, l)| l.as_str());
        let call = if e.meet_url.is_some() {
            "Meet".to_string()
        } else {
            e.other_links
                .first()
                .map(|l| match l.provider {
                    VideoProvider::Zoom => "Zoom",
                    VideoProvider::Teams => "Teams",
                    VideoProvider::Telemost => "Telemost",
                    VideoProvider::Jitsi => "Jitsi",
                    VideoProvider::Meet => "Meet",
                    VideoProvider::Other => "link",
                })
                .unwrap_or("-")
                .to_string()
        };
        println!(
            "  {}-{}  {:<8} {}  [{label}]{}",
            start.format("%H:%M"),
            e.end.with_timezone(&Local).format("%H:%M"),
            call,
            e.title,
            if e.overlaps.is_empty() {
                ""
            } else {
                "  (overlaps)"
            }
        );
    }
    Ok(())
}
