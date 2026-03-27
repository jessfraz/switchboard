use std::collections::BTreeMap;

use serde_json::json;

use super::support::*;
use crate::state::{MyChartState, StateStore};

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

    let output: AppointmentsOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "appointments",
        "upcoming",
    ]);

    assert_eq!(output.status, "ok");
    assert_eq!(output.appointments.len(), 1);
    assert_eq!(output.appointments[0].id, "appt-future");
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

    let output: AppointmentsOutput = run_command_json(&[
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

    assert_eq!(output.status, "ok");
    assert_eq!(output.query.as_deref(), Some("derm"));
    assert_eq!(output.appointments.len(), 1);
    assert_eq!(output.appointments[0].id, "appt-derm-soon");
    let requests = server.requests();
    assert!(requests[1]
        .query_value("date")
        .is_some_and(|value| value.starts_with("ge")));
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
