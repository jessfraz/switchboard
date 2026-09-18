use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::config::Config;
use crate::domain::{AuthorizedCall, CallId, CallRequest, TerminationReason};
use crate::error::CallError;
use crate::journal::{Journal, Record};
use crate::livekit::LiveKitBackend;

#[derive(Parser)]
#[command(
    name = "phone",
    version,
    about = "Run an approved phone call locally and keep encrypted transcripts"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Call a number with a task. This command authorizes the call and provider charges.
    Call(CallArgs),
    /// Make one call. This can contact a third party and incur provider charges.
    Run(RunArgs),
    /// Check local configuration without contacting a provider or placing a call.
    Doctor,
    /// List call IDs or explicitly decrypt their local transcripts.
    Transcripts {
        #[command(subcommand)]
        command: TranscriptCommand,
    },
}

#[derive(Args)]
struct CallArgs {
    /// Destination in E.164 format, including its country code.
    #[arg(value_name = "NUMBER")]
    destination: String,
    /// What the assistant should accomplish, including any constraints.
    #[arg(value_name = "PROMPT")]
    task: String,
    /// Whose assistant is calling; defaults to caller_name in the configuration.
    #[arg(long)]
    caller_name: Option<String>,
    /// Whole-call limit, including holds; defaults to config or 600 seconds.
    #[arg(long)]
    max_duration_seconds: Option<u64>,
}

#[derive(Args)]
struct RunArgs {
    /// Explicitly authorize this call (Switchboard supplies this after approval).
    #[arg(long)]
    approve: bool,
    /// Read a CallRequest JSON object from stdin, avoiding sensitive process args.
    #[arg(long, conflicts_with_all = ["destination", "task", "caller_name", "call_id", "max_duration_seconds"])]
    request_stdin: bool,
    #[arg(long, required_unless_present = "request_stdin")]
    destination: Option<String>,
    #[arg(long, required_unless_present = "request_stdin")]
    task: Option<String>,
    #[arg(long, required_unless_present = "request_stdin")]
    caller_name: Option<String>,
    /// Reusing a call ID is refused, including after a failed or interrupted call.
    #[arg(long)]
    call_id: Option<CallId>,
    #[arg(long)]
    max_duration_seconds: Option<u64>,
}

#[derive(Subcommand)]
enum TranscriptCommand {
    List,
    /// Decrypt to stdout using PHONE_TRANSCRIPT_IDENTITY from your secret manager.
    Show {
        call_id: CallId,
    },
}

#[derive(Serialize)]
pub struct RunOutput {
    pub call_id: CallId,
    pub status: TerminationReason,
    pub transcript_path: PathBuf,
    pub remote_hangup_confirmed: bool,
}

#[derive(Serialize)]
struct DoctorOutput {
    configuration_valid: bool,
    encryption_ready: bool,
    worker_project_exists: bool,
    credentials_present: bool,
}

#[derive(Serialize)]
struct CallsOutput {
    calls: Vec<CallId>,
}

impl CallArgs {
    fn request(self, config: &Config) -> Result<CallRequest, CallError> {
        Ok(CallRequest {
            call_id: CallId::new(),
            destination: self.destination,
            task: self.task,
            caller_name: self.caller_name.or_else(|| config.caller_name.clone()).ok_or_else(|| {
                CallError::Configuration("set caller_name in the configuration or pass --caller-name".into())
            })?,
            max_duration_seconds: self.max_duration_seconds.or(config.max_duration_seconds).unwrap_or(600),
        })
    }
}

impl RunArgs {
    fn request(&self) -> Result<CallRequest, CallError> {
        if self.request_stdin {
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(65_537)
                .read_to_end(&mut bytes)
                .map_err(|_| CallError::InvalidRequest("could not read request stdin".into()))?;
            if bytes.len() > 65_536 {
                return Err(CallError::InvalidRequest("request exceeds 65536 bytes".into()));
            }
            serde_json::from_slice(&bytes).map_err(|_| CallError::InvalidRequest("invalid call request JSON".into()))
        } else {
            Ok(CallRequest {
                call_id: self.call_id.clone().unwrap_or_default(),
                destination: self
                    .destination
                    .clone()
                    .ok_or_else(|| CallError::InvalidRequest("destination is required".into()))?,
                task: self
                    .task
                    .clone()
                    .ok_or_else(|| CallError::InvalidRequest("task is required".into()))?,
                caller_name: self
                    .caller_name
                    .clone()
                    .ok_or_else(|| CallError::InvalidRequest("caller_name is required".into()))?,
                max_duration_seconds: self.max_duration_seconds.unwrap_or(600),
            })
        }
    }
}

