use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::{run, Cli};
use crate::args::GlobalArgs;
use crate::state::{MyChartState, ResolvedContext, StateStore};

#[test]
fn authorize_url_discovers_smart_endpoints_and_stores_pkce_state() {
    let server = TestServer::spawn(vec![ResponseSpec::json(
        200,
        capability_statement_json("http://placeholder", &[]),
        Vec::new(),
    )]);
    let temp_dir = temp_dir("mychart-authorize-url");
    let config_path = temp_dir.join("config.json");

    let output = run_command(&[
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

    assert_eq!(output["status"], "ok");
    assert_eq!(output["opened_browser"], false);
    assert!(output["authorize_url"]
        .as_str()
        .expect("authorize url should be string")
        .contains("response_type=code"));
    let authorize_url = reqwest::Url::parse(
        output["authorize_url"]
            .as_str()
            .expect("authorize url should be string"),
    )
    .expect("authorize url should parse");
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

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "exchange-code",
        "--code",
        "oauth-code",
    ]);

    assert_eq!(output["status"], "authenticated");

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

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "exchange-url",
        "https://jessfraz.github.io/switchboard/mychart-callback/?code=oauth-code&state=test-state",
    ]);

    assert_eq!(output["status"], "authenticated");

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

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "finish",
        "https://jessfraz.github.io/switchboard/mychart-callback/?code=oauth-code&state=test-state",
    ]);

    assert_eq!(output["status"], "authenticated");

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

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "finish",
        "oauth-code",
    ]);

    assert_eq!(output["status"], "authenticated");
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

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "finish",
        "mychart finish 'oauth-code';",
    ]);

    assert_eq!(output["status"], "authenticated");
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

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "exchange-code",
        "--code",
        "oauth-code",
    ]);

    assert_eq!(output["status"], "authenticated");
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

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "refresh",
    ]);

    assert_eq!(output["status"], "refreshed");
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

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "refresh",
    ]);

    assert_eq!(output["status"], "refreshed");
    assert_eq!(output["renewal_method"], "dynamic_client_jwt_bearer");
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

#[test]
fn auth_login_receives_loopback_callback_and_exchanges_code() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
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
    let temp_dir = temp_dir("mychart-auth-login");
    let config_path = temp_dir.join("config.json");
    let callback_listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let callback_port = callback_listener
        .local_addr()
        .expect("listener should have local addr")
        .port();
    drop(callback_listener);
    let redirect_uri = format!("http://127.0.0.1:{callback_port}/callback");
    let config_path_for_thread = config_path.clone();
    let server_base_url = format!("{}/", server.base_url());

    let handle = thread::spawn(move || {
        run_command(&[
            "mychart",
            "--config",
            config_path_for_thread.to_str().expect("config path should be utf-8"),
            "--base-url",
            &server_base_url,
            "--client-id",
            "client-123",
            "--redirect-uri",
            &redirect_uri,
            "--compact",
            "auth",
            "login",
            "--no-open",
            "--scope",
            "patient/*.read",
            "--state",
            "test-state",
            "--code-verifier",
            "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK",
        ])
    });

    let callback_sent = wait_for_callback_response(
        callback_port,
        "GET /callback?code=oauth-code&state=test-state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(callback_sent.contains("You can close this tab"));

    let output = handle.join().expect("auth login thread should finish");
    assert_eq!(output["status"], "authenticated");
    assert_eq!(output["patient_id"], "patient-123");

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(account.access_token.as_deref(), Some("access-token"));
    assert_eq!(account.patient_id.as_deref(), Some("patient-123"));
}

#[test]
fn top_level_login_with_hosted_redirect_starts_authorization_flow() {
    let server = TestServer::spawn(vec![ResponseSpec::json(
        200,
        capability_statement_json("http://placeholder", &[]),
        Vec::new(),
    )]);
    let temp_dir = temp_dir("mychart-easy-login-hosted");
    let config_path = temp_dir.join("config.json");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &format!("{}/", server.base_url()),
        "--client-id",
        "client-123",
        "--redirect-uri",
        "https://jessfraz.github.io/switchboard/mychart-callback/",
        "--compact",
        "login",
        "--no-open",
    ]);

    assert_eq!(output["status"], "authorization_pending");
    assert_eq!(output["opened_browser"], false);
    assert!(output["next_step"]
        .as_str()
        .expect("next step should be a string")
        .contains("mychart finish"));

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert!(account.pending_oauth_state.is_some());
    assert!(account.pending_code_verifier.is_some());
}

#[test]
fn top_level_login_with_hosted_redirect_can_finish_with_callback_url() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
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
    let temp_dir = temp_dir("mychart-easy-login-hosted-complete");
    let config_path = temp_dir.join("config.json");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &format!("{}/", server.base_url()),
        "--client-id",
        "client-123",
        "--redirect-uri",
        "https://jessfraz.github.io/switchboard/mychart-callback/",
        "--compact",
        "login",
        "--no-open",
        "--callback-url",
        "https://jessfraz.github.io/switchboard/mychart-callback/?code=oauth-code&state=test-state",
        "--state",
        "test-state",
        "--code-verifier",
        "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK",
    ]);

    assert_eq!(output["status"], "authenticated");
    assert_eq!(output["patient_id"], "patient-123");

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(account.access_token.as_deref(), Some("access-token"));
    assert_eq!(account.patient_id.as_deref(), Some("patient-123"));
}

#[test]
fn top_level_login_with_hosted_redirect_can_finish_with_bare_authorization_code() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
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
    let temp_dir = temp_dir("mychart-easy-login-hosted-complete-compact");
    let config_path = temp_dir.join("config.json");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &format!("{}/", server.base_url()),
        "--client-id",
        "client-123",
        "--redirect-uri",
        "https://jessfraz.github.io/switchboard/mychart-callback/",
        "--compact",
        "login",
        "--no-open",
        "--callback-url",
        "oauth-code",
        "--state",
        "test-state",
        "--code-verifier",
        "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK",
    ]);

    assert_eq!(output["status"], "authenticated");
    assert_eq!(output["patient_id"], "patient-123");
}

