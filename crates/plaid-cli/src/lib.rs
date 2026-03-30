mod client;
mod commands;
mod state;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result as AnyhowResult};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::Value;

pub(crate) use crate::{
    client::{PlaidClient, PlaidCredentials},
    state::ResolvedContext,
};
use crate::{
    commands::{
        run_accounts, run_auth, run_institutions, run_item, run_link, run_sandbox, run_transactions, AccountsCommand,
        AuthCommand, InstitutionsCommand, ItemCommand, LinkCommand, SandboxCommand, TransactionsCommand,
    },
    state::{
        PlaidEnvironment, ENV_PLAID_ACCESS_TOKEN, ENV_PLAID_BASE_URL, ENV_PLAID_CLIENT_ID, ENV_PLAID_CLIENT_NAME,
        ENV_PLAID_CONFIG, ENV_PLAID_ENVIRONMENT, ENV_PLAID_ITEM_ID, ENV_PLAID_SECRET, ENV_PLAID_VERSION,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  plaid auth status\n",
    "  plaid sandbox public-token-create --institution-id ins_109508 --product transactions --product auth\n",
    "  plaid auth exchange-public-token --public-token public-sandbox-...\n",
    "  plaid accounts get --account-id account-123\n",
    "  plaid accounts balance --account-id account-123\n",
    "  plaid transactions sync --cursor now --count 250 --days-requested 180\n",
    "  plaid link token-create --client-user-id user-123 --product transactions --country-code US --days-requested 180\n",
    "\n",
    "This CLI is aimed at Plaid Item, account, institution, and transaction workflows,\n",
    "with the same boring structured JSON output contract as the rest of the repo.\n",
);

/// Run the Plaid CLI and return a process exit code.
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
    let mut context = ResolvedContext::from_global(&cli.global).context("failed to resolve Plaid runtime context")?;
    let client = PlaidClient::new(context.base_url.clone(), context.plaid_version.clone())
        .context("failed to build Plaid client")?;

    let output = match cli.command {
        Commands::Auth(command) => run_auth(command.command, &client, &mut context),
        Commands::Link(command) => run_link(command.command, &client, &context),
        Commands::Institutions(command) => run_institutions(command.command, &client, &context),
        Commands::Item(command) => run_item(command.command, &client, &context),
        Commands::Accounts(command) => run_accounts(command.command, &client, &context),
        Commands::Transactions(command) => run_transactions(command.command, &client, &context),
        Commands::Sandbox(command) => run_sandbox(command.command, &client, &context),
    }
    .context("Plaid command failed")?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "plaid",
    version,
    about = "CLI for Plaid item, account, institution, and transaction workflows",
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
    #[arg(long, global = true, env = ENV_PLAID_CONFIG, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_PLAID_ENVIRONMENT, value_enum)]
    environment: Option<PlaidEnvironment>,

    #[arg(long, global = true, env = ENV_PLAID_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long, global = true, env = ENV_PLAID_CLIENT_ID, value_name = "CLIENT_ID")]
    client_id: Option<String>,

    #[arg(long, global = true, env = ENV_PLAID_SECRET, value_name = "SECRET")]
    secret: Option<String>,

    #[arg(long, global = true, env = ENV_PLAID_ACCESS_TOKEN, value_name = "TOKEN")]
    access_token: Option<String>,

    #[arg(long, global = true, env = ENV_PLAID_ITEM_ID, value_name = "ITEM_ID")]
    item_id: Option<String>,

    #[arg(long = "plaid-version", global = true, env = ENV_PLAID_VERSION, value_name = "DATE")]
    plaid_version: Option<String>,

    #[arg(long = "client-name", global = true, env = ENV_PLAID_CLIENT_NAME, value_name = "NAME")]
    client_name: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Auth(AuthCommand),
    Link(LinkCommand),
    Institutions(InstitutionsCommand),
    Item(ItemCommand),
    Accounts(AccountsCommand),
    Transactions(TransactionsCommand),
    Sandbox(SandboxCommand),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid arguments: {0}")]
    Arguments(String),
    #[error("Plaid API returned HTTP {status_code}")]
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
    format!(
        "{{\"status\":\"error\",\"kind\":\"serialization\",\"message\":{}}}",
        serde_json::Value::String(format!("failed to serialize response: {error}"))
    )
}

#[cfg(test)]
mod tests;
