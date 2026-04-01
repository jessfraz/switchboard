use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use crate::{
    cache::{CachedTransactionQuery, PlaidCacheStore},
    ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct CacheCommand {
    #[command(subcommand)]
    pub(crate) command: CacheSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CacheSubcommand {
    Items(CacheItemsArgs),
    Accounts(CacheAccountsArgs),
    Transactions(CacheTransactionsArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CacheItemsArgs {
    #[arg(long)]
    all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct CacheAccountsArgs {
    #[arg(long)]
    all: bool,

    #[arg(long = "account-id")]
    account_ids: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct CacheTransactionsArgs {
    #[arg(long)]
    all: bool,

    #[arg(long)]
    account_id: Option<String>,

    #[arg(long = "transaction-id")]
    transaction_ids: Vec<String>,

    #[arg(long)]
    include_removed: bool,

    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    limit: Option<u32>,
}

pub(crate) fn run_cache(command: CacheSubcommand, context: &ResolvedContext) -> Result<Value> {
    match command {
        CacheSubcommand::Items(args) => cache_items(args, context),
        CacheSubcommand::Accounts(args) => cache_accounts(args, context),
        CacheSubcommand::Transactions(args) => cache_transactions(args, context),
    }
}

fn cache_items(args: CacheItemsArgs, context: &ResolvedContext) -> Result<Value> {
    let item_scope = item_scope(context, args.all);
    let rows = context.cache.cached_items(item_scope)?;

    let mut response = Map::new();
    response.insert("source".into(), Value::String("cache".into()));
    response.insert("count".into(), Value::Number((rows.len() as u64).into()));
    response.insert(
        "items".into(),
        Value::Array(
            rows.into_iter()
                .map(|row| {
                    json!({
                        "item_id": row.item_id,
                        "institution_id": row.institution_id,
                        "updated_at": row.updated_at,
                        "item": row.item,
                    })
                })
                .collect(),
        ),
    );
    maybe_insert_item_scope(&mut response, item_scope);

    Ok(Value::Object(response))
}

fn cache_accounts(args: CacheAccountsArgs, context: &ResolvedContext) -> Result<Value> {
    let item_scope = item_scope(context, args.all);
    let rows = context.cache.cached_accounts(item_scope, &args.account_ids)?;

    let mut response = Map::new();
    response.insert("source".into(), Value::String("cache".into()));
    response.insert("count".into(), Value::Number((rows.len() as u64).into()));
    response.insert(
        "accounts".into(),
        Value::Array(
            rows.into_iter()
                .map(|row| {
                    json!({
                        "account_id": row.account_id,
                        "item_id": row.item_id,
                        "updated_at": row.updated_at,
                        "account": row.account,
                    })
                })
                .collect(),
        ),
    );
    maybe_insert_item_scope(&mut response, item_scope);
    if !args.account_ids.is_empty() {
        response.insert(
            "account_ids".into(),
            Value::Array(args.account_ids.into_iter().map(Value::String).collect()),
        );
    }

    Ok(Value::Object(response))
}

fn cache_transactions(args: CacheTransactionsArgs, context: &ResolvedContext) -> Result<Value> {
    let item_scope = item_scope(context, args.all);
    let rows = context.cache.cached_transactions(CachedTransactionQuery {
        item_id: item_scope,
        account_id: args.account_id.as_deref(),
        transaction_ids: &args.transaction_ids,
        include_removed: args.include_removed,
        limit: args.limit,
    })?;

    let mut response = Map::new();
    response.insert("source".into(), Value::String("cache".into()));
    response.insert("count".into(), Value::Number((rows.len() as u64).into()));
    response.insert(
        "transactions".into(),
        Value::Array(
            rows.into_iter()
                .map(|row| {
                    json!({
                        "transaction_id": row.transaction_id,
                        "item_id": row.item_id,
                        "account_id": row.account_id,
                        "removed": row.removed,
                        "updated_at": row.updated_at,
                        "transaction": row.transaction,
                    })
                })
                .collect(),
        ),
    );
    maybe_insert_item_scope(&mut response, item_scope);
    if let Some(account_id) = args.account_id.as_deref() {
        response.insert("account_id".into(), Value::String(account_id.to_owned()));
    }
    if !args.transaction_ids.is_empty() {
        response.insert(
            "transaction_ids".into(),
            Value::Array(args.transaction_ids.into_iter().map(Value::String).collect()),
        );
    }
    if args.include_removed {
        response.insert("include_removed".into(), Value::Bool(true));
    }
    if let Some(limit) = args.limit {
        response.insert("limit".into(), Value::Number(limit.into()));
    }
    if let Some(item_id) = item_scope {
        if let Some(cursor) = transaction_cursor(&context.cache, item_id, args.account_id.as_deref())? {
            response.insert("cursor".into(), Value::String(cursor));
        }
    }

    Ok(Value::Object(response))
}

fn item_scope(context: &ResolvedContext, all: bool) -> Option<&str> {
    if all {
        None
    } else {
        context.item_id.as_deref()
    }
}

fn maybe_insert_item_scope(response: &mut Map<String, Value>, item_scope: Option<&str>) {
    if let Some(item_id) = item_scope {
        response.insert("item_id".into(), Value::String(item_id.to_owned()));
    }
}

fn transaction_cursor(cache: &PlaidCacheStore, item_id: &str, account_id: Option<&str>) -> Result<Option<String>> {
    cache.cached_cursor(item_id, account_id)
}
