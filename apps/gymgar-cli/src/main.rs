//! `gymgar-cli`: the headless entry point of Gymgar.
//!
//! `gymgar-cli mcp` serves the engine's commands to an agent over MCP on stdin and stdout, on the same database as the
//! desktop app. Stdout belongs to the protocol, so everything else (usage, errors) goes to stderr. The CLI hands the
//! engine no secret store, no Garmin client and no background spawner: an agent can read and change settings, but
//! can never make this process open the keychain or touch the network.

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;

use gymgar_automation::AutomationError;
use gymgar_engine::{BoxFuture, Services, Session};
use gymgar_store::{Store, StoreError, paths};

const USAGE: &str = "\
Usage: gymgar-cli <command>

Commands:
  mcp    Serve the Gymgar commands to an agent over MCP (stdin and stdout)
";

/// The exit code of a command line that cannot be understood, as in most Unix tools.
const EXIT_USAGE: u8 = 2;

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Mcp,
}

/// Why the command line is not a command.
#[derive(Debug, PartialEq, Eq)]
enum UsageError {
    /// `--help`, `-h` or `help`.
    Help,
    NoCommand,
    UnknownCommand(String),
    UnexpectedArgument(String),
}

impl UsageError {
    /// What to print before the usage text; nothing when the user only asked for help.
    fn message(&self) -> Option<String> {
        match self {
            UsageError::Help => None,
            UsageError::NoCommand => Some("missing command".to_owned()),
            UsageError::UnknownCommand(name) => Some(format!("unknown command `{name}`")),
            UsageError::UnexpectedArgument(argument) => Some(format!("unexpected argument `{argument}`")),
        }
    }
}

/// Reads the arguments after the program name. Works on `OsString`s: a non-UTF-8 argument is an unknown command, not
/// a panic.
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else { return Err(UsageError::NoCommand) };
    let command = match command.to_str() {
        Some("mcp") => Command::Mcp,
        Some("--help" | "-h" | "help") => return Err(UsageError::Help),
        _ => return Err(UsageError::UnknownCommand(command.to_string_lossy().into_owned())),
    };
    match args.next() {
        None => Ok(command),
        Some(extra) => Err(UsageError::UnexpectedArgument(extra.to_string_lossy().into_owned())),
    }
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("cannot start the async runtime: {0}")]
    Runtime(#[from] std::io::Error),
    #[error("{0}")]
    Mcp(#[from] AutomationError),
}

/// What the CLI gives the engine: nothing that reaches outside the database. Without a spawner no job ever runs, so
/// the sleep, which only jobs use, need not wait at all.
fn headless_services() -> Services {
    Services {
        secrets: None,
        garmin: None,
        spawner: None,
        notify: Arc::new(|| {}),
        clock: Arc::new(chrono::Utc::now),
        sleep: Arc::new(|_| -> BoxFuture<()> { Box::pin(std::future::ready(())) }),
    }
}

fn run_mcp() -> Result<(), CliError> {
    let store = Store::open(&paths::db_path(&paths::data_dir()?))?;
    let session = Session::new(store, headless_services());
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(gymgar_automation::serve_stdio(session))?;
    Ok(())
}

fn main() -> ExitCode {
    match parse_args(std::env::args_os().skip(1)) {
        Ok(Command::Mcp) => match run_mcp() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("gymgar-cli: {error}");
                ExitCode::FAILURE
            }
        },
        Err(usage) => {
            if let Some(message) = usage.message() {
                eprintln!("gymgar-cli: {message}\n");
            }
            eprint!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

#[cfg(test)]
mod tests {
    use gymgar_engine::{Caller, EngineError, Outcome};
    use serde_json::json;

    use super::*;

    fn parse(args: &[&str]) -> Result<Command, UsageError> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn mcp_is_the_only_command() {
        assert_eq!(parse(&["mcp"]), Ok(Command::Mcp));
    }

    #[test]
    fn help_and_missing_or_unknown_commands_are_usage_errors() {
        assert_eq!(parse(&["--help"]), Err(UsageError::Help));
        assert_eq!(parse(&["-h"]), Err(UsageError::Help));
        assert_eq!(parse(&["help"]), Err(UsageError::Help));
        assert_eq!(parse(&[]), Err(UsageError::NoCommand));
        assert_eq!(parse(&["sync"]), Err(UsageError::UnknownCommand("sync".into())));
        assert_eq!(parse(&["MCP"]), Err(UsageError::UnknownCommand("MCP".into())));
        assert_eq!(parse(&["mcp", "--verbose"]), Err(UsageError::UnexpectedArgument("--verbose".into())));
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_is_not_utf8_is_unknown_not_a_panic() {
        use std::os::unix::ffi::OsStringExt;
        let result = parse_args([OsString::from_vec(vec![0x6d, 0xff, 0x70])]);
        assert!(matches!(result, Err(UsageError::UnknownCommand(_))), "{result:?}");
    }

    #[test]
    fn only_help_has_no_message() {
        assert_eq!(UsageError::Help.message(), None);
        assert!(UsageError::NoCommand.message().is_some());
    }

    #[test]
    fn the_headless_session_works_on_the_database_alone() {
        let mut session = Session::new(Store::open_in_memory().expect("in-memory store"), headless_services());
        let language = session.execute("settings.setLanguage", json!({ "language": "ru" }), Caller::Agent).expect("settings work");
        assert_eq!(language, Outcome::Done(json!({ "language": "ru" })));
        // No secret stores, no Garmin, no spawner: even the user-level connect command is unavailable.
        let connect = session.execute("garmin.connect", json!({ "apiKey": "k", "keyTerm": 365 }), Caller::User);
        assert!(matches!(connect, Err(EngineError::Disabled { .. })), "{connect:?}");
    }

    #[test]
    fn the_headless_sleep_does_not_wait() {
        // No job runs in the CLI; should one ever sleep, it must not hold the process for real.
        let mut wait = (headless_services().sleep)(std::time::Duration::from_secs(3600));
        let ready = wait.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
        assert!(ready.is_ready());
    }
}
