mod cli;
mod lifecycle;
mod server;

use clap::Parser;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    cli::execute(cli::Cli::parse()).await
}
