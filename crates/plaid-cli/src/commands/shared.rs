use clap::ValueEnum;
use serde_json::{json, Map, Value};

use crate::{Error, PlaidClient, PlaidCredentials, ResolvedContext, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Product {
    #[value(name = "assets")]
    Assets,
    #[value(name = "auth")]
    Auth,
    #[value(name = "balance")]
    Balance,
    #[value(name = "beacon")]
    Beacon,
    #[value(name = "employment")]
    Employment,
    #[value(name = "identity")]
    Identity,
    #[value(name = "income_verification")]
    IncomeVerification,
    #[value(name = "identity_verification")]
    IdentityVerification,
    #[value(name = "investments")]
    Investments,
    #[value(name = "investments_auth")]
    InvestmentsAuth,
    #[value(name = "liabilities")]
    Liabilities,
    #[value(name = "payment_initiation")]
    PaymentInitiation,
    #[value(name = "standing_orders")]
    StandingOrders,
    #[value(name = "signal")]
    Signal,
    #[value(name = "statements")]
    Statements,
    #[value(name = "transactions")]
    Transactions,
    #[value(name = "transfer")]
    Transfer,
    #[value(name = "cra_base_report")]
    CraBaseReport,
    #[value(name = "cra_income_insights")]
    CraIncomeInsights,
    #[value(name = "cra_cashflow_insights")]
    CraCashflowInsights,
    #[value(name = "cra_lend_score")]
    CraLendScore,
    #[value(name = "cra_partner_insights")]
    CraPartnerInsights,
    #[value(name = "cra_network_insights")]
    CraNetworkInsights,
    #[value(name = "cra_monitoring")]
    CraMonitoring,
    #[value(name = "layer")]
    Layer,
    #[value(name = "protect_linked_bank")]
    ProtectLinkedBank,
}

impl Product {
    pub(crate) fn as_api_value(self) -> &'static str {
        match self {
            Self::Assets => "assets",
            Self::Auth => "auth",
            Self::Balance => "balance",
            Self::Beacon => "beacon",
            Self::Employment => "employment",
            Self::Identity => "identity",
            Self::IncomeVerification => "income_verification",
            Self::IdentityVerification => "identity_verification",
            Self::Investments => "investments",
            Self::InvestmentsAuth => "investments_auth",
            Self::Liabilities => "liabilities",
            Self::PaymentInitiation => "payment_initiation",
            Self::StandingOrders => "standing_orders",
            Self::Signal => "signal",
            Self::Statements => "statements",
            Self::Transactions => "transactions",
            Self::Transfer => "transfer",
            Self::CraBaseReport => "cra_base_report",
            Self::CraIncomeInsights => "cra_income_insights",
            Self::CraCashflowInsights => "cra_cashflow_insights",
            Self::CraLendScore => "cra_lend_score",
            Self::CraPartnerInsights => "cra_partner_insights",
            Self::CraNetworkInsights => "cra_network_insights",
            Self::CraMonitoring => "cra_monitoring",
            Self::Layer => "layer",
            Self::ProtectLinkedBank => "protect_linked_bank",
        }
    }
}

pub(crate) fn product_values(products: &[Product]) -> Value {
    Value::Array(
        products
            .iter()
            .copied()
            .map(|product| Value::String(product.as_api_value().to_owned()))
            .collect(),
    )
}

pub(crate) fn string_values(values: &[String]) -> Value {
    Value::Array(values.iter().cloned().map(Value::String).collect())
}

pub(crate) fn non_empty_country_codes(country_codes: Vec<String>) -> Vec<String> {
    if country_codes.is_empty() {
        vec!["US".into()]
    } else {
        country_codes
    }
}

pub(crate) fn maybe_insert_options(body: &mut Map<String, Value>, options: Map<String, Value>) {
    if !options.is_empty() {
        body.insert("options".into(), Value::Object(options));
    }
}

pub(crate) fn require_response_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::Config(format!("Plaid response is missing required field {key:?}")))
}

pub(crate) fn credentials(context: &ResolvedContext) -> Result<PlaidCredentials<'_>> {
    let (client_id, secret) = context.require_client_credentials()?;
    Ok(PlaidCredentials { client_id, secret })
}

pub(crate) fn ensure_item_id(client: &PlaidClient, context: &mut ResolvedContext) -> Result<String> {
    if let Some(item_id) = context.item_id.clone() {
        return Ok(item_id);
    }

    let response = client.post(
        credentials(context)?,
        "/item/get",
        json!({
            "access_token": context.require_access_token()?,
        }),
    )?;
    let item = response
        .get("item")
        .ok_or_else(|| Error::Config("Plaid item lookup response was missing the item payload".into()))?;
    let item_id = context
        .cache
        .cache_item(item)?
        .ok_or_else(|| Error::Config("Plaid item lookup response was missing item_id".into()))?;
    context.remember_item_id(item_id.clone())?;

    Ok(item_id)
}

pub(crate) fn redact_secret(value: &str) -> Value {
    let visible = value.chars().rev().take(4).collect::<Vec<_>>();
    let suffix = visible.into_iter().rev().collect::<String>();
    json!(format!("***{}", suffix))
}
