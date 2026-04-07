use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    commands::shared::{credentials, ensure_item_id, serialize_payload, AccessTokenRequest},
    Error, PlaidClient, ResolvedContext, Result,
};

const MAX_SYNC_RESTARTS: u32 = 3;

#[derive(Debug, Args)]
pub(crate) struct TransactionsCommand {
    #[command(subcommand)]
    pub(crate) command: TransactionsSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TransactionsSubcommand {
    Sync(TransactionsSyncArgs),
    Refresh,
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

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TransactionsSyncRequest {
    pub(crate) access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) options: Option<TransactionsSyncOptions>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TransactionsSyncOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) include_original_description: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) days_requested: Option<u32>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct TransactionsSyncOutput {
    pub(crate) item_id: String,
    pub(crate) added: Vec<Value>,
    pub(crate) modified: Vec<Value>,
    pub(crate) removed: Vec<Value>,
    pub(crate) next_cursor: String,
    pub(crate) has_more: bool,
    pub(crate) pages_fetched: u64,
    pub(crate) restart_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) request_id: Option<String>,
    pub(crate) request_ids: Vec<String>,
}

pub(crate) fn run_transactions(
    command: TransactionsSubcommand,
    client: &PlaidClient,
    context: &mut ResolvedContext,
) -> Result<Value> {
    match command {
        TransactionsSubcommand::Sync(args) => run_transactions_sync(args, client, context),
        TransactionsSubcommand::Refresh => run_transactions_refresh(client, context),
    }
}

fn run_transactions_refresh(client: &PlaidClient, context: &mut ResolvedContext) -> Result<Value> {
    let access_token = context.require_access_token()?.to_owned();
    let item_id = ensure_item_id(client, context)?;
    let mut response = client.post(
        credentials(context)?,
        "/transactions/refresh",
        serialize_payload(AccessTokenRequest { access_token })?,
    )?;

    if let Some(object) = response.as_object_mut() {
        object.insert("item_id".into(), Value::String(item_id));
    }

    Ok(response)
}

fn run_transactions_sync(
    args: TransactionsSyncArgs,
    client: &PlaidClient,
    context: &mut ResolvedContext,
) -> Result<Value> {
    let access_token = context.require_access_token()?.to_owned();
    let item_id = ensure_item_id(client, context)?;
    let account_scope = args.account_id.clone();
    let initial_cursor = if let Some(cursor) = args.cursor.clone() {
        Some(cursor)
    } else {
        context.cache.cached_cursor(&item_id, account_scope.as_deref())?
    };

    let mut restart_count = 0_u32;
    'pagination: loop {
        let mut cursor = initial_cursor.clone();
        let mut added = Vec::new();
        let mut modified = Vec::new();
        let mut removed = Vec::new();
        let mut request_ids = Vec::new();
        let mut pages_fetched = 0_u64;

        loop {
            let response = match client.post(
                credentials(context)?,
                "/transactions/sync",
                transactions_sync_body(
                    &access_token,
                    cursor.clone(),
                    args.count,
                    args.include_original_description,
                    account_scope.clone(),
                    args.days_requested,
                )?,
            ) {
                Ok(response) => response,
                Err(Error::Api { status_code, body })
                    if is_sync_mutation_during_pagination(status_code, &body) && restart_count < MAX_SYNC_RESTARTS =>
                {
                    restart_count += 1;
                    continue 'pagination;
                }
                Err(Error::Api { status_code, body }) if is_sync_mutation_during_pagination(status_code, &body) => {
                    return Err(Error::Http(format!(
                        "Plaid transactions sync kept mutating during pagination after {MAX_SYNC_RESTARTS} restart attempts"
                    )));
                }
                Err(error) => return Err(error),
            };
            pages_fetched += 1;

            if let Some(request_id) = response.get("request_id").and_then(Value::as_str) {
                request_ids.push(request_id.to_owned());
            }
            added.extend(value_array(&response, "added"));
            modified.extend(value_array(&response, "modified"));
            removed.extend(value_array(&response, "removed"));

            let next_cursor = response
                .get("next_cursor")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_default();
            let has_more = response.get("has_more").and_then(Value::as_bool).unwrap_or(false);
            cursor = Some(next_cursor);

            if !has_more {
                break;
            }
        }

        let final_cursor = cursor.unwrap_or_default();
        context.cache.cache_transactions_sync(
            &item_id,
            account_scope.as_deref(),
            &final_cursor,
            &added,
            &modified,
            &removed,
        )?;

        return serialize_payload(TransactionsSyncOutput {
            item_id,
            added,
            modified,
            removed,
            next_cursor: final_cursor,
            has_more: false,
            pages_fetched,
            restart_count,
            request_id: request_ids.last().cloned(),
            request_ids,
        });
    }
}

fn transactions_sync_body(
    access_token: &str,
    cursor: Option<String>,
    count: Option<u32>,
    include_original_description: bool,
    account_id: Option<String>,
    days_requested: Option<u32>,
) -> Result<Value> {
    let options = if include_original_description || account_id.is_some() || days_requested.is_some() {
        Some(TransactionsSyncOptions {
            include_original_description: include_original_description.then_some(true),
            account_id,
            days_requested,
        })
    } else {
        None
    };

    serialize_payload(TransactionsSyncRequest {
        access_token: access_token.to_owned(),
        cursor,
        count,
        options,
    })
}

fn value_array(response: &Value, key: &str) -> Vec<Value> {
    response.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn is_sync_mutation_during_pagination(status_code: u16, body: &Value) -> bool {
    status_code == 400
        && body
            .get("error_code")
            .and_then(Value::as_str)
            .map(|error_code| error_code == "TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION")
            .unwrap_or(false)
}
