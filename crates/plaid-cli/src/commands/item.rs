use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{
    commands::shared::{credentials, ensure_item_id},
    PlaidClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct ItemCommand {
    #[command(subcommand)]
    pub(crate) command: ItemSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ItemSubcommand {
    Get,
    Remove,
}

pub(crate) fn run_item(command: ItemSubcommand, client: &PlaidClient, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        ItemSubcommand::Get => {
            let credentials = credentials(context)?;
            let response = client.post(
                credentials,
                "/item/get",
                json!({
                    "access_token": context.require_access_token()?,
                }),
            )?;

            if let Some(item) = response.get("item") {
                if let Some(item_id) = context.cache.cache_item(item)? {
                    context.remember_item_id(item_id)?;
                }
            }

            Ok(response)
        }
        ItemSubcommand::Remove => {
            let item_id = ensure_item_id(client, context)?;
            let access_token = context.require_access_token()?.to_owned();
            let mut response = client.post(
                credentials(context)?,
                "/item/remove",
                json!({
                    "access_token": access_token.clone(),
                }),
            )?;
            let purged = context.cache.purge_item(&item_id)?;
            let forgotten = context.forget_removed_item(Some(&item_id), Some(&access_token))?;

            if let Some(object) = response.as_object_mut() {
                object.insert("item_id".into(), Value::String(item_id));
                object.insert(
                    "local_cache_purged".into(),
                    json!({
                        "items_deleted": purged.items_deleted,
                        "accounts_deleted": purged.accounts_deleted,
                        "transactions_deleted": purged.transactions_deleted,
                        "cursors_deleted": purged.cursors_deleted,
                    }),
                );
                object.insert(
                    "local_state".into(),
                    json!({
                        "access_token_cleared": forgotten.access_token_cleared,
                        "item_id_cleared": forgotten.item_id_cleared,
                    }),
                );
            }

            Ok(response)
        }
    }
}
