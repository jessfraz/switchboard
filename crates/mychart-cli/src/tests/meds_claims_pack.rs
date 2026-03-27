use std::collections::BTreeMap;

use serde_json::json;

use super::support::*;
use crate::state::{MyChartState, StateStore};

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

    let output: ClaimsAuditOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "claims",
        "audit",
    ]);

    assert_eq!(output.status, "ok");
    assert_eq!(output.duplicate_charge_candidates.len(), 1);
    assert_eq!(output.denied_or_problematic_claims.len(), 1);
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

    let output: PackDoctorOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "pack",
        "doctor",
    ]);

    assert_eq!(output.status, "ok");
    assert_eq!(output.upcoming_appointment.description, "Neurology follow-up");
    assert_eq!(output.recent_labs[0].label, "Ferritin");
    assert_eq!(output.active_medications[0].name, "Topiramate");
    assert_eq!(output.active_conditions[0].condition, "Migraine");
    assert!(!output.suggested_questions.is_empty());
}

#[test]
fn sha256_matches_known_vector() {
    assert_eq!(
        hex(&crate::sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
