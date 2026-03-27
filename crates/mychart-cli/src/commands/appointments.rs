use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{
    commands::shared::{
        concept_text, current_utc_date_string, first_string, iso_on_or_after, open_patient_session, resolve_since_floor,
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

pub(crate) fn run_appointments(
    command: AppointmentsSubcommand,
    context: &crate::state::ResolvedContext,
) -> Result<Value> {
    match command {
        AppointmentsSubcommand::Upcoming(args) => run_upcoming(args, context),
    }
}

fn run_upcoming(args: AppointmentsUpcomingArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let session = open_patient_session(context, args.patient)?;
    let floor = resolve_since_floor(args.since.as_deref())?.unwrap_or_else(current_utc_date_string);
    let mut query = vec![("_count".into(), args.limit.max(25).to_string())];
    if session
        .resource("Appointment")
        .is_some_and(|resource| resource.search_params.iter().any(|parameter| parameter.name == "date"))
    {
        query.push(("date".into(), format!("ge{floor}")));
    }

    let appointments = session
        .search_resource("Appointment", &query, args.all_pages)?
        .map(|bundle| crate::commands::shared::bundle_entries(&bundle))
        .unwrap_or_default();

    let mut upcoming = appointments
        .into_iter()
        .filter_map(|resource| {
            let start = first_string(&resource, &["/start"])?;
            if !iso_on_or_after(&start, &floor) {
                return None;
            }
            let status = first_string(&resource, &["/status"]).unwrap_or_else(|| "unknown".into());
            if matches!(
                status.as_str(),
                "cancelled" | "noshow" | "entered-in-error" | "fulfilled" | "checked-in"
            ) {
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
    upcoming.truncate(args.limit);

    Ok(json!({
        "status": "ok",
        "patient_id": session.patient_id,
        "since": floor,
        "appointments": upcoming,
    }))
}
