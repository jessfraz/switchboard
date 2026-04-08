use clap::{Args, Subcommand};
use serde_json::Value;

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    commands::shared::resolve_account_id,
    ResolvedContext, Result,
};

#[derive(Clone, Debug, Args)]
pub(crate) struct AccountCommand {
    #[command(subcommand)]
    pub(crate) command: AccountSubcommand,
}

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum AccountSubcommand {
    Numbers,
    List(AccountListArgs),
    Get(AccountGetArgs),
}

#[derive(Clone, Debug, Args)]
pub(crate) struct AccountListArgs {
    #[arg(long)]
    positions: bool,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct AccountGetArgs {
    account: String,

    #[arg(long)]
    positions: bool,
}

pub(crate) fn run_accounts(command: AccountSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    let client = trader_client(context)?;

    match command {
        AccountSubcommand::Numbers => {
            let response = client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: "/accounts/accountNumbers".into(),
                query: Vec::new(),
                headers: context.trader_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })?;
            context.remember_account_numbers(&response)?;
            Ok(response)
        }
        AccountSubcommand::List(args) => client.execute(RequestSpec {
            method: reqwest::Method::GET,
            path: "/accounts".into(),
            query: account_fields_query(args.positions),
            headers: context.trader_headers(),
            body: RequestBody::None,
            auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
        }),
        AccountSubcommand::Get(args) => {
            let account_id = resolve_account_id(&client, context, &args.account)?;
            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: format!("/accounts/{account_id}"),
                query: account_fields_query(args.positions),
                headers: context.trader_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
    }
}

fn account_fields_query(positions: bool) -> Vec<(String, String)> {
    if positions {
        vec![("fields".into(), "positions".into())]
    } else {
        Vec::new()
    }
}

fn trader_client(context: &ResolvedContext) -> Result<SchwabClient> {
    SchwabClient::new(
        context.base_url.clone(),
        format!("schwab-cli/{}", env!("CARGO_PKG_VERSION")),
    )
}
