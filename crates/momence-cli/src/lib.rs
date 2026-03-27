mod client;
mod commands;
mod state;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result as AnyhowResult};
use clap::{Args, Parser, Subcommand};
use reqwest::Method;
use serde::Serialize;
use serde_json::Value;

pub(crate) use crate::{client::MomenceClient, state::ResolvedContext};
use crate::{
    client::RequestBody,
    commands::{run_auth, run_member, AuthCommand, MemberCommand},
    state::{
        ENV_MOMENCE_ACCESS_TOKEN, ENV_MOMENCE_BASE_URL, ENV_MOMENCE_CLIENT_ID, ENV_MOMENCE_CLIENT_SECRET,
        ENV_MOMENCE_CONFIG, ENV_MOMENCE_REFRESH_TOKEN,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  momence auth login-password --client-id <id> --client-secret <secret> \\\n",
    "    --username you@example.com --password 'super-secret'\n",
    "  momence member sessions list --start-after 2026-03-01T00:00:00Z\n",
    "  momence member host sessions --type fitness --sort-by startsAt\n",
    "  momence member addresses create --body '{\"address\":\"123 Main St\",\"city\":\"LA\",\"country\":\"US\",\"zipcode\":\"90001\"}'\n",
    "  momence member checkout compatible-memberships --body-file cart.json\n",
    "\n",
    "This CLI is aimed at Momence member workflows, booking Pilates classes and the surrounding account-management chaos.\n",
    "Use --body or --body-file for endpoints that accept JSON request payloads.\n",
);

/// Run the Momence CLI and return a process exit code.
pub fn main_entry<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let exit_code = match error.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            };
            let _ = error.print();
            return exit_code;
        }
    };
    let compact = cli.global.compact;

    match run(cli) {
        Ok((output, compact)) => {
            println!("{}", render_json(&output, compact));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", render_cli_error(&error, compact));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> AnyhowResult<(Value, bool)> {
    let compact = cli.global.compact;
    let mut context = ResolvedContext::from_global(&cli.global).context("failed to resolve Momence runtime context")?;
    let client = MomenceClient::new(context.base_url.clone()).context("failed to build Momence client")?;

    let output = match cli.command {
        Commands::Auth(command) => run_auth(command.command, &client, &mut context),
        Commands::Member(command) => run_member(command.command, &client, &mut context),
    }
    .context("Momence command failed")?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "momence",
    version,
    about = "CLI for booking Pilates classes and handling Momence member account workflows",
    disable_help_subcommand = true,
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Args)]
pub(crate) struct GlobalArgs {
    #[arg(long, global = true, env = ENV_MOMENCE_CONFIG, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_MOMENCE_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_CLIENT_ID, value_name = "CLIENT_ID")]
    client_id: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_CLIENT_SECRET, value_name = "CLIENT_SECRET")]
    client_secret: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_ACCESS_TOKEN, value_name = "TOKEN")]
    access_token: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_REFRESH_TOKEN, value_name = "TOKEN")]
    refresh_token: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Auth(AuthCommand),
    Member(MemberCommand),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid arguments: {0}")]
    Arguments(String),
    #[error("Momence API returned HTTP {status_code}")]
    Api { status_code: u16, body: Value },
    #[error("config error: {0}")]
    Config(String),
    #[error("HTTP failure: {0}")]
    Http(String),
    #[error("I/O failure: {0}")]
    Io(String),
}

impl Error {
    fn render(&self, compact: bool) -> String {
        match self {
            Self::Arguments(message) => render_json(
                &MessageErrorResponse {
                    status: "error",
                    kind: "arguments",
                    message,
                },
                compact,
            ),
            Self::Api { status_code, body } => render_json(
                &ApiErrorResponse {
                    status: "error",
                    kind: "api",
                    status_code: *status_code,
                    body,
                },
                compact,
            ),
            Self::Config(message) => render_json(
                &MessageErrorResponse {
                    status: "error",
                    kind: "config",
                    message,
                },
                compact,
            ),
            Self::Http(message) => render_json(
                &MessageErrorResponse {
                    status: "error",
                    kind: "http",
                    message,
                },
                compact,
            ),
            Self::Io(message) => render_json(
                &MessageErrorResponse {
                    status: "error",
                    kind: "io",
                    message,
                },
                compact,
            ),
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

fn render_cli_error(error: &anyhow::Error, compact: bool) -> String {
    if let Some(error) = error.chain().find_map(|cause| cause.downcast_ref::<Error>()) {
        return error.render(compact);
    }

    render_json(
        &OwnedMessageErrorResponse {
            status: "error",
            kind: "internal",
            message: format!("{error:#}"),
        },
        compact,
    )
}

pub(crate) fn execute_bearer(
    client: &MomenceClient,
    token: String,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Option<Value>,
) -> Result<Value> {
    client.execute(crate::client::RequestSpec {
        method,
        path: path.into(),
        query,
        body: match body {
            Some(body) => RequestBody::Json(body),
            None => RequestBody::None,
        },
        auth: crate::client::AuthMode::Bearer(token),
    })
}

pub(crate) fn execute_bearer_json(
    client: &MomenceClient,
    token: String,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Value,
) -> Result<Value> {
    execute_bearer(client, token, method, path, query, Some(body))
}

fn render_json<T: Serialize>(value: &T, compact: bool) -> String {
    let serialized = if compact {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    };

    match serialized {
        Ok(serialized) => serialized,
        Err(error) => render_serialization_error(error),
    }
}

#[derive(Serialize)]
struct MessageErrorResponse<'a> {
    status: &'static str,
    kind: &'static str,
    message: &'a str,
}

#[derive(Serialize)]
struct OwnedMessageErrorResponse {
    status: &'static str,
    kind: &'static str,
    message: String,
}

#[derive(Serialize)]
struct ApiErrorResponse<'a> {
    status: &'static str,
    kind: &'static str,
    status_code: u16,
    body: &'a Value,
}

fn render_serialization_error(error: serde_json::Error) -> String {
    serde_json::to_string(&OwnedMessageErrorResponse {
        status: "error",
        kind: "serialization",
        message: error.to_string(),
    })
    .unwrap_or_else(|_| {
        "{\"status\":\"error\",\"kind\":\"serialization\",\"message\":\"failed to serialize error payload\"}".to_owned()
    })
}

#[cfg(test)]
mod tests;
