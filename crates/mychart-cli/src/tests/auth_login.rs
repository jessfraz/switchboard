use std::{net::TcpListener, thread};

use serde_json::json;

use super::support::*;
use crate::state::StateStore;
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
        run_command_json(&[
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

    let output: AuthenticatedOutput = handle.join().expect("auth login thread should finish");
    assert_eq!(output.status, "authenticated");
    assert_eq!(output.patient_id.as_deref(), Some("patient-123"));

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

    let output: AuthorizationPendingOutput = run_command_json(&[
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

    assert_eq!(output.status, "authorization_pending");
    assert!(!output.opened_browser);
    assert!(output.next_step.contains("mychart finish"));

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert!(account.pending_oauth_state.is_some());
    assert!(account.pending_code_verifier.is_some());
}

#[test]
fn auth_login_with_hosted_redirect_starts_authorization_flow() {
    let server = TestServer::spawn(vec![ResponseSpec::json(
        200,
        capability_statement_json("http://placeholder", &[]),
        Vec::new(),
    )]);
    let temp_dir = temp_dir("mychart-auth-login-hosted");
    let config_path = temp_dir.join("config.json");

    let output: AuthorizationPendingOutput = run_command_json(&[
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
        "auth",
        "login",
        "--no-open",
        "--state",
        "test-state",
        "--code-verifier",
        "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK",
    ]);

    assert_eq!(output.status, "authorization_pending");
    assert!(!output.opened_browser);
    assert!(output.next_step.contains("mychart finish"));

    let state = StateStore::new(config_path).load().expect("state should load");
    let account = state
        .accounts
        .get("default")
        .expect("default account should be persisted");
    assert_eq!(
        account.redirect_uri.as_deref(),
        Some("https://jessfraz.github.io/switchboard/mychart-callback/")
    );
    assert_eq!(account.pending_oauth_state.as_deref(), Some("test-state"));
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

    let output: AuthenticatedOutput = run_command_json(&[
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

    assert_eq!(output.status, "authenticated");
    assert_eq!(output.patient_id.as_deref(), Some("patient-123"));

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

    let output: AuthenticatedOutput = run_command_json(&[
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

    assert_eq!(output.status, "authenticated");
    assert_eq!(output.patient_id.as_deref(), Some("patient-123"));
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
        run_command_json(&[
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

    let output: AuthenticatedOutput = handle.join().expect("auth login thread should finish");
    assert_eq!(output.status, "authenticated");
    assert_eq!(output.dynamic_client_id.as_deref(), Some("dynamic-client-123"));
    assert_eq!(output.renewal_method.as_deref(), Some("dynamic_client_jwt_bearer"));
    assert_eq!(output.patient_id.as_deref(), Some("patient-123"));

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
    let registration_body: DynamicClientRegistrationBody = requests[4].json_body();
    assert_eq!(registration_body.software_id, "client-123");
    assert_eq!(registration_body.jwks.keys[0].kty, "RSA");
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
