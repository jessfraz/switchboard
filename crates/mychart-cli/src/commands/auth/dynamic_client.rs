use reqwest::Method;
use serde_json::{json, Value};

use crate::{
    api_client, ensure_json_success, expires_at_epoch_seconds, fetch_capability_summary, parse_oauth_token_response,
    split_scopes,
    state::{ApiSessionState, ResolvedContext},
    Error, Result,
};

use super::{
    auth_debug, auth_debug_token_response, exchange_code_token, PreparedAuthorization, TokenExchangeAuth,
    TokenExchangeResult,
};

pub(super) fn login_with_dynamic_client(
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
    let key_material = crate::oauth::generate_dynamic_client_key_material()?;
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

pub(super) fn register_dynamic_client(
    context: &ResolvedContext,
    software_client_id: &str,
    base_url: &str,
    initial_access_token: &str,
    private_key_pem: &str,
    jwks: &crate::oauth::DynamicJwkSet,
) -> Result<crate::oauth::DynamicClientState> {
    let client = api_client(base_url)?;
    let capability = fetch_capability_summary(&client, Some(software_client_id))?;
    let register_endpoint = capability.require_register_url()?;
    let request_body = serde_json::to_value(crate::oauth::dynamic_client_registration_request(
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
    let registration: crate::oauth::DynamicClientRegistrationResponse = serde_json::from_value(response.body.clone())
        .map_err(|error| Error::Auth {
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
            .any(|grant_type| grant_type == crate::oauth::JWT_BEARER_GRANT_TYPE)
    {
        return Err(Error::Auth {
            message: "Epic registered a dynamic client that does not advertise JWT bearer grants".into(),
            details: json!({
                "grant_types": registration.grant_types,
                "body": response.body,
            }),
        });
    }

    Ok(crate::oauth::DynamicClientState {
        client_id: registration.client_id,
        private_key_pem: private_key_pem.to_owned(),
        registration_endpoint: Some(register_endpoint),
        client_id_issued_at_epoch_seconds: registration.client_id_issued_at,
    })
}

pub(super) fn exchange_dynamic_client_token(
    context: &ResolvedContext,
    dynamic_client_override: Option<crate::oauth::DynamicClientState>,
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
    let assertion = crate::oauth::sign_dynamic_client_assertion(
        &dynamic_client.client_id,
        &token_endpoint,
        &dynamic_client.private_key_pem,
    )?;
    let form = vec![
        ("grant_type".into(), crate::oauth::JWT_BEARER_GRANT_TYPE.into()),
        ("client_id".into(), dynamic_client.client_id.clone()),
        ("assertion".into(), assertion.clone()),
    ];

    auth_debug(
        context,
        "oauth_dynamic_jwt_request",
        json!({
            "account": context.account,
            "token_endpoint": token_endpoint,
            "grant_type": crate::oauth::JWT_BEARER_GRANT_TYPE,
            "dynamic_client_id": dynamic_client.client_id,
            "assertion_length": assertion.len(),
            "body_fields": super::form_field_names(&form),
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