#[test]
fn auth_login_dynamic_client_registers_and_uses_jwt_bearer() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "initial-access-token",
                "token_type": "Bearer",
                "scope": "patient/*.read",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            201,
            json!({
                "client_id": "dynamic-client-123",
                "client_id_issued_at": 1_800_000_000u64,
                "token_endpoint_auth_method": "none",
                "grant_types": [crate::oauth::JWT_BEARER_GRANT_TYPE]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "persistent-access-token",
                "token_type": "Bearer",
                "scope": "patient/*.read",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-auth-login-dynamic-client");
    let config_path = temp_dir.join("config.json");
    let callback_listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let callback_port = callback_listener
        .local_addr()
        .expect("listener should have local addr")
        .port();
    drop(callback_listener);
    let redirect_uri = format!("http://127.0.0.1:{callback_port}/callback");
    let config_path_for_thread = config_path.clone();
    let server_base_url = format!("{}/", server.base_url());

    let handle = thread::spawn(move || {
        run_command(&[
            "mychart",
            "--config",
            config_path_for_thread.to_str().expect("config path should be utf-8"),
            "--base-url",
            &server_base_url,
            "--client-id",
            "client-123",
            "--redirect-uri",
            &redirect_uri,
            "--compact",
            "auth",
            "login",
            "--dynamic-client",
            "--no-open",
            "--scope",
            "patient/*.read",
            "--state",
            "test-state",
            "--code-verifier",
            "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK",
        ])
    });

    let callback_sent = wait_for_callback_response(
        callback_port,
        "GET /callback?code=oauth-code&state=test-state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(callback_sent.contains("You can close this tab"));

    let output = handle.join().expect("auth login thread should finish");
    assert_eq!(output["status"], "authenticated");
    assert_eq!(output["dynamic_client_id"], "dynamic-client-123");
    assert_eq!(output["renewal_method"], "dynamic_client_jwt_bearer");
    assert_eq!(output["patient_id"], "patient-123");

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(account.access_token.as_deref(), Some("persistent-access-token"));
    assert_eq!(
        account
            .dynamic_client
            .as_ref()
            .map(|dynamic_client| dynamic_client.client_id.as_str()),
        Some("dynamic-client-123")
    );

    let requests = server.requests();
    assert_eq!(requests[0].header("epic-client-id"), Some("client-123"));
    assert_eq!(requests[2].method, "POST");
    assert_eq!(requests[2].path, "/oauth2/token");
    assert_eq!(requests[2].form_value("client_id").as_deref(), Some("client-123"));
    assert!(requests[2].header("authorization").is_none());
    assert_eq!(requests[4].method, "POST");
    assert_eq!(requests[4].path, "/oauth2/register");
    assert_eq!(requests[4].header("authorization"), Some("Bearer initial-access-token"));
    let registration_body: Value = requests[4].json_body();
    assert_eq!(registration_body["software_id"], "client-123");
    assert_eq!(registration_body["jwks"]["keys"][0]["kty"], "RSA");
    assert_eq!(requests[6].method, "POST");
    assert_eq!(requests[6].path, "/oauth2/token");
    assert_eq!(
        requests[6].form_value("grant_type").as_deref(),
        Some(crate::oauth::JWT_BEARER_GRANT_TYPE)
    );
    assert_eq!(
        requests[6].form_value("client_id").as_deref(),
        Some("dynamic-client-123")
    );
    assert!(requests[6].form_value("assertion").is_some());
}

#[test]
fn api_resources_lists_patient_facing_resource_metadata() {
    let server = TestServer::spawn(vec![ResponseSpec::json(
        200,
        capability_statement_json(
            "http://placeholder",
            &[
                resource_capability("Patient", &["read", "search-type"]),
                resource_capability("Observation", &["create", "read", "search-type", "update"]),
            ],
        ),
        Vec::new(),
    )]);
    let temp_dir = temp_dir("mychart-resources");
    let config_path = temp_dir.join("config.json");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &server.base_url(),
        "--compact",
        "api",
        "resources",
    ]);

    assert_eq!(output["resource_count"], 2);
    assert_eq!(output["resources"][0]["resource"], "Observation");
    assert_eq!(output["resources"][1]["resource"], "Patient");
}

#[test]
fn api_resource_auto_refreshes_before_fetch_when_token_is_expired() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
        ResponseSpec::json(
            200,
            json!({
                "access_token": "fresh-access-token",
                "refresh_token": "next-refresh-token",
                "token_type": "Bearer",
                "scope": "patient/*.read",
                "patient": "patient-123",
                "expires_in": 3600
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("Patient", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Patient",
                "id": "patient-123"
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-api-auto-refresh");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            client_id: Some("client-123".into()),
            redirect_uri: Some("http://127.0.0.1:8910/callback".into()),
            access_token: Some("expired-access-token".into()),
            refresh_token: Some("refresh-token".into()),
            expires_at_epoch_seconds: Some(1),
            patient_id: Some("patient-123".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "api",
        "patient",
        "get",
        "patient-123",
    ]);

    assert_eq!(output["status"], "ok");
    assert_eq!(output["resource"], "Patient");
    assert_eq!(output["body"]["id"], "patient-123");

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(account.access_token.as_deref(), Some("fresh-access-token"));
    assert_eq!(account.refresh_token.as_deref(), Some("next-refresh-token"));

    let requests = server.requests();
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/oauth2/token");
    assert_eq!(requests[3].method, "GET");
    assert_eq!(requests[3].path, "/Patient/patient-123");
    assert_eq!(requests[3].header("authorization"), Some("Bearer fresh-access-token"));
}

#[test]
fn api_search_maps_dynamic_flags_to_fhir_query_params() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("Appointment", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{"resource": {"resourceType": "Appointment", "id": "appt-1"}}]
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-search");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "api",
        "appointment",
        "search",
        "--patient",
        "patient-123",
        "--date",
        "ge2026-03-01",
        "--count",
        "1",
    ]);

    assert_eq!(output["status"], "ok");
    let requests = server.requests();
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].path, "/Appointment");
    assert_eq!(requests[1].query_value("patient"), Some("patient-123"));
    assert_eq!(requests[1].query_value("date"), Some("ge2026-03-01"));
    assert_eq!(requests[1].query_value("_count"), Some("1"));
    assert_eq!(requests[1].header("authorization"), Some("Bearer access-token"));
}

#[test]
fn api_write_operations_are_rejected() {
    let server = TestServer::spawn(vec![ResponseSpec::json(
        200,
        capability_statement_json(
            "http://placeholder",
            &[resource_capability("Observation", &["read", "search-type"])],
        ),
        Vec::new(),
    )]);
    let temp_dir = temp_dir("mychart-api-write-rejected");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let error = run_command_error(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "api",
        "observation",
        "create",
        "--body",
        "{\"valueString\":\"nope\"}",
    ]);

    assert!(error.contains("unsupported resource operation"));
    assert!(error.contains("use get/read or search"));
}

