//! `yadacall detect-call [--watch]`.

use std::time::Duration;

use clap::Args;
use yc_connect::detect::{DetectionScan, MicState, system_detector};

/// `detect-call` options.
#[derive(Debug, Args)]
pub struct DetectArgs {
    /// Keep running and report when a call starts or ends (Ctrl-C to stop).
    #[arg(long)]
    pub watch: bool,
    /// Seconds between checks in --watch mode.
    #[arg(long, default_value_t = 3)]
    pub interval: u64,
    /// Do not look into browser tabs/windows. On macOS reading tabs asks for the Automation
    /// permission once per browser; with this flag only the microphone gate and call apps are used.
    #[arg(long)]
    pub no_tabs: bool,
    /// Print JSON.
    #[arg(long)]
    pub json: bool,
}

/// Runs call detection.
pub async fn run(args: DetectArgs) -> anyhow::Result<()> {
    let detector = std::sync::Arc::new(system_detector(!args.no_tabs));
    let json = args.json;
    let probe = {
        let detector = detector.clone();
        move || {
            let d = detector.clone();
            tokio::task::spawn_blocking(move || d.scan())
        }
    };
    if !args.watch {
        let scan = probe().await?;
        report(&scan, json)?;
        return Ok(());
    }
    if !args.no_tabs && !json {
        eprintln!(
            "watching; browser tabs are inspected only while the microphone is in use \
             (macOS may ask once for Automation permission per browser; use --no-tabs to avoid it)"
        );
    }
    let mut last: Option<DetectionScan> = None;
    let mut timer = tokio::time::interval(Duration::from_secs(args.interval.max(1)));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = timer.tick() => {}
        }
        let scan = probe().await?;
        if Some(&scan) != last.as_ref() {
            report(&scan, json)?;
            last = Some(scan);
        }
    }
}

fn report(scan: &DetectionScan, json: bool) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "microphone": format!("{:?}", scan.microphone),
                "microphoneUsers": scan.microphone_users.iter().map(|u| serde_json::json!({
                    "processName": u.process_name,
                    "bundleId": u.bundle_id,
                    "pid": u.pid,
                })).collect::<Vec<_>>(),
                "call": scan.call,
            })
        );
        return Ok(());
    }
    for user in &scan.microphone_users {
        let mut details = Vec::new();
        if let Some(bundle_id) = &user.bundle_id {
            details.push(bundle_id.clone());
        }
        if let Some(pid) = user.pid {
            details.push(format!("pid {pid}"));
        }
        println!(
            "microphone held by {}{}",
            user.process_name,
            if details.is_empty() {
                String::new()
            } else {
                format!(" ({})", details.join(", "))
            }
        );
    }
    match scan.call.as_ref() {
        Some(c) => {
            println!(
                "call detected: {}{}{}",
                c.title,
                c.browser
                    .as_ref()
                    .map(|b| format!(" [browser: {b}]"))
                    .unwrap_or_default(),
                c.url.as_ref().map(|u| format!(" {u}")).unwrap_or_default(),
            );
            if let Some(browser) = &c.automation_permission_required {
                println!(
                    "  tab access to {browser} is not authorized: allow it in System Settings \
                     > Privacy & Security > Automation"
                );
            }
        }
        None => println!(
            "no call detected (microphone: {})",
            match scan.microphone {
                MicState::InUse => "in use, but no known call app or Meet tab found",
                MicState::Idle => "idle",
                MicState::Unknown => "unknown on this system",
            }
        ),
    }
    Ok(())
}
