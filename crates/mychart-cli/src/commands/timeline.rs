use clap::Args;
use serde_json::{json, Value};

use crate::{
    commands::shared::{
        accounts_used_json, bundle_entries, concept_text, first_string, iso_on_or_after, observation_effective_date,
        open_patient_sessions, resolve_since_floor, resource_timestamp, single_patient_id, value_summary,
    },
    Result,
};

#[derive(Debug, Args)]
pub(crate) struct TimelineCommand {
    #[arg(long)]
    patient: Option<String>,

    #[arg(long, alias = "all-providers")]
    all_accounts: bool,

    #[arg(long)]
    since: Option<String>,

    #[arg(long, default_value_t = 50)]
    limit: usize,

    #[arg(long)]
    all_pages: bool,
}

pub(crate) fn run_timeline(args: TimelineCommand, context: &crate::state::ResolvedContext) -> Result<Value> {
    let selection = open_patient_sessions(context, args.patient, args.all_accounts)?;
    let floor = resolve_since_floor(args.since.as_deref())?;
    let resource_queries = [
        ("Appointment", vec![("_count".into(), "25".into())]),
        ("Encounter", vec![("_count".into(), "25".into())]),
        (
            "Observation",
            vec![("_count".into(), "50".into()), ("category".into(), "laboratory".into())],
        ),
        ("DiagnosticReport", vec![("_count".into(), "25".into())]),
        ("MedicationRequest", vec![("_count".into(), "25".into())]),
        ("DocumentReference", vec![("_count".into(), "25".into())]),
        ("ExplanationOfBenefit", vec![("_count".into(), "25".into())]),
    ];

    let mut events = Vec::new();
    for session in &selection.sessions {
        for (resource_type, query) in &resource_queries {
            let Some(bundle) = session.search_resource(resource_type, query, args.all_pages)? else {
                continue;
            };
            for resource in bundle_entries(&bundle) {
                let Some(timestamp) = resource_timestamp(&resource) else {
                    continue;
                };
                if floor
                    .as_deref()
                    .is_some_and(|floor| !iso_on_or_after(&timestamp, floor))
                {
                    continue;
                }
                let mut event = render_event(&resource, &timestamp);
                if let Some(object) = event.as_object_mut() {
                    object.insert("account".into(), Value::String(session.account_name.clone()));
                    object.insert("provider".into(), Value::String(session.provider_name.clone()));
                    object.insert("patient_id".into(), Value::String(session.patient_id.clone()));
                }
                events.push(event);
            }
        }
    }

    events.sort_by(|left, right| {
        right
            .get("timestamp")
            .and_then(Value::as_str)
            .cmp(&left.get("timestamp").and_then(Value::as_str))
    });
    events.truncate(args.limit);

    Ok(json!({
        "status": "ok",
        "patient_id": single_patient_id(&selection),
        "accounts_used": accounts_used_json(&selection),
        "accounts_skipped": selection.skipped_accounts,
        "events": events,
    }))
}

fn render_event(resource: &Value, timestamp: &str) -> Value {
    let resource_type = resource
        .get("resourceType")
        .and_then(Value::as_str)
        .unwrap_or("Unknown");
    let (title, summary, status) = match resource_type {
        "Appointment" => (
            first_string(resource, &["/description"])
                .or_else(|| {
                    resource
                        .get("serviceType")
                        .and_then(Value::as_array)
                        .and_then(|values| values.first())
                        .and_then(concept_text)
                })
                .unwrap_or_else(|| "Appointment".into()),
            first_string(resource, &["/start", "/end"]).unwrap_or_else(|| timestamp.into()),
            first_string(resource, &["/status"]),
        ),
        "Encounter" => (
            resource
                .get("type")
                .and_then(Value::as_array)
                .and_then(|types| types.first())
                .and_then(concept_text)
                .unwrap_or_else(|| "Encounter".into()),
            first_string(resource, &["/class/display", "/serviceType/text"]).unwrap_or_default(),
            first_string(resource, &["/status"]),
        ),
        "Observation" => (
            resource
                .pointer("/code")
                .and_then(concept_text)
                .unwrap_or_else(|| "Observation".into()),
            value_summary(resource).unwrap_or_else(|| "No recorded value".into()),
            crate::commands::shared::interpretation_summary(resource),
        ),
        "DiagnosticReport" => (
            resource
                .pointer("/code")
                .and_then(concept_text)
                .unwrap_or_else(|| "Diagnostic report".into()),
            first_string(resource, &["/conclusion"]).unwrap_or_else(|| "Diagnostic report".into()),
            first_string(resource, &["/status"]),
        ),
        "MedicationRequest" => (
            resource
                .pointer("/medicationCodeableConcept")
                .and_then(concept_text)
                .unwrap_or_else(|| "Medication request".into()),
            first_string(resource, &["/dosageInstruction/0/text"]).unwrap_or_default(),
            first_string(resource, &["/status"]),
        ),
        "DocumentReference" => (
            resource
                .pointer("/type")
                .and_then(concept_text)
                .unwrap_or_else(|| "Clinical note".into()),
            first_string(resource, &["/description"]).unwrap_or_default(),
            None,
        ),
        "ExplanationOfBenefit" => (
            "Claim".into(),
            first_string(resource, &["/outcome"]).unwrap_or_else(|| "Explanation of benefit".into()),
            first_string(resource, &["/status"]),
        ),
        _ => (
            resource_type.to_owned(),
            observation_effective_date(resource).unwrap_or_default(),
            None,
        ),
    };

    json!({
        "resource_type": resource_type,
        "id": first_string(resource, &["/id"]),
        "timestamp": timestamp,
        "title": title,
        "summary": summary,
        "status": status,
    })
}