#[test]
fn portal_login_still_works_under_portal_namespace() {
    let server = TestServer::spawn(vec![
        ResponseSpec::html(
            200,
            login_page_html("csrf-token"),
            vec![("Set-Cookie".into(), "MyChartAffinity=affinity-cookie; Path=/".into())],
        ),
        ResponseSpec::empty(
            302,
            vec![
                ("Location".into(), "/inside.asp".into()),
                ("Set-Cookie".into(), "MyChartSession=session-cookie; Path=/".into()),
            ],
        ),
        ResponseSpec::html(200, app_page_html("Dashboard"), Vec::new()),
    ]);
    let temp_dir = temp_dir("mychart-portal-login");
    let config_path = temp_dir.join("config.json");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--portal-base-url",
        &server.base_url(),
        "--compact",
        "portal",
        "auth",
        "login-password",
        "--username",
        "person@example.com",
        "--password",
        "super-secret",
    ]);

    assert_eq!(output["status"], "authenticated");

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(account.portal_base_url.as_deref(), Some(server.base_url().as_str()));
    assert_eq!(account.cookies.len(), 2);
}

#[test]
fn portal_request_post_subcommand_is_removed() {
    let error = Cli::try_parse_from([
        "mychart",
        "portal",
        "request",
        "post",
        "/inside.asp",
        "--form",
        "foo=bar",
    ])
    .expect_err("portal request post should fail to parse")
    .to_string();

    assert!(error.contains("unrecognized subcommand"));
    assert!(error.contains("post"));
}

#[test]
fn connect_resolve_uses_cached_brand_catalog() {
    let temp_dir = temp_dir("mychart-connect-resolve");
    let config_path = temp_dir.join("config.json");
    write_brands_cache(
        &temp_dir.join("brands-cache.json"),
        json!({
            "source_url": "https://open.epic.com/Endpoints/Brands",
            "fetched_at_epoch_seconds": 1_800_000_000u64,
            "bundle_last_updated": "2026-03-27T03:00:03Z",
            "brands": [{
                "brand_id": "brand-1",
                "brand_name": "UCLA Medical Center",
                "account_slug": "ucla-medical-center",
                "fhir_base_url": "https://arrprox.mednet.ucla.edu/FHIRPRD/api/FHIR/R4",
                "endpoint_id": "endpoint-1",
                "endpoint_name": "UCLA Medical Center",
                "managing_organization_id": "341",
                "managing_organization_name": "UCLA Health",
                "state": "CA",
                "country": "USA",
                "facilities": [{
                    "name": "UCLA Santa Monica",
                    "city": "Santa Monica",
                    "state": "CA"
                }]
            }]
        }),
    );

    let mut context = resolved_context(&config_path);
    let output = crate::commands::connect::run_resolve_output(vec!["santa".into(), "monica".into()], &mut context)
        .expect("connect resolve should succeed");

    assert_eq!(output.status, "connected");
    assert_eq!(output.selected_account.as_deref(), Some("ucla-medical-center"));

    let state = StateStore::new(config_path).load().expect("state should load");
    assert_eq!(state.current_account.as_deref(), Some("ucla-medical-center"));
    let account = state
        .accounts
        .get("ucla-medical-center")
        .expect("named account should be stored");
    assert_eq!(
        account.api_base_url.as_deref(),
        Some("https://arrprox.mednet.ucla.edu/FHIRPRD/api/FHIR/R4")
    );
    assert_eq!(
        account
            .discovery
            .as_ref()
            .and_then(|discovery| discovery.brand_name.as_deref()),
        Some("UCLA Medical Center")
    );
}

#[test]
fn connect_resolve_builtin_ucla_preset_prefills_defaults() {
    let temp_dir = temp_dir("mychart-connect-ucla-preset");
    let config_path = temp_dir.join("config.json");

    let mut context = resolved_context(&config_path);
    let output = crate::commands::connect::run_resolve_output(vec!["ucla".into()], &mut context)
        .expect("connect resolve should succeed");

    assert_eq!(output.status, "connected");
    assert_eq!(output.selected_account.as_deref(), Some("ucla"));

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state.accounts.get("ucla").expect("ucla account should be stored");
    assert_eq!(
        account.api_base_url.as_deref(),
        Some(crate::presets::UCLA_FHIR_BASE_URL)
    );
    assert_eq!(
        account.portal_base_url.as_deref(),
        Some(crate::presets::UCLA_PORTAL_BASE_URL)
    );
    assert_eq!(
        account.client_id.as_deref(),
        Some(crate::presets::UCLA_PRODUCTION_CLIENT_ID)
    );
    assert_eq!(
        account.redirect_uri.as_deref(),
        Some(crate::presets::PAGES_REDIRECT_URI)
    );
    assert!(account.client_secret.is_none());
}

#[test]
fn connect_resolve_builtin_sandbox_alias_uses_canonical_account_and_clears_secret() {
    let temp_dir = temp_dir("mychart-connect-sandbox-preset");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            current_account: Some("epic-sandbox".into()),
            accounts: BTreeMap::from([(
                "epic-sandbox".into(),
                crate::state::MyChartAccountState {
                    api_base_url: Some(crate::presets::EPIC_SANDBOX_FHIR_BASE_URL.into()),
                    client_id: Some(crate::presets::EPIC_SANDBOX_CLIENT_ID.into()),
                    client_secret: Some("stale-secret".into()),
                    redirect_uri: Some(crate::presets::LOOPBACK_REDIRECT_URI.into()),
                    access_token: Some("still-good-token".into()),
                    ..crate::state::MyChartAccountState::default()
                },
            )]),
            ..MyChartState::default()
        })
        .expect("state should save");

    let mut context = resolved_context(&config_path);
    let output = crate::commands::connect::run_resolve_output(vec!["sandbox".into()], &mut context)
        .expect("connect resolve should succeed");

    assert_eq!(output.status, "connected");
    assert_eq!(output.selected_account.as_deref(), Some("epic-sandbox"));

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("epic-sandbox")
        .expect("epic-sandbox account should be stored");
    assert!(account.client_secret.is_none());
    assert_eq!(account.access_token.as_deref(), Some("still-good-token"));
}

