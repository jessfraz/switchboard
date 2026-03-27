use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::Value;

use crate::{
    commands::shared::{
        push_optional_query_string, push_optional_query_u64, push_ordering_query, push_window_query, OrderingArgs,
        WindowedQueryArgs,
    },
    execute, MindbodyClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct PurchaseCommand {
    #[command(subcommand)]
    pub(crate) command: PurchaseSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PurchaseSubcommand {
    List(ListPurchasesArgs),
    Get(PurchaseIdArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ListPurchasesArgs {
    #[arg(long = "location-id")]
    location_id: Option<u64>,

    #[arg(long = "subscriber-id")]
    subscriber_id: Option<u64>,

    #[arg(long = "from-purchase-date-time")]
    from_purchase_date_time: Option<String>,

    #[arg(long = "to-purchase-date-time")]
    to_purchase_date_time: Option<String>,

    #[command(flatten)]
    window: WindowedQueryArgs,

    #[command(flatten)]
    ordering: OrderingArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PurchaseIdArgs {
    #[arg(value_name = "PURCHASE_ID")]
    purchase_id: String,
}

pub(crate) fn run_purchases(
    command: PurchaseSubcommand,
    client: &MindbodyClient,
    context: &ResolvedContext,
) -> Result<Value> {
    let user_id = context.require_user_id()?.to_owned();
    match command {
        PurchaseSubcommand::List(args) => {
            let mut query = Vec::new();
            push_optional_query_u64(&mut query, "locationId", args.location_id);
            push_optional_query_u64(&mut query, "subscriberId", args.subscriber_id);
            push_optional_query_string(&mut query, "fromPurchaseDateTime", args.from_purchase_date_time);
            push_optional_query_string(&mut query, "toPurchaseDateTime", args.to_purchase_date_time);
            push_window_query(&mut query, &args.window);
            push_ordering_query(&mut query, &args.ordering);
            execute(
                client,
                context,
                Method::GET,
                &format!("/users/{user_id}/purchases"),
                query,
            )
        }
        PurchaseSubcommand::Get(args) => execute(
            client,
            context,
            Method::GET,
            &format!("/users/{user_id}/purchases/{}", args.purchase_id),
            Vec::new(),
        ),
    }
}
