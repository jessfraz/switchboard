use clap::{Args, Subcommand};
use serde_json::Value;

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    commands::shared::{
        load_json_body, optional_query, resolve_account_id, resolve_latest_rfc3339_window, JsonBodyArgs,
    },
    ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct OrderCommand {
    #[command(subcommand)]
    pub(crate) command: OrderSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum OrderSubcommand {
    List(OrderListArgs),
    Get(OrderGetArgs),
    Preview(OrderPreviewArgs),
    Place(OrderPlaceArgs),
    Replace(OrderReplaceArgs),
    Cancel(OrderCancelArgs),
}

#[derive(Debug, Args)]
pub(crate) struct OrderListArgs {
    #[arg(long)]
    account: Option<String>,

    #[arg(
        long,
        value_name = "RFC3339",
        help = "RFC3339 start time. Defaults to 30 days before --to-entered-time or now."
    )]
    from_entered_time: Option<String>,

    #[arg(long, value_name = "RFC3339", help = "RFC3339 end time. Defaults to now.")]
    to_entered_time: Option<String>,

    #[arg(long)]
    max_results: Option<u64>,

    #[arg(long)]
    status: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct OrderGetArgs {
    #[arg(long)]
    account: String,

    order_id: u64,
}

#[derive(Debug, Args)]
pub(crate) struct OrderPlaceArgs {
    #[arg(long)]
    account: String,

    #[command(flatten)]
    body: JsonBodyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct OrderPreviewArgs {
    #[arg(long)]
    account: String,

    #[command(flatten)]
    body: JsonBodyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct OrderReplaceArgs {
    #[arg(long)]
    account: String,

    order_id: u64,

    #[command(flatten)]
    body: JsonBodyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct OrderCancelArgs {
    #[arg(long)]
    account: String,

    order_id: u64,
}

pub(crate) fn run_orders(command: OrderSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    let client = trader_client(context)?;

    match command {
        OrderSubcommand::List(args) => {
            let (from_entered_time, to_entered_time) =
                resolve_latest_rfc3339_window(args.from_entered_time, args.to_entered_time)?;
            let mut query = vec![
                ("fromEnteredTime".into(), from_entered_time),
                ("toEnteredTime".into(), to_entered_time),
            ];
            optional_query(
                &mut query,
                "maxResults",
                args.max_results.map(|value| value.to_string()),
            );
            optional_query(&mut query, "status", args.status);

            let path = if let Some(account) = args.account {
                let account_id = resolve_account_id(&client, context, &account)?;
                format!("/accounts/{account_id}/orders")
            } else {
                "/orders".into()
            };

            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path,
                query,
                headers: context.trader_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        OrderSubcommand::Get(args) => {
            let account_id = resolve_account_id(&client, context, &args.account)?;
            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: format!("/accounts/{account_id}/orders/{}", args.order_id),
                query: Vec::new(),
                headers: context.trader_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        OrderSubcommand::Preview(args) => {
            let account_id = resolve_account_id(&client, context, &args.account)?;
            client.execute(RequestSpec {
                method: reqwest::Method::POST,
                path: format!("/accounts/{account_id}/previewOrder"),
                query: Vec::new(),
                headers: context.trader_headers(),
                body: RequestBody::Json(load_json_body(&args.body)?),
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        OrderSubcommand::Place(args) => {
            let account_id = resolve_account_id(&client, context, &args.account)?;
            client
                .execute_response(RequestSpec {
                    method: reqwest::Method::POST,
                    path: format!("/accounts/{account_id}/orders"),
                    query: Vec::new(),
                    headers: context.trader_headers(),
                    body: RequestBody::Json(load_json_body(&args.body)?),
                    auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
                })
                .map(|response| response.into_output())
        }
        OrderSubcommand::Replace(args) => {
            let account_id = resolve_account_id(&client, context, &args.account)?;
            client
                .execute_response(RequestSpec {
                    method: reqwest::Method::PUT,
                    path: format!("/accounts/{account_id}/orders/{}", args.order_id),
                    query: Vec::new(),
                    headers: context.trader_headers(),
                    body: RequestBody::Json(load_json_body(&args.body)?),
                    auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
                })
                .map(|response| response.into_output())
        }
        OrderSubcommand::Cancel(args) => {
            let account_id = resolve_account_id(&client, context, &args.account)?;
            client.execute(RequestSpec {
                method: reqwest::Method::DELETE,
                path: format!("/accounts/{account_id}/orders/{}", args.order_id),
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
