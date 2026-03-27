use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::Value;

use crate::{
    commands::shared::{
        push_optional_query_f64, push_optional_query_string, push_optional_query_u64, push_ordering_query,
        push_window_query, OrderingArgs, WindowedQueryArgs,
    },
    execute, MindbodyClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct LocationCommand {
    #[command(subcommand)]
    pub(crate) command: LocationSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LocationSubcommand {
    Search(SearchLocationsArgs),
    Get(LocationIdArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SearchLocationsArgs {
    #[arg(long)]
    address: Option<String>,

    #[arg(long)]
    latitude: Option<f64>,

    #[arg(long)]
    longitude: Option<f64>,

    #[arg(long)]
    radius: Option<f64>,

    #[arg(long = "location-id")]
    location_id: Option<u64>,

    #[arg(long = "subscriber-id")]
    subscriber_id: Option<u64>,

    #[arg(long = "country-code")]
    country_code: Option<String>,

    #[arg(long = "search-text")]
    search_text: Option<String>,

    #[command(flatten)]
    window: WindowedQueryArgs,

    #[command(flatten)]
    ordering: OrderingArgs,
}

#[derive(Debug, Args)]
pub(crate) struct LocationIdArgs {
    #[arg(value_name = "LOCATION_ID")]
    location_id: u64,
}

pub(crate) fn run_locations(
    command: LocationSubcommand,
    client: &MindbodyClient,
    context: &ResolvedContext,
) -> Result<Value> {
    match command {
        LocationSubcommand::Search(args) => {
            validate_coordinate_pair(args.latitude, args.longitude, "locations search")?;
            let mut query = Vec::new();
            push_optional_query_string(&mut query, "address", args.address);
            push_optional_query_f64(&mut query, "latitude", args.latitude);
            push_optional_query_f64(&mut query, "longitude", args.longitude);
            push_optional_query_f64(&mut query, "radius", args.radius);
            push_optional_query_u64(&mut query, "locationId", args.location_id);
            push_optional_query_u64(&mut query, "subscriberId", args.subscriber_id);
            push_optional_query_string(&mut query, "countryCode", args.country_code);
            push_optional_query_string(&mut query, "searchText", args.search_text);
            push_window_query(&mut query, &args.window);
            push_ordering_query(&mut query, &args.ordering);
            execute(client, context, Method::GET, "/locations", query)
        }
        LocationSubcommand::Get(args) => execute(
            client,
            context,
            Method::GET,
            &format!("/locations/{}", args.location_id),
            Vec::new(),
        ),
    }
}

fn validate_coordinate_pair(latitude: Option<f64>, longitude: Option<f64>, command: &str) -> Result<()> {
    if latitude.is_some() ^ longitude.is_some() {
        return Err(crate::Error::Arguments(format!(
            "{command} requires both --latitude and --longitude when either is provided"
        )));
    }

    Ok(())
}
