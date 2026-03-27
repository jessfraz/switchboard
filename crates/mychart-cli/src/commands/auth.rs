use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{json, Value};

use crate::{
    api_client, build_authorize_url, dedupe_preserving_order, default_patient_scopes, ensure_code_verifier,
    ensure_json_success, expires_at_epoch_seconds, fetch_capability_summary, generate_nonce,
    parse_oauth_token_response, split_scopes,
    state::{ApiSessionState, ResolvedContext},
    Result,
};

#[derive(Debug, Args)]
pub(crate) struct AuthCommand {
    #[command(subcommand)]
    pub(crate) command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthSubcommand {
    #[command(name = "authorize-url")]
    AuthorizeUrl(AuthAuthorizeUrlArgs),
    #[command(name = "exchange-code")]
    ExchangeCode(AuthExchangeCodeArgs),
    Refresh(AuthRefreshArgs),
    Status,
    Logout,
}

#[derive(Debug, Args)]
pub(crate) struct AuthAuthorizeUrlArgs {
    #[arg(long, value_name = "URL")]
    redirect_uri: Option<String>,

    #[arg(long = "scope", value_name = "SCOPE")]
    scopes: Vec<String>,

    #[arg(long)]
    state: Option<String>,

    #[arg(long = "code-verifier", value_name = "VERIFIER")]
    code_verifier: Option<String>,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthExchangeCodeArgs {
    #[arg(long)]
    code: String,

    #[arg(long, value_name = "URL")]
    redirect_uri: Option<String>,

    #[arg(long = "code-verifier", value_name = "VERIFIER")]
    code_verifier: Option<String>,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthRefreshArgs {
    #[arg(long)]
    refresh_token: Option<String>,

    #[arg(long)]
    no_store: bool,
}

pub(crate) fn run_auth(command: AuthSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        AuthSubcommand::AuthorizeUrl(args) => {
            let base_url = context.require_api_base_url()?;
            let client_id = context.require_client_id()?;
            let redirect_uri = context.require_redirect_uri(args.redirect_uri)?;
            let client = api_client(&base_url)?;
            let capability = fetch_capability_summary(&client)?;
            let authorize_endpoint = capability.require_authorize_url()?;
            let token_endpoint = capability.require_token_url()?;
            let oauth_state = match args.state {
                Some(state) => state,
                None => generate_nonce(24)?,
            };
            let code_verifier = match args.code_verifier {
                Some(verifier) => ensure_code_verifier(verifier)?,
                None => ensure_code_verifier(generate_nonce(48)?)?,
            };
            let scopes = if args.scopes.is_empty() {
                default_patient_scopes()
            } else {
                dedupe_preserving_order(args.scopes)
            };
            let authorize_url = build_authorize_url(
                &authorize_endpoint,
                &client_id,
                &redirect_uri,
                &base_url,
                &oauth_state,
                &code_verifier,
                &scopes,
            )?;

            if !args.no_store {
                context.store_pending_oauth(
                    base_url.clone(),
                    client_id.clone(),
                    context.client_secret.clone(),
                    redirect_uri.clone(),
                    oauth_state.clone(),
                    code_verifier.clone(),
                )?;
            }

            Ok(json!({
                "status": "ok",
                "base_url": base_url,
                "client_id": client_id,
                "redirect_uri": redirect_uri,
                "authorize_url": authorize_url.as_str(),
                "authorize_endpoint": authorize_endpoint,
                "token_endpoint": token_endpoint,
                "state": oauth_state,
                "scopes": scopes,
                "code_challenge_method": "S256",
                "code_verifier": if args.no_store {
                    Value::String(code_verifier)
                } else {
                    Value::Null
                },
                "stored": !args.no_store,
            }))
        }
        AuthSubcommand::ExchangeCode(args) => {
            let base_url = context.require_api_base_url()?;
            let client_id = context.require_client_id()?;
            let redirect_uri = context.require_redirect_uri(args.redirect_uri)?;
            let code_verifier = context.require_code_verifier(args.code_verifier)?;
            let client_secret = context.client_secret.clone();
            let client = api_client(&base_url)?;
            let capability = fetch_capability_summary(&client)?;
            let token_endpoint = capability.require_token_url()?;
            let mut form = vec![
                ("grant_type".into(), "authorization_code".into()),
                ("code".into(), args.code),
                ("redirect_uri".into(), redirect_uri.clone()),
                ("client_id".into(), client_id.clone()),
                ("code_verifier".into(), code_verifier),
            ];
            if let Some(client_secret) = client_secret.clone() {
                form.push(("client_secret".into(), client_secret));
            }

            let response = client.exchange_oauth_token(&token_endpoint, &form)?;
            ensure_json_success(&response)?;
            let token = parse_oauth_token_response(&response.body)?;
            let refresh_token = token.refresh_token.clone().or_else(|| context.refresh_token.clone());
            let expires_at_epoch_seconds = token.expires_in.map(expires_at_epoch_seconds);

            if !args.no_store {
                context.store_api_tokens(ApiSessionState {
                    base_url: base_url.clone(),
                    client_id: client_id.clone(),
                    client_secret,
                    redirect_uri: redirect_uri.clone(),
                    access_token: token.access_token.clone(),
                    refresh_token: refresh_token.clone(),
                    token_type: token.token_type.clone(),
                    scope: token.scope.clone(),
                    patient_id: token.patient.clone(),
                    expires_at_epoch_seconds,
                })?;
            }

            Ok(json!({
                "status": "authenticated",
                "base_url": base_url,
                "client_id": client_id,
                "redirect_uri": redirect_uri,
                "token_endpoint": token_endpoint,
                "patient_id": token.patient,
                "scope": split_scopes(token.scope.as_deref()),
                "token_type": token.token_type,
                "expires_at_epoch_seconds": expires_at_epoch_seconds,
                "refresh_token_available": refresh_token.is_some(),
                "stored": !args.no_store,
            }))
        }
        AuthSubcommand::Refresh(args) => {
            let base_url = context.require_api_base_url()?;
            let client_id = context.require_client_id()?;
            let redirect_uri = context.require_redirect_uri(None)?;
            let refresh_token = context.require_refresh_token(args.refresh_token)?;
            let client_secret = context.client_secret.clone();
            let client = api_client(&base_url)?;
            let capability = fetch_capability_summary(&client)?;
            let token_endpoint = capability.require_token_url()?;
            let mut form = vec![
                ("grant_type".into(), "refresh_token".into()),
                ("refresh_token".into(), refresh_token.clone()),
                ("client_id".into(), client_id.clone()),
            ];
            if let Some(client_secret) = client_secret.clone() {
                form.push(("client_secret".into(), client_secret));
            }

            let response = client.exchange_oauth_token(&token_endpoint, &form)?;
            ensure_json_success(&response)?;
            let token = parse_oauth_token_response(&response.body)?;
            let next_refresh_token = token.refresh_token.clone().or(Some(refresh_token));
            let expires_at_epoch_seconds = token.expires_in.map(expires_at_epoch_seconds);

            if !args.no_store {
                context.store_api_tokens(ApiSessionState {
                    base_url: base_url.clone(),
                    client_id: client_id.clone(),
                    client_secret,
                    redirect_uri,
                    access_token: token.access_token.clone(),
                    refresh_token: next_refresh_token.clone(),
                    token_type: token.token_type.clone(),
                    scope: token.scope.clone().or_else(|| context.scope.clone()),
                    patient_id: token.patient.clone().or_else(|| context.patient_id.clone()),
                    expires_at_epoch_seconds,
                })?;
            }

            Ok(json!({
                "status": "refreshed",
                "base_url": base_url,
                "client_id": client_id,
                "token_endpoint": token_endpoint,
                "patient_id": token.patient.or_else(|| context.patient_id.clone()),
                "scope": split_scopes(token.scope.as_deref().or(context.scope.as_deref())),
                "token_type": token.token_type,
                "expires_at_epoch_seconds": expires_at_epoch_seconds,
                "refresh_token_available": next_refresh_token.is_some(),
                "stored": !args.no_store,
            }))
        }
        AuthSubcommand::Status => {
            if !context.api_authenticated() {
                return Ok(json!({
                    "status": "ok",
                    "authenticated": false,
                    "reason": "no_stored_token",
                    "base_url": context.api_base_url,
                    "client_id": context.client_id,
                    "refresh_token_available": context.refresh_token.is_some(),
                }));
            }

            let probe = if let (Some(base_url), Some(access_token)) =
                (context.api_base_url.clone(), context.access_token.clone())
            {
                let client = api_client(&base_url)?;
                let response = client.execute_bearer_json(
                    Method::GET,
                    "Patient",
                    &[("_count".into(), "1".into())],
                    &access_token,
                    None,
                )?;
                Some(json!({
                    "status_code": response.status_code,
                    "final_url": response.final_url.as_str(),
                    "body": response.body,
                }))
            } else {
                None
            };
            let authenticated = probe
                .as_ref()
                .and_then(|value| value.get("status_code"))
                .and_then(Value::as_u64)
                .map(|status| status < 400 && status != 401 && status != 403)
                .unwrap_or(false);

            Ok(json!({
                "status": "ok",
                "authenticated": authenticated,
                "base_url": context.api_base_url,
                "client_id": context.client_id,
                "patient_id": context.patient_id,
                "scope": split_scopes(context.scope.as_deref()),
                "expires_at_epoch_seconds": context.expires_at_epoch_seconds,
                "refresh_token_available": context.refresh_token.is_some(),
                "probe": probe,
            }))
        }
        AuthSubcommand::Logout => {
            context.clear_api_session()?;
            Ok(json!({
                "status": "logged_out",
                "base_url": context.api_base_url,
                "client_id": context.client_id,
            }))
        }
    }
}
