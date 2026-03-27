use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{
    commands::shared::{
        concept_text, current_utc_date_string, first_string, iso_on_or_after, normalize_match_text,
        open_patient_session, resolve_since_floor, resolve_until_ceiling,
    },
    Result,
};

#[derive(Debug, Args)]
pub(crate) struct AppointmentsCommand {
    #[command(subcommand)]
    pub(crate) command: AppointmentsSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AppointmentsSubcommand {
    Upcoming(AppointmentsUpcomingArgs),
    Find(AppointmentsFindArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AppointmentsUpcomingArgs {
    #[arg(long)]
    patient: Option<String>,

    #[arg(long)]
    since: Option<String>,

    #[arg(long, default_value_t = 10)]
    limit: usize,

    #[arg(long)]
    all_pages: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AppointmentsFindArgs {
    #[arg(value_name = "QUERY")]
    query: Vec<String>,

    #[arg(long)]
    patient: Option<String>,

    #[arg(long, default_value = "30d", value_name = "WINDOW")]
    next: String,

    #[arg(long, default_value_t = 10)]
    limit: usize,

    #[arg(long)]
    all_pages: bool,
}

pub(crate) fn run_appointments(
    command: AppointmentsSubcommand,
    context: &crate::state::ResolvedContext,
) -> Result<Value> {
    match command {
        AppointmentsSubcommand::Upcoming(args) => run_upcoming(args, context),
        AppointmentsSubcommand::Find(args) => run_find(args, context),
    }
}

fn run_upcoming(args: AppointmentsUpcomingArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let session = open_patient_session(context, args.patient)?;
    let floor = resolve_since_floor(args.since.as_deref())?.unwrap_or_else(current_utc_date_string);
    let mut upcoming = load_upcoming_appointments(&session, args.limit.max(25), args.all_pages, &floor)?
        .into_iter()
        .filter_map(|resource| render_appointment(&resource))
        .collect::<Vec<_>>();

    upcoming.truncate(args.limit);

    Ok(json!({
        "status": "ok",
        "patient_id": session.patient_id,
        "since": floor,
        "appointments": upcoming,
    }))
}

fn run_find(args: AppointmentsFindArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let session = open_patient_session(context, args.patient)?;
    let floor = current_utc_date_string();
    let through = resolve_until_ceiling(Some(&args.next), 30)?;
    let normalized_query = normalize_match_text(&args.query.join(" "));

    let mut appointments = load_upcoming_appointments(&session, args.limit.max(50), args.all_pages, &floor)?
        .into_iter()
        .filter(|resource| {
            let start = first_string(resource, &["/start"]).unwrap_or_default();
            iso_on_or_before(&start, &through)
                && (normalized_query.is_empty() || appointment_matches_query(resource, &normalized_query))
        })
        .filter_map(|resource| render_appointment(&resource))
        .collect::<Vec<_>>();

    appointments.truncate(args.limit);

    Ok(json!({
        "status": "ok",
        "patient_id": session.patient_id,
        "query": if args.query.is_empty() {
            Value::Null
        } else {
            Value::String(args.query.join(" "))
        },
        "through": through,
        "appointments": appointments,
    }))
}

fn load_upcoming_appointments(
    session: &crate::commands::shared::PatientSession,
    count: usize,
    all_pages: bool,
    floor: &str,
) -> Result<Vec<Value>> {
    let mut query = vec![("_count".into(), count.to_string())];
    if session
        .resource("Appointment")
        .is_some_and(|resource| resource.search_params.iter().any(|parameter| parameter.name == "date"))
    {
        query.push(("date".into(), format!("ge{floor}")));
    }

    let appointments = session
        .search_resource("Appointment", &query, all_pages)?
        .map(|bundle| crate::commands::shared::bundle_entries(&bundle))
        .unwrap_or_default();

    let mut upcoming = appointments
        .into_iter()
        .filter(|resource| {
            let Some(start) = first_string(resource, &["/start"]) else {
                return false;
            };
            let status = first_string(resource, &["/status"]).unwrap_or_else(|| "unknown".into());
            iso_on_or_after(&start, floor) && !appointment_status_excluded(&status)
        })
        .collect::<Vec<_>>();

    upcoming.sort_by_key(|appointment| first_string(appointment, &["/start"]));
    Ok(upcoming)
}

fn appointment_matches_query(resource: &Value, normalized_query: &str) -> bool {
    normalize_match_text(&appointment_search_text(resource)).contains(normalized_query)
}

fn appointment_search_text(resource: &Value) -> String {
    let mut fields = Vec::new();

    if let Some(description) = first_string(resource, &["/description"]) {
        fields.push(description);
    }

    for path in [
        "/appointmentType",
        "/serviceCategory/0",
        "/specialty/0",
        "/reasonCode/0",
    ] {
        if let Some(value) = resource.pointer(path).and_then(concept_text) {
            fields.push(value);
        }
    }

    if let Some(values) = resource.get("serviceType").and_then(Value::as_array) {
        fields.extend(values.iter().filter_map(concept_text));
    }

    if let Some(values) = resource.get("specialty").and_then(Value::as_array) {
        fields.extend(values.iter().filter_map(concept_text));
    }

    if let Some(values) = resource.get("reasonCode").and_then(Value::as_array) {
        fields.extend(values.iter().filter_map(concept_text));
    }

    if let Some(values) = resource.get("participant").and_then(Value::as_array) {
        fields.extend(
            values
                .iter()
                .filter_map(|participant| participant.pointer("/actor/display").and_then(Value::as_str))
                .map(ToOwned::to_owned),
        );
    }

    fields.join(" ")
}

fn render_appointment(resource: &Value) -> Option<Value> {
    let start = first_string(resource, &["/start"])?;
    let status = first_string(resource, &["/status"]).unwrap_or_else(|| "unknown".into());
    let specialty = resource
        .get("specialty")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(concept_text).collect::<Vec<_>>())
        .unwrap_or_default();
    let providers = resource
        .get("participant")
        .and_then(Value::as_array)
        .map(|participants| {
            participants
                .iter()
                .filter_map(|participant| participant.pointer("/actor/display").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(json!({
        "id": first_string(resource, &["/id"]),
        "status": status,
        "start": start,
        "end": first_string(resource, &["/end"]),
        "description": appointment_description(resource),
        "specialty": specialty,
        "location": providers.first().cloned(),
        "participants": providers,
    }))
}

fn appointment_description(resource: &Value) -> Option<String> {
    first_string(resource, &["/description"]).or_else(|| {
        resource
            .get("serviceType")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(concept_text)
    })
}

fn appointment_status_excluded(status: &str) -> bool {
    matches!(
        status,
        "cancelled" | "noshow" | "entered-in-error" | "fulfilled" | "checked-in"
    )
}

fn iso_on_or_before(candidate: &str, ceiling: &str) -> bool {
    candidate.chars().take(10).collect::<String>() <= ceiling.chars().take(10).collect::<String>()
}
