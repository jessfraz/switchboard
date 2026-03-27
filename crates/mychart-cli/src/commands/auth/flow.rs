use std::time::Duration;

use reqwest::Url;
use serde_json::{json, Value};

use crate::{
    api_client, build_authorize_url, dedupe_preserving_order, default_patient_scopes, ensure_code_verifier,
    ensure_json_success, expires_at_epoch_seconds, fetch_capability_summary, generate_nonce,
    parse_oauth_token_response, split_scopes,
    state::{ApiSessionState, ResolvedContext},
    Error, Result,
};

use super::{
    access_token_is_fresh, auth_debug, auth_debug_token_response, callback::launch_browser_for_authorization,
    callback::loopback_bind_address, callback::parse_callback_input, callback::prompt_for_callback_url,
    callback::redirect_uri_uses_loopback, callback::wait_for_oauth_callback, can_fallback_to_interactive_auth,
    dynamic_client::exchange_dynamic_client_token, dynamic_client::login_with_dynamic_client, renewal_method,
    token_exchange_auth_label, ApiSessionBootstrap, AuthAuthorizeOptions, AuthAuthorizeUrlArgs, AuthExchangeUrlArgs,
    AuthLoginArgs, AuthRefreshArgs, HostedAuthorizationOutcome, PreparedAuthorization, TokenExchangeAuth,
    TokenExchangeResult, AUTO_LOGIN_TIMEOUT_SECONDS,
};

pub(super) fn run_login(args: AuthLoginArgs, context: &mut ResolvedContext) -> Result<Value> {
    let mut options = args.options.clone();
    if args.dynamic_client {
        options.scopes.retain(|scope| scope != "offline_access");
    }
    let token_auth = if args.dynamic_client {
        TokenExchangeAuth::ForcePublic
    } else {
        TokenExchangeAuth::StoredClientStrategy
    };
    let prepared = prepare_authorization(context, options, true, token_auth)?;
    let redirect_uri = Url::parse(&prepared.redirect_uri)
        .map_err(|error| Error::Config(format!("invalid redirect URI {:?}: {error}", prepared.redirect_uri)))?;
    let bind_address = loopback_bind_address(&redirect_uri)?;
    let listener = std::net::TcpListener::bind(&bind_address).map_err(|error| {
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
        super::callback::open_browser(prepared.authorize_url.as_str())?;
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

pub(super) fn run_authorize_url(args: AuthAuthorizeUrlArgs, context: &mut ResolvedContext) -> Result<Value> {
    let prepared = prepare_authorization(
        context,
        args.options,
        !args.no_store,
        TokenExchangeAuth::StoredClientStrategy,
    )?;
    let browser_launch = launch_browser_for_authorization(context, prepared.authorize_url.as_str(), args.no_open);
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
        "browser_open_attempted": browser_launch.attempted,
        "opened_browser": browser_launch.opened,
        "browser_open_error": browser_launch.error,
        "next_step": "After the browser finishes, paste the copied login code into the waiting terminal, or run `mychart finish '<auth-code>'` in this repo.",
    }))
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
    let mut scopes = if options.scopes.is_empty() {
        default_patient_scopes()
    } else {
        options.scopes
    };
    if matches!(token_auth, TokenExchangeAuth::ForcePublic) {
        scopes.retain(|scope| scope != "offline_access");
    }
    let scopes = dedupe_preserving_order(scopes);
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

pub(super) fn interactive_auth_bootstrap(context: &mut ResolvedContext) -> Result<ApiSessionBootstrap> {
    let redirect_uri = context.require_redirect_uri(None)?;
    if redirect_uri_uses_loopback(&redirect_uri)? {
        run_login(
            AuthLoginArgs {
                options: AuthAuthorizeOptions {
                    redirect_uri: None,
                    scopes: Vec::new(),
                    state: None,
                    code_verifier: None,
                },
                timeout_seconds: AUTO_LOGIN_TIMEOUT_SECONDS,
                no_open: false,
                dynamic_client: false,
            },
            context,
        )?;
        return Ok(ApiSessionBootstrap::Ready);
    }

    let output = run_authorize_url(
        AuthAuthorizeUrlArgs {
            options: AuthAuthorizeOptions {
                redirect_uri: None,
                scopes: Vec::new(),
                state: None,
                code_verifier: None,
            },
            no_store: false,
            no_open: false,
        },
        context,
    )?;
    match complete_or_wait_for_hosted_authorization(
        context,
        output,
        None,
        "Finish the browser login, paste the copied login code back into this terminal, or run `mychart finish '<auth-code>'` later, then rerun the original command.",
    )? {
        HostedAuthorizationOutcome::Completed(_) => Ok(ApiSessionBootstrap::Ready),
        HostedAuthorizationOutcome::Pending(output) => Ok(ApiSessionBootstrap::Pending(output)),
    }
}

pub(crate) fn complete_or_wait_for_hosted_authorization(
    context: &mut ResolvedContext,
    mut authorize_output: Value,
    callback_url: Option<String>,
    pending_next_step: &str,
) -> Result<HostedAuthorizationOutcome> {
    if let Some(callback_url) = callback_url.or_else(prompt_for_callback_url) {
        let output = exchange_url(
            AuthExchangeUrlArgs {
                callback_input: callback_url,
                no_store: false,
            },
            context,
        )?;
        return Ok(HostedAuthorizationOutcome::Completed(output));
    }

    if let Some(object) = authorize_output.as_object_mut() {
        object.insert("status".into(), Value::String("authorization_pending".into()));
        object.insert(
            "selected_account".into(),
            context
                .active_account_name()
                .map(|name| Value::String(name.to_owned()))
                .unwrap_or(Value::Null),
        );
        object.insert("next_step".into(), Value::String(pending_next_step.into()));
    }

    Ok(HostedAuthorizationOutcome::Pending(authorize_output))
}

pub(super) fn exchange_code(
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

pub(super) fn exchange_url(args: AuthExchangeUrlArgs, context: &mut ResolvedContext) -> Result<Value> {
    let redirect_uri = context.require_redirect_uri(None)?;
    let expected_redirect_uri = Url::parse(&redirect_uri)
        .map_err(|error| Error::Config(format!("invalid stored redirect URI {redirect_uri:?}: {error}")))?;
    let parsed_url = parse_callback_input(
        &args.callback_input,
        &expected_redirect_uri,
        context.pending_oauth_state.as_deref(),
    )?;
    let mut normalized_callback = parsed_url.clone();
    normalized_callback.set_query(None);
    normalized_callback.set_fragment(None);
    let mut normalized_expected = expected_redirect_uri.clone();
    normalized_expected.set_query(None);
    normalized_expected.set_fragment(None);
    if normalized_callback != normalized_expected {
        return Err(Error::Auth {
            message: "OAuth callback URL did not match the configured redirect URI".into(),
            details: json!({
                "expected_redirect_uri": normalized_expected.as_str(),
                "received_callback_url": normalized_callback.as_str(),
            }),
        });
    }

    let params = parsed_url.query_pairs().collect::<Vec<_>>();
    if let Some(error) = params
        .iter()
        .find(|(key, _)| key == "error")
        .map(|(_, value)| value.to_string())
    {
        let error_description = params
            .iter()
            .find(|(key, _)| key == "error_description")
            .map(|(_, value)| value.to_string());
        return Err(Error::Auth {
            message: format!("OAuth authorization failed with error {error}"),
            details: json!({
                "error": error,
                "error_description": error_description,
                "callback_url": parsed_url.as_str(),
            }),
        });
    }

    let returned_state = params
        .iter()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.to_string())
        .ok_or_else(|| Error::Auth {
            message: "OAuth callback URL was missing the state parameter".into(),
            details: json!({
                "callback_url": parsed_url.as_str(),
            }),
        })?;
    let expected_state = context
        .pending_oauth_state
        .clone()
        .ok_or_else(|| Error::Config("missing pending OAuth state, run mychart auth authorize-url first".into()))?;
    if returned_state != expected_state {
        return Err(Error::Auth {
            message: "OAuth callback state mismatch".into(),
            details: json!({
                "expected_state": expected_state,
                "received_state": returned_state,
            }),
        });
    }

    let code = params
        .iter()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.to_string())
        .ok_or_else(|| Error::Auth {
            message: "OAuth callback URL was missing the authorization code".into(),
            details: json!({
                "callback_url": parsed_url.as_str(),
            }),
        })?;

    exchange_code(
        context,
        code,
        Some(normalized_expected.to_string()),
        None,
        args.no_store,
    )
}

