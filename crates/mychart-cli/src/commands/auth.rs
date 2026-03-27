use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use clap::{Args, Subcommand};
use reqwest::{Method, Url};
use serde_json::{json, Value};

use crate::{
    api_client, build_authorize_url, dedupe_preserving_order, default_patient_scopes, ensure_code_verifier,
    ensure_json_success, expires_at_epoch_seconds, fetch_capability_summary, generate_nonce,
    oauth::{
        dynamic_client_registration_request, generate_dynamic_client_key_material, sign_dynamic_client_assertion,
        DynamicClientRegistrationResponse, DynamicClientState, JWT_BEARER_GRANT_TYPE,
    },
    parse_oauth_token_response, split_scopes,
    state::{ApiSessionState, ResolvedContext},
    Error, Result,
};

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
    Refresh(AuthRefreshArgs),
    Status,
    Logout,
}

#[derive(Debug, Args, Clone)]
struct AuthAuthorizeOptions {
    #[arg(long, value_name = "URL")]
    redirect_uri: Option<String>,

    #[arg(long = "scope", value_name = "SCOPE")]
    scopes: Vec<String>,

    #[arg(long)]
    state: Option<String>,

    #[arg(long = "code-verifier", value_name = "VERIFIER")]
    code_verifier: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AuthAuthorizeUrlArgs {
    #[command(flatten)]
    options: AuthAuthorizeOptions,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthLoginArgs {
    #[command(flatten)]
    options: AuthAuthorizeOptions,

    #[arg(long, default_value_t = 300)]
    timeout_seconds: u64,

    #[arg(long)]
    no_open: bool,

    #[arg(long)]
    dynamic_client: bool,
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
        AuthSubcommand::Login(args) => run_login(args, context),
        AuthSubcommand::AuthorizeUrl(args) => {
            let prepared = prepare_authorization(
                context,
                args.options,
                !args.no_store,
                TokenExchangeAuth::StoredClientStrategy,
            )?;
            Ok(json!({
                "status": "ok",
                "base_url": prepared.base_url,
                "client_id": prepared.client_id,
                "redirect_uri": prepared.redirect_uri,
                "authorize_url": prepared.authorize_url.as_str(),
                "authorize_endpoint": prepared.authorize_endpoint,
                "token_endpoint": prepared.token_endpoint,
                "state": prepared.oauth_state,
                "scopes": prepared.scopes,
                "code_challenge_method": "S256",
                "code_verifier": if args.no_store {
                    Value::String(prepared.code_verifier)
                } else {
                    Value::Null
                },
                "stored": !args.no_store,
            }))
        }
        AuthSubcommand::ExchangeCode(args) => {
            exchange_code(context, args.code, args.redirect_uri, args.code_verifier, args.no_store)
        }
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

fn run_login(args: AuthLoginArgs, context: &mut ResolvedContext) -> Result<Value> {
    let token_auth = if args.dynamic_client {
        TokenExchangeAuth::ForcePublic
    } else {
        TokenExchangeAuth::StoredClientStrategy
    };
    let prepared = prepare_authorization(context, args.options, true, token_auth)?;
    if args.dynamic_client && !prepared.scopes.iter().any(|scope| scope == "offline_access") {
        return Err(Error::Arguments(
            "auth login --dynamic-client requires the offline_access scope so Epic can grant persistent access".into(),
        ));
    }
    let redirect_uri = Url::parse(&prepared.redirect_uri)
        .map_err(|error| Error::Config(format!("invalid redirect URI {:?}: {error}", prepared.redirect_uri)))?;
    let bind_address = loopback_bind_address(&redirect_uri)?;
    let listener = TcpListener::bind(&bind_address).map_err(|error| {
        Error::Io(format!(
            "failed to bind OAuth callback listener on {bind_address}: {error}"
        ))
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|error| Error::Io(format!("failed to configure OAuth callback listener: {error}")))?;

    if args.no_open {
        eprintln!("Open this URL in a browser: {}", prepared.authorize_url);
    } else {
        eprintln!("Opening browser for MyChart OAuth login...");
        open_browser(prepared.authorize_url.as_str())?;
    }
    eprintln!("Waiting for OAuth callback on {}", prepared.redirect_uri);

    let callback = wait_for_oauth_callback(
        listener,
        &redirect_uri,
        &prepared.oauth_state,
        Duration::from_secs(args.timeout_seconds),
    )?;
    auth_debug(
        context,
        "oauth_callback_received",
        json!({
            "account": context.account,
            "redirect_uri": prepared.redirect_uri,
            "code_length": callback.code.len(),
            "state_length": prepared.oauth_state.len(),
        }),
    );

    if args.dynamic_client {
        return login_with_dynamic_client(context, prepared, callback.code);
    }

    exchange_code(
        context,
        callback.code,
        Some(prepared.redirect_uri),
        Some(prepared.code_verifier),
        false,
    )
}

fn prepare_authorization(
    context: &mut ResolvedContext,
    options: AuthAuthorizeOptions,
    store_pending: bool,
    token_auth: TokenExchangeAuth,
) -> Result<PreparedAuthorization> {
    let base_url = context.require_api_base_url()?;
    let client_id = context.require_client_id()?;
    let redirect_uri = context.require_redirect_uri(options.redirect_uri)?;
    let client = api_client(&base_url)?;
    let capability = fetch_capability_summary(&client, Some(&client_id))?;
    let authorize_endpoint = capability.require_authorize_url()?;
    let token_endpoint = capability.require_token_url()?;
    let oauth_state = match options.state {
        Some(state) => state,
        None => generate_nonce(24)?,
    };
    let code_verifier = match options.code_verifier {
        Some(verifier) => ensure_code_verifier(verifier)?,
        None => ensure_code_verifier(generate_nonce(48)?)?,
    };
    let scopes = if options.scopes.is_empty() {
        default_patient_scopes()
    } else {
        dedupe_preserving_order(options.scopes)
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

    if store_pending {
        context.store_pending_oauth(
            base_url.clone(),
            client_id.clone(),
            context.client_secret.clone(),
            redirect_uri.clone(),
            oauth_state.clone(),
            code_verifier.clone(),
        )?;
    }

    auth_debug(
        context,
        "oauth_authorize_prepared",
        json!({
            "account": context.account,
            "base_url": base_url,
            "authorize_endpoint": authorize_endpoint,
            "token_endpoint": token_endpoint,
            "redirect_uri": redirect_uri,
            "scope_count": scopes.len(),
            "scopes": scopes,
            "client_secret_present": context.client_secret.is_some(),
            "token_authentication": token_exchange_auth_label(token_auth, context.client_secret.is_some()),
        }),
    );

    Ok(PreparedAuthorization {
        base_url,
        client_id,
        redirect_uri,
        authorize_endpoint,
        token_endpoint,
        authorize_url,
        oauth_state,
        code_verifier,
        scopes,
    })
}

fn exchange_code(
    context: &mut ResolvedContext,
    code: String,
    redirect_uri_override: Option<String>,
    code_verifier_override: Option<String>,
    no_store: bool,
) -> Result<Value> {
    let result = exchange_code_token(
        context,
        code,
        redirect_uri_override,
        code_verifier_override,
        TokenExchangeAuth::StoredClientStrategy,
    )?;
    let refresh_token = result
        .token
        .refresh_token
        .clone()
        .or_else(|| context.refresh_token.clone());

    if !no_store {
        context.store_api_tokens(ApiSessionState {
            base_url: result.base_url.clone(),
            client_id: result.client_id.clone(),
            client_secret: result.client_secret.clone(),
            redirect_uri: result.redirect_uri.clone(),
            access_token: result.token.access_token.clone(),
            refresh_token: refresh_token.clone(),
            token_type: result.token.token_type.clone(),
            scope: result.token.scope.clone(),
            patient_id: result.token.patient.clone(),
            expires_at_epoch_seconds: result.expires_at_epoch_seconds,
        })?;
    }

    Ok(json!({
        "status": "authenticated",
        "base_url": result.base_url,
        "client_id": result.client_id,
        "redirect_uri": result.redirect_uri,
        "token_endpoint": result.token_endpoint,
        "patient_id": result.token.patient,
        "scope": split_scopes(result.token.scope.as_deref()),
        "token_type": result.token.token_type,
        "expires_at_epoch_seconds": result.expires_at_epoch_seconds,
        "dynamic_client_id": context.dynamic_client().map(|dynamic_client| dynamic_client.client_id.clone()),
        "dynamic_client_registered": context.dynamic_client().is_some(),
        "renewal_method": renewal_method(context),
        "refresh_token_available": refresh_token.is_some(),
        "stored": !no_store,
    }))
}

fn login_with_dynamic_client(
    context: &mut ResolvedContext,
    prepared: PreparedAuthorization,
    code: String,
) -> Result<Value> {
    let initial_token = exchange_code_token(
        context,
        code,
        Some(prepared.redirect_uri.clone()),
        Some(prepared.code_verifier),
        TokenExchangeAuth::ForcePublic,
    )?;
    let key_material = generate_dynamic_client_key_material()?;
    let dynamic_client = register_dynamic_client(
        context,
        &prepared.client_id,
        &initial_token.base_url,
        &initial_token.token.access_token,
        &key_material.private_key_pem,
        &key_material.jwks,
    )?;
    context.store_dynamic_client(dynamic_client.clone())?;
    let refreshed = exchange_dynamic_client_token(context, Some(dynamic_client.clone()))?;

    context.store_api_tokens(ApiSessionState {
        base_url: refreshed.base_url.clone(),
        client_id: refreshed.client_id.clone(),
        client_secret: refreshed.client_secret.clone(),
        redirect_uri: refreshed.redirect_uri.clone(),
        access_token: refreshed.token.access_token.clone(),
        refresh_token: refreshed.token.refresh_token.clone(),
        token_type: refreshed.token.token_type.clone(),
        scope: refreshed.token.scope.clone(),
        patient_id: refreshed.token.patient.clone(),
        expires_at_epoch_seconds: refreshed.expires_at_epoch_seconds,
    })?;

    Ok(json!({
        "status": "authenticated",
        "base_url": refreshed.base_url,
        "client_id": refreshed.client_id,
        "dynamic_client_id": dynamic_client.client_id,
        "dynamic_client_registered": true,
        "renewal_method": "dynamic_client_jwt_bearer",
        "registration_endpoint": dynamic_client.registration_endpoint,
        "redirect_uri": refreshed.redirect_uri,
        "token_endpoint": refreshed.token_endpoint,
        "patient_id": refreshed.token.patient,
        "scope": split_scopes(refreshed.token.scope.as_deref()),
        "token_type": refreshed.token.token_type,
        "expires_at_epoch_seconds": refreshed.expires_at_epoch_seconds,
        "refresh_token_available": refreshed.token.refresh_token.is_some(),
        "stored": true,
    }))
}

fn exchange_code_token(
    context: &ResolvedContext,
    code: String,
    redirect_uri_override: Option<String>,
    code_verifier_override: Option<String>,
    token_auth: TokenExchangeAuth,
) -> Result<TokenExchangeResult> {
    let base_url = context.require_api_base_url()?;
    let client_id = context.require_client_id()?;
    let redirect_uri = context.require_redirect_uri(redirect_uri_override)?;
    let code_verifier = context.require_code_verifier(code_verifier_override)?;
    let stored_client_secret = context.client_secret.clone();
    let client_secret = match token_auth {
        TokenExchangeAuth::StoredClientStrategy => stored_client_secret.clone(),
        TokenExchangeAuth::ForcePublic => None,
    };
    let authorization_header = client_secret
        .as_deref()
        .map(|client_secret| basic_auth_header(&client_id, client_secret));
    let client = api_client(&base_url)?;
    let capability = fetch_capability_summary(&client, Some(&client_id))?;
    let token_endpoint = capability.require_token_url()?;
    let mut form = vec![
        ("grant_type".into(), "authorization_code".into()),
        ("code".into(), code),
        ("redirect_uri".into(), redirect_uri.clone()),
        ("code_verifier".into(), code_verifier),
    ];
    if authorization_header.is_none() {
        form.push(("client_id".into(), client_id.clone()));
    }

    auth_debug(
        context,
        "oauth_token_exchange_request",
        json!({
            "account": context.account,
            "token_endpoint": token_endpoint,
            "grant_type": "authorization_code",
            "redirect_uri": redirect_uri,
            "code_length": form
                .iter()
                .find(|(key, _)| key == "code")
                .map(|(_, value)| value.len())
                .unwrap_or_default(),
            "code_verifier_length": form
                .iter()
                .find(|(key, _)| key == "code_verifier")
                .map(|(_, value)| value.len())
                .unwrap_or_default(),
            "body_fields": form_field_names(&form),
            "authorization_header": authorization_header
                .as_ref()
                .map(|_| "Basic <redacted>")
                .unwrap_or("none"),
            "client_secret_present": client_secret.is_some(),
            "token_authentication": token_exchange_auth_label(token_auth, client_secret.is_some()),
        }),
    );

    let response = client.exchange_oauth_token(&token_endpoint, &form, authorization_header.as_deref())?;
    auth_debug_token_response(context, "oauth_token_exchange_response", &response);
    ensure_json_success(&response)?;
    let token = parse_oauth_token_response(&response.body)?;
    let expires_at_epoch_seconds = token.expires_in.map(expires_at_epoch_seconds);

    Ok(TokenExchangeResult {
        base_url,
        client_id,
        client_secret: stored_client_secret,
        redirect_uri,
        token_endpoint,
        token,
        expires_at_epoch_seconds,
    })
}

fn refresh_with_refresh_token(context: &mut ResolvedContext, args: AuthRefreshArgs) -> Result<Value> {
    let base_url = context.require_api_base_url()?;
    let client_id = context.require_client_id()?;
    let redirect_uri = context.require_redirect_uri(None)?;
    let refresh_token = context.require_refresh_token(args.refresh_token)?;
    let client_secret = context.client_secret.clone();
    let authorization_header = client_secret
        .as_deref()
        .map(|client_secret| basic_auth_header(&client_id, client_secret));
    let client = api_client(&base_url)?;
    let capability = fetch_capability_summary(&client, Some(&client_id))?;
    let token_endpoint = capability.require_token_url()?;
    let mut form = vec![
        ("grant_type".into(), "refresh_token".into()),
        ("refresh_token".into(), refresh_token.clone()),
    ];
    if authorization_header.is_none() {
        form.push(("client_id".into(), client_id.clone()));
    }

    auth_debug(
        context,
        "oauth_refresh_request",
        json!({
            "account": context.account,
            "token_endpoint": token_endpoint,
            "grant_type": "refresh_token",
            "redirect_uri": redirect_uri,
            "refresh_token_present": true,
            "body_fields": form_field_names(&form),
            "authorization_header": authorization_header
                .as_ref()
                .map(|_| "Basic <redacted>")
                .unwrap_or("none"),
            "client_secret_present": client_secret.is_some(),
        }),
    );

    let response = client.exchange_oauth_token(&token_endpoint, &form, authorization_header.as_deref())?;
    auth_debug_token_response(context, "oauth_refresh_response", &response);
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
        "dynamic_client_id": context.dynamic_client().map(|dynamic_client| dynamic_client.client_id.clone()),
        "dynamic_client_registered": context.dynamic_client().is_some(),
        "renewal_method": renewal_method(context),
        "token_endpoint": token_endpoint,
        "patient_id": token.patient.or_else(|| context.patient_id.clone()),
        "scope": split_scopes(token.scope.as_deref().or(context.scope.as_deref())),
        "token_type": token.token_type,
        "expires_at_epoch_seconds": expires_at_epoch_seconds,
        "refresh_token_available": next_refresh_token.is_some(),
        "stored": !args.no_store,
    }))
}

fn refresh_with_dynamic_client(context: &mut ResolvedContext, no_store: bool) -> Result<Value> {
    let result = exchange_dynamic_client_token(context, None)?;
    let refresh_token = result
        .token
        .refresh_token
        .clone()
        .or_else(|| context.refresh_token.clone());

    if !no_store {
        context.store_api_tokens(ApiSessionState {
            base_url: result.base_url.clone(),
            client_id: result.client_id.clone(),
            client_secret: result.client_secret.clone(),
            redirect_uri: result.redirect_uri.clone(),
            access_token: result.token.access_token.clone(),
            refresh_token: refresh_token.clone(),
            token_type: result.token.token_type.clone(),
            scope: result.token.scope.clone().or_else(|| context.scope.clone()),
            patient_id: result.token.patient.clone().or_else(|| context.patient_id.clone()),
            expires_at_epoch_seconds: result.expires_at_epoch_seconds,
        })?;
    }

    Ok(json!({
        "status": "refreshed",
        "base_url": result.base_url,
        "client_id": result.client_id,
        "dynamic_client_id": context.dynamic_client().map(|dynamic_client| dynamic_client.client_id.clone()),
        "dynamic_client_registered": context.dynamic_client().is_some(),
        "renewal_method": "dynamic_client_jwt_bearer",
        "token_endpoint": result.token_endpoint,
        "patient_id": result.token.patient.or_else(|| context.patient_id.clone()),
        "scope": split_scopes(result.token.scope.as_deref().or(context.scope.as_deref())),
        "token_type": result.token.token_type,
        "expires_at_epoch_seconds": result.expires_at_epoch_seconds,
        "refresh_token_available": refresh_token.is_some(),
        "stored": !no_store,
    }))
}

fn register_dynamic_client(
    context: &ResolvedContext,
    software_client_id: &str,
    base_url: &str,
    initial_access_token: &str,
    private_key_pem: &str,
    jwks: &crate::oauth::DynamicJwkSet,
) -> Result<DynamicClientState> {
    let client = api_client(base_url)?;
    let capability = fetch_capability_summary(&client, Some(software_client_id))?;
    let register_endpoint = capability.require_register_url()?;
    let request_body = serde_json::to_value(dynamic_client_registration_request(
        software_client_id.to_owned(),
        jwks.clone(),
    ))
    .map_err(|error| Error::Auth {
        message: "failed to serialize the Epic dynamic client registration request".into(),
        details: json!({
            "error": error.to_string(),
        }),
    })?;

    auth_debug(
        context,
        "oauth_dynamic_registration_request",
        json!({
            "account": context.account,
            "register_endpoint": register_endpoint,
            "software_client_id": software_client_id,
            "body": request_body,
        }),
    );

    let response = client.execute_bearer_json_absolute(
        Method::POST,
        &register_endpoint,
        initial_access_token,
        Some(&request_body),
    )?;
    auth_debug_token_response(context, "oauth_dynamic_registration_response", &response);
    ensure_json_success(&response)?;
    let registration: DynamicClientRegistrationResponse =
        serde_json::from_value(response.body.clone()).map_err(|error| Error::Auth {
            message: "Epic returned a dynamic client registration response we could not parse".into(),
            details: json!({
                "error": error.to_string(),
                "body": response.body,
            }),
        })?;
    if let Some(token_endpoint_auth_method) = registration.token_endpoint_auth_method.as_deref() {
        if token_endpoint_auth_method != "none" {
            return Err(Error::Auth {
                message: format!(
                    "Epic registered a dynamic client with unsupported token endpoint auth method {token_endpoint_auth_method:?}"
                ),
                details: json!({
                    "body": response.body,
                }),
            });
        }
    }
    if !registration.grant_types.is_empty()
        && !registration
            .grant_types
            .iter()
            .any(|grant_type| grant_type == JWT_BEARER_GRANT_TYPE)
    {
        return Err(Error::Auth {
            message: "Epic registered a dynamic client that does not advertise JWT bearer grants".into(),
            details: json!({
                "grant_types": registration.grant_types,
                "body": response.body,
            }),
        });
    }

    Ok(DynamicClientState {
        client_id: registration.client_id,
        private_key_pem: private_key_pem.to_owned(),
        registration_endpoint: Some(register_endpoint),
        client_id_issued_at_epoch_seconds: registration.client_id_issued_at,
    })
}

fn exchange_dynamic_client_token(
    context: &ResolvedContext,
    dynamic_client_override: Option<DynamicClientState>,
) -> Result<TokenExchangeResult> {
    let base_url = context.require_api_base_url()?;
    let client_id = context.require_client_id()?;
    let redirect_uri = context.require_redirect_uri(None)?;
    let dynamic_client = dynamic_client_override
        .or_else(|| context.dynamic_client().cloned())
        .ok_or_else(|| {
            Error::Config(
                "missing Epic dynamic client registration, run `mychart auth login --dynamic-client` first".into(),
            )
        })?;
    let client = api_client(&base_url)?;
    let capability = fetch_capability_summary(&client, Some(&client_id))?;
    let token_endpoint = capability.require_token_url()?;
    let assertion = sign_dynamic_client_assertion(
        &dynamic_client.client_id,
        &token_endpoint,
        &dynamic_client.private_key_pem,
    )?;
    let form = vec![
        ("grant_type".into(), JWT_BEARER_GRANT_TYPE.into()),
        ("client_id".into(), dynamic_client.client_id.clone()),
        ("assertion".into(), assertion.clone()),
    ];

    auth_debug(
        context,
        "oauth_dynamic_jwt_request",
        json!({
            "account": context.account,
            "token_endpoint": token_endpoint,
            "grant_type": JWT_BEARER_GRANT_TYPE,
            "dynamic_client_id": dynamic_client.client_id,
            "assertion_length": assertion.len(),
            "body_fields": form_field_names(&form),
        }),
    );

    let response = client.exchange_oauth_token(&token_endpoint, &form, None)?;
    auth_debug_token_response(context, "oauth_dynamic_jwt_response", &response);
    ensure_json_success(&response)?;
    let token = parse_oauth_token_response(&response.body)?;
    let expires_at_epoch_seconds = token.expires_in.map(expires_at_epoch_seconds);

    Ok(TokenExchangeResult {
        base_url,
        client_id,
        client_secret: context.client_secret.clone(),
        redirect_uri,
        token_endpoint,
        token,
        expires_at_epoch_seconds,
    })
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

fn token_exchange_auth_label(token_auth: TokenExchangeAuth, client_secret_present: bool) -> &'static str {
    match token_auth {
        TokenExchangeAuth::StoredClientStrategy if client_secret_present => "basic",
        TokenExchangeAuth::StoredClientStrategy => "public_pkce",
        TokenExchangeAuth::ForcePublic => "public_pkce",
    }
}

#[derive(Debug)]
struct OAuthCallback {
    code: String,
}

fn loopback_bind_address(redirect_uri: &Url) -> Result<String> {
    let host = redirect_uri
        .host_str()
        .ok_or_else(|| Error::Arguments("auth login requires a redirect URI with an explicit host".into()))?;
    if host != "127.0.0.1" && host != "localhost" {
        return Err(Error::Arguments(
            "auth login currently requires a loopback redirect URI like http://127.0.0.1:8910/callback".into(),
        ));
    }
    let port = redirect_uri
        .port_or_known_default()
        .ok_or_else(|| Error::Arguments("auth login requires a redirect URI with an explicit port".into()))?;
    Ok(format!("{host}:{port}"))
}

fn wait_for_oauth_callback(
    listener: TcpListener,
    redirect_uri: &Url,
    expected_state: &str,
    timeout: Duration,
) -> Result<OAuthCallback> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                return read_oauth_callback(&mut stream, redirect_uri, expected_state);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(Error::Auth {
                        message: format!("timed out waiting {} seconds for the OAuth callback", timeout.as_secs()),
                        details: json!({
                            "redirect_uri": redirect_uri.as_str(),
                        }),
                    });
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(Error::Io(format!("failed while waiting for OAuth callback: {error}")));
            }
        }
    }
}