#[test]
fn connect_resolve_reports_ambiguity_for_broad_queries() {
    let temp_dir = temp_dir("mychart-connect-ambiguous");
    let config_path = temp_dir.join("config.json");
    write_brands_cache(
        &temp_dir.join("brands-cache.json"),
        json!({
            "source_url": "https://open.epic.com/Endpoints/Brands",
            "fetched_at_epoch_seconds": 1_800_000_000u64,
            "bundle_last_updated": "2026-03-27T03:00:03Z",
            "brands": [
                {
                    "brand_id": "brand-1",
                    "brand_name": "Acme Clinics East",
                    "account_slug": "acme-clinics-east",
                    "fhir_base_url": "https://east.example.org/FHIR/R4",
                    "endpoint_id": "endpoint-1",
                    "endpoint_name": "Acme Clinics East",
                    "managing_organization_id": "east",
                    "managing_organization_name": "Acme Health",
                    "state": "CA",
                    "country": "US",
                    "facilities": []
                },
                {
                    "brand_id": "brand-2",
                    "brand_name": "Acme Clinics West",
                    "account_slug": "acme-clinics-west",
                    "fhir_base_url": "https://west.example.org/FHIR/R4",
                    "endpoint_id": "endpoint-2",
                    "endpoint_name": "Acme Clinics West",
                    "managing_organization_id": "west",
                    "managing_organization_name": "Acme Health",
                    "state": null,
                    "country": null,
                    "facilities": []
                }
            ]
        }),
    );

    let mut context = resolved_context(&config_path);
    let output = crate::commands::connect::run_resolve_output(vec!["acme".into(), "clinics".into()], &mut context)
        .expect("connect resolve should succeed");

    assert_eq!(output.status, "ambiguous");
    assert_eq!(output.matches.len(), 2);
}

#[test]
fn connect_add_can_clear_a_stored_client_secret() {
    let temp_dir = temp_dir("mychart-connect-clear-client-secret");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            current_account: Some("epic-sandbox".into()),
            accounts: BTreeMap::from([(
                "epic-sandbox".into(),
                crate::state::MyChartAccountState {
                    api_base_url: Some("https://fhir.epic.com/interconnect-fhir-oauth/api/FHIR/R4".into()),
                    client_id: Some("client-123".into()),
                    client_secret: Some("stale-secret".into()),
                    redirect_uri: Some("http://127.0.0.1:8910/callback".into()),
                    ..crate::state::MyChartAccountState::default()
                },
            )]),
            ..MyChartState::default()
        })
        .expect("state should save");

    let mut context = resolved_context(&config_path);
    let output = crate::commands::connect::run_add_output(
        crate::commands::connect::ConnectAddArgs {
            name: "epic-sandbox".into(),
            base_url: "https://fhir.epic.com/interconnect-fhir-oauth/api/FHIR/R4".into(),
            portal_base_url: None,
            client_id: None,
            client_secret: None,
            clear_client_secret: true,
            redirect_uri: None,
            no_use: false,
        },
        &mut context,
    )
    .expect("connect add should succeed");

    assert_eq!(output.status, "connected");
    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("epic-sandbox")
        .expect("epic-sandbox account should be stored");
    assert!(account.client_secret.is_none());
}

#[test]
fn timeline_skips_resources_not_granted_by_token_scope() {
    #[derive(Debug, serde::Deserialize)]
    struct TimelineOutput {
        status: String,
        events: Vec<TimelineEvent>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct TimelineEvent {
        resource_type: String,
    }

    let server = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[
                    resource_capability("Appointment", &["read", "search-type"]),
                    resource_capability("Encounter", &["read", "search-type"]),
                    resource_capability("Observation", &["read", "search-type"]),
                    resource_capability("DiagnosticReport", &["read", "search-type"]),
                    resource_capability("MedicationRequest", &["read", "search-type"]),
                    resource_capability("DocumentReference", &["read", "search-type"]),
                    resource_capability("ExplanationOfBenefit", &["read", "search-type"]),
                ],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "Encounter",
                        "id": "enc-1",
                        "period": {"start": "2100-01-01"},
                        "status": "finished",
                        "type": [{"text": "Office visit"}]
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "Observation",
                        "id": "obs-1",
                        "effectiveDateTime": "2100-01-02T08:00:00Z",
                        "code": {"text": "Ferritin"},
                        "valueQuantity": {"value": 14.0, "unit": "ng/mL"}
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "DiagnosticReport",
                        "id": "report-1",
                        "issued": "2100-01-03T08:00:00Z",
                        "code": {"text": "CBC"},
                        "conclusion": "Normal"
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "MedicationRequest",
                        "id": "med-1",
                        "authoredOn": "2100-01-04",
                        "status": "active",
                        "medicationCodeableConcept": {"text": "Topiramate"}
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "DocumentReference",
                        "id": "note-1",
                        "date": "2100-01-05",
                        "type": {"text": "Progress Note"},
                        "description": "Neurology note"
                    }
                }]
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-timeline-scope-filter");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            scope: Some(
                "openid patient/Encounter.read patient/Observation.read patient/DiagnosticReport.read \
                     patient/MedicationRequest.read patient/DocumentReference.read"
                    .into(),
            ),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: TimelineOutput = serde_json::from_value(run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "timeline",
        "--limit",
        "25",
    ]))
    .expect("timeline output should deserialize");

    assert_eq!(output.status, "ok");
    assert_eq!(output.events.len(), 5);
    assert!(output
        .events
        .iter()
        .all(|event| event.resource_type != "Appointment" && event.resource_type != "ExplanationOfBenefit"));

    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert!(!requests.iter().any(|request| request.path == "/Appointment"));
    assert!(!requests.iter().any(|request| request.path == "/ExplanationOfBenefit"));
}

#[test]
fn labs_shorthand_returns_trend_series() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("Observation", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [
                    {
                        "resource": {
                            "resourceType": "Observation",
                            "id": "obs-1",
                            "effectiveDateTime": "2026-03-01T00:00:00Z",
                            "code": {"text": "Hemoglobin A1c"},
                            "valueQuantity": {"value": 6.3, "unit": "%"}
                        }
                    },
                    {
                        "resource": {
                            "resourceType": "Observation",
                            "id": "obs-2",
                            "effectiveDateTime": "2026-02-01T00:00:00Z",
                            "code": {"text": "Hemoglobin A1c"},
                            "valueQuantity": {"value": 6.1, "unit": "%"}
                        }
                    }
                ]
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-labs-trend");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "labs",
        "a1c",
        "--spark",
    ]);

    assert_eq!(output["status"], "ok");
    assert_eq!(output["series"][0]["label"], "Hemoglobin A1c");
    assert_eq!(output["series"][0]["point_count"], 2);
    assert!(!output["series"][0]["spark"]
        .as_str()
        .expect("sparkline should be present")
        .is_empty());
}

