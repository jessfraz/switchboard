use std::collections::BTreeSet;

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    commands::shared::{
        accounts_used_json, concept_text, current_utc_date_string, first_string, iso_on_or_after, normalize_match_text,
        open_patient_sessions, resolve_since_floor, resolve_until_ceiling, single_patient_id,
    },
    Error, Result,
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

#[derive(Debug, Args, Clone)]
pub(crate) struct AppointmentsUpcomingArgs {
    #[arg(long)]
    pub(crate) patient: Option<String>,

    #[arg(long, alias = "all-providers")]
    pub(crate) all_accounts: bool,

    #[arg(long)]
    pub(crate) since: Option<String>,

    #[arg(long, default_value_t = 10)]
    pub(crate) limit: usize,

    #[arg(long)]
    pub(crate) all_pages: bool,
}

#[derive(Debug, Args, Clone)]
pub(crate) struct AppointmentsFindArgs {
    #[arg(value_name = "QUERY")]
    pub(crate) query: Vec<String>,

    #[arg(long)]
    pub(crate) patient: Option<String>,

    #[arg(long, alias = "all-providers")]
    pub(crate) all_accounts: bool,

    #[arg(long, default_value = "30d", value_name = "WINDOW")]
    pub(crate) next: String,

    #[arg(long, default_value_t = 10)]
    pub(crate) limit: usize,

    #[arg(long)]
    pub(crate) all_pages: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct AccountUsage {
    pub(crate) account: String,
    pub(crate) provider: String,
    pub(crate) patient_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct SkippedAccount {
    pub(crate) account: String,
    pub(crate) provider: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AppointmentRecord {
    pub(crate) account: String,
    pub(crate) provider: String,
    pub(crate) patient_id: String,
    pub(crate) id: Option<String>,
    pub(crate) status: String,
    pub(crate) start: String,
    pub(crate) end: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) specialty: Vec<String>,
    pub(crate) location: Option<String>,
    pub(crate) participants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AppointmentsOutput {
    pub(crate) status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) patient_id: Option<String>,
    pub(crate) accounts_used: Vec<AccountUsage>,
    pub(crate) accounts_skipped: Vec<SkippedAccount>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) through: Option<String>,
    pub(crate) appointments: Vec<AppointmentRecord>,
}

pub(crate) fn run_appointments(
    command: AppointmentsSubcommand,
    context: &crate::state::ResolvedContext,
) -> Result<Value> {
    match command {
        AppointmentsSubcommand::Upcoming(args) => render_output(run_upcoming_output(args, context)?),
        AppointmentsSubcommand::Find(args) => render_output(run_find_output(args, context)?),
    }
}

pub(crate) fn run_upcoming_output(
    args: AppointmentsUpcomingArgs,
    context: &crate::state::ResolvedContext,
) -> Result<AppointmentsOutput> {
    let selection = open_patient_sessions(context, args.patient, args.all_accounts)?;
    let floor = resolve_since_floor(args.since.as_deref())?.unwrap_or_else(current_utc_date_string);
    let mut upcoming = Vec::new();
    let mut warnings = BTreeSet::new();

    for session in &selection.sessions {
        if let Some(reason) = session.resource_unavailable_reason("Appointment") {
            warnings.insert(reason);
        }
        upcoming.extend(
            load_upcoming_appointments(session, args.limit.max(25), args.all_pages, &floor)?
                .into_iter()
                .filter_map(|resource| {
                    let mut appointment = render_appointment(&resource)?;
                    appointment.account = session.account_name.clone();
                    appointment.provider = session.provider_name.clone();
                    appointment.patient_id = session.patient_id.clone();
                    Some(appointment)
                }),
        );
    }

    upcoming.sort_by(|left, right| left.start.cmp(&right.start));
    upcoming.truncate(args.limit);

    Ok(AppointmentsOutput {
        status: "ok".into(),
        patient_id: single_patient_id(&selection),
        accounts_used: deserialize_account_usage(accounts_used_json(&selection))?,
        accounts_skipped: deserialize_skipped_accounts(selection.skipped_accounts)?,
        warnings: warnings.into_iter().collect(),
        since: Some(floor),
        query: None,
        through: None,
        appointments: upcoming,
    })
}

pub(crate) fn run_find_output(
    args: AppointmentsFindArgs,
    context: &crate::state::ResolvedContext,
) -> Result<AppointmentsOutput> {
    let selection = open_patient_sessions(context, args.patient, args.all_accounts)?;
    let floor = current_utc_date_string();
    let through = resolve_until_ceiling(Some(&args.next), 30)?;
    let normalized_query = normalize_match_text(&args.query.join(" "));

    let mut appointments = Vec::new();
    let mut warnings = BTreeSet::new();
    for session in &selection.sessions {
        if let Some(reason) = session.resource_unavailable_reason("Appointment") {
            warnings.insert(reason);
        }
        appointments.extend(
            load_upcoming_appointments(session, args.limit.max(50), args.all_pages, &floor)?
                .into_iter()
                .filter(|resource| {
                    let start = first_string(resource, &["/start"]).unwrap_or_default();
                    iso_on_or_before(&start, &through)
                        && (normalized_query.is_empty() || appointment_matches_query(resource, &normalized_query))
                })
                .filter_map(|resource| {
                    let mut appointment = render_appointment(&resource)?;
                    appointment.account = session.account_name.clone();
                    appointment.provider = session.provider_name.clone();
                    appointment.patient_id = session.patient_id.clone();
                    Some(appointment)
                }),
        );
    }

    appointments.sort_by(|left, right| left.start.cmp(&right.start));
    appointments.truncate(args.limit);

    Ok(AppointmentsOutput {
        status: "ok".into(),
        patient_id: single_patient_id(&selection),
        accounts_used: deserialize_account_usage(accounts_used_json(&selection))?,
        accounts_skipped: deserialize_skipped_accounts(selection.skipped_accounts)?,
        warnings: warnings.into_iter().collect(),
        since: None,
        query: (!args.query.is_empty()).then(|| args.query.join(" ")),
        through: Some(through),
        appointments,
    })
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

fn render_appointment(resource: &Value) -> Option<AppointmentRecord> {
    let start = first_string(resource, &["/start"])?;
    let status = first_string(resource, &["/status"]).unwrap_or_else(|| "unknown".into());
    let specialty = resource
        .get("specialty")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(concept_text).collect::<Vec<_>>())
        .unwrap_or_default();
    let participants = resource
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

    Some(AppointmentRecord {
        account: String::new(),
        provider: String::new(),
        patient_id: String::new(),
        id: first_string(resource, &["/id"]),
        status,
        start,
        end: first_string(resource, &["/end"]),
        description: appointment_description(resource),
        specialty,
        location: participants.first().cloned(),
        participants,
    })
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

fn render_output(output: AppointmentsOutput) -> Result<Value> {
    serde_json::to_value(output)
        .map_err(|error| Error::Config(format!("failed to serialize appointments output: {error}")))
}

fn deserialize_account_usage(value: Vec<Value>) -> Result<Vec<AccountUsage>> {
    serde_json::from_value(Value::Array(value))
        .map_err(|error| Error::Config(format!("failed to materialize account usage output: {error}")))
}

fn deserialize_skipped_accounts(value: Vec<Value>) -> Result<Vec<SkippedAccount>> {
    serde_json::from_value(Value::Array(value))
        .map_err(|error| Error::Config(format!("failed to materialize skipped account output: {error}")))
}
