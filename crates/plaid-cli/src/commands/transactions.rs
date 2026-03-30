use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use crate::{commands::shared::maybe_insert_options, PlaidClient, PlaidCredentials, ResolvedContext, Result};

#[derive(Debug, Args)]
pub(crate) struct TransactionsCommand {
    #[command(subcommand)]
    pub(crate) command: TransactionsSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TransactionsSubcommand {
    Sync(TransactionsSyncArgs),
}

#[derive(Debug, Args)]
pub(crate) struct TransactionsSyncArgs {
    #[arg(long)]
    cursor: Option<String>,

    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=500))]
    count: Option<u32>,

    #[arg(long)]
    include_original_description: bool,

    #[arg(long)]
    account_id: Option<String>,

    #[arg(long)]
    days_requested: Option<u32>,
}

pub(crate) fn run_transactions(
    command: TransactionsSubcommand,
    client: &PlaidClient,
    context: &ResolvedContext,
) -> Result<Value> {
    match command {
        TransactionsSubcommand::Sync(args) => {
            let credentials = credentials(context)?;
            let mut body = Map::new();
            body.insert(
                "access_token".into(),
                Value::String(context.require_access_token()?.to_owned()),
            );
            if let Some(cursor) = args.cursor {
                body.insert("cursor".into(), Value::String(cursor));
            }
            if let Some(count) = args.count {
                body.insert("count".into(), Value::Number(count.into()));
            }

            let mut options = Map::new();
            if args.include_original_description {
                options.insert("include_original_description".into(), Value::Bool(true));
            }
            if let Some(account_id) = args.account_id {
                options.insert("account_id".into(), Value::String(account_id));
            }
            if let Some(days_requested) = args.days_requested {
                options.insert("days_requested".into(), Value::Number(days_requested.into()));
            }
            maybe_insert_options(&mut body, options);

            client.post(credentials, "/transactions/sync", Value::Object(body))
        }
    }
}

fn credentials(context: &ResolvedContext) -> Result<PlaidCredentials<'_>> {
    let (client_id, secret) = context.require_client_credentials()?;
    Ok(PlaidCredentials { client_id, secret })
}
