use std::collections::BTreeMap;

use reqwest::Url;
use serde_json::json;

use super::support::*;
use crate::state::{MyChartState, StateStore};
#[test]
fn authorize_url_discovers_smart_endpoints_and_stores_pkce_state() {
    let server = TestServer::spawn(vec![ResponseSpec::json(
        200,
        capability_statement_json("http://placeholder", &[]),
        Vec::new(),
    )]);
    let temp_dir = temp_dir("mychart-authorize-url");
    let config_path = temp_dir.join("config.json");

    let output: AuthorizeUrlOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &format!("{}/", server.base_url()),
        "--client-id",
        "client-123",
        "--redirect-uri",
        "http://127.0.0.1:8910/callback",
        "--compact",
        "auth",
        "authorize-url",
        "--no-open",
    ]);

    assert_eq!(output.status, "ok");
    assert!(!output.opened_browser);
    assert!(output.authorize_url.contains("response_type=code"));
    let authorize_url = Url::parse(&output.authorize_url).expect("authorize url should parse");
    let expected_base_url = server.base_url();
    let aud = authorize_url
        .query_pairs()
        .find(|(key, _)| key == "aud")
        .map(|(_, value)| value.to_string())
        .expect("aud query param should be present");
    assert_eq!(aud, expected_base_url);

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(state.current_account.as_deref(), Some("default"));
    assert_eq!(account.api_base_url.as_deref(), Some(expected_base_url.as_str()));
    assert_eq!(account.client_id.as_deref(), Some("client-123"));
    assert!(account.pending_code_verifier.is_some());
}
#[test]
fn exchange_code_stores_tokens_for_api_use() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "token_type": "Bearer",
                "scope": "patient/*.read",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-exchange");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            client_id: Some("client-123".into()),
            redirect_uri: Some("http://127.0.0.1:8910/callback".into()),
            pending_code_verifier: Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: AuthenticatedOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "exchange-code",
        "--code",
        "oauth-code",
    ]);

    assert_eq!(output.status, "authenticated");

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(account.access_token.as_deref(), Some("access-token"));
    assert_eq!(account.patient_id.as_deref(), Some("patient-123"));
}
#[test]
fn exchange_url_parses_callback_and_stores_tokens_for_api_use() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "token_type": "Bearer",
                "scope": "patient/*.read",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-exchange-url");
    let config_path = temp_dir.join("config.json");
    let redirect_uri = "https://jessfraz.github.io/switchboard/mychart-callback/";
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            client_id: Some("client-123".into()),
            redirect_uri: Some(redirect_uri.into()),
            pending_oauth_state: Some("test-state".into()),
            pending_code_verifier: Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: AuthenticatedOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "exchange-url",
        "https://jessfraz.github.io/switchboard/mychart-callback/?code=oauth-code&state=test-state",
    ]);

    assert_eq!(output.status, "authenticated");

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(account.access_token.as_deref(), Some("access-token"));
    assert_eq!(account.patient_id.as_deref(), Some("patient-123"));

    let requests = server.requests();
    let token_request = requests.get(1).expect("token request should be captured");
    assert_eq!(
        token_request.form_value("redirect_uri").as_deref(),
        Some("https://jessfraz.github.io/switchboard/mychart-callback/")
    );
}

#[test]
fn top_level_finish_parses_callback_and_stores_tokens_for_api_use() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "token_type": "Bearer",
                "scope": "patient/*.read",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-finish");
    let config_path = temp_dir.join("config.json");
    let redirect_uri = "https://jessfraz.github.io/switchboard/mychart-callback/";
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            client_id: Some("client-123".into()),
            redirect_uri: Some(redirect_uri.into()),
            pending_oauth_state: Some("test-state".into()),
            pending_code_verifier: Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: AuthenticatedOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "finish",
        "https://jessfraz.github.io/switchboard/mychart-callback/?code=oauth-code&state=test-state",
    ]);

    assert_eq!(output.status, "authenticated");

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(account.access_token.as_deref(), Some("access-token"));
    assert_eq!(account.patient_id.as_deref(), Some("patient-123"));
}

#[test]
fn top_level_finish_accepts_bare_authorization_code() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "token_type": "Bearer",
                "scope": "patient/*.read",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-finish-compact-payload");
    let config_path = temp_dir.join("config.json");
    let redirect_uri = "https://jessfraz.github.io/switchboard/mychart-callback/";
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            client_id: Some("client-123".into()),
            redirect_uri: Some(redirect_uri.into()),
            pending_oauth_state: Some("test-state".into()),
            pending_code_verifier: Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: AuthenticatedOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "finish",
        "oauth-code",
    ]);

    assert_eq!(output.status, "authenticated");
    let requests = server.requests();
    let token_request = requests.get(1).expect("token request should be captured");
    assert_eq!(
        token_request.form_value("redirect_uri").as_deref(),
        Some("https://jessfraz.github.io/switchboard/mychart-callback/")
    );
}

