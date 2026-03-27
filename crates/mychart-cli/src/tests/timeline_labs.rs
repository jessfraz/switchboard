use serde_json::json;

use super::support::*;
use crate::state::{MyChartState, StateStore};

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

    let output: LabsOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "labs",
        "a1c",
        "--spark",
    ]);

    assert_eq!(output.status, "ok");
    assert_eq!(output.series[0].label, "Hemoglobin A1c");
    assert_eq!(output.series[0].point_count, 2);
    assert!(output.series[0].spark.as_ref().is_some_and(|spark| !spark.is_empty()));
}
