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
        TransactionsSubcommand::Sync(args) => run_transactions_sync(args, client, context),
    }
}

fn credentials(context: &ResolvedContext) -> Result<PlaidCredentials<'_>> {
    let (client_id, secret) = context.require_client_credentials()?;
    Ok(PlaidCredentials { client_id, secret })
}

fn run_transactions_sync(args: TransactionsSyncArgs, client: &PlaidClient, context: &ResolvedContext) -> Result<Value> {
    let access_token = context.require_access_token()?.to_owned();
    let account_scope = args.account_id.clone();
    let initial_cursor = if let Some(cursor) = args.cursor.clone() {
        Some(cursor)
    } else if let Some(item_id) = context.item_id.as_deref() {
        context.cache.cached_cursor(item_id, account_scope.as_deref())?
    } else {
        None
    };

    let mut cursor = initial_cursor.clone();
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut removed = Vec::new();
    let mut request_ids = Vec::new();
    let mut pages_fetched = 0_u64;

    loop {
        let response = client.post(
            credentials(context)?,
            "/transactions/sync",
            transactions_sync_body(
                &access_token,
                cursor.clone(),
                args.count,
                args.include_original_description,
                account_scope.clone(),
                args.days_requested,
            ),
        )?;
        pages_fetched += 1;

        if let Some(request_id) = response.get("request_id").and_then(Value::as_str) {
            request_ids.push(Value::String(request_id.to_owned()));
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
    if let Some(item_id) = context.item_id.as_deref() {
        context.cache.cache_transactions_sync(
            item_id,
            account_scope.as_deref(),
            &final_cursor,
            &added,
            &modified,
            &removed,
        )?;
    }

    let mut output = Map::new();
    output.insert("added".into(), Value::Array(added));
    output.insert("modified".into(), Value::Array(modified));
    output.insert("removed".into(), Value::Array(removed));
    output.insert("next_cursor".into(), Value::String(final_cursor));
    output.insert("has_more".into(), Value::Bool(false));
    output.insert("pages_fetched".into(), Value::Number(pages_fetched.into()));
    if let Some(last_request_id) = request_ids.last().cloned() {
        output.insert("request_id".into(), last_request_id);
    }
    output.insert("request_ids".into(), Value::Array(request_ids));

    Ok(Value::Object(output))
}

fn transactions_sync_body(
    access_token: &str,
    cursor: Option<String>,
    count: Option<u32>,
    include_original_description: bool,
    account_id: Option<String>,
    days_requested: Option<u32>,
) -> Value {
    let mut body = Map::new();
    body.insert("access_token".into(), Value::String(access_token.to_owned()));
    if let Some(cursor) = cursor {
        body.insert("cursor".into(), Value::String(cursor));
    }
    if let Some(count) = count {
        body.insert("count".into(), Value::Number(count.into()));
    }

    let mut options = Map::new();
    if include_original_description {
        options.insert("include_original_description".into(), Value::Bool(true));
    }
    if let Some(account_id) = account_id {
        options.insert("account_id".into(), Value::String(account_id));
    }
    if let Some(days_requested) = days_requested {
        options.insert("days_requested".into(), Value::Number(days_requested.into()));
    }
    maybe_insert_options(&mut body, options);

    Value::Object(body)
}

fn value_array(response: &Value, key: &str) -> Vec<Value> {
    response.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}
