use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    commands::shared::{credentials, non_empty_country_codes, product_names, serialize_payload, Product},
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

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct InstitutionsSearchRequest {
    pub(crate) query: String,
    pub(crate) country_codes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) products: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) options: Option<InstitutionRequestOptions>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct InstitutionsGetByIdRequest {
    pub(crate) institution_id: String,
    pub(crate) country_codes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) options: Option<InstitutionRequestOptions>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct InstitutionRequestOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) oauth: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) include_optional_metadata: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) include_status: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) include_auth_metadata: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) include_payment_initiation_metadata: Option<bool>,
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
    let options = institution_options(
        args.include_optional_metadata,
        false,
        args.include_auth_metadata,
        args.include_payment_initiation_metadata,
        args.oauth,
    );

    client.post(
        credentials,
        "/institutions/search",
        serialize_payload(InstitutionsSearchRequest {
            query: args.query,
            country_codes,
            products: (!args.products.is_empty()).then(|| product_names(&args.products)),
            options,
        })?,
    )
}

fn run_institutions_get_by_id(
    args: InstitutionsGetByIdArgs,
    client: &PlaidClient,
    context: &ResolvedContext,
) -> Result<Value> {
    let credentials = credentials(context)?;
    let country_codes = non_empty_country_codes(args.country_codes);
    let options = institution_options(
        args.include_optional_metadata,
        args.include_status,
        args.include_auth_metadata,
        args.include_payment_initiation_metadata,
        None,
    );

    client.post(
        credentials,
        "/institutions/get_by_id",
        serialize_payload(InstitutionsGetByIdRequest {
            institution_id: args.institution_id,
            country_codes,
            options,
        })?,
    )
}

fn institution_options(
    include_optional_metadata: bool,
    include_status: bool,
    include_auth_metadata: bool,
    include_payment_initiation_metadata: bool,
    oauth: Option<bool>,
) -> Option<InstitutionRequestOptions> {
    let options = InstitutionRequestOptions {
        oauth,
        include_optional_metadata: include_optional_metadata.then_some(true),
        include_status: include_status.then_some(true),
        include_auth_metadata: include_auth_metadata.then_some(true),
        include_payment_initiation_metadata: include_payment_initiation_metadata.then_some(true),
    };

    if options
        == (InstitutionRequestOptions {
            oauth: None,
            include_optional_metadata: None,
            include_status: None,
            include_auth_metadata: None,
            include_payment_initiation_metadata: None,
        })
    {
        None
    } else {
        Some(options)
    }
}
