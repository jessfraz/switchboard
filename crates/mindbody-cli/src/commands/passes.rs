use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::Value;

use crate::{
    commands::shared::{
        push_optional_query_bool, push_optional_query_u64, push_ordering_query, push_window_query, OrderingArgs,
        WindowedQueryArgs,
    },
    execute, MindbodyClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct PassCommand {
    #[command(subcommand)]
    pub(crate) command: PassSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PassSubcommand {
    List(ListPassesArgs),
    Get(PassIdArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ListPassesArgs {
    #[arg(long = "subscriber-id")]
    subscriber_id: Option<u64>,

    #[arg(long = "limit-to-usable", value_parser = clap::builder::BoolishValueParser::new())]
    limit_to_usable: Option<bool>,

    #[command(flatten)]
    window: WindowedQueryArgs,

    #[command(flatten)]
    ordering: OrderingArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PassIdArgs {
    #[arg(value_name = "PASS_ID")]
    pass_id: String,
}

pub(crate) fn run_passes(command: PassSubcommand, client: &MindbodyClient, context: &ResolvedContext) -> Result<Value> {
    let user_id = context.require_user_id()?.to_owned();
    match command {
        PassSubcommand::List(args) => {
            let mut query = Vec::new();
            push_optional_query_u64(&mut query, "subscriberId", args.subscriber_id);
            push_optional_query_bool(&mut query, "limitToUsable", args.limit_to_usable);
            push_window_query(&mut query, &args.window);
            push_ordering_query(&mut query, &args.ordering);
            execute(client, context, Method::GET, &format!("/users/{user_id}/passes"), query)
        }
        PassSubcommand::Get(args) => execute(
            client,
            context,
            Method::GET,
            &format!("/users/{user_id}/passes/{}", args.pass_id),
            Vec::new(),
        ),
    }
}
