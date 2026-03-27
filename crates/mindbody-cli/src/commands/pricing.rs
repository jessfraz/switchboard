use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::Value;

use crate::{commands::shared::push_query_csv_u64, execute, MindbodyClient, ResolvedContext, Result};

#[derive(Debug, Args)]
pub(crate) struct PricingCommand {
    #[command(subcommand)]
    pub(crate) command: PricingSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PricingSubcommand {
    Class(ClassPricingArgs),
    Location(LocationPricingArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ClassPricingArgs {
    #[arg(long = "location-id")]
    location_id: u64,

    #[arg(value_name = "CLASS_ID")]
    class_id: u64,
}

#[derive(Debug, Args)]
pub(crate) struct LocationPricingArgs {
    #[arg(value_name = "LOCATION_ID")]
    location_id: u64,

    #[arg(long = "service-category-id")]
    service_category_ids: Vec<u64>,
}

pub(crate) fn run_pricing(
    command: PricingSubcommand,
    client: &MindbodyClient,
    context: &ResolvedContext,
) -> Result<Value> {
    match command {
        PricingSubcommand::Class(args) => execute(
            client,
            context,
            Method::GET,
            &format!(
                "/locations/{}/classes/{}/pricingOptions",
                args.location_id, args.class_id
            ),
            Vec::new(),
        ),
        PricingSubcommand::Location(args) => {
            let mut query = Vec::new();
            push_query_csv_u64(&mut query, "serviceCategoryIds", args.service_category_ids);
            execute(
                client,
                context,
                Method::GET,
                &format!("/locations/{}/pricingOptions", args.location_id),
                query,
            )
        }
    }
}
