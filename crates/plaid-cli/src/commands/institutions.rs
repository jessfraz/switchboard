use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use crate::{
    commands::shared::{credentials, non_empty_country_codes, product_values, string_values, Product},
    PlaidClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct InstitutionsCommand {
    #[command(subcommand)]
    pub(crate) command: InstitutionsSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum InstitutionsSubcommand {
    Search(InstitutionsSearchArgs),
    #[command(name = "get-by-id")]
    GetById(InstitutionsGetByIdArgs),
}

#[derive(Debug, Args)]
pub(crate) struct InstitutionsSearchArgs {
    query: String,

    #[arg(long = "product", value_enum)]
    products: Vec<Product>,

    #[arg(long = "country-code", value_name = "CODE")]
    country_codes: Vec<String>,

    #[arg(long)]
    oauth: Option<bool>,

    #[arg(long)]
    include_optional_metadata: bool,

    #[arg(long)]
    include_auth_metadata: bool,

    #[arg(long)]
    include_payment_initiation_metadata: bool,
}

#[derive(Debug, Args)]
pub(crate) struct InstitutionsGetByIdArgs {
    institution_id: String,

    #[arg(long = "country-code", value_name = "CODE")]
    country_codes: Vec<String>,

    #[arg(long)]
    include_optional_metadata: bool,

    #[arg(long)]
    include_status: bool,

    #[arg(long)]
    include_auth_metadata: bool,

    #[arg(long)]
    include_payment_initiation_metadata: bool,
}

pub(crate) fn run_institutions(
    command: InstitutionsSubcommand,
    client: &PlaidClient,
    context: &ResolvedContext,
) -> Result<Value> {
    match command {
        InstitutionsSubcommand::Search(args) => run_institutions_search(args, client, context),
        InstitutionsSubcommand::GetById(args) => run_institutions_get_by_id(args, client, context),
    }
}

fn run_institutions_search(
    args: InstitutionsSearchArgs,
    client: &PlaidClient,
    context: &ResolvedContext,
) -> Result<Value> {
    let credentials = credentials(context)?;
    let country_codes = non_empty_country_codes(args.country_codes);
    let mut body = Map::new();
    body.insert("query".into(), Value::String(args.query));
    body.insert("country_codes".into(), string_values(&country_codes));

    if !args.products.is_empty() {
        body.insert("products".into(), product_values(&args.products));
    }

    let mut options = institution_options(
        args.include_optional_metadata,
        false,
        args.include_auth_metadata,
        args.include_payment_initiation_metadata,
    );
    if let Some(oauth) = args.oauth {
        options.insert("oauth".into(), Value::Bool(oauth));
    }
    if !options.is_empty() {
        body.insert("options".into(), Value::Object(options));
    }

    client.post(credentials, "/institutions/search", Value::Object(body))
}

fn run_institutions_get_by_id(
    args: InstitutionsGetByIdArgs,
    client: &PlaidClient,
    context: &ResolvedContext,
) -> Result<Value> {
    let credentials = credentials(context)?;
    let country_codes = non_empty_country_codes(args.country_codes);
    let mut body = Map::new();
    body.insert("institution_id".into(), Value::String(args.institution_id));
    body.insert("country_codes".into(), string_values(&country_codes));

    let options = institution_options(
        args.include_optional_metadata,
        args.include_status,
        args.include_auth_metadata,
        args.include_payment_initiation_metadata,
    );
    if !options.is_empty() {
        body.insert("options".into(), Value::Object(options));
    }

    client.post(credentials, "/institutions/get_by_id", Value::Object(body))
}

fn institution_options(
    include_optional_metadata: bool,
    include_status: bool,
    include_auth_metadata: bool,
    include_payment_initiation_metadata: bool,
) -> Map<String, Value> {
    let mut options = Map::new();
    if include_optional_metadata {
        options.insert("include_optional_metadata".into(), Value::Bool(true));
    }
    if include_status {
        options.insert("include_status".into(), Value::Bool(true));
    }
    if include_auth_metadata {
        options.insert("include_auth_metadata".into(), Value::Bool(true));
    }
    if include_payment_initiation_metadata {
        options.insert("include_payment_initiation_metadata".into(), Value::Bool(true));
    }
    options
}
