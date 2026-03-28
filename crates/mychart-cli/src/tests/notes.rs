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
                    br#"<ClinicalDocument><recordTarget><patientRole><patient><name><given>Jessica</given><given>Leigh</given><family>Frazelle</family></name></patient></patientRole></recordTarget><section><title>Progress Note</title><text>Patient reports migraine improvement after medication change.</text></section></ClinicalDocument>"#
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
    assert!(!output.notes[0]
        .body_excerpt
        .as_deref()
        .is_some_and(|excerpt| excerpt.contains("Jessica Leigh Frazelle")));
}

#[test]
fn notes_search_centers_excerpt_on_matching_body_section() {
    #[derive(Debug, serde::Deserialize)]
    struct NotesSearchOutput {
        status: String,
        notes: Vec<NotesSearchMatch>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct NotesSearchMatch {
        body_excerpt: Option<String>,
    }

    let cda = br#"
<ClinicalDocument xmlns="urn:hl7-org:v3">
  <title>Encounter Summary</title>
  <component>
    <structuredBody>
      <component>
        <section>
          <title>Allergies</title>
          <text>No known active allergies.</text>
        </section>
      </component>
      <component>
        <section>
          <title>Progress Notes</title>
          <text>
            After the medication change, the patient reports migraine improvement and fewer aura episodes over the last month.
          </text>
        </section>
      </component>
    </structuredBody>
  </component>
</ClinicalDocument>
"#;
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
                        "id": "note-excerpt",
                        "date": "2100-01-01T00:00:00Z",
                        "type": {"text": "Progress Note"},
                        "description": "Routine follow-up",
                        "content": [{
                            "attachment": {
                                "contentType": "application/xml",
                                "title": "Visit note",
                                "url": "/Binary/note-excerpt-body"
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
                "data": crate::base64_encode(cda),
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-notes-search-excerpt");
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
    let excerpt = output.notes[0]
        .body_excerpt
        .as_deref()
        .expect("body excerpt should exist");
    assert!(excerpt.contains("migraine improvement"), "excerpt was: {excerpt}");
    assert!(!excerpt.contains("No known active allergies"), "excerpt was: {excerpt}");
}

#[test]
fn notes_get_normalizes_xml_boundary_mush() {
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
                "id": "note-xml",
                "date": "2100-01-02",
                "type": {"text": "Encounter Summary"},
                "description": "After visit summary",
                "content": [{
                    "attachment": {
                        "title": "Summary body",
                        "contentType": "application/xml",
                        "url": "/Binary/note-xml-body"
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
                    br#"<ClinicalDocument><recordTarget><patientRole><addr><streetAddressLine>21607 Mulholland Drive</streetAddressLine><city>LOS ANGELES</city><state>CA</state></addr><patient><name><given>Jessica</given><given>Leigh</given><family>Frazelle</family></name></patient></patientRole></recordTarget><section><title>After Visit Summary</title><text>Submitted on (March 27, 2026)Apply warm compresses as needed.</text></section></ClinicalDocument>"#
                )
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-notes-get-xml");
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

    let output: NoteGetOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "notes",
        "get",
        "note-xml",
    ]);

    assert_eq!(output.status, "ok");
    let body = output.note.body_text.as_deref().expect("body text should exist");
    assert!(body.starts_with("After Visit Summary"), "body was: {body}");
    assert!(
        body.contains("Submitted on (March 27, 2026) Apply warm compresses as needed."),
        "body was: {body}"
    );
    assert!(!body.contains("Jessica Leigh Frazelle"));
    assert!(!body.contains("21607 Mulholland Drive"));
    assert!(!body.contains("2026)Apply"));
}

#[test]
fn notes_get_decodes_embedded_base64_rtf_payloads() {
    let embedded_rtf = crate::base64_encode(
        br"{\rtf1\ansi CASE REPORT\par Negative for Intraepithelial Lesion or Malignancy.\par SPECIMEN ADEQUACY\par Satisfactory for evaluation.}",
    );
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
                "id": "note-rtf",
                "date": "2100-01-03",
                "type": {"text": "Lab report"},
                "content": [{
                    "attachment": {
                        "title": "Embedded report",
                        "contentType": "application/xml",
                        "url": "/Binary/note-rtf-body"
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
                    format!(
                        "<ClinicalDocument><section><title>Pathology</title><text>Intro text {embedded_rtf} Trailing text.</text></section></ClinicalDocument>"
                    )
                    .as_bytes()
                )
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-notes-get-embedded-rtf");
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

    let output: NoteGetOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "notes",
        "get",
        "note-rtf",
    ]);

    assert_eq!(output.status, "ok");
    let body = output.note.body_text.as_deref().expect("body text should exist");
    assert!(body.contains("Negative for Intraepithelial Lesion or Malignancy."));
    assert!(body.contains("Satisfactory for evaluation."));
    assert!(body.contains("Trailing text."));
    assert!(!body.contains("e1xydGY"));
    assert!(!body.contains("{\\rtf1"));
}

#[test]
fn notes_get_extracts_cda_sections_instead_of_full_document_soup() {
    let cda = br#"
<ClinicalDocument xmlns="urn:hl7-org:v3">
  <title>Patient Health Summary</title>
  <recordTarget>
    <patientRole>
      <addr>
        <streetAddressLine>21607 Mulholland Drive</streetAddressLine>
        <city>Woodland Hills</city>
      </addr>
      <patient>
        <name>
          <given>Jessica</given>
          <family>Frazelle</family>
        </name>
      </patient>
    </patientRole>
  </recordTarget>
  <component>
    <structuredBody>
      <component>
        <section>
          <title>Allergies</title>
          <text>
            <paragraph>No known active allergies</paragraph>
          </text>
        </section>
      </component>
      <component>
        <section>
          <title>Results</title>
          <text>
            <table>
              <tbody>
                <tr><th>Test</th><th>Value</th></tr>
                <tr><td>TSH</td><td>2.1</td></tr>
              </tbody>
            </table>
          </text>
        </section>
      </component>
    </structuredBody>
  </component>
</ClinicalDocument>
"#;
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
                "id": "note-cda",
                "date": "2100-01-04",
                "type": {"text": "Patient Summary"},
                "content": [{
                    "attachment": {
                        "title": "Summary body",
                        "contentType": "application/xml",
                        "url": "/Binary/note-cda-body"
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
                "data": crate::base64_encode(cda)
            }),
            Vec::new(),
        ),
    ]);
    let temp_dir = temp_dir("mychart-notes-get-cda-sections");
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

    let output: NoteGetOutput = run_command_json(&[
        "mychart",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "notes",
        "get",
        "note-cda",
    ]);

    assert_eq!(output.status, "ok");
    let body = output.note.body_text.as_deref().expect("body text should exist");
    assert!(body.starts_with("Patient Health Summary"));
    assert!(body.contains("Allergies\nNo known active allergies"));
    assert!(body.contains("Results\nTest Value\nTSH 2.1"));
    assert!(!body.contains("21607 Mulholland Drive"));
    assert!(!body.contains("Jessica Frazelle"));
}
