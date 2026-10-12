#[path = "cmd/hw.rs"]
mod hw;
#[path = "cmd/models.rs"]
mod models;
#[path = "cmd/runtime.rs"]
mod runtime;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "yadacall", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Hw(hw::HwArgs),
    Models(models::ModelsArgs),
    Runtime(runtime::RuntimeArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Hw(args) => hw::run(args).await?,
        Command::Models(args) => models::run(args).await?,
        Command::Runtime(args) => runtime::run(args).await?,
    }
    Ok(())
}
