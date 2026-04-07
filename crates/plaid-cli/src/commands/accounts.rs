use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    cache::AccountSnapshotSource,
    commands::shared::{credentials, serialize_payload},
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

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AccountsGetRequest {
    pub(crate) access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) options: Option<AccountsRequestOptions>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AccountsBalanceRequest {
    pub(crate) access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) options: Option<AccountsRequestOptions>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AccountsRequestOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) account_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) min_last_updated_datetime: Option<String>,
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
                accounts_get_body(context.require_access_token()?, &args.account_ids)?,
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
                )?,
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

fn accounts_get_body(access_token: &str, account_ids: &[String]) -> Result<Value> {
    serialize_payload(AccountsGetRequest {
        access_token: access_token.to_owned(),
        options: accounts_options(account_ids, None),
    })
}

fn accounts_balance_body(
    access_token: &str,
    account_ids: &[String],
    min_last_updated_datetime: Option<String>,
) -> Result<Value> {
    serialize_payload(AccountsBalanceRequest {
        access_token: access_token.to_owned(),
        options: accounts_options(account_ids, min_last_updated_datetime),
    })
}

fn accounts_options(
    account_ids: &[String],
    min_last_updated_datetime: Option<String>,
) -> Option<AccountsRequestOptions> {
    if account_ids.is_empty() && min_last_updated_datetime.is_none() {
        return None;
    }

    Some(AccountsRequestOptions {
        account_ids: (!account_ids.is_empty()).then(|| account_ids.to_vec()),
        min_last_updated_datetime,
    })
}