#[test]
fn appointments_upcoming_filters_past_and_cancelled_entries() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("Appointment", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [
                    {
                        "resource": {
                            "resourceType": "Appointment",
                            "id": "appt-future",
                            "status": "booked",
                            "start": "2100-01-01T10:00:00Z",
                            "description": "Future visit"
                        }
                    },
                    {
                        "resource": {
                            "resourceType": "Appointment",
                            "id": "appt-past",
                            "status": "booked",
                            "start": "2000-01-01T10:00:00Z",
                            "description": "Past visit"
                        }
                    },
                    {
                        "resource": {
                            "resourceType": "Appointment",
                            "id": "appt-cancelled",
                            "status": "cancelled",
                            "start": "2100-01-02T10:00:00Z",
                            "description": "Cancelled visit"
                        }
                    }
                ]
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-appointments-upcoming");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "appointments",
        "upcoming",
    ]);

    assert_eq!(output["status"], "ok");
    assert_eq!(output["appointments"].as_array().map(Vec::len), Some(1));
    assert_eq!(output["appointments"][0]["id"], "appt-future");
}

#[test]
fn appointments_upcoming_reports_missing_scope_warning() {
    let server = TestServer::spawn(vec![ResponseSpec::json(
        200,
        capability_statement_json(
            "http://placeholder",
            &[resource_capability("Appointment", &["read", "search-type"])],
        ),
        Vec::new(),
    )]);
    let temp_dir = temp_dir("mychart-appointments-scope-warning");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            scope: Some("openid patient/Observation.read patient/DocumentReference.read".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let context = resolved_context(&config_path);
    let output = crate::commands::appointments::run_upcoming_output(
        crate::commands::appointments::AppointmentsUpcomingArgs {
            patient: None,
            all_accounts: false,
            since: None,
            limit: 10,
            all_pages: false,
        },
        &context,
    )
    .expect("upcoming appointments should still render");

    assert_eq!(output.status, "ok");
    assert!(output.appointments.is_empty());
    assert_eq!(output.warnings.len(), 1);
    assert!(output.warnings[0].contains("patient/Appointment.read"));
}

#[test]
fn appointments_upcoming_reports_missing_capability_warning() {
    let server = TestServer::spawn(vec![ResponseSpec::json(
        200,
        capability_statement_json(
            "http://placeholder",
            &[resource_capability("Observation", &["read", "search-type"])],
        ),
        Vec::new(),
    )]);
    let temp_dir = temp_dir("mychart-appointments-capability-warning");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            scope: Some("openid patient/Observation.read".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let context = resolved_context(&config_path);
    let output = crate::commands::appointments::run_upcoming_output(
        crate::commands::appointments::AppointmentsUpcomingArgs {
            patient: None,
            all_accounts: false,
            since: None,
            limit: 10,
            all_pages: false,
        },
        &context,
    )
    .expect("upcoming appointments should still render");

    assert_eq!(output.status, "ok");
    assert!(output.appointments.is_empty());
    assert_eq!(output.warnings.len(), 1);
    assert!(output.warnings[0].contains("Appointment"));
    assert!(output.warnings[0].contains("not exposed"));
}

#[test]
fn appointments_find_filters_by_text_and_future_window() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[json!({
                    "type": "Appointment",
                    "interaction": [
                        {"code": "read"},
                        {"code": "search-type"}
                    ],
                    "searchParam": [
                        {"name": "patient", "type": "reference"},
                        {"name": "date", "type": "date"}
                    ]
                })],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [
                    {
                        "resource": {
                            "resourceType": "Appointment",
                            "id": "appt-derm-soon",
                            "status": "booked",
                            "start": "2100-01-10T10:00:00Z",
                            "description": "Dermatology consult",
                            "specialty": [{"text": "Dermatology"}]
                        }
                    },
                    {
                        "resource": {
                            "resourceType": "Appointment",
                            "id": "appt-cardio",
                            "status": "booked",
                            "start": "2100-01-11T10:00:00Z",
                            "description": "Cardiology follow-up",
                            "specialty": [{"text": "Cardiology"}]
                        }
                    },
                    {
                        "resource": {
                            "resourceType": "Appointment",
                            "id": "appt-derm-late",
                            "status": "booked",
                            "start": "2100-03-20T10:00:00Z",
                            "description": "Dermatology follow-up",
                            "specialty": [{"text": "Dermatology"}]
                        }
                    }
                ]
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-appointments-find");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "appointments",
        "find",
        "derm",
        "--next",
        "2100-02-01",
    ]);

    assert_eq!(output["status"], "ok");
    assert_eq!(output["query"], "derm");
    assert_eq!(output["appointments"].as_array().map(Vec::len), Some(1));
    assert_eq!(output["appointments"][0]["id"], "appt-derm-soon");
    let requests = server.requests();
    assert!(requests[1]
        .query_value("date")
        .is_some_and(|value| value.starts_with("ge")));
}

#[test]
fn notes_get_fetches_binary_body_text() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("DocumentReference", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "DocumentReference",
                "id": "note-1",
                "date": "2099-12-02",
                "type": {"text": "Progress Note"},
                "description": "Neurology note",
                "author": [{"display": "Dr. Headache"}],
                "content": [{
                    "attachment": {
                        "title": "Note body",
                        "contentType": "text/plain",
                        "url": "Binary/note-1-body"
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Binary",
                "contentType": "text/plain",
                "data": "UGF0aWVudCByZXBvcnRzIG1pZ3JhaW5lIGltcHJvdmVtZW50Lg=="
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-notes-get");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "notes",
        "get",
        "note-1",
    ]);

    assert_eq!(output["status"], "ok");
    assert_eq!(output["note"]["body_text"], "Patient reports migraine improvement.");
    assert_eq!(
        output["note"]["content"][0]["body_text"],
        "Patient reports migraine improvement."
    );
    let requests = server.requests();
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].path, "/DocumentReference/note-1");
    assert_eq!(requests[2].method, "GET");
    assert_eq!(requests[2].path, "/Binary/note-1-body");
}

