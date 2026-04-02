use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use crate::{
    cache::AccountSnapshotSource,
    commands::shared::{credentials, maybe_insert_options},
    PlaidClient, ResolvedContext, Result,
};

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
    context: &mut ResolvedContext,
) -> Result<Value> {
    let credentials = credentials(context)?;
    match command {
        AccountsSubcommand::Get(args) => {
            let response = client.post(
                credentials,
                "/accounts/get",
                accounts_get_body(context.require_access_token()?, &args.account_ids),
            )?;
            cache_accounts_response(context, &response, AccountSnapshotSource::AccountsGet)?;
            Ok(response)
        }
        AccountsSubcommand::Balance(args) => {
            let response = client.post(
                credentials,
                "/accounts/balance/get",
                accounts_balance_body(
                    context.require_access_token()?,
                    &args.account_ids,
                    args.min_last_updated_datetime,
                ),
            )?;
            cache_accounts_response(context, &response, AccountSnapshotSource::AccountsBalanceGet)?;
            Ok(response)
        }
    }
}

fn cache_accounts_response(
    context: &mut ResolvedContext,
    response: &Value,
    source: AccountSnapshotSource,
) -> Result<()> {
    let Some(item) = response.get("item") else {
        return Ok(());
    };
    let Some(item_id) = context.cache.cache_item(item)? else {
        return Ok(());
    };
    context.remember_item_id(item_id.clone())?;

    let accounts = response
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    context.cache.cache_accounts(&item_id, &accounts, source)
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
