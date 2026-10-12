use clap::{Parser, Subcommand};

#[path = "cmd/summarize.rs"]
mod summarize;

#[derive(Parser)]
#[command(name = "yadacall", version, about = "YadaCall meeting tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarize a transcript file.
    Summarize(summarize::SummarizeArgs),
}

#[tokio::main]
async fn main() {
    let result = match Cli::parse().command {
        Command::Summarize(args) => summarize::run(args).await,
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
