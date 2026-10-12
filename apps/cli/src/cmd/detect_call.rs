//! `yadacall detect-call [--watch]`.

use std::time::Duration;

use clap::Args;
use yc_connect::detect::{MicState, system_detector};
use yc_connect::types::DetectedCall;

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
            tokio::task::spawn_blocking(move || (d.mic_state(), d.detect()))
        }
    };
    if !args.watch {
        let (mic, call) = probe().await?;
        report(mic, call.as_ref(), json)?;
        return Ok(());
    }
    if !args.no_tabs && !json {
        eprintln!(
            "watching; browser tabs are inspected only while the microphone is in use \
             (macOS may ask once for Automation permission per browser; use --no-tabs to avoid it)"
        );
    }
    let mut last: Option<DetectedCall> = None;
    let mut timer = tokio::time::interval(Duration::from_secs(args.interval.max(1)));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = timer.tick() => {}
        }
        let (mic, call) = probe().await?;
        if call != last {
            report(mic, call.as_ref(), json)?;
            last = call;
        }
    }
}

fn report(mic: MicState, call: Option<&DetectedCall>, json: bool) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::json!({ "microphone": format!("{mic:?}"), "call": call })
        );
        return Ok(());
    }
    match call {
        Some(c) => println!(
            "call detected: {:?}{}{}{}",
            c.app,
            c.meet_code
                .as_ref()
                .map(|m| format!(" {m}"))
                .unwrap_or_default(),
            c.browser
                .as_ref()
                .map(|b| format!(" in {b}"))
                .unwrap_or_default(),
            if c.certain {
                ""
            } else {
                " (likely: a call app is running while the microphone is in use)"
            }
        ),
        None => println!(
            "no call detected (microphone: {})",
            match mic {
                MicState::InUse => "in use, but no known call app or Meet tab found",
                MicState::Idle => "idle",
                MicState::Unknown => "unknown on this system",
            }
        ),
    }
    Ok(())
}