#[test]
fn top_level_finish_accepts_pasted_finish_command() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "token_type": "Bearer",
                "scope": "patient/*.read",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-finish-command-paste");
    let config_path = temp_dir.join("config.json");
    let redirect_uri = "https://jessfraz.github.io/switchboard/mychart-callback/";
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            client_id: Some("client-123".into()),
            redirect_uri: Some(redirect_uri.into()),
            pending_oauth_state: Some("test-state".into()),
            pending_code_verifier: Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: AuthenticatedOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "finish",
        "mychart finish 'oauth-code';",
    ]);

    assert_eq!(output.status, "authenticated");
}

#[test]
fn exchange_code_uses_basic_auth_for_confidential_clients() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "token_type": "Bearer",
                "scope": "patient/*.read offline_access",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-exchange-confidential");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            client_id: Some("d45049c3-3441-40ef-ab4d-b9cd86a17225".into()),
            client_secret: Some("this-is-the-secret-2/7".into()),
            redirect_uri: Some("http://127.0.0.1:8910/callback".into()),
            pending_code_verifier: Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: AuthenticatedOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "exchange-code",
        "--code",
        "oauth-code",
    ]);

    assert_eq!(output.status, "authenticated");
    let requests = server.requests();
    let token_request = requests.get(1).expect("token request should be captured");
    assert_eq!(
        token_request.header("authorization"),
        Some("Basic ZDQ1MDQ5YzMtMzQ0MS00MGVmLWFiNGQtYjljZDg2YTE3MjI1OnRoaXMtaXMtdGhlLXNlY3JldC0yJTJGNw==")
    );
    assert!(token_request.form_value("client_secret").is_none());
    assert!(token_request.form_value("client_id").is_none());
}

#[test]
fn refresh_uses_basic_auth_for_confidential_clients() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "new-access-token",
                "refresh_token": "next-refresh-token",
                "token_type": "Bearer",
                "scope": "patient/*.read offline_access",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-refresh-confidential");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            client_id: Some("d45049c3-3441-40ef-ab4d-b9cd86a17225".into()),
            client_secret: Some("this-is-the-secret-2/7".into()),
            redirect_uri: Some("http://127.0.0.1:8910/callback".into()),
            refresh_token: Some("refresh-token".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: AuthenticatedOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "refresh",
    ]);

    assert_eq!(output.status, "refreshed");
    let requests = server.requests();
    let token_request = requests.get(1).expect("token request should be captured");
    assert_eq!(
        token_request.header("authorization"),
        Some("Basic ZDQ1MDQ5YzMtMzQ0MS00MGVmLWFiNGQtYjljZDg2YTE3MjI1OnRoaXMtaXMtdGhlLXNlY3JldC0yJTJGNw==")
    );
    assert!(token_request.form_value("client_secret").is_none());
    assert!(token_request.form_value("client_id").is_none());
}

#[test]
fn refresh_uses_dynamic_client_jwt_bearer_when_registered() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "fresh-access-token",
                "token_type": "Bearer",
                "scope": "patient/*.read offline_access",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
    ]);
    let key_material =
        crate::oauth::generate_dynamic_client_key_material().expect("dynamic key material should generate");
    let temp_dir = temp_dir("mychart-refresh-dynamic-client");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            current_account: Some("default".into()),
            accounts: BTreeMap::from([(
                "default".into(),
                crate::state::MyChartAccountState {
                    api_base_url: Some(server.base_url()),
                    client_id: Some("client-123".into()),
                    redirect_uri: Some("http://127.0.0.1:8910/callback".into()),
                    patient_id: Some("patient-123".into()),
                    dynamic_client: Some(crate::oauth::DynamicClientState {
                        client_id: "dynamic-client-123".into(),
                        private_key_pem: key_material.private_key_pem,
                        registration_endpoint: Some(format!("{}/oauth2/register", server.base_url())),
                        client_id_issued_at_epoch_seconds: Some(1_800_000_000u64),
                    }),
                    ..crate::state::MyChartAccountState::default()
                },
            )]),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: AuthenticatedOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "refresh",
    ]);

    assert_eq!(output.status, "refreshed");
    assert_eq!(output.renewal_method.as_deref(), Some("dynamic_client_jwt_bearer"));
    let requests = server.requests();
    assert_eq!(requests[0].header("epic-client-id"), Some("client-123"));
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/oauth2/token");
    assert_eq!(
        requests[1].form_value("grant_type").as_deref(),
        Some(crate::oauth::JWT_BEARER_GRANT_TYPE)
    );
    assert_eq!(
        requests[1].form_value("client_id").as_deref(),
        Some("dynamic-client-123")
    );
    assert!(requests[1].form_value("assertion").is_some());
}
