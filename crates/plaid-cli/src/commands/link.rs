use clap::{ArgGroup, Args, Subcommand};
use serde_json::{json, Map, Value};

use crate::{
    commands::shared::{credentials, non_empty_country_codes, product_values, string_values, Product},
    Error, PlaidClient, ResolvedContext, Result,
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

    #[arg(long = "optional-product", value_enum)]
    optional_products: Vec<Product>,

    #[arg(long = "required-if-supported-product", value_enum)]
    required_if_supported_products: Vec<Product>,

    #[arg(long = "additional-consented-product", value_enum)]
    additional_consented_products: Vec<Product>,

    #[arg(long = "country-code", value_name = "CODE")]
    country_codes: Vec<String>,

    #[arg(long, default_value = "en")]
    language: String,

    #[arg(long)]
    client_name: Option<String>,

    #[arg(long)]
    link_customization_name: Option<String>,

    #[arg(long)]
    redirect_uri: Option<String>,

    #[arg(long)]
    webhook: Option<String>,

    #[arg(long)]
    android_package_name: Option<String>,

    #[arg(long)]
    institution_id: Option<String>,

    #[arg(long)]
    routing_number: Option<String>,

    #[arg(long)]
    update_mode: bool,

    #[arg(long, value_name = "DAYS")]
    days_requested: Option<u32>,

    #[arg(long = "depository-subtype")]
    depository_subtypes: Vec<String>,

    #[arg(long = "credit-subtype")]
    credit_subtypes: Vec<String>,

    #[arg(long = "loan-subtype")]
    loan_subtypes: Vec<String>,

    #[arg(long = "investment-subtype")]
    investment_subtypes: Vec<String>,

    #[arg(long = "other-subtype")]
    other_subtypes: Vec<String>,
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
    if args.routing_number.is_some() && args.institution_id.is_some() {
        return Err(Error::Arguments(
            "link token create accepts either --institution-id or --routing-number, not both".into(),
        ));
    }
    if !args.additional_consented_products.is_empty() && !args.update_mode {
        return Err(Error::Arguments(
            "--additional-consented-product requires --update-mode".into(),
        ));
    }
    validate_primary_products(&args.products)?;
    validate_balance_products("--optional-product", &args.optional_products)?;
    validate_balance_products("--required-if-supported-product", &args.required_if_supported_products)?;
    validate_balance_products("--additional-consented-product", &args.additional_consented_products)?;
    validate_no_product_overlap(
        "--product",
        &args.products,
        "--optional-product",
        &args.optional_products,
    )?;
    validate_no_product_overlap(
        "--product",
        &args.products,
        "--required-if-supported-product",
        &args.required_if_supported_products,
    )?;
    validate_no_product_overlap(
        "--product",
        &args.products,
        "--additional-consented-product",
        &args.additional_consented_products,
    )?;
    validate_no_product_overlap(
        "--optional-product",
        &args.optional_products,
        "--required-if-supported-product",
        &args.required_if_supported_products,
    )?;
    validate_no_product_overlap(
        "--optional-product",
        &args.optional_products,
        "--additional-consented-product",
        &args.additional_consented_products,
    )?;
    validate_no_product_overlap(
        "--required-if-supported-product",
        &args.required_if_supported_products,
        "--additional-consented-product",
        &args.additional_consented_products,
    )?;
    if args.days_requested.is_some()
        && !products_contain_transactions([
            args.products.as_slice(),
            args.optional_products.as_slice(),
            args.required_if_supported_products.as_slice(),
            args.additional_consented_products.as_slice(),
        ])
    {
        return Err(Error::Arguments(
            "--days-requested requires a transactions product selection".into(),
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
    if let Some(link_customization_name) = args.link_customization_name {
        body.insert("link_customization_name".into(), Value::String(link_customization_name));
    }
    if let Some(android_package_name) = args.android_package_name {
        body.insert("android_package_name".into(), Value::String(android_package_name));
    }

    if let Some(user_id) = args.user_id {
        body.insert("user_id".into(), Value::String(user_id));
    } else if let Some(client_user_id) = args.client_user_id {
        body.insert("user".into(), json!({ "client_user_id": client_user_id }));
    }

    if !args.products.is_empty() {
        body.insert("products".into(), product_values(&args.products));
    }
    if !args.optional_products.is_empty() {
        body.insert("optional_products".into(), product_values(&args.optional_products));
    }
    if !args.required_if_supported_products.is_empty() {
        body.insert(
            "required_if_supported_products".into(),
            product_values(&args.required_if_supported_products),
        );
    }
    if !args.additional_consented_products.is_empty() {
        body.insert(
            "additional_consented_products".into(),
            product_values(&args.additional_consented_products),
        );
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
    if let Some(routing_number) = args.routing_number {
        body.insert(
            "institution_data".into(),
            json!({
                "routing_number": routing_number,
            }),
        );
    }
    if let Some(days_requested) = args.days_requested {
        body.insert("transactions".into(), json!({ "days_requested": days_requested }));
    }
    let account_filters = account_filters(
        args.depository_subtypes,
        args.credit_subtypes,
        args.loan_subtypes,
        args.investment_subtypes,
        args.other_subtypes,
    );
    if !account_filters.is_empty() {
        body.insert("account_filters".into(), Value::Object(account_filters));
    }

    client.post(credentials, "/link/token/create", Value::Object(body))
}

fn validate_primary_products(products: &[Product]) -> Result<()> {
    validate_balance_products("--product", products)?;
    if products.contains(&Product::BalancePlus) {
        return Err(Error::Arguments(
            "link token create does not accept balance_plus in --product; use --additional-consented-product balance_plus instead".into(),
        ));
    }
    if products.contains(&Product::ProtectTransactions) {
        return Err(Error::Arguments(
            "link token create does not accept protect_transactions in --product".into(),
        ));
    }
    if products.contains(&Product::TransactionsRefresh) {
        return Err(Error::Arguments(
            "link token create does not accept transactions_refresh in --product".into(),
        ));
    }
    Ok(())
}

fn validate_balance_products(flag: &str, products: &[Product]) -> Result<()> {
    if products.contains(&Product::Balance) {
        return Err(Error::Arguments(format!(
            "link token create does not accept balance in {flag}; Plaid initializes balance automatically"
        )));
    }
    Ok(())
}

fn validate_no_product_overlap(left_flag: &str, left: &[Product], right_flag: &str, right: &[Product]) -> Result<()> {
    if let Some(product) = left.iter().find(|product| right.contains(product)) {
        return Err(Error::Arguments(format!(
            "link token create received duplicate product {} across {left_flag} and {right_flag}",
            product.as_api_value()
        )));
    }
    Ok(())
}

fn products_contain_transactions<'a>(groups: impl IntoIterator<Item = &'a [Product]>) -> bool {
    groups
        .into_iter()
        .flatten()
        .any(|product| *product == Product::Transactions)
}

fn account_filters(
    depository_subtypes: Vec<String>,
    credit_subtypes: Vec<String>,
    loan_subtypes: Vec<String>,
    investment_subtypes: Vec<String>,
    other_subtypes: Vec<String>,
) -> Map<String, Value> {
    let mut filters = Map::new();
    maybe_insert_account_filter(&mut filters, "depository", depository_subtypes);
    maybe_insert_account_filter(&mut filters, "credit", credit_subtypes);
    maybe_insert_account_filter(&mut filters, "loan", loan_subtypes);
    maybe_insert_account_filter(&mut filters, "investment", investment_subtypes);
    maybe_insert_account_filter(&mut filters, "other", other_subtypes);
    filters
}

fn maybe_insert_account_filter(filters: &mut Map<String, Value>, account_type: &str, subtypes: Vec<String>) {
    if subtypes.is_empty() {
        return;
    }

    filters.insert(
        account_type.into(),
        json!({
            "account_subtypes": subtypes,
        }),
    );
}
