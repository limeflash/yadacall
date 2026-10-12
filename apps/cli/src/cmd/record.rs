//! `yadacall record` — capture both tracks for a while and report what was heard.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Args as ClapArgs;
use yc_audio::{DeviceSel, Recorder, RecorderConfig, RecorderEvent};
use yc_core::events::Level;
use yc_core::meeting::TrackKind;

#[derive(ClapArgs)]
pub struct Args {
    /// How long to record, in seconds.
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    /// Directory for mic.pcm / them.pcm and their json files (must not hold a recording).
    #[arg(long, default_value = "yadacall-recording")]
    out: PathBuf,
    /// Microphone: device id or part of its name (default: the system default input).
    #[arg(long)]
    mic: Option<String>,
    /// Do not record the microphone.
    #[arg(long)]
    no_mic: bool,
    /// Do not record system audio.
    #[arg(long)]
    no_system: bool,
    /// Silence-warning threshold in seconds.
    #[arg(long, default_value_t = 60)]
    silence_warn: u64,
}

fn dbfs(v: f32) -> f32 {
    if v <= 1e-6 { -120.0 } else { 20.0 * v.log10() }
}

fn bar(db: f32) -> String {
    let n = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * 20.0).round() as usize;
    format!("{}{}", "#".repeat(n), ".".repeat(20 - n))
}

#[derive(Default)]
struct Seen {
    max_rms: f32,
    max_peak: f32,
    frames: u64,
    frame_samples: u64,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let mut cfg = RecorderConfig::new(&args.out);
    cfg.mic = (!args.no_mic).then(|| {
        args.mic
            .clone()
            .map_or(DeviceSel::Default, DeviceSel::Named)
    });
    cfg.system = !args.no_system;
    cfg.silence.warn_after = Duration::from_secs(args.silence_warn);

    println!("pid {}  ->  {}", std::process::id(), args.out.display());
    let rec = Recorder::start(cfg)?;
    let levels = rec.levels();
    let events = rec.events();
    let mic_frames = rec.frames(TrackKind::Mic);
    let sys_frames = rec.frames(TrackKind::System);
    let mut seen = [Seen::default(), Seen::default()];

    let end = Instant::now() + Duration::from_secs(args.seconds);
    let mut next_print = Instant::now();
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(40));
        for (i, (kind, frames)) in [
            (TrackKind::Mic, &mic_frames),
            (TrackKind::System, &sys_frames),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(l) = levels.get(kind) {
                seen[i].max_rms = seen[i].max_rms.max(l.rms);
                seen[i].max_peak = seen[i].max_peak.max(l.peak);
            }
            if let Some(rx) = frames {
                for f in rx.try_iter() {
                    seen[i].frames += 1;
                    seen[i].frame_samples += f.samples.len() as u64;
                }
            }
        }
        for ev in events.try_iter() {
            println!("  event: {ev:?}");
            if let RecorderEvent::PermissionSuspected { permission, .. } = ev {
                println!(
                    "  hint: no signal at all from the {permission} track — check the macOS privacy permission for this app"
                );
            }
        }
        if Instant::now() >= next_print {
            next_print += Duration::from_millis(500);
            let show = |name: &str, l: Option<Level>| match l {
                Some(l) => format!(
                    "{name} {} rms {:6.1} dB peak {:6.1} dB",
                    bar(dbfs(l.rms)),
                    dbfs(l.rms),
                    dbfs(l.peak)
                ),
                None => format!("{name} (off)"),
            };
            println!(
                "{} | {}",
                show("me  ", levels.get(TrackKind::Mic)),
                show("them", levels.get(TrackKind::System))
            );
        }
    }

    let tracks = rec.stop()?;
    println!("\nSummary");
    for (i, kind) in [TrackKind::Mic, TrackKind::System].into_iter().enumerate() {
        match tracks.iter().find(|t| t.kind == kind) {
            Some(t) => println!(
                "  {:<4} device {:?}, offset {} ms, {} samples ({:.2} s), max rms {:.1} dBFS, max peak {:.1} dBFS, {} live frames ({} samples)",
                kind.file_stem(),
                t.device,
                t.offset_ms,
                t.samples,
                t.samples as f64 / 16_000.0,
                dbfs(seen[i].max_rms),
                dbfs(seen[i].max_peak),
                seen[i].frames,
                seen[i].frame_samples,
            ),
            None => println!("  {:<4} no audio recorded", kind.file_stem()),
        }
    }
    Ok(())
}
