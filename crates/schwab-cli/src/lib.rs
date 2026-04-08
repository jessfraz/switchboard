mod client;
mod commands;
mod state;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result as AnyhowResult};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::Value;

pub(crate) use crate::state::ResolvedContext;
use crate::{
    commands::{
        run_accounts, run_auth, run_market, run_orders, run_preferences, run_transactions, AccountCommand, AuthCommand,
        MarketCommand, OrderCommand, PreferenceCommand, TransactionCommand,
    },
    state::{
        ENV_SCHWAB_ACCESS_TOKEN, ENV_SCHWAB_AUTHORIZE_URL, ENV_SCHWAB_BASE_URL, ENV_SCHWAB_CLIENT_FUNCTION_ID,
        ENV_SCHWAB_CLIENT_ID, ENV_SCHWAB_CLIENT_SECRET, ENV_SCHWAB_CONFIG, ENV_SCHWAB_MARKET_DATA_BASE_URL,
        ENV_SCHWAB_REDIRECT_URI, ENV_SCHWAB_REFRESH_TOKEN, ENV_SCHWAB_RESOURCE_VERSION, ENV_SCHWAB_RRBUS_PILOT_ROLLOUT,
        ENV_SCHWAB_THIRD_PARTY_ID, ENV_SCHWAB_TOKEN_URL, ENV_SCHWAB_TRADER_CLIENT_APP_ID,
        ENV_SCHWAB_TRADER_CLIENT_CHANNEL,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  schwab auth login\n",
    "  schwab auth authorize-url\n",
    "  schwab auth exchange-url '<auth-code>'\n",
    "  schwab accounts numbers\n",
    "  schwab accounts get 123456789 --positions\n",
    "  schwab transactions list --account 123456789\n",
    "  schwab transactions list --account 123456789 \\\n",
    "    --start-date 2026-03-01T00:00:00.000Z --end-date 2026-03-27T23:59:59.000Z --types TRADE\n",
    "  schwab orders list\n",
    "  schwab orders list --from-entered-time 2026-03-01T00:00:00.000Z \\\n",
    "    --to-entered-time 2026-03-27T23:59:59.000Z\n",
    "  schwab market quotes --symbol AAPL,MSFT --field quote,reference\n",
    "\n",
    "This CLI is aimed at Charles Schwab consumer account, brokerage, and market-data workflows.\n",
    "Schwab account numbers are automatically resolved to the encrypted hashes required by the trader API.\n",
    "Use --body or --body-file for order endpoints that expect the official JSON order schema.\n",
);

/// Run the Schwab CLI and return a process exit code.
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
    let mut context = ResolvedContext::from_global(&cli.global).context("failed to resolve Schwab runtime context")?;

    let output = match cli.command {
        Commands::Auth(command) => run_auth(command.command, &mut context),
        Commands::Accounts(command) => run_accounts(command.command, &mut context),
        Commands::Orders(command) => run_orders(command.command, &mut context),
        Commands::Transactions(command) => run_transactions(command.command, &mut context),
        Commands::Preferences(command) => run_preferences(command.command, &context),
        Commands::Market(command) => run_market(command.command, &context),
    }
    .context("Schwab command failed")?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "schwab",
    version,
    about = "CLI for Charles Schwab consumer account, trading, and market-data workflows",
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
    #[arg(long, global = true, env = ENV_SCHWAB_CONFIG, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_SCHWAB_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long = "marketdata-base-url", global = true, env = ENV_SCHWAB_MARKET_DATA_BASE_URL, value_name = "URL")]
    market_data_base_url: Option<String>,

    #[arg(long, global = true, env = ENV_SCHWAB_AUTHORIZE_URL, value_name = "URL")]
    authorize_url: Option<String>,

    #[arg(long, global = true, env = ENV_SCHWAB_TOKEN_URL, value_name = "URL")]
    token_url: Option<String>,

    #[arg(long, global = true, env = ENV_SCHWAB_CLIENT_ID, value_name = "CLIENT_ID")]
    client_id: Option<String>,

    #[arg(long, global = true, env = ENV_SCHWAB_CLIENT_SECRET, value_name = "CLIENT_SECRET")]
    client_secret: Option<String>,

    #[arg(long = "third-party-id", global = true, env = ENV_SCHWAB_THIRD_PARTY_ID, value_name = "VALUE")]
    third_party_id: Option<String>,

    #[arg(long = "client-channel", global = true, env = ENV_SCHWAB_TRADER_CLIENT_CHANNEL, value_name = "VALUE")]
    client_channel: Option<String>,

    #[arg(long = "client-app-id", global = true, env = ENV_SCHWAB_TRADER_CLIENT_APP_ID, value_name = "VALUE")]
    client_app_id: Option<String>,

    #[arg(long = "client-function-id", global = true, env = ENV_SCHWAB_CLIENT_FUNCTION_ID, value_name = "VALUE")]
    client_function_id: Option<String>,

    #[arg(long = "resource-version", global = true, env = ENV_SCHWAB_RESOURCE_VERSION, value_name = "VALUE")]
    resource_version: Option<String>,

    #[arg(long = "pilot-rollout", global = true, env = ENV_SCHWAB_RRBUS_PILOT_ROLLOUT, value_name = "VALUE")]
    rrbus_pilot_rollout: Option<String>,

    #[arg(long, global = true, env = ENV_SCHWAB_REDIRECT_URI, value_name = "URL")]
    redirect_uri: Option<String>,

    #[arg(long, global = true, env = ENV_SCHWAB_ACCESS_TOKEN, value_name = "TOKEN")]
    access_token: Option<String>,

    #[arg(long, global = true, env = ENV_SCHWAB_REFRESH_TOKEN, value_name = "TOKEN")]
    refresh_token: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Auth(AuthCommand),
    Accounts(AccountCommand),
    Orders(OrderCommand),
    Transactions(TransactionCommand),
    Preferences(PreferenceCommand),
    Market(MarketCommand),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid arguments: {0}")]
    Arguments(String),
    #[error("Schwab API returned HTTP {status_code}")]
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
