use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use crate::{
    commands::shared::{credentials, product_values, Product},
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
            let mut body = Map::new();
            body.insert("institution_id".into(), Value::String(args.institution_id.clone()));
            body.insert("initial_products".into(), product_values(&args.initial_products));

            let mut options = Map::new();
            if let Some(webhook) = args.webhook {
                options.insert("webhook".into(), Value::String(webhook));
            }
            if let Some(username) = args.username {
                options.insert("override_username".into(), Value::String(username));
            }
            if let Some(password) = args.password {
                options.insert("override_password".into(), Value::String(password));
            }
            if uses_transactions_options {
                let mut transactions = Map::new();
                if let Some(start_date) = args.start_date {
                    transactions.insert("start_date".into(), Value::String(start_date));
                }
                if let Some(end_date) = args.end_date {
                    transactions.insert("end_date".into(), Value::String(end_date));
                }
                if let Some(days_requested) = args.days_requested {
                    transactions.insert("days_requested".into(), Value::Number(days_requested.into()));
                }
                options.insert("transactions".into(), Value::Object(transactions));
            }
            if !options.is_empty() {
                body.insert("options".into(), Value::Object(options));
            }

            client.post(credentials, "/sandbox/public_token/create", Value::Object(body))
        }
    }
}

fn uses_transactions_options(args: &SandboxPublicTokenCreateArgs) -> bool {
    args.start_date.is_some() || args.end_date.is_some() || args.days_requested.is_some()
}
