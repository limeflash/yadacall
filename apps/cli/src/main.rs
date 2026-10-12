mod cmd;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "yadacall", version, about = "YadaCall command line")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List audio input/output devices and capture capabilities.
    Devices(cmd::devices::Args),
    /// Record the microphone and system audio into a directory (crash-safe .pcm tracks).
    Record(cmd::record::Args),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Devices(args)) => cmd::devices::run(args),
        Some(Command::Record(args)) => cmd::record::run(args),
        None => {
            println!("yadacall {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
