use std::collections::BTreeMap;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{
    commands::shared::{
        bundle_entries, concept_text, first_string, iso_on_or_after, normalize_match_text, open_patient_session,
        resolve_since_floor,
    },
    Result,
};

#[derive(Debug, Args)]
pub(crate) struct MedsCommand {
    #[command(subcommand)]
    pub(crate) command: MedsSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MedsSubcommand {
    Reconcile(MedsReconcileArgs),
}

#[derive(Debug, Args)]
pub(crate) struct MedsReconcileArgs {
    #[arg(long)]
    patient: Option<String>,

    #[arg(long)]
    since: Option<String>,

    #[arg(long, default_value_t = 100)]
    limit: usize,

    #[arg(long)]
    all_pages: bool,
}

pub(crate) fn run_meds(command: MedsSubcommand, context: &crate::state::ResolvedContext) -> Result<Value> {
    match command {
        MedsSubcommand::Reconcile(args) => run_reconcile(args, context),
    }
}

fn run_reconcile(args: MedsReconcileArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let session = open_patient_session(context, args.patient)?;
    let floor = resolve_since_floor(args.since.as_deref())?;
    let medications = session
        .search_resource(
            "MedicationRequest",
            &[("_count".into(), args.limit.max(100).to_string())],
            args.all_pages,
        )?
        .map(|bundle| bundle_entries(&bundle))
        .unwrap_or_default();

    let mut duplicates = BTreeMap::<String, usize>::new();
    let mut reconciled = medications
        .into_iter()
        .filter_map(|resource| {
            let authored_on = first_string(&resource, &["/authoredOn", "/meta/lastUpdated"])?;
            if floor
                .as_deref()
                .is_some_and(|floor| !iso_on_or_after(&authored_on, floor))
            {
                return None;
            }

            let name = resource
                .pointer("/medicationCodeableConcept")
                .and_then(concept_text)
                .or_else(|| first_string(&resource, &["/medicationReference/display"]))
                .unwrap_or_else(|| "Unknown medication".into());
            *duplicates.entry(normalize_match_text(&name)).or_default() += 1;

            Some(json!({
                "id": first_string(&resource, &["/id"]),
                "name": name,
                "status": first_string(&resource, &["/status"]),
                "intent": first_string(&resource, &["/intent"]),
                "authored_on": authored_on,
                "dosage": resource
                    .get("dosageInstruction")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find_map(|dose| dose.get("text").and_then(Value::as_str))
                    .map(ToOwned::to_owned),
                "prescriber": first_string(&resource, &["/requester/display"]),
            }))
        })
        .collect::<Vec<_>>();

    reconciled.sort_by(|left, right| {
        right
            .get("authored_on")
            .and_then(Value::as_str)
            .cmp(&left.get("authored_on").and_then(Value::as_str))
    });

    Ok(json!({
        "status": "ok",
        "patient_id": session.patient_id,
        "medications": reconciled,
        "duplicate_name_candidates": duplicates
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(name, count)| json!({"name": name, "count": count}))
            .collect::<Vec<_>>(),
    }))
}
