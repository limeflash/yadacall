//! YadaCall command line interface.

mod cmd;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "yadacall",
    version,
    about = "On-device call transcription tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Transcribe an audio file on-device.
    Transcribe(cmd::transcribe::Args),
    /// Benchmark an installed ASR engine on the fixed sample.
    BenchAsr(cmd::bench_asr::Args),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Transcribe(args) => cmd::transcribe::run(args),
        Command::BenchAsr(args) => cmd::bench_asr::run(args),
    }
}