pub fn main_entry<I, T>(arguments: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    match execute(cli) {
        Ok(success) => ExitCode::from(if success { 0 } else { 1 }),
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "phone: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_json(value: &impl Serialize, pretty: bool) -> Result<(), CallError> {
    let mut output = std::io::stdout().lock();
    if pretty {
        serde_json::to_writer_pretty(&mut output, value).map_err(|_| CallError::Output)?;
    } else {
        serde_json::to_writer(&mut output, value).map_err(|_| CallError::Output)?;
    }
    writeln!(output).map_err(|_| CallError::Output)
}

fn execute(cli: Cli) -> Result<bool, CallError> {
    // Approval errors take precedence over config and side effects.
    if matches!(&cli.command, Command::Run(args) if !args.approve) {
        return Err(CallError::ApprovalRequired);
    }
    let config = Config::load(cli.config.as_deref())?;
    match cli.command {
        // Choosing the standalone `call` command is the user's authorization.
        // The supervised `run` path retains its separate approval requirement.
        Command::Call(args) => run(&config, args.request(&config)?.authorize(true)?, cli.json),
        Command::Doctor => {
            config.validate_runtime()?;
            crate::journal::check_encryption(&config.transcript_recipient)?;
            let credentials_present = [
                ("PHONE_API_KEY", "LIVEKIT_API_KEY"),
                ("PHONE_API_SECRET", "LIVEKIT_API_SECRET"),
                ("PHONE_MODEL_API_KEY", "OPENAI_API_KEY"),
            ]
            .iter()
            .all(|(generic, backend)| {
                std::env::var_os(generic)
                    .or_else(|| std::env::var_os(backend))
                    .is_some_and(|value| !value.is_empty())
            });
            let output = DoctorOutput {
                configuration_valid: true,
                encryption_ready: true,
                worker_project_exists: config.worker_command.is_some()
                    || config.worker_project().join("pyproject.toml").is_file(),
                credentials_present,
            };
            print_json(&output, !cli.json)?;
            Ok(output.worker_project_exists && output.credentials_present)
        }
        Command::Run(args) => {
            let authorized = args.request()?.authorize(args.approve)?;
            run(&config, authorized, cli.json)
        }
        Command::Transcripts {
            command: TranscriptCommand::List,
        } => {
            print_json(
                &CallsOutput {
                    calls: crate::journal::list(&config.state_dir)?,
                },
                !cli.json,
            )?;
            Ok(true)
        }
        Command::Transcripts {
            command: TranscriptCommand::Show { call_id },
        } => {
            let secret = std::env::var("PHONE_TRANSCRIPT_IDENTITY").map_err(|_| {
                CallError::Configuration("PHONE_TRANSCRIPT_IDENTITY must be supplied by your secret manager".into())
            })?;
            let identity = secret.trim().parse::<age::x25519::Identity>().map_err(|_| {
                CallError::Configuration("PHONE_TRANSCRIPT_IDENTITY is not an age X25519 identity".into())
            })?;
            let records = crate::journal::read(&config.state_dir, &call_id, &identity)?;
            print_json(&records, !cli.json)?;
            Ok(true)
        }
    }
}

fn run(config: &Config, authorized: AuthorizedCall, json: bool) -> Result<bool, CallError> {
    config.validate_runtime()?;
    let supervisor = supervisor_pid()?;
    let mut journal = Journal::create(
        &config.state_dir,
        &config.transcript_recipient,
        authorized.request().call_id.clone(),
    )?;
    journal.append(Record::Intent(authorized.request().clone()))?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&cancelled);
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)).map_err(|_| CallError::SignalHandler)?;
    let outcome = crate::runner::run(&authorized, &LiveKitBackend::new(config), &mut journal, || {
        cancelled.load(Ordering::Relaxed) || supervisor_disconnected(supervisor)
    })?;
    let output = RunOutput {
        call_id: authorized.request().call_id.clone(),
        status: outcome.reason,
        transcript_path: journal.path().to_path_buf(),
        remote_hangup_confirmed: outcome.remote_hangup_confirmed,
    };
    print_json(&output, !json)?;
    Ok(outcome.reason == TerminationReason::Completed && outcome.remote_hangup_confirmed)
}

fn supervisor_pid() -> Result<Option<i32>, CallError> {
    std::env::var("PHONE_SUPERVISOR_PID")
        .ok()
        .map(|value| {
            value.parse::<i32>().ok().filter(|pid| *pid > 1).ok_or_else(|| {
                CallError::Configuration("PHONE_SUPERVISOR_PID must identify the direct supervising process".into())
            })
        })
        .transpose()
}

fn supervisor_disconnected(expected: Option<i32>) -> bool {
    #[cfg(unix)]
    if let Some(expected) = expected {
        // A process whose supervisor exits is reparented. Check the expected
        // PID supplied by the supervisor, rather than capturing an already
        // orphaned parent at startup. Wrappers must exec the real phone binary.
        return unsafe { libc::getppid() } != expected;
    }
    #[cfg(not(unix))]
    let _ = expected;
    false
}
