use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    commands::shared::{credentials, redact_secret, require_response_string, serialize_payload},
    Error, PlaidClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct AuthCommand {
    #[command(subcommand)]
    pub(crate) command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthSubcommand {
    Status,
    #[command(name = "exchange-public-token")]
    ExchangePublicToken(AuthExchangePublicTokenArgs),
    #[command(name = "import-access-token")]
    ImportAccessToken(AuthImportAccessTokenArgs),
    #[command(name = "invalidate-access-token")]
    InvalidateAccessToken(AuthInvalidateAccessTokenArgs),
    Logout,
}

#[derive(Debug, Args)]
pub(crate) struct AuthExchangePublicTokenArgs {
    #[arg(long)]
    public_token: String,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ExchangePublicTokenRequest {
    pub(crate) public_token: String,
}

#[derive(Debug, Args)]
pub(crate) struct AuthImportAccessTokenArgs {
    #[arg(long)]
    access_token: String,

    #[arg(long)]
    item_id: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AuthInvalidateAccessTokenArgs {
    #[arg(long)]
    no_store: bool,
}

pub(crate) fn run_auth(command: AuthSubcommand, client: &PlaidClient, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        AuthSubcommand::Status => auth_status(client, context),
        AuthSubcommand::ExchangePublicToken(args) => {
            let credentials = credentials(context)?;
            let response = client.post(
                credentials,
                "/item/public_token/exchange",
                serialize_payload(ExchangePublicTokenRequest {
                    public_token: args.public_token,
                })?,
            )?;

            if !args.no_store {
                let access_token = require_response_string(&response, "access_token")?;
                let item_id = require_response_string(&response, "item_id")?;
                context.store_access_token(access_token, Some(item_id))?;
            }

            Ok(response)
        }
        AuthSubcommand::ImportAccessToken(args) => {
            let item_id = args.item_id.clone();
            context.store_access_token(args.access_token.clone(), item_id.clone())?;
            Ok(json!({
                "status": "ok",
                "stored": true,
                "item_id": item_id,
                "access_token": redact_secret(&args.access_token),
            }))
        }
        AuthSubcommand::InvalidateAccessToken(args) => {
            let credentials = credentials(context)?;
            let response = client.post(
                credentials,
                "/item/access_token/invalidate",
                json!({
                    "access_token": context.require_access_token()?,
                }),
            )?;

            if !args.no_store {
                let access_token = require_response_string(&response, "new_access_token")?;
                context.store_access_token(access_token, context.item_id.clone())?;
            }

            Ok(response)
        }
        AuthSubcommand::Logout => {
            context.clear_auth_state()?;
            Ok(json!({
                "status": "logged_out",
                "environment": context.environment,
                "base_url": context.base_url,
                "plaid_version": context.plaid_version,
            }))
        }
    }
}

fn auth_status(client: &PlaidClient, context: &ResolvedContext) -> Result<Value> {
    if context.access_token.is_none() {
        return Ok(json!({
            "status": "ok",
            "authenticated": false,
            "reason": "no_stored_access_token",
            "environment": context.environment,
            "base_url": context.base_url,
            "plaid_version": context.plaid_version,
            "client_name": context.client_name,
            "has_client_id": context.client_id.is_some(),
            "has_secret": context.secret.is_some(),
            "item_id": context.item_id,
        }));
    }

    let credentials = credentials(context)?;
    let probe = match client.post(
        credentials,
        "/item/get",
        json!({
            "access_token": context.require_access_token()?,
        }),
    ) {
        Ok(body) => json!({
            "status_code": 200,
            "body": body,
        }),
        Err(Error::Api { status_code, body }) => json!({
            "status_code": status_code,
            "body": body,
        }),
        Err(error) => return Err(error),
    };

    let authenticated = probe
        .get("status_code")
        .and_then(Value::as_u64)
        .map(|status_code| status_code < 400)
        .unwrap_or(false);

    Ok(json!({
        "status": "ok",
        "authenticated": authenticated,
        "environment": context.environment,
        "base_url": context.base_url,
        "plaid_version": context.plaid_version,
        "client_name": context.client_name,
        "has_client_id": context.client_id.is_some(),
        "has_secret": context.secret.is_some(),
        "item_id": context.item_id,
        "probe": probe,
    }))
}
