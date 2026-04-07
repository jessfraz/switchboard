use clap::{ArgGroup, Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    commands::shared::{credentials, non_empty_country_codes, product_names, serialize_payload, Product},
    Error, PlaidClient, ResolvedContext, Result, PLAID_GITHUB_PAGES_REDIRECT_URI,
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

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LinkTokenCreateRequest {
    pub(crate) client_name: String,
    pub(crate) language: String,
    pub(crate) country_codes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) link_customization_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) redirect_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) webhook: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) android_package_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) institution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user: Option<LinkTokenUser>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) products: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) optional_products: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) required_if_supported_products: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) additional_consented_products: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) institution_data: Option<LinkTokenInstitutionData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) transactions: Option<LinkTokenTransactions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) account_filters: Option<LinkTokenAccountFilters>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LinkTokenUser {
    pub(crate) client_user_id: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LinkTokenInstitutionData {
    pub(crate) routing_number: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LinkTokenTransactions {
    pub(crate) days_requested: u32,
}

#[derive(Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LinkTokenAccountFilters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) depository: Option<LinkTokenAccountSubtypeFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) credit: Option<LinkTokenAccountSubtypeFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) loan: Option<LinkTokenAccountSubtypeFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) investment: Option<LinkTokenAccountSubtypeFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) other: Option<LinkTokenAccountSubtypeFilter>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LinkTokenAccountSubtypeFilter {
    pub(crate) account_subtypes: Vec<String>,
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
    let LinkTokenCreateArgs {
        client_user_id,
        user_id,
        products,
        optional_products,
        required_if_supported_products,
        additional_consented_products,
        country_codes,
        language,
        client_name,
        link_customization_name,
        redirect_uri,
        webhook,
        android_package_name,
        institution_id,
        routing_number,
        update_mode,
        days_requested,
        depository_subtypes,
        credit_subtypes,
        loan_subtypes,
        investment_subtypes,
        other_subtypes,
    } = args;

    client.post(
        credentials,
        "/link/token/create",
        serialize_payload(LinkTokenCreateRequest {
            client_name: client_name.unwrap_or_else(|| context.client_name.clone()),
            language,
            country_codes: non_empty_country_codes(country_codes),
            link_customization_name,
            redirect_uri: Some(redirect_uri.unwrap_or_else(|| PLAID_GITHUB_PAGES_REDIRECT_URI.to_owned())),
            webhook,
            android_package_name,
            institution_id,
            access_token: if update_mode {
                Some(context.require_access_token()?.to_owned())
            } else {
                None
            },
            user: client_user_id.map(|client_user_id| LinkTokenUser { client_user_id }),
            user_id,
            products: (!products.is_empty()).then(|| product_names(&products)),
            optional_products: (!optional_products.is_empty()).then(|| product_names(&optional_products)),
            required_if_supported_products: (!required_if_supported_products.is_empty())
                .then(|| product_names(&required_if_supported_products)),
            additional_consented_products: (!additional_consented_products.is_empty())
                .then(|| product_names(&additional_consented_products)),
            institution_data: routing_number.map(|routing_number| LinkTokenInstitutionData { routing_number }),
            transactions: days_requested.map(|days_requested| LinkTokenTransactions { days_requested }),
            account_filters: account_filters(
                depository_subtypes,
                credit_subtypes,
                loan_subtypes,
                investment_subtypes,
                other_subtypes,
            ),
        })?,
    )
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
) -> Option<LinkTokenAccountFilters> {
    let filters = LinkTokenAccountFilters {
        depository: subtype_filter(depository_subtypes),
        credit: subtype_filter(credit_subtypes),
        loan: subtype_filter(loan_subtypes),
        investment: subtype_filter(investment_subtypes),
        other: subtype_filter(other_subtypes),
    };

    if filters == LinkTokenAccountFilters::default() {
        None
    } else {
        Some(filters)
    }
}

fn subtype_filter(subtypes: Vec<String>) -> Option<LinkTokenAccountSubtypeFilter> {
    (!subtypes.is_empty()).then_some(LinkTokenAccountSubtypeFilter {
        account_subtypes: subtypes,
    })
}
