use clap::{Args, Subcommand};
use serde_json::Value;

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    commands::shared::{optional_query, resolve_account_id},
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
    account: String,

    #[arg(long, value_name = "RFC3339")]
    start_date: String,

    #[arg(long, value_name = "RFC3339")]
    end_date: String,

    #[arg(long)]
    types: String,

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
            let account_id = resolve_account_id(&client, context, &args.account)?;
            let mut query = vec![
                ("startDate".into(), args.start_date),
                ("endDate".into(), args.end_date),
                ("types".into(), args.types),
            ];
            optional_query(&mut query, "symbol", args.symbol);

            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: format!("/accounts/{account_id}/transactions"),
                query,
                headers: context.trader_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
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

fn trader_client(context: &ResolvedContext) -> Result<SchwabClient> {
    SchwabClient::new(
        context.base_url.clone(),
        format!("schwab-cli/{}", env!("CARGO_PKG_VERSION")),
    )
}
