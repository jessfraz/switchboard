use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{state::validate_unique_user_id, ResolvedContext, Result};

#[derive(Debug, Args)]
pub(crate) struct AccountCommand {
    #[command(subcommand)]
    pub(crate) command: AccountSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AccountSubcommand {
    Status,
}

pub(crate) fn run_account(command: AccountSubcommand, context: &ResolvedContext) -> Result<Value> {
    match command {
        AccountSubcommand::Status => {
            if let Some(user_id) = context.user_id.as_deref() {
                validate_unique_user_id(user_id)?;
            }

            Ok(json!({
                "status": "ok",
                "provider": "mindbody",
                "base_url": context.base_url,
                "app_name": context.app_name,
                "user_id": context.user_id,
                "has_api_key": context.api_key.is_some(),
                "has_client_key": context.client_key.is_some(),
                "has_client_secret": context.client_secret.is_some(),
            }))
        }
    }
}
