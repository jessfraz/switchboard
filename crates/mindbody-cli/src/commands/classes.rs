use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::Value;

use crate::{
    commands::shared::{
        push_optional_query_bool, push_optional_query_string, push_optional_query_u64, push_ordering_query,
        push_query_csv_u64, push_window_query, OrderingArgs, WindowedQueryArgs,
    },
    execute, MindbodyClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct ClassCommand {
    #[command(subcommand)]
    pub(crate) command: ClassSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ClassSubcommand {
    List(ListClassesArgs),
    Get(GetClassArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ListClassesArgs {
    #[arg(long = "location-id")]
    location_id: u64,

    #[arg(long = "start-date-time")]
    start_date_time: Option<String>,

    #[arg(long = "end-date-time")]
    end_date_time: Option<String>,

    #[arg(long = "class-type-id")]
    class_type_id: Option<u64>,

    #[arg(long = "staff-last-name")]
    staff_last_name: Option<String>,

    #[arg(long = "available-for-booking", value_parser = clap::builder::BoolishValueParser::new())]
    available_for_booking: Option<bool>,

    #[arg(long = "service-category-id")]
    service_category_ids: Vec<u64>,

    #[command(flatten)]
    window: WindowedQueryArgs,

    #[command(flatten)]
    ordering: OrderingArgs,
}

#[derive(Debug, Args)]
pub(crate) struct GetClassArgs {
    #[arg(long = "location-id")]
    location_id: u64,

    #[arg(value_name = "CLASS_ID")]
    class_id: u64,
}

pub(crate) fn run_classes(
    command: ClassSubcommand,
    client: &MindbodyClient,
    context: &ResolvedContext,
) -> Result<Value> {
    match command {
        ClassSubcommand::List(args) => {
            let mut query = Vec::new();
            push_optional_query_string(&mut query, "startDateTime", args.start_date_time);
            push_optional_query_string(&mut query, "endDateTime", args.end_date_time);
            push_optional_query_u64(&mut query, "classTypeId", args.class_type_id);
            push_optional_query_string(&mut query, "staffLastName", args.staff_last_name);
            push_optional_query_bool(&mut query, "availableForBooking", args.available_for_booking);
            push_query_csv_u64(&mut query, "serviceCategoryIds", args.service_category_ids);
            push_window_query(&mut query, &args.window);
            push_ordering_query(&mut query, &args.ordering);
            execute(
                client,
                context,
                Method::GET,
                &format!("/locations/{}/classes", args.location_id),
                query,
            )
        }
        ClassSubcommand::Get(args) => execute(
            client,
            context,
            Method::GET,
            &format!("/locations/{}/classes/{}", args.location_id, args.class_id),
            Vec::new(),
        ),
    }
}