fn read_oauth_callback(stream: &mut TcpStream, redirect_uri: &Url, expected_state: &str) -> Result<OAuthCallback> {
    let request = read_http_request(stream)?;
    let request_target = request_target(&request)?;
    let callback_url = redirect_uri.join(&request_target).map_err(|error| {
        Error::Http(format!(
            "failed to parse OAuth callback request target {request_target:?}: {error}"
        ))
    })?;

    if callback_url.path() != redirect_uri.path() {
        write_http_response(
            stream,
            404,
            "Not Found",
            callback_page_html("Wrong callback path. Return to the terminal and try again."),
        )?;
        return Err(Error::Auth {
            message: "OAuth callback hit the wrong path".into(),
            details: json!({
                "expected_path": redirect_uri.path(),
                "received_path": callback_url.path(),
            }),
        });
    }

    let params = callback_url.query_pairs().collect::<Vec<_>>();
    if let Some(error) = params
        .iter()
        .find(|(key, _)| key == "error")
        .map(|(_, value)| value.to_string())
    {
        let error_description = params
            .iter()
            .find(|(key, _)| key == "error_description")
            .map(|(_, value)| value.to_string());
        write_http_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("OAuth authorization failed. Return to the terminal for details."),
        )?;
        return Err(Error::Auth {
            message: format!("OAuth authorization failed with error {error}"),
            details: json!({
                "error": error,
                "error_description": error_description,
            }),
        });
    }

    let Some(state) = params
        .iter()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.to_string())
    else {
        write_http_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("OAuth callback was missing state. Return to the terminal and start over."),
        )?;
        return Err(Error::Auth {
            message: "OAuth callback was missing the state parameter".into(),
            details: json!({ "callback_url": callback_url.as_str() }),
        });
    };
    if state != expected_state {
        write_http_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("OAuth state mismatch. Return to the terminal and start over."),
        )?;
        return Err(Error::Auth {
            message: "OAuth callback state mismatch".into(),
            details: json!({
                "expected_state": expected_state,
                "received_state": state,
            }),
        });
    }

    let Some(code) = params
        .iter()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.to_string())
    else {
        write_http_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("OAuth callback was missing a code. Return to the terminal and start over."),
        )?;
        return Err(Error::Auth {
            message: "OAuth callback was missing the authorization code".into(),
            details: json!({ "callback_url": callback_url.as_str() }),
        });
    };

    write_http_response(
        stream,
        200,
        "OK",
        callback_page_html("MyChart authorization received. You can close this tab and go back to the terminal."),
    )?;

    Ok(OAuthCallback { code })
}

