mod client;
mod commands;
mod state;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result as AnyhowResult};
use clap::{Args, Parser, Subcommand};
use reqwest::Method;
use serde::Serialize;
use serde_json::Value;

pub(crate) use crate::{client::MindbodyClient, state::ResolvedContext};
use crate::{
    client::{Credentials, RequestBody, RequestSpec},
    commands::{
        run_account, run_bookings, run_classes, run_liability_waivers, run_locations, run_passes, run_pricing,
        run_purchases, AccountCommand, BookingCommand, ClassCommand, LiabilityWaiverCommand, LocationCommand,
        PassCommand, PricingCommand, PurchaseCommand,
    },
    state::{
        ENV_MINDBODY_API_KEY, ENV_MINDBODY_APP_NAME, ENV_MINDBODY_BASE_URL, ENV_MINDBODY_CLIENT_KEY,
        ENV_MINDBODY_CLIENT_SECRET, ENV_MINDBODY_CONFIG, ENV_MINDBODY_USER_ID,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  mindbody locations search --search-text pilates --address '90210'\n",
    "  mindbody classes list --location-id 86784 --available-for-booking true --start-date-time 2026-03-27T00:00:00Z\n",
    "  mindbody pricing class --location-id 86784 5134512\n",
    "  mindbody bookings create --location-id 86784 --class-id 5134512 \\\n",
    "    --reconciliation-type pass --reconciliation-id 598a6916-7876-406e-9537-db6af825f9a2\n",
    "  mindbody purchases list --location-id 86784 --from-purchase-date-time 2026-03-01T00:00:00Z\n",
    "  mindbody liability-waivers sign --booking-id f5405d87-46a0-4b48-a384-e26159e130d6 \\\n",
    "    --liability-waiver-hashed-text <hash> --signature-png-file signature.png\n",
    "\n",
    "This CLI is aimed at booking Pilates classes and the surrounding Mindbody member account chaos,\n",
    "with a switchboard-friendly command grammar instead of raw endpoint confetti.\n",
);

/// Run the Mindbody CLI and return a process exit code.
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
    let context = ResolvedContext::from_global(&cli.global).context("failed to resolve Mindbody runtime context")?;
    let client = MindbodyClient::new(context.base_url.clone(), context.app_name.clone())
        .context("failed to build Mindbody client")?;

    let output = match cli.command {
        Commands::Account(command) => run_account(command.command, &context),
        Commands::Locations(command) => run_locations(command.command, &client, &context),
        Commands::Classes(command) => run_classes(command.command, &client, &context),
        Commands::Pricing(command) => run_pricing(command.command, &client, &context),
        Commands::Bookings(command) => run_bookings(command.command, &client, &context),
        Commands::Passes(command) => run_passes(command.command, &client, &context),
        Commands::Purchases(command) => run_purchases(command.command, &client, &context),
        Commands::LiabilityWaivers(command) => run_liability_waivers(command.command, &client, &context),
    }
    .context("Mindbody command failed")?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "mindbody",
    version,
    about = "CLI for booking Pilates classes and handling Mindbody member account workflows",
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
    #[arg(long, global = true, env = ENV_MINDBODY_CONFIG, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_MINDBODY_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_API_KEY, value_name = "API_KEY")]
    api_key: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_CLIENT_KEY, value_name = "CLIENT_KEY")]
    client_key: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_CLIENT_SECRET, value_name = "CLIENT_SECRET")]
    client_secret: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_USER_ID, value_name = "USER_ID")]
    user_id: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_APP_NAME, value_name = "NAME")]
    app_name: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Account(AccountCommand),
    Locations(LocationCommand),
    Classes(ClassCommand),
    Pricing(PricingCommand),
    Bookings(BookingCommand),
    Passes(PassCommand),
    Purchases(PurchaseCommand),
    LiabilityWaivers(LiabilityWaiverCommand),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid arguments: {0}")]
    Arguments(String),
    #[error("Mindbody API returned HTTP {status_code}")]
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

pub(crate) fn execute(
    client: &MindbodyClient,
    context: &ResolvedContext,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
) -> Result<Value> {
    let (api_key, client_key, client_secret) = context.require_credentials()?;
    client.execute(
        Credentials {
            api_key,
            client_key,
            client_secret,
        },
        RequestSpec {
            method,
            path: path.into(),
            query,
            body: RequestBody::None,
            idempotency_key: None,
        },
    )
}

pub(crate) fn execute_json(
    client: &MindbodyClient,
    context: &ResolvedContext,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Value,
    idempotency_key: Option<String>,
) -> Result<Value> {
    let (api_key, client_key, client_secret) = context.require_credentials()?;
    client.execute(
        Credentials {
            api_key,
            client_key,
            client_secret,
        },
        RequestSpec {
            method,
            path: path.into(),
            query,
            body: RequestBody::Json(body),
            idempotency_key,
        },
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
