mod callback;
mod dynamic_client;
mod flow;

use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use reqwest::{Method, Url};
use serde_json::{json, Value};

use self::flow::{
    exchange_code, exchange_code_token, exchange_url, refresh_with_dynamic_client, refresh_with_refresh_token,
    run_authorize_url, run_login,
};
pub(crate) use self::{
    callback::redirect_uri_uses_loopback,
    flow::{complete_or_wait_for_hosted_authorization, ensure_api_session},
};
use crate::{api_client, split_scopes, state::ResolvedContext, Error, Result};

#[derive(Debug, Args)]
pub(crate) struct AuthCommand {
    #[command(subcommand)]
    pub(crate) command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthSubcommand {
    Login(AuthLoginArgs),
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

#[derive(Debug, Args, Clone)]
pub(crate) struct AuthAuthorizeOptions {
    #[arg(long, value_name = "URL")]
    pub(crate) redirect_uri: Option<String>,

    #[arg(long = "scope", value_name = "SCOPE")]
    pub(crate) scopes: Vec<String>,

    #[arg(long)]
    pub(crate) state: Option<String>,

    #[arg(long = "code-verifier", value_name = "VERIFIER")]
    pub(crate) code_verifier: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AuthAuthorizeUrlArgs {
    #[command(flatten)]
    pub(crate) options: AuthAuthorizeOptions,

    #[arg(long)]
    pub(crate) no_store: bool,

    #[arg(long)]
    pub(crate) no_open: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthLoginArgs {
    #[command(flatten)]
    pub(crate) options: AuthAuthorizeOptions,

    #[arg(long, default_value_t = 300)]
    pub(crate) timeout_seconds: u64,

    #[arg(long)]
    pub(crate) no_open: bool,

    #[arg(long)]
    pub(crate) dynamic_client: bool,
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
pub(crate) struct AuthExchangeUrlArgs {
    pub(crate) callback_input: String,

    #[arg(long)]
    pub(crate) no_store: bool,
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
        AuthSubcommand::Login(args) => run_login(args, context),
        AuthSubcommand::AuthorizeUrl(args) => run_authorize_url(args, context),
        AuthSubcommand::ExchangeCode(args) => {
            exchange_code(context, args.code, args.redirect_uri, args.code_verifier, args.no_store)
        }
        AuthSubcommand::ExchangeUrl(args) => exchange_url(args, context),
        AuthSubcommand::Refresh(args) => {
            if args.refresh_token.is_none() && context.dynamic_client().is_some() {
                return refresh_with_dynamic_client(context, args.no_store);
            }

            refresh_with_refresh_token(context, args)
        }
        AuthSubcommand::Status => {
            if !context.api_authenticated() {
                return Ok(json!({
                    "status": "ok",
                    "authenticated": false,
                    "reason": "no_stored_token",
                    "base_url": context.api_base_url,
                    "client_id": context.client_id,
                    "dynamic_client_id": context.dynamic_client().map(|dynamic_client| dynamic_client.client_id.clone()),
                    "dynamic_client_registered": context.dynamic_client().is_some(),
                    "renewal_method": renewal_method(context),
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
                "dynamic_client_id": context.dynamic_client().map(|dynamic_client| dynamic_client.client_id.clone()),
                "dynamic_client_registered": context.dynamic_client().is_some(),
                "renewal_method": renewal_method(context),
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

pub(crate) fn run_login_command(args: AuthLoginArgs, context: &mut ResolvedContext) -> Result<Value> {
    run_login(args, context)
}

pub(crate) fn run_authorize_url_command(args: AuthAuthorizeUrlArgs, context: &mut ResolvedContext) -> Result<Value> {
    run_authorize_url(args, context)
}

pub(crate) fn run_exchange_url_command(args: AuthExchangeUrlArgs, context: &mut ResolvedContext) -> Result<Value> {
    exchange_url(args, context)
}

pub(crate) enum ApiSessionBootstrap {
    Ready,
    Pending(Value),
}

pub(crate) enum HostedAuthorizationOutcome {
    Completed(Value),
    Pending(Value),
}

const ACCESS_TOKEN_REFRESH_SKEW_SECONDS: u64 = 60;
const AUTO_LOGIN_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug)]
struct PreparedAuthorization {
    base_url: String,
    client_id: String,
    redirect_uri: String,
    authorize_endpoint: String,
    token_endpoint: String,
    authorize_url: Url,
    oauth_state: String,
    code_verifier: String,
    scopes: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
enum TokenExchangeAuth {
    StoredClientStrategy,
    ForcePublic,
}

#[derive(Debug)]
struct TokenExchangeResult {
    base_url: String,
    client_id: String,
    client_secret: Option<String>,
    redirect_uri: String,
    token_endpoint: String,
    token: crate::OAuthTokenResponse,
    expires_at_epoch_seconds: Option<u64>,
}

fn renewal_method(context: &ResolvedContext) -> &'static str {
    if context.dynamic_client().is_some() {
        "dynamic_client_jwt_bearer"
    } else if context.refresh_token.is_some() {
        "refresh_token"
    } else {
        "none"
    }
}

fn access_token_is_fresh(context: &ResolvedContext) -> bool {
    if context.access_token.is_none() {
        return false;
    }

    let Some(expires_at_epoch_seconds) = context.expires_at_epoch_seconds else {
        return true;
    };

    current_epoch_seconds().saturating_add(ACCESS_TOKEN_REFRESH_SKEW_SECONDS) < expires_at_epoch_seconds
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn can_fallback_to_interactive_auth(error: &Error) -> bool {
    matches!(
        error,
        Error::Api {
            status_code: 400 | 401 | 403,
            ..
        } | Error::Auth { .. }
    )
}

fn token_exchange_auth_label(token_auth: TokenExchangeAuth, client_secret_present: bool) -> &'static str {
    match token_auth {
        TokenExchangeAuth::StoredClientStrategy if client_secret_present => "basic",
        TokenExchangeAuth::StoredClientStrategy => "public_pkce",
        TokenExchangeAuth::ForcePublic => "public_pkce",
    }
}

fn auth_debug(context: &ResolvedContext, stage: &str, details: Value) {
    if !context.debug_auth {
        return;
    }
    eprintln!("[mychart auth debug] {stage}\n{}", crate::render_json(&details, false));
}

fn auth_debug_token_response(context: &ResolvedContext, stage: &str, response: &crate::client::JsonResponse) {
    if !context.debug_auth {
        return;
    }
    auth_debug(
        context,
        stage,
        json!({
            "status_code": response.status_code,
            "final_url": response.final_url.as_str(),
            "content_type": response.content_type,
            "body": if response.status_code >= 400 {
                response.body.clone()
            } else {
                json!({
                    "body_keys": response
                        .body
                        .as_object()
                        .map(|body| body.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default(),
                })
            },
        }),
    );
}

fn form_field_names(form: &[(String, String)]) -> Vec<&str> {
    form.iter().map(|(key, _)| key.as_str()).collect()
}

fn basic_auth_header(client_id: &str, client_secret: &str) -> String {
    let encoded_client_id = oauth_form_component(client_id);
    let encoded_client_secret = oauth_form_component(client_secret);
    let credentials = format!("{encoded_client_id}:{encoded_client_secret}");
    format!("Basic {}", crate::base64_encode(credentials.as_bytes()))
}

fn oauth_form_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => encoded.push(byte as char),
            _ => {
                encoded.push('%');
                encoded.push(hex_digit(byte >> 4));
                encoded.push(hex_digit(byte & 0x0f));
            }
        }
    }
    encoded
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + value - 10) as char,
        _ => unreachable!("nibble should fit in hex"),
    }
}
