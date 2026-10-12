//! YadaCall command line interface.

mod cmd;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "yadacall",
    version,
    about = "YadaCall: on-device call recording and transcription"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List audio input/output devices and capture capabilities.
    Devices(cmd::devices::Args),
    /// Record the microphone and system audio into a directory (crash-safe .pcm tracks).
    Record(cmd::record::Args),
    /// Transcribe an audio file on-device.
    Transcribe(cmd::transcribe::Args),
    /// Benchmark an installed ASR engine on the fixed sample.
    BenchAsr(cmd::bench_asr::Args),
    /// Summarize a transcript file with a local or remote LLM.
    Summarize(cmd::summarize::SummarizeArgs),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Devices(args) => cmd::devices::run(args),
        Command::Record(args) => cmd::record::run(args),
        Command::Transcribe(args) => cmd::transcribe::run(args),
        Command::BenchAsr(args) => cmd::bench_asr::run(args),
        Command::Summarize(args) => tokio::runtime::Runtime::new()?
            .block_on(cmd::summarize::run(args))
            .map_err(|error| anyhow::anyhow!("{error}")),
    }
}
