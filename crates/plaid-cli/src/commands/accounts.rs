use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use crate::{commands::shared::maybe_insert_options, PlaidClient, PlaidCredentials, ResolvedContext, Result};

#[derive(Debug, Args)]
pub(crate) struct AccountsCommand {
    #[command(subcommand)]
    pub(crate) command: AccountsSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AccountsSubcommand {
    Get(AccountsGetArgs),
    Balance(AccountsBalanceArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AccountsGetArgs {
    #[arg(long = "account-id")]
    account_ids: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AccountsBalanceArgs {
    #[arg(long = "account-id")]
    account_ids: Vec<String>,

    #[arg(long = "min-last-updated-datetime")]
    min_last_updated_datetime: Option<String>,
}

pub(crate) fn run_accounts(
    command: AccountsSubcommand,
    client: &PlaidClient,
    context: &ResolvedContext,
) -> Result<Value> {
    let credentials = credentials(context)?;
    match command {
        AccountsSubcommand::Get(args) => client.post(
            credentials,
            "/accounts/get",
            accounts_get_body(context.require_access_token()?, &args.account_ids),
        ),
        AccountsSubcommand::Balance(args) => client.post(
            credentials,
            "/accounts/balance/get",
            accounts_balance_body(
                context.require_access_token()?,
                &args.account_ids,
                args.min_last_updated_datetime,
            ),
        ),
    }
}

fn accounts_get_body(access_token: &str, account_ids: &[String]) -> Value {
    let mut body = Map::new();
    body.insert("access_token".into(), Value::String(access_token.to_owned()));

    let mut options = Map::new();
    if !account_ids.is_empty() {
        options.insert(
            "account_ids".into(),
            Value::Array(account_ids.iter().cloned().map(Value::String).collect()),
        );
    }
    maybe_insert_options(&mut body, options);
    Value::Object(body)
}

fn accounts_balance_body(
    access_token: &str,
    account_ids: &[String],
    min_last_updated_datetime: Option<String>,
) -> Value {
    let mut body = Map::new();
    body.insert("access_token".into(), Value::String(access_token.to_owned()));

    let mut options = Map::new();
    if !account_ids.is_empty() {
        options.insert(
            "account_ids".into(),
            Value::Array(account_ids.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(value) = min_last_updated_datetime {
        options.insert("min_last_updated_datetime".into(), Value::String(value));
    }
    maybe_insert_options(&mut body, options);
    Value::Object(body)
}

fn credentials(context: &ResolvedContext) -> Result<PlaidCredentials<'_>> {
    let (client_id, secret) = context.require_client_credentials()?;
    Ok(PlaidCredentials { client_id, secret })
}