pub(super) fn exchange_code_token(
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
        .map(|client_secret| super::basic_auth_header(&client_id, client_secret));
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
            "body_fields": super::form_field_names(&form),
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

pub(super) fn refresh_with_refresh_token(context: &mut ResolvedContext, args: AuthRefreshArgs) -> Result<Value> {
    let base_url = context.require_api_base_url()?;
    let client_id = context.require_client_id()?;
    let redirect_uri = context.require_redirect_uri(None)?;
    let refresh_token = context.require_refresh_token(args.refresh_token)?;
    let client_secret = context.client_secret.clone();
    let authorization_header = client_secret
        .as_deref()
        .map(|client_secret| super::basic_auth_header(&client_id, client_secret));
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
            "body_fields": super::form_field_names(&form),
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

pub(super) fn refresh_with_dynamic_client(context: &mut ResolvedContext, no_store: bool) -> Result<Value> {
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

pub(crate) fn ensure_api_session(context: &mut ResolvedContext) -> Result<ApiSessionBootstrap> {
    if access_token_is_fresh(context) {
        return Ok(ApiSessionBootstrap::Ready);
    }

    if context.dynamic_client().is_some() {
        match refresh_with_dynamic_client(context, false) {
            Ok(_) => return Ok(ApiSessionBootstrap::Ready),
            Err(error) if can_fallback_to_interactive_auth(&error) => {}
            Err(error) => return Err(error),
        }
    }

    if context.refresh_token.is_some() {
        match refresh_with_refresh_token(
            context,
            AuthRefreshArgs {
                refresh_token: None,
                no_store: false,
            },
        ) {
            Ok(_) => return Ok(ApiSessionBootstrap::Ready),
            Err(error) if can_fallback_to_interactive_auth(&error) => {}
            Err(error) => return Err(error),
        }
    }

    interactive_auth_bootstrap(context)
}
