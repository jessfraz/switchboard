use clap::{ArgGroup, Args, Subcommand};
use serde_json::{json, Map, Value};

use crate::{
    commands::shared::{non_empty_country_codes, product_values, string_values, Product},
    Error, PlaidClient, PlaidCredentials, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct LinkCommand {
    #[command(subcommand)]
    pub(crate) command: LinkSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LinkSubcommand {
    #[command(name = "token-create")]
    TokenCreate(LinkTokenCreateArgs),
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("user_identity")
        .required(true)
        .args(["client_user_id", "user_id"])
))]
pub(crate) struct LinkTokenCreateArgs {
    #[arg(long)]
    client_user_id: Option<String>,

    #[arg(long)]
    user_id: Option<String>,

    #[arg(long = "product", value_enum)]
    products: Vec<Product>,

    #[arg(long = "country-code", value_name = "CODE")]
    country_codes: Vec<String>,

    #[arg(long, default_value = "en")]
    language: String,

    #[arg(long)]
    client_name: Option<String>,

    #[arg(long)]
    redirect_uri: Option<String>,

    #[arg(long)]
    webhook: Option<String>,

    #[arg(long)]
    institution_id: Option<String>,

    #[arg(long)]
    update_mode: bool,

    #[arg(long, value_name = "DAYS")]
    days_requested: Option<u32>,
}

pub(crate) fn run_link(command: LinkSubcommand, client: &PlaidClient, context: &ResolvedContext) -> Result<Value> {
    match command {
        LinkSubcommand::TokenCreate(args) => run_link_token_create(args, client, context),
    }
}

fn run_link_token_create(args: LinkTokenCreateArgs, client: &PlaidClient, context: &ResolvedContext) -> Result<Value> {
    if args.products.is_empty() && !args.update_mode {
        return Err(Error::Arguments(
            "link token create requires at least one --product unless --update-mode is set".into(),
        ));
    }
    if args.products.contains(&Product::Balance) {
        return Err(Error::Arguments(
            "link token create does not accept balance in --product; Plaid initializes balance automatically".into(),
        ));
    }
    if args.days_requested.is_some() && !args.products.contains(&Product::Transactions) {
        return Err(Error::Arguments(
            "--days-requested requires --product transactions".into(),
        ));
    }

    let credentials = credentials(context)?;
    let country_codes = non_empty_country_codes(args.country_codes);
    let mut body = Map::new();
    body.insert(
        "client_name".into(),
        Value::String(args.client_name.unwrap_or_else(|| context.client_name.clone())),
    );
    body.insert("language".into(), Value::String(args.language));
    body.insert("country_codes".into(), string_values(&country_codes));

    if let Some(user_id) = args.user_id {
        body.insert("user_id".into(), Value::String(user_id));
    } else if let Some(client_user_id) = args.client_user_id {
        body.insert("user".into(), json!({ "client_user_id": client_user_id }));
    }

    if !args.products.is_empty() {
        body.insert("products".into(), product_values(&args.products));
    }
    if args.update_mode {
        body.insert(
            "access_token".into(),
            Value::String(context.require_access_token()?.to_owned()),
        );
    }
    if let Some(redirect_uri) = args.redirect_uri {
        body.insert("redirect_uri".into(), Value::String(redirect_uri));
    }
    if let Some(webhook) = args.webhook {
        body.insert("webhook".into(), Value::String(webhook));
    }
    if let Some(institution_id) = args.institution_id {
        body.insert("institution_id".into(), Value::String(institution_id));
    }
    if let Some(days_requested) = args.days_requested {
        body.insert("transactions".into(), json!({ "days_requested": days_requested }));
    }

    client.post(credentials, "/link/token/create", Value::Object(body))
}

fn credentials(context: &ResolvedContext) -> Result<PlaidCredentials<'_>> {
    let (client_id, secret) = context.require_client_credentials()?;
    Ok(PlaidCredentials { client_id, secret })
}
