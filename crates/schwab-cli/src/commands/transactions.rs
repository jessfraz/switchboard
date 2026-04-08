use clap::{Args, Subcommand};
use serde_json::Value;

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    commands::shared::{optional_query, resolve_account_id, resolve_all_account_ids, resolve_latest_rfc3339_window},
    ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct TransactionCommand {
    #[command(subcommand)]
    pub(crate) command: TransactionSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TransactionSubcommand {
    List(TransactionListArgs),
    Get(TransactionGetArgs),
}

#[derive(Debug, Args)]
pub(crate) struct TransactionListArgs {
    #[arg(long)]
    account: Option<String>,

    #[arg(
        long,
        value_name = "RFC3339",
        help = "RFC3339 start time. Defaults to 30 days before --end-date or now."
    )]
    start_date: Option<String>,

    #[arg(long, value_name = "RFC3339", help = "RFC3339 end time. Defaults to now.")]
    end_date: Option<String>,

    #[arg(
        long,
        help = "Comma-separated transaction types. Omit or pass ALL to return every type."
    )]
    types: Option<String>,

    #[arg(long)]
    symbol: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct TransactionGetArgs {
    #[arg(long)]
    account: String,

    transaction_id: u64,
}

pub(crate) fn run_transactions(command: TransactionSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    let client = trader_client(context)?;

    match command {
        TransactionSubcommand::List(args) => {
            let (start_date, end_date) = resolve_latest_rfc3339_window(args.start_date, args.end_date)?;
            let account_ids = match args.account.as_deref() {
                Some(account) => vec![resolve_account_id(&client, context, account)?],
                None => resolve_all_account_ids(&client, context)?,
            };
            let mut query = vec![("startDate".into(), start_date), ("endDate".into(), end_date)];
            optional_query(&mut query, "types", normalize_transaction_types(args.types));
            optional_query(&mut query, "symbol", args.symbol);

            let mut combined = Vec::new();
            for account_id in account_ids {
                let response = client.execute(RequestSpec {
                    method: reqwest::Method::GET,
                    path: format!("/accounts/{account_id}/transactions"),
                    query: query.clone(),
                    headers: context.trader_headers(),
                    body: RequestBody::None,
                    auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
                })?;
                let transactions = response.as_array().ok_or_else(|| {
                    crate::Error::Config("Schwab transaction list response was not the expected array payload".into())
                })?;
                combined.extend(transactions.iter().cloned());
            }
            sort_transactions_descending(&mut combined);
            Ok(Value::Array(combined))
        }
        TransactionSubcommand::Get(args) => {
            let account_id = resolve_account_id(&client, context, &args.account)?;
            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: format!("/accounts/{account_id}/transactions/{}", args.transaction_id),
                query: Vec::new(),
                headers: context.trader_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
    }
}

fn normalize_transaction_types(types: Option<String>) -> Option<String> {
    types.and_then(|types| {
        let trimmed = types.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("ALL") {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

fn sort_transactions_descending(transactions: &mut [Value]) {
    transactions.sort_by(|left, right| transaction_sort_key(right).cmp(&transaction_sort_key(left)));
}

fn transaction_sort_key(transaction: &Value) -> (&str, u64) {
    let time = transaction
        .get("time")
        .and_then(Value::as_str)
        .or_else(|| transaction.get("tradeDate").and_then(Value::as_str))
        .unwrap_or("");
    let activity_id = transaction
        .get("activityId")
        .and_then(Value::as_u64)
        .or_else(|| transaction.get("transactionId").and_then(Value::as_u64))
        .unwrap_or(0);
    (time, activity_id)
}

fn trader_client(context: &ResolvedContext) -> Result<SchwabClient> {
    SchwabClient::new(
        context.base_url.clone(),
        format!("schwab-cli/{}", env!("CARGO_PKG_VERSION")),
    )
}