fn read_http_request(stream: &mut TcpStream) -> Result<String> {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 1024];
    loop {
        let bytes_read = stream
            .read(&mut temp)
            .map_err(|error| Error::Io(format!("failed to read OAuth callback request: {error}")))?;
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..bytes_read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buffer)
        .map_err(|error| Error::Http(format!("OAuth callback request was not valid UTF-8: {error}")))
}

fn request_target(request: &str) -> Result<String> {
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| Error::Http("received an empty OAuth callback request".into()))?;
    let mut parts = first_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| Error::Http("OAuth callback request line was missing the HTTP method".into()))?;
    if method != "GET" {
        return Err(Error::Http(format!(
            "OAuth callback used unsupported HTTP method {method:?}, expected GET"
        )));
    }
    parts
        .next()
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::Http("OAuth callback request line was missing the request target".into()))
}

fn write_http_response(stream: &mut TcpStream, status_code: u16, reason: &str, body: String) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status_code} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| Error::Io(format!("failed to write OAuth callback response: {error}")))
}

fn callback_page_html(message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>MyChart OAuth</title></head><body><p>{message}</p></body></html>"
    )
}

fn open_browser(url: &str) -> Result<()> {
    let (command, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[url])
    } else if cfg!(target_os = "windows") {
        ("cmd", &["/C", "start", "", url])
    } else {
        ("xdg-open", &[url])
    };

    Command::new(command)
        .args(args)
        .spawn()
        .map_err(|error| Error::Io(format!("failed to launch browser with {command}: {error}")))?;

    Ok(())
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
