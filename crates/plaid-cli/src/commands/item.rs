use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    commands::shared::{credentials, ensure_item_id, serialize_payload, AccessTokenRequest},
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

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ItemRemoveOutput {
    pub(crate) removed: bool,
    pub(crate) request_id: String,
    pub(crate) item_id: String,
    pub(crate) local_cache_purged: ItemRemoveLocalCachePurged,
    pub(crate) local_state: ItemRemoveLocalState,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ItemRemoveLocalCachePurged {
    pub(crate) items_deleted: u64,
    pub(crate) accounts_deleted: u64,
    pub(crate) transactions_deleted: u64,
    pub(crate) cursors_deleted: u64,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ItemRemoveLocalState {
    pub(crate) access_token_cleared: bool,
    pub(crate) item_id_cleared: bool,
}

pub(crate) fn run_item(command: ItemSubcommand, client: &PlaidClient, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        ItemSubcommand::Get => {
            let credentials = credentials(context)?;
            let response = client.post(
                credentials,
                "/item/get",
                serialize_payload(AccessTokenRequest {
                    access_token: context.require_access_token()?.to_owned(),
                })?,
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
            let response = client.post(
                credentials(context)?,
                "/item/remove",
                serialize_payload(AccessTokenRequest {
                    access_token: access_token.clone(),
                })?,
            )?;
            let purged = context.cache.purge_item(&item_id)?;
            let forgotten = context.forget_removed_item(Some(&item_id), Some(&access_token))?;
            let removed = response.get("removed").and_then(Value::as_bool).unwrap_or(false);
            let request_id = response
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();

            serialize_payload(ItemRemoveOutput {
                removed,
                request_id,
                item_id,
                local_cache_purged: ItemRemoveLocalCachePurged {
                    items_deleted: purged.items_deleted,
                    accounts_deleted: purged.accounts_deleted,
                    transactions_deleted: purged.transactions_deleted,
                    cursors_deleted: purged.cursors_deleted,
                },
                local_state: ItemRemoveLocalState {
                    access_token_cleared: forgotten.access_token_cleared,
                    item_id_cleared: forgotten.item_id_cleared,
                },
            })
        }
    }
}