#[test]
fn notes_search_matches_note_body_text() {
    #[derive(Debug, serde::Deserialize)]
    struct NotesSearchOutput {
        status: String,
        notes: Vec<NotesSearchMatch>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct NotesSearchMatch {
        id: Option<String>,
        match_source: Option<String>,
        body_excerpt: Option<String>,
    }

    let server = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("DocumentReference", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "DocumentReference",
                        "id": "note-1",
                        "date": "2100-01-01T00:00:00Z",
                        "type": {"text": "Progress Note"},
                        "description": "Routine follow-up",
                        "content": [{
                            "attachment": {
                                "contentType": "text/plain",
                                "title": "Visit note",
                                "url": "/Binary/note-1-body"
                            }
                        }]
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Binary",
                "contentType": "text/plain",
                "data": crate::base64_encode(b"Patient reports migraine improvement after medication change.")
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-notes-search-body");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            scope: Some("openid patient/DocumentReference.read patient/Binary.read".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output: NotesSearchOutput = serde_json::from_value(run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "notes",
        "search",
        "--query",
        "migraine",
    ]))
    .expect("notes search output should deserialize");

    assert_eq!(output.status, "ok");
    assert_eq!(output.notes.len(), 1);
    assert_eq!(output.notes[0].id.as_deref(), Some("note-1"));
    assert_eq!(output.notes[0].match_source.as_deref(), Some("body"));
    assert!(output.notes[0]
        .body_excerpt
        .as_deref()
        .is_some_and(|excerpt| excerpt.contains("migraine")));
}

#[test]
fn meds_reconcile_can_merge_all_provider_accounts() {
    let server_a = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("MedicationRequest", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "MedicationRequest",
                        "id": "med-a",
                        "status": "active",
                        "intent": "order",
                        "authoredOn": "2100-01-01",
                        "medicationCodeableConcept": {"text": "Aspirin"},
                        "requester": {"display": "Dr. A"}
                    }
                }]
            }),
            Vec::new(),
        ),
    ]);
    let server_b = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("MedicationRequest", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "MedicationRequest",
                        "id": "med-b",
                        "status": "active",
                        "intent": "order",
                        "authoredOn": "2100-01-02",
                        "medicationCodeableConcept": {"text": "Aspirin"},
                        "requester": {"display": "Dr. B"}
                    }
                }]
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-meds-all-providers");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            current_account: Some("ucla".into()),
            accounts: BTreeMap::from([
                (
                    "ucla".into(),
                    crate::state::MyChartAccountState {
                        api_base_url: Some(server_a.base_url()),
                        access_token: Some("access-a".into()),
                        patient_id: Some("patient-a".into()),
                        discovery: Some(crate::state::AccountDiscoveryState {
                            brand_name: Some("UCLA Medical Center".into()),
                            ..crate::state::AccountDiscoveryState::default()
                        }),
                        ..crate::state::MyChartAccountState::default()
                    },
                ),
                (
                    "cedars".into(),
                    crate::state::MyChartAccountState {
                        api_base_url: Some(server_b.base_url()),
                        access_token: Some("access-b".into()),
                        patient_id: Some("patient-b".into()),
                        discovery: Some(crate::state::AccountDiscoveryState {
                            brand_name: Some("Cedars-Sinai".into()),
                            ..crate::state::AccountDiscoveryState::default()
                        }),
                        ..crate::state::MyChartAccountState::default()
                    },
                ),
                (
                    "stale".into(),
                    crate::state::MyChartAccountState {
                        api_base_url: Some("https://example.invalid/FHIR/R4".into()),
                        ..crate::state::MyChartAccountState::default()
                    },
                ),
            ]),
            ..MyChartState::default()
        })
        .expect("state should save");

    let context = resolved_context(&config_path);
    let output = crate::commands::meds::run_reconcile_output(
        crate::commands::meds::MedsReconcileArgs {
            patient: None,
            all_accounts: true,
            since: None,
            limit: 100,
            all_pages: false,
        },
        &context,
    )
    .expect("med reconciliation should succeed");

    assert_eq!(output.status, "ok");
    assert_eq!(output.patient_id, None);
    assert_eq!(output.accounts_used.len(), 2);
    assert_eq!(output.accounts_skipped.len(), 1);
    assert_eq!(output.duplicate_name_candidates.len(), 1);
    assert_eq!(output.duplicate_name_candidates[0].name, "aspirin");
    assert_eq!(output.duplicate_name_candidates[0].count, 2);
    let accounts = output
        .medications
        .iter()
        .map(|entry| entry.account.as_str())
        .collect::<Vec<_>>();
    assert!(accounts.contains(&"ucla"));
    assert!(accounts.contains(&"cedars"));
}

#[test]
fn appointments_upcoming_can_merge_all_provider_accounts() {
    let server_a = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("Appointment", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "Appointment",
                        "id": "appt-a",
                        "status": "booked",
                        "start": "2100-01-01T10:00:00Z",
                        "description": "UCLA visit"
                    }
                }]
            }),
            Vec::new(),
        ),
    ]);
    let server_b = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("Appointment", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "Appointment",
                        "id": "appt-b",
                        "status": "booked",
                        "start": "2100-01-02T10:00:00Z",
                        "description": "Cedars visit"
                    }
                }]
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-appointments-all-providers");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            current_account: Some("ucla".into()),
            accounts: BTreeMap::from([
                (
                    "ucla".into(),
                    crate::state::MyChartAccountState {
                        api_base_url: Some(server_a.base_url()),
                        access_token: Some("access-a".into()),
                        patient_id: Some("patient-a".into()),
                        discovery: Some(crate::state::AccountDiscoveryState {
                            brand_name: Some("UCLA Medical Center".into()),
                            ..crate::state::AccountDiscoveryState::default()
                        }),
                        ..crate::state::MyChartAccountState::default()
                    },
                ),
                (
                    "cedars".into(),
                    crate::state::MyChartAccountState {
                        api_base_url: Some(server_b.base_url()),
                        access_token: Some("access-b".into()),
                        patient_id: Some("patient-b".into()),
                        discovery: Some(crate::state::AccountDiscoveryState {
                            brand_name: Some("Cedars-Sinai".into()),
                            ..crate::state::AccountDiscoveryState::default()
                        }),
                        ..crate::state::MyChartAccountState::default()
                    },
                ),
            ]),
            ..MyChartState::default()
        })
        .expect("state should save");

    let context = resolved_context(&config_path);
    let output = crate::commands::appointments::run_upcoming_output(
        crate::commands::appointments::AppointmentsUpcomingArgs {
            patient: None,
            all_accounts: true,
            since: None,
            limit: 10,
            all_pages: false,
        },
        &context,
    )
    .expect("upcoming appointments should succeed");

    assert_eq!(output.status, "ok");
    assert_eq!(output.patient_id, None);
    assert_eq!(output.accounts_used.len(), 2);
    assert_eq!(output.appointments.len(), 2);
    assert_eq!(output.appointments[0].account, "ucla");
    assert_eq!(output.appointments[1].account, "cedars");
}

