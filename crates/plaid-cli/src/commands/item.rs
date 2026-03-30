use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{PlaidClient, PlaidCredentials, ResolvedContext, Result};

#[derive(Debug, Args)]
pub(crate) struct ItemCommand {
    #[command(subcommand)]
    pub(crate) command: ItemSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ItemSubcommand {
    Get,
}

pub(crate) fn run_item(command: ItemSubcommand, client: &PlaidClient, context: &ResolvedContext) -> Result<Value> {
    match command {
        ItemSubcommand::Get => {
            let credentials = credentials(context)?;
            client.post(
                credentials,
                "/item/get",
                json!({
                    "access_token": context.require_access_token()?,
                }),
            )
        }
    }
}

fn credentials(context: &ResolvedContext) -> Result<PlaidCredentials<'_>> {
    let (client_id, secret) = context.require_client_credentials()?;
    Ok(PlaidCredentials { client_id, secret })
}
