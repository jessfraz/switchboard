use serde_json::json;

use super::support::*;
use crate::state::{MyChartState, StateStore};

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

    let output: NoteGetOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "notes",
        "get",
        "note-1",
    ]);

    assert_eq!(output.status, "ok");
    assert_eq!(
        output.note.body_text.as_deref(),
        Some("Patient reports migraine improvement.")
    );
    assert_eq!(
        output.note.content[0].body_text.as_deref(),
        Some("Patient reports migraine improvement.")
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
                "contentType": "application/xml",
                "data": crate::base64_encode(
                    br#"<ClinicalDocument><section><title>Progress Note</title><text>Patient reports migraine improvement after medication change.</text></section></ClinicalDocument>"#
                )
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
    assert!(!output.notes[0]
        .body_excerpt
        .as_deref()
        .is_some_and(|excerpt| excerpt.contains("<ClinicalDocument")));
}
