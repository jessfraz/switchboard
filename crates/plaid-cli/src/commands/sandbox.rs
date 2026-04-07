use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    commands::shared::{credentials, product_names, serialize_payload, Product},
    Error, PlaidClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct SandboxCommand {
    #[command(subcommand)]
    pub(crate) command: SandboxSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SandboxSubcommand {
    #[command(name = "public-token-create")]
    PublicTokenCreate(SandboxPublicTokenCreateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SandboxPublicTokenCreateArgs {
    #[arg(long)]
    institution_id: String,

    #[arg(long = "product", value_enum, required = true)]
    initial_products: Vec<Product>,

    #[arg(long)]
    webhook: Option<String>,

    #[arg(long)]
    username: Option<String>,

    #[arg(long)]
    password: Option<String>,

    #[arg(long)]
    start_date: Option<String>,

    #[arg(long)]
    end_date: Option<String>,

    #[arg(long)]
    days_requested: Option<u32>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SandboxPublicTokenCreateRequest {
    pub(crate) institution_id: String,
    pub(crate) initial_products: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) options: Option<SandboxPublicTokenCreateOptions>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SandboxPublicTokenCreateOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) webhook: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "override_username")]
    pub(crate) username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "override_password")]
    pub(crate) password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) transactions: Option<SandboxTransactionsOptions>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SandboxTransactionsOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) start_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) end_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) days_requested: Option<u32>,
}

pub(crate) fn run_sandbox(
    command: SandboxSubcommand,
    client: &PlaidClient,
    context: &ResolvedContext,
) -> Result<Value> {
    match command {
        SandboxSubcommand::PublicTokenCreate(args) => {
            let uses_transactions_options = uses_transactions_options(&args);
            if uses_transactions_options && !args.initial_products.contains(&Product::Transactions) {
                return Err(Error::Arguments(
                    "transaction sandbox options require --product transactions".into(),
                ));
            }

            let credentials = credentials(context)?;
            let options = if args.webhook.is_some()
                || args.username.is_some()
                || args.password.is_some()
                || uses_transactions_options
            {
                Some(SandboxPublicTokenCreateOptions {
                    webhook: args.webhook,
                    username: args.username,
                    password: args.password,
                    transactions: uses_transactions_options.then_some(SandboxTransactionsOptions {
                        start_date: args.start_date,
                        end_date: args.end_date,
                        days_requested: args.days_requested,
                    }),
                })
            } else {
                None
            };

            client.post(
                credentials,
                "/sandbox/public_token/create",
                serialize_payload(SandboxPublicTokenCreateRequest {
                    institution_id: args.institution_id,
                    initial_products: product_names(&args.initial_products),
                    options,
                })?,
            )
        }
    }
}

fn uses_transactions_options(args: &SandboxPublicTokenCreateArgs) -> bool {
    args.start_date.is_some() || args.end_date.is_some() || args.days_requested.is_some()
}
