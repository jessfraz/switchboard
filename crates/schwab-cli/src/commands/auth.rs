use clap::{Args, Subcommand};
use reqwest::Url;
use serde_json::{json, Value};

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    Error, ResolvedContext, Result,
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
    #[command(name = "exchange-url")]
    ExchangeUrl(AuthExchangeUrlArgs),
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
}

#[derive(Debug, Args)]
pub(crate) struct AuthExchangeCodeArgs {
    #[arg(long)]
    code: String,

    #[arg(long, value_name = "URL")]
    redirect_uri: Option<String>,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthExchangeUrlArgs {
    callback_input: String,

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
    let client = trader_client(context)?;

    match command {
        AuthSubcommand::AuthorizeUrl(args) => {
            let client_id = context.require_client_id()?;
            let redirect_uri = context.require_redirect_uri(args.redirect_uri)?;
            let scopes = resolve_scopes(args.scopes);
            let mut url = Url::parse(&context.authorize_url).map_err(|error| {
                Error::Config(format!(
                    "invalid Schwab authorize URL {:?}: {error}",
                    context.authorize_url
                ))
            })?;
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair("response_type", "code");
                pairs.append_pair("client_id", client_id);
                pairs.append_pair("redirect_uri", &redirect_uri);
                pairs.append_pair("scope", &scopes.join(" "));
                if let Some(state) = args.state.as_ref() {
                    pairs.append_pair("state", state);
                }
            }

            Ok(json!({
                "authorize_url": url.as_str(),
                "redirect_uri": redirect_uri,
                "scope": scopes,
            }))
        }
        AuthSubcommand::ExchangeCode(args) => exchange_code(
            &client,
            context,
            args.code,
            context.require_redirect_uri(args.redirect_uri)?,
            args.no_store,
        ),
        AuthSubcommand::ExchangeUrl(args) => {
            let callback = parse_callback_input(&args.callback_input)?;
            exchange_code(&client, context, callback.code, callback.redirect_uri, args.no_store)
        }
        AuthSubcommand::Refresh(args) => {
            let (client_id, client_secret) = context.require_client_credentials()?;
            let refresh_token = args
                .refresh_token
                .or_else(|| context.refresh_token.clone())
                .ok_or_else(|| Error::Config("missing refresh token, pass --refresh-token or login first".into()))?;

            let response = client.execute(RequestSpec {
                method: reqwest::Method::POST,
                path: context.token_url.clone(),
                query: Vec::new(),
                body: RequestBody::Form(vec![
                    ("grant_type".into(), "refresh_token".into()),
                    ("refresh_token".into(), refresh_token),
                ]),
                auth: AuthMode::Basic {
                    username: client_id.to_owned(),
                    password: client_secret.to_owned(),
                },
            })?;

            if !args.no_store {
                context.store_oauth_token_response(&response)?;
            }

            Ok(response)
        }
        AuthSubcommand::Status => auth_status(&client, context),
        AuthSubcommand::Logout => {
            context.clear_auth_state()?;
            Ok(json!({
                "status": "logged_out",
                "base_url": context.base_url,
                "market_data_base_url": context.market_data_base_url,
            }))
        }
    }
}

fn exchange_code(
    client: &SchwabClient,
    context: &mut ResolvedContext,
    code: String,
    redirect_uri: String,
    no_store: bool,
) -> Result<Value> {
    let (client_id, client_secret) = context.require_client_credentials()?;
    let response = client.execute(RequestSpec {
        method: reqwest::Method::POST,
        path: context.token_url.clone(),
        query: Vec::new(),
        body: RequestBody::Form(vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code),
            ("redirect_uri".into(), redirect_uri.clone()),
        ]),
        auth: AuthMode::Basic {
            username: client_id.to_owned(),
            password: client_secret.to_owned(),
        },
    })?;

    if !no_store {
        context.remember_redirect_uri(redirect_uri)?;
        context.store_oauth_token_response(&response)?;
    }

    Ok(response)
}

fn auth_status(client: &SchwabClient, context: &ResolvedContext) -> Result<Value> {
    if context.access_token.is_none() {
        return Ok(json!({
            "status": "ok",
            "authenticated": false,
            "reason": "no_stored_token",
            "base_url": context.base_url,
            "market_data_base_url": context.market_data_base_url,
            "authorize_url": context.authorize_url,
            "token_url": context.token_url,
            "redirect_uri": context.redirect_uri,
            "has_client_id": context.client_id.is_some(),
            "has_client_secret": context.client_secret.is_some(),
            "refresh_token_available": context.refresh_token.is_some(),
            "scope": split_scope(context.scope.as_deref()),
            "expires_at_epoch_seconds": context.expires_at_epoch_seconds,
            "cached_account_numbers": context.account_number_cache().len(),
        }));
    }

    let probe = match client.execute(RequestSpec {
        method: reqwest::Method::GET,
        path: "/userPreference".into(),
        query: Vec::new(),
        body: RequestBody::None,
        auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
    }) {
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
        "base_url": context.base_url,
        "market_data_base_url": context.market_data_base_url,
        "authorize_url": context.authorize_url,
        "token_url": context.token_url,
        "redirect_uri": context.redirect_uri,
        "has_client_id": context.client_id.is_some(),
        "has_client_secret": context.client_secret.is_some(),
        "refresh_token_available": context.refresh_token.is_some(),
        "scope": split_scope(context.scope.as_deref()),
        "expires_at_epoch_seconds": context.expires_at_epoch_seconds,
        "cached_account_numbers": context.account_number_cache().len(),
        "probe": probe,
    }))
}

fn trader_client(context: &ResolvedContext) -> Result<SchwabClient> {
    SchwabClient::new(
        context.base_url.clone(),
        format!("schwab-cli/{}", env!("CARGO_PKG_VERSION")),
    )
}

fn resolve_scopes(scopes: Vec<String>) -> Vec<String> {
    if scopes.is_empty() {
        vec!["readonly".into()]
    } else {
        scopes
    }
}

fn split_scope(scope: Option<&str>) -> Vec<String> {
    scope
        .unwrap_or_default()
        .split_whitespace()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

struct CallbackInput {
    code: String,
    redirect_uri: String,
}

fn parse_callback_input(input: &str) -> Result<CallbackInput> {
    let url =
        Url::parse(input).map_err(|error| Error::Arguments(format!("callback input must be a valid URL: {error}")))?;
    let code = url
        .query_pairs()
        .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
        .ok_or_else(|| Error::Arguments("callback URL is missing the code query parameter".into()))?;

    let mut redirect_uri = url.clone();
    redirect_uri.set_query(None);
    redirect_uri.set_fragment(None);

    Ok(CallbackInput {
        code,
        redirect_uri: redirect_uri.to_string(),
    })
}