#[test]
fn claims_audit_flags_duplicate_and_problem_claims() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[resource_capability("ExplanationOfBenefit", &["read", "search-type"])],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [
                    {
                        "resource": {
                            "resourceType": "ExplanationOfBenefit",
                            "id": "claim-1",
                            "status": "active",
                            "outcome": "complete",
                            "use": "claim",
                            "billablePeriod": {"start": "2100-01-01"},
                            "provider": {"display": "UCLA Health"},
                            "total": [{"amount": {"value": 250.0, "currency": "USD"}}],
                            "item": [{"productOrService": {"text": "MRI Brain"}}]
                        }
                    },
                    {
                        "resource": {
                            "resourceType": "ExplanationOfBenefit",
                            "id": "claim-2",
                            "status": "active",
                            "outcome": "complete",
                            "use": "claim",
                            "billablePeriod": {"start": "2100-01-01"},
                            "provider": {"display": "UCLA Health"},
                            "total": [{"amount": {"value": 250.0, "currency": "USD"}}],
                            "item": [{"productOrService": {"text": "MRI Brain"}}]
                        }
                    },
                    {
                        "resource": {
                            "resourceType": "ExplanationOfBenefit",
                            "id": "claim-3",
                            "status": "active",
                            "outcome": "partial",
                            "use": "claim",
                            "billablePeriod": {"start": "2100-01-03"},
                            "provider": {"display": "UCLA Health"},
                            "total": [{"amount": {"value": 99.0, "currency": "USD"}}],
                            "item": [{"productOrService": {"text": "Lab Panel"}}]
                        }
                    }
                ]
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-claims-audit");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "claims",
        "audit",
    ]);

    assert_eq!(output["status"], "ok");
    assert_eq!(
        output["duplicate_charge_candidates"]
            .as_array()
            .expect("duplicate groups should be an array")
            .len(),
        1
    );
    assert_eq!(
        output["denied_or_problematic_claims"]
            .as_array()
            .expect("problem claims should be an array")
            .len(),
        1
    );
}

#[test]
fn pack_doctor_assembles_visit_packet() {
    let server = TestServer::spawn(vec![
        ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[
                    resource_capability("Appointment", &["read", "search-type"]),
                    resource_capability("Observation", &["read", "search-type"]),
                    resource_capability("MedicationRequest", &["read", "search-type"]),
                    resource_capability("Condition", &["read", "search-type"]),
                    resource_capability("Encounter", &["read", "search-type"]),
                    resource_capability("DocumentReference", &["read", "search-type"]),
                ],
            ),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "Appointment",
                        "id": "appt-1",
                        "status": "booked",
                        "start": "2100-01-01T10:00:00Z",
                        "description": "Neurology follow-up"
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "Observation",
                        "id": "obs-1",
                        "effectiveDateTime": "2100-01-01T08:00:00Z",
                        "code": {"text": "Ferritin"},
                        "valueQuantity": {"value": 14.0, "unit": "ng/mL"},
                        "interpretation": [{"text": "low"}]
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "MedicationRequest",
                        "id": "med-1",
                        "status": "active",
                        "authoredOn": "2099-12-15",
                        "medicationCodeableConcept": {"text": "Topiramate"},
                        "dosageInstruction": [{"text": "Take once daily"}]
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "Condition",
                        "id": "cond-1",
                        "recordedDate": "2099-12-10",
                        "clinicalStatus": {"text": "active"},
                        "verificationStatus": {"text": "confirmed"},
                        "code": {"text": "Migraine"}
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "Encounter",
                        "id": "enc-1",
                        "period": {"start": "2099-12-01"},
                        "status": "finished",
                        "class": {"display": "outpatient"},
                        "type": [{"text": "Office visit"}]
                    }
                }]
            }),
            Vec::new(),
        ),
        ResponseSpec::json(
            200,
            json!({
                "resourceType": "Bundle",
                "entry": [{
                    "resource": {
                        "resourceType": "DocumentReference",
                        "id": "note-1",
                        "date": "2099-12-02",
                        "type": {"text": "Progress Note"},
                        "description": "Neurology note",
                        "author": [{"display": "Dr. Headache"}]
                    }
                }]
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-pack-doctor");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&MyChartState {
            api_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            patient_id: Some("patient-123".into()),
            ..MyChartState::default()
        })
        .expect("state should save");

    let output = run_command(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "pack",
        "doctor",
    ]);

    assert_eq!(output["status"], "ok");
    assert_eq!(output["upcoming_appointment"]["description"], "Neurology follow-up");
    assert_eq!(output["recent_labs"][0]["label"], "Ferritin");
    assert_eq!(output["active_medications"][0]["name"], "Topiramate");
    assert_eq!(output["active_conditions"][0]["condition"], "Migraine");
    assert!(!output["suggested_questions"]
        .as_array()
        .expect("suggested questions should be an array")
        .is_empty());
}

#[test]
fn sha256_matches_known_vector() {
    assert_eq!(
        hex(&super::sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

fn run_command(args: &[&str]) -> Value {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let (value, _) = run(cli).unwrap_or_else(|error| panic!("{}", crate::output::render_cli_error(&error, compact)));
    value
}

fn run_command_error(args: &[&str]) -> String {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let error = run(cli).expect_err("CLI should fail");
    crate::output::render_cli_error(&error, compact)
}

fn resolved_context(config_path: &std::path::Path) -> crate::state::ResolvedContext {
    ResolvedContext::from_global(&GlobalArgs {
        config: Some(config_path.to_path_buf()),
        account: None,
        base_url: None,
        portal_base_url: None,
        client_id: None,
        client_secret: None,
        redirect_uri: None,
        access_token: None,
        refresh_token: None,
        username: None,
        debug_auth: false,
        compact: true,
    })
    .expect("context should resolve")
}

fn temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nanos:x}"));
    fs::create_dir_all(&path).expect("temp dir should exist");
    path
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
}

fn wait_for_callback_response(port: u16, request: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(mut stream) => {
                stream
                    .write_all(request.as_bytes())
                    .expect("callback request should write");
                return read_response_text(&mut stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                if std::time::Instant::now() >= deadline {
                    panic!("callback listener did not start in time");
                }
                thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) => panic!("failed to connect to callback listener: {error}"),
        }
    }
}

fn write_brands_cache(path: &std::path::Path, value: Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("brands cache parent should exist");
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("brands cache should serialize"),
    )
    .expect("brands cache should write");
}

