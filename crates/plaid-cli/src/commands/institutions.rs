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
}

pub(crate) fn run_institutions(
    command: InstitutionsSubcommand,
    client: &PlaidClient,
    context: &ResolvedContext,
) -> Result<Value> {
    match command {
        InstitutionsSubcommand::Search(args) => {
            let credentials = credentials(context)?;
            let country_codes = non_empty_country_codes(args.country_codes);
            let mut body = Map::new();
            body.insert("query".into(), Value::String(args.query));
            body.insert("country_codes".into(), string_values(&country_codes));

            if !args.products.is_empty() {
                body.insert("products".into(), product_values(&args.products));
            }

            let mut options = Map::new();
            if let Some(oauth) = args.oauth {
                options.insert("oauth".into(), Value::Bool(oauth));
            }
            if args.include_optional_metadata {
                options.insert("include_optional_metadata".into(), Value::Bool(true));
            }
            if args.include_auth_metadata {
                options.insert("include_auth_metadata".into(), Value::Bool(true));
            }
            if !options.is_empty() {
                body.insert("options".into(), Value::Object(options));
            }

            client.post(credentials, "/institutions/search", Value::Object(body))
        }
    }
}
