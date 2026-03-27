use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{
    commands::shared::{
        bundle_entries, concept_text, first_string, interpretation_summary, iso_on_or_after,
        observation_effective_date, open_patient_session, resolve_since_floor, value_summary,
    },
    Result,
};

#[derive(Debug, Args)]
pub(crate) struct PackCommand {
    #[command(subcommand)]
    pub(crate) command: PackSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PackSubcommand {
    Doctor(PackDoctorArgs),
}

#[derive(Debug, Args)]
pub(crate) struct PackDoctorArgs {
    #[arg(long)]
    patient: Option<String>,

    #[arg(long)]
    since: Option<String>,

    #[arg(long, default_value_t = 5)]
    limit: usize,

    #[arg(long)]
    all_pages: bool,
}

pub(crate) fn run_pack(command: PackSubcommand, context: &crate::state::ResolvedContext) -> Result<Value> {
    match command {
        PackSubcommand::Doctor(args) => run_doctor(args, context),
    }
}

fn run_doctor(args: PackDoctorArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let session = open_patient_session(context, args.patient)?;
    let floor = resolve_since_floor(args.since.as_deref())?;

    let upcoming_appointment = nearest_upcoming_appointment(&session, args.all_pages)?;
    let recent_labs = recent_labs(&session, floor.as_deref(), args.limit, args.all_pages)?;
    let active_medications = active_medications(&session, floor.as_deref(), args.limit, args.all_pages)?;
    let active_conditions = active_conditions(&session, floor.as_deref(), args.limit, args.all_pages)?;
    let recent_encounters = recent_encounters(&session, floor.as_deref(), args.limit, args.all_pages)?;
    let recent_notes = recent_notes(&session, floor.as_deref(), args.limit, args.all_pages)?;
    let suggested_questions = build_suggested_questions(
        &upcoming_appointment,
        &recent_labs,
        &active_medications,
        &active_conditions,
    );

    Ok(json!({
        "status": "ok",
        "patient_id": session.patient_id,
        "upcoming_appointment": upcoming_appointment,
        "recent_labs": recent_labs,
        "active_medications": active_medications,
        "active_conditions": active_conditions,
        "recent_encounters": recent_encounters,
        "recent_notes": recent_notes,
        "suggested_questions": suggested_questions,
    }))
}

fn nearest_upcoming_appointment(session: &crate::commands::shared::PatientSession, all_pages: bool) -> Result<Value> {
    let today = crate::commands::shared::current_utc_date_string();
    let query = vec![("_count".into(), "25".into()), ("date".into(), format!("ge{today}"))];
    let appointments = session
        .search_resource("Appointment", &query, all_pages)?
        .map(|bundle| bundle_entries(&bundle))
        .unwrap_or_default();
    let mut upcoming = appointments
        .into_iter()
        .filter_map(|resource| {
            let start = first_string(&resource, &["/start"])?;
            let status = first_string(&resource, &["/status"]).unwrap_or_else(|| "unknown".into());
            if !iso_on_or_after(&start, &today)
                || matches!(
                    status.as_str(),
                    "cancelled" | "entered-in-error" | "noshow" | "fulfilled"
                )
            {
                return None;
            }

            Some(json!({
                "id": first_string(&resource, &["/id"]),
                "status": status,
                "start": start,
                "end": first_string(&resource, &["/end"]),
                "description": first_string(&resource, &["/description"])
                    .or_else(|| resource.get("serviceType").and_then(Value::as_array).and_then(|values| values.first()).and_then(concept_text)),
                "location": resource
                    .get("participant")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find_map(|participant| participant.pointer("/actor/display").and_then(Value::as_str))
                    .map(ToOwned::to_owned),
            }))
        })
        .collect::<Vec<_>>();

    upcoming.sort_by(|left, right| {
        left.get("start")
            .and_then(Value::as_str)
            .cmp(&right.get("start").and_then(Value::as_str))
    });
    Ok(upcoming.into_iter().next().unwrap_or(Value::Null))
}

fn recent_labs(
    session: &crate::commands::shared::PatientSession,
    floor: Option<&str>,
    limit: usize,
    all_pages: bool,
) -> Result<Vec<Value>> {
    let observations = session
        .search_resource(
            "Observation",
            &[
                ("_count".into(), limit.max(25).to_string()),
                ("category".into(), "laboratory".into()),
            ],
            all_pages,
        )?
        .map(|bundle| bundle_entries(&bundle))
        .unwrap_or_default();

    let mut labs = observations
        .into_iter()
        .filter_map(|resource| {
            let observed_at = observation_effective_date(&resource)?;
            if floor.is_some_and(|floor| !iso_on_or_after(&observed_at, floor)) {
                return None;
            }
            Some(json!({
                "id": first_string(&resource, &["/id"]),
                "label": resource.pointer("/code").and_then(concept_text),
                "observed_at": observed_at,
                "value": value_summary(&resource),
                "interpretation": interpretation_summary(&resource),
            }))
        })
        .collect::<Vec<_>>();

    labs.sort_by(|left, right| {
        right
            .get("observed_at")
            .and_then(Value::as_str)
            .cmp(&left.get("observed_at").and_then(Value::as_str))
    });
    labs.truncate(limit);
    Ok(labs)
}

fn active_medications(
    session: &crate::commands::shared::PatientSession,
    floor: Option<&str>,
    limit: usize,
    all_pages: bool,
) -> Result<Vec<Value>> {
    let meds = session
        .search_resource(
            "MedicationRequest",
            &[("_count".into(), limit.max(50).to_string())],
            all_pages,
        )?
        .map(|bundle| bundle_entries(&bundle))
        .unwrap_or_default();

    let mut active = meds
        .into_iter()
        .filter_map(|resource| {
            let authored_on = first_string(&resource, &["/authoredOn", "/meta/lastUpdated"])?;
            if floor.is_some_and(|floor| !iso_on_or_after(&authored_on, floor)) {
                return None;
            }
            let status = first_string(&resource, &["/status"]).unwrap_or_else(|| "unknown".into());
            if !matches!(
                status.as_str(),
                "active" | "on-hold" | "completed" | "draft" | "unknown"
            ) {
                return None;
            }
            Some(json!({
                "id": first_string(&resource, &["/id"]),
                "name": resource.pointer("/medicationCodeableConcept").and_then(concept_text)
                    .or_else(|| first_string(&resource, &["/medicationReference/display"])),
                "status": status,
                "authored_on": authored_on,
                "dosage": first_string(&resource, &["/dosageInstruction/0/text"]),
            }))
        })
        .collect::<Vec<_>>();
    active.sort_by(|left, right| {
        right
            .get("authored_on")
            .and_then(Value::as_str)
            .cmp(&left.get("authored_on").and_then(Value::as_str))
    });
    active.truncate(limit);
    Ok(active)
}

fn active_conditions(
    session: &crate::commands::shared::PatientSession,
    floor: Option<&str>,
    limit: usize,
    all_pages: bool,
) -> Result<Vec<Value>> {
    let conditions = session
        .search_resource("Condition", &[("_count".into(), limit.max(50).to_string())], all_pages)?
        .map(|bundle| bundle_entries(&bundle))
        .unwrap_or_default();

    let mut active = conditions
        .into_iter()
        .filter_map(|resource| {
            let recorded = first_string(&resource, &["/recordedDate", "/onsetDateTime", "/meta/lastUpdated"])?;
            if floor.is_some_and(|floor| !iso_on_or_after(&recorded, floor)) {
                return None;
            }
            let clinical_status = resource
                .pointer("/clinicalStatus")
                .and_then(concept_text)
                .unwrap_or_else(|| "unknown".into());
            if !matches!(
                clinical_status.to_ascii_lowercase().as_str(),
                "active" | "recurrence" | "relapse" | "unknown"
            ) {
                return None;
            }
            Some(json!({
                "id": first_string(&resource, &["/id"]),
                "condition": resource.pointer("/code").and_then(concept_text),
                "clinical_status": clinical_status,
                "verification_status": resource.pointer("/verificationStatus").and_then(concept_text),
                "recorded_at": recorded,
            }))
        })
        .collect::<Vec<_>>();
    active.sort_by(|left, right| {
        right
            .get("recorded_at")
            .and_then(Value::as_str)
            .cmp(&left.get("recorded_at").and_then(Value::as_str))
    });
    active.truncate(limit);
    Ok(active)
}

fn recent_encounters(
    session: &crate::commands::shared::PatientSession,
    floor: Option<&str>,
    limit: usize,
    all_pages: bool,
) -> Result<Vec<Value>> {
    let encounters = session
        .search_resource("Encounter", &[("_count".into(), limit.max(25).to_string())], all_pages)?
        .map(|bundle| bundle_entries(&bundle))
        .unwrap_or_default();

    let mut recent = encounters
        .into_iter()
        .filter_map(|resource| {
            let started = first_string(&resource, &["/period/start", "/meta/lastUpdated"])?;
            if floor.is_some_and(|floor| !iso_on_or_after(&started, floor)) {
                return None;
            }
            Some(json!({
                "id": first_string(&resource, &["/id"]),
                "started_at": started,
                "status": first_string(&resource, &["/status"]),
                "class": first_string(&resource, &["/class/display"]),
                "type": resource.get("type").and_then(Value::as_array).and_then(|values| values.first()).and_then(concept_text),
            }))
        })
        .collect::<Vec<_>>();
    recent.sort_by(|left, right| {
        right
            .get("started_at")
            .and_then(Value::as_str)
            .cmp(&left.get("started_at").and_then(Value::as_str))
    });
    recent.truncate(limit);
    Ok(recent)
}

fn recent_notes(
    session: &crate::commands::shared::PatientSession,
    floor: Option<&str>,
    limit: usize,
    all_pages: bool,
) -> Result<Vec<Value>> {
    let notes = session
        .search_resource(
            "DocumentReference",
            &[("_count".into(), limit.max(25).to_string())],
            all_pages,
        )?
        .map(|bundle| bundle_entries(&bundle))
        .unwrap_or_default();

    let mut recent = notes
        .into_iter()
        .filter_map(|resource| {
            let date = first_string(&resource, &["/date", "/meta/lastUpdated"])?;
            if floor.is_some_and(|floor| !iso_on_or_after(&date, floor)) {
                return None;
            }
            Some(json!({
                "id": first_string(&resource, &["/id"]),
                "date": date,
                "type": resource.pointer("/type").and_then(concept_text),
                "description": first_string(&resource, &["/description"]),
                "author": resource.get("author").and_then(Value::as_array).and_then(|authors| authors.first()).and_then(|author| author.get("display")).and_then(Value::as_str),
            }))
        })
        .collect::<Vec<_>>();
    recent.sort_by(|left, right| {
        right
            .get("date")
            .and_then(Value::as_str)
            .cmp(&left.get("date").and_then(Value::as_str))
    });
    recent.truncate(limit);
    Ok(recent)
}

fn build_suggested_questions(
    upcoming_appointment: &Value,
    recent_labs: &[Value],
    active_medications: &[Value],
    active_conditions: &[Value],
) -> Vec<String> {
    let mut questions = Vec::new();

    if let Some(description) = upcoming_appointment.get("description").and_then(Value::as_str) {
        questions.push(format!(
            "What should I make sure we cover during my upcoming {description} visit?"
        ));
    }

    for lab in recent_labs.iter().take(2) {
        if let (Some(label), Some(interpretation)) = (
            lab.get("label").and_then(Value::as_str),
            lab.get("interpretation").and_then(Value::as_str),
        ) {
            questions.push(format!(
                "What does my recent {label} result marked {interpretation} mean for my care plan?"
            ));
        }
    }

    if !active_medications.is_empty() {
        questions.push("Should any of my current medications change based on my recent results or symptoms?".into());
    }

    if !active_conditions.is_empty() {
        questions.push(
            "Which of my active diagnoses matter most for this visit, and what should I watch between appointments?"
                .into(),
        );
    }

    questions
}