fn capability_statement_json(base_url: &str, resources: &[Value]) -> Value {
    json!({
        "resourceType": "CapabilityStatement",
        "fhirVersion": "4.0.1",
        "software": {
            "name": "Epic",
            "version": "February 2026"
        },
        "implementation": {
            "url": base_url
        },
        "rest": [{
            "mode": "server",
            "security": {
                "extension": [{
                    "url": "http://fhir-registry.smarthealthit.org/StructureDefinition/oauth-uris",
                    "extension": [
                        {"url": "authorize", "valueUri": format!("{base_url}/oauth2/authorize")},
                        {"url": "token", "valueUri": format!("{base_url}/oauth2/token")},
                        {"url": "register", "valueUri": format!("{base_url}/oauth2/register")}
                    ]
                }]
            },
            "resource": if resources.is_empty() {
                vec![resource_capability("Patient", &["read", "search-type"])]
            } else {
                resources.to_vec()
            }
        }]
    })
}

fn resource_capability(resource_type: &str, interactions: &[&str]) -> Value {
    json!({
        "type": resource_type,
        "interaction": interactions.iter().map(|interaction| json!({ "code": interaction })).collect::<Vec<_>>(),
        "searchParam": [{
            "name": "patient",
            "type": "reference"
        }]
    })
}

fn login_page_html(token: &str) -> String {
    format!(
        "<html><head><title>Generic MyChart - Login Page</title></head><body>\
             <form id=\"loginForm\"></form>\
             <form class=\"hidden\" action=\"/Authentication/Login/DoLogin\">\
             <input name=\"__RequestVerificationToken\" type=\"hidden\" value=\"{token}\" />\
             </form>\
             </body></html>"
    )
}

fn app_page_html(title: &str) -> String {
    format!(
        "<html><head><title>{title}</title></head><body>\
             <div id=\"app\">hello from {title}</div>\
             <input name=\"__RequestVerificationToken\" type=\"hidden\" value=\"page-token\" />\
             </body></html>"
    )
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    query: BTreeMap<String, Vec<String>>,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl CapturedRequest {
    fn parse(buffer: &[u8]) -> Self {
        let headers_end = find_headers_end(buffer).expect("request should include headers");
        let headers = String::from_utf8_lossy(&buffer[..headers_end]);
        let mut lines = headers.lines();
        let request_line = lines.next().expect("request line should exist");
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().expect("method should exist").to_owned();
        let target = request_parts.next().expect("target should exist");
        let (path, query) = split_target(target);
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect();

        Self {
            method,
            path,
            query,
            headers,
            body: buffer[headers_end + 4..].to_vec(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    fn query_value(&self, name: &str) -> Option<&str> {
        self.query.get(name)?.last().map(String::as_str)
    }

    fn form_value(&self, name: &str) -> Option<String> {
        let form = parse_www_form(&String::from_utf8_lossy(&self.body));
        form.get(name).and_then(|values| values.last()).cloned()
    }

    fn json_body<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).expect("request body should deserialize")
    }
}

#[derive(Clone)]
struct ResponseSpec {
    status_code: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl ResponseSpec {
    fn html(status_code: u16, body: String, headers: Vec<(String, String)>) -> Self {
        let mut headers = headers;
        headers.push(("Content-Type".into(), "text/html; charset=utf-8".into()));
        Self {
            status_code,
            headers,
            body,
        }
    }

    fn json(status_code: u16, body: Value, headers: Vec<(String, String)>) -> Self {
        let mut headers = headers;
        headers.push(("Content-Type".into(), "application/fhir+json".into()));
        Self {
            status_code,
            headers,
            body: serde_json::to_string(&body).expect("body should serialize"),
        }
    }

    fn empty(status_code: u16, headers: Vec<(String, String)>) -> Self {
        Self {
            status_code,
            headers,
            body: String::new(),
        }
    }
}

struct TestServer {
    address: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    _handle: thread::JoinHandle<()>,
}

impl TestServer {
    fn spawn(responses: Vec<ResponseSpec>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener.local_addr().expect("listener should have local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();

        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("server should accept request");
                let request = read_captured_request(&mut stream);
                if let Ok(mut captured) = requests_clone.lock() {
                    captured.push(request);
                }

                let mut headers = response.headers;
                let body = response
                    .body
                    .replace("http://placeholder", &format!("http://{address}"));
                headers.push(("Content-Length".into(), body.len().to_string()));
                headers.push(("Connection".into(), "close".into()));

                let mut response_text = format!(
                    "HTTP/1.1 {} {}\r\n",
                    response.status_code,
                    status_text(response.status_code)
                );
                for (name, value) in headers {
                    response_text.push_str(&format!("{name}: {value}\r\n"));
                }
                response_text.push_str("\r\n");
                response_text.push_str(&body);
                stream
                    .write_all(response_text.as_bytes())
                    .expect("response should write");
            }
        });

        Self {
            address: format!("http://{address}"),
            requests,
            _handle: handle,
        }
    }

    fn base_url(&self) -> String {
        self.address.clone()
    }

    fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

fn read_response_text(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 1024];
    loop {
        let bytes_read = stream.read(&mut temp).expect("request should read");
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..bytes_read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buffer).expect("request should be utf-8")
}

fn read_captured_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 4096];
    loop {
        let bytes_read = stream.read(&mut temp).expect("request should read");
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..bytes_read]);

        if let Some(headers_end) = find_headers_end(&buffer) {
            let headers = String::from_utf8_lossy(&buffer[..headers_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let mut parts = line.splitn(2, ':');
                    let name = parts.next()?.trim();
                    if !name.eq_ignore_ascii_case("content-length") {
                        return None;
                    }
                    parts.next()?.trim().parse::<usize>().ok()
                })
                .unwrap_or(0);
            let total_length = headers_end + 4 + content_length;
            if buffer.len() >= total_length {
                break;
            }
        }
    }

    CapturedRequest::parse(&buffer)
}

fn find_headers_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn split_target(target: &str) -> (String, BTreeMap<String, Vec<String>>) {
    let Some((path, query)) = target.split_once('?') else {
        return (target.to_owned(), BTreeMap::new());
    };

    (path.to_owned(), parse_www_form(query))
}

fn parse_www_form(input: &str) -> BTreeMap<String, Vec<String>> {
    let mut values = BTreeMap::new();
    for pair in input.split('&').filter(|pair| !pair.is_empty()) {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = url_decode_component(raw_key);
        let value = url_decode_component(raw_value);
        values.entry(key).or_insert_with(Vec::new).push(value);
    }
    values
}

fn url_decode_component(input: &str) -> String {
    let mut decoded = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = decode_hex(bytes[index + 1]);
                let low = decode_hex(bytes[index + 2]);
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).expect("decoded form component should be utf-8")
}

fn decode_hex(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => panic!("invalid hex byte {byte:?}"),
    }
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        500 => "Internal Server Error",
        _ => "OK",
    }
}
