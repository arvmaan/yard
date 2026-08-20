use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::{lifecycle, server};

#[derive(Debug, Parser)]
#[command(
    name = "yard",
    version,
    about = "Run and manage the local Yard control plane",
    arg_required_else_help = true
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start Yard in the background and open its UI.
    Start(StartArgs),
    /// Show the current Yard lifecycle owner, URL, and PID.
    Status,
    /// Gracefully stop the managed Yard instance.
    Stop,
    /// Run Yard in the foreground with logs attached.
    Run,
    #[command(name = "__managed-run", hide = true)]
    ManagedRun(ManagedRunArgs),
}

#[derive(Debug, Args)]
struct StartArgs {
    /// Start Yard without opening a browser.
    #[arg(long)]
    no_open: bool,
}

#[derive(Debug, Args)]
struct ManagedRunArgs {
    #[arg(long, hide = true)]
    instance_id: String,
}

pub(crate) async fn execute(cli: Cli) -> ExitCode {
    let result: Result<ExitCode, CommandFailure> = match cli.command {
        Command::Start(args) => lifecycle::start(args.no_open)
            .await
            .map_err(|error| CommandFailure::lifecycle(&error)),
        Command::Status => lifecycle::status()
            .await
            .map_err(|error| CommandFailure::lifecycle(&error)),
        Command::Stop => lifecycle::stop()
            .await
            .map_err(|error| CommandFailure::lifecycle(&error)),
        Command::Run => {
            server::init_tracing();
            server::run_foreground()
                .await
                .map(|()| ExitCode::SUCCESS)
                .map_err(|error| CommandFailure::server(&error))
        }
        Command::ManagedRun(args) => server::run_managed(&args.instance_id)
            .await
            .map(|()| ExitCode::SUCCESS)
            .map_err(|error| CommandFailure::server(&error)),
    };

    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("yard: {}", error.message);
            ExitCode::from(error.exit_code)
        }
    }
}

struct CommandFailure {
    message: String,
    exit_code: u8,
}

impl CommandFailure {
    fn lifecycle(error: &lifecycle::LifecycleError) -> Self {
        Self {
            exit_code: error.exit_code(),
            message: error.to_string(),
        }
    }

    fn server(error: &server::ServerError) -> Self {
        Self {
            exit_code: error.exit_code(),
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command};

    #[test]
    fn parses_start_with_browser_suppressed() {
        let cli = Cli::try_parse_from(["yard", "start", "--no-open"]).expect("parse start");
        assert!(matches!(
            cli.command,
            Command::Start(super::StartArgs { no_open: true })
        ));
    }

    #[test]
    fn parses_public_lifecycle_commands() {
        for command in ["status", "stop", "run"] {
            Cli::try_parse_from(["yard", command]).unwrap_or_else(|error| {
                panic!("parse {command}: {error}");
            });
        }
    }

    #[test]
    fn rejects_browser_flag_on_foreground_run() {
        assert!(Cli::try_parse_from(["yard", "run", "--no-open"]).is_err());
    }

    #[test]
    fn help_lists_public_commands_and_hides_internal_command() {
        let mut command = Cli::command();
        let help = command.render_long_help().to_string();
        assert!(help.contains("start"));
        assert!(help.contains("status"));
        assert!(help.contains("stop"));
        assert!(help.contains("run"));
        assert!(!help.contains("__managed-run"));
    }
}
