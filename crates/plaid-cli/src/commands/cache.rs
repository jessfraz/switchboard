use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    cache::{CachedTransactionQuery, PlaidCacheStore},
    commands::shared::serialize_payload,
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

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct CacheItemsOutput {
    pub(crate) source: String,
    pub(crate) count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_id: Option<String>,
    pub(crate) items: Vec<CacheItemRow>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct CacheItemRow {
    pub(crate) item_id: String,
    pub(crate) institution_id: Option<String>,
    pub(crate) updated_at: String,
    pub(crate) item: Value,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct CacheAccountsOutput {
    pub(crate) source: String,
    pub(crate) count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) account_ids: Option<Vec<String>>,
    pub(crate) accounts: Vec<CacheAccountRow>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct CacheAccountRow {
    pub(crate) account_id: String,
    pub(crate) item_id: String,
    pub(crate) source_endpoint: String,
    pub(crate) balance_freshness: String,
    pub(crate) updated_at: String,
    pub(crate) account: Value,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct CacheTransactionsOutput {
    pub(crate) source: String,
    pub(crate) count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) transaction_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) include_removed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cursor: Option<String>,
    pub(crate) transactions: Vec<CacheTransactionRow>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct CacheTransactionRow {
    pub(crate) transaction_id: String,
    pub(crate) item_id: String,
    pub(crate) account_id: Option<String>,
    pub(crate) removed: bool,
    pub(crate) updated_at: String,
    pub(crate) removed_at: Option<String>,
    pub(crate) transaction: Value,
    pub(crate) removal: Option<Value>,
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
    serialize_payload(CacheItemsOutput {
        source: "cache".into(),
        count: rows.len() as u64,
        item_id: item_scope.map(ToOwned::to_owned),
        items: rows
            .into_iter()
            .map(|row| CacheItemRow {
                item_id: row.item_id,
                institution_id: row.institution_id,
                updated_at: row.updated_at,
                item: row.item,
            })
            .collect(),
    })
}

fn cache_accounts(args: CacheAccountsArgs, context: &ResolvedContext) -> Result<Value> {
    let item_scope = item_scope(context, args.all);
    let rows = context.cache.cached_accounts(item_scope, &args.account_ids)?;
    serialize_payload(CacheAccountsOutput {
        source: "cache".into(),
        count: rows.len() as u64,
        item_id: item_scope.map(ToOwned::to_owned),
        account_ids: (!args.account_ids.is_empty()).then_some(args.account_ids),
        accounts: rows
            .into_iter()
            .map(|row| CacheAccountRow {
                account_id: row.account_id,
                item_id: row.item_id,
                source_endpoint: row.source.endpoint().to_owned(),
                balance_freshness: row.source.balance_freshness().to_owned(),
                updated_at: row.updated_at,
                account: row.account,
            })
            .collect(),
    })
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
    let cursor = if let Some(item_id) = item_scope {
        transaction_cursor(&context.cache, item_id, args.account_id.as_deref())?
    } else {
        None
    };

    serialize_payload(CacheTransactionsOutput {
        source: "cache".into(),
        count: rows.len() as u64,
        item_id: item_scope.map(ToOwned::to_owned),
        account_id: args.account_id,
        transaction_ids: (!args.transaction_ids.is_empty()).then_some(args.transaction_ids),
        include_removed: args.include_removed.then_some(true),
        limit: args.limit,
        cursor,
        transactions: rows
            .into_iter()
            .map(|row| CacheTransactionRow {
                transaction_id: row.transaction_id,
                item_id: row.item_id,
                account_id: row.account_id,
                removed: row.removed,
                updated_at: row.updated_at,
                removed_at: row.removed_at,
                transaction: row.transaction,
                removal: row.removal,
            })
            .collect(),
    })
}

fn item_scope(context: &ResolvedContext, all: bool) -> Option<&str> {
    if all {
        None
    } else {
        context.item_id.as_deref()
    }
}

fn transaction_cursor(cache: &PlaidCacheStore, item_id: &str, account_id: Option<&str>) -> Result<Option<String>> {
    cache.cached_cursor(item_id, account_id)
}
