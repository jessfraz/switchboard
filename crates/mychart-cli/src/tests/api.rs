use std::collections::BTreeMap;

use clap::Parser;
use serde_json::json;

use super::support::*;
use crate::{
    state::{MyChartState, StateStore},
    Cli,
};
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

    let output: ApiResourcesOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &server.base_url(),
        "--compact",
        "api",
        "resources",
    ]);

    assert_eq!(output.resource_count, 2);
    assert_eq!(output.resources[0].resource, "Observation");
    assert_eq!(output.resources[1].resource, "Patient");
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

    let output: ApiGetOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "api",
        "patient",
        "get",
        "patient-123",
    ]);

    assert_eq!(output.status, "ok");
    assert_eq!(output.resource, "Patient");
    assert_eq!(output.body.id, "patient-123");

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

    let output: StatusOutput = run_command_json(&[
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

    assert_eq!(output.status, "ok");
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

    let error: MessageErrorOutput = run_command_error_json(&[
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

    assert_eq!(error.status, "error");
    assert_eq!(error.kind, "arguments");
    assert_eq!(
        error.message,
        "unsupported resource operation \"create\", use get/read or search"
    );
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

    let output: AuthenticatedOutput = run_command_json(&[
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

    assert_eq!(output.status, "authenticated");

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
    .expect_err("portal request post should fail to parse");

    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
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
