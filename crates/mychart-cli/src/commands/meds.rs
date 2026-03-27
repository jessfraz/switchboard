use std::collections::BTreeMap;

use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::Value;

use crate::{
    commands::{
        appointments::{AccountUsage, SkippedAccount},
        shared::{
            accounts_used_json, bundle_entries, concept_text, first_string, iso_on_or_after, normalize_match_text,
            open_patient_sessions, resolve_since_floor, single_patient_id,
        },
    },
    Error, Result,
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

#[derive(Debug, Args, Clone)]
pub(crate) struct MedsReconcileArgs {
    #[arg(long)]
    pub(crate) patient: Option<String>,

    #[arg(long, alias = "all-providers")]
    pub(crate) all_accounts: bool,

    #[arg(long)]
    pub(crate) since: Option<String>,

    #[arg(long, default_value_t = 100)]
    pub(crate) limit: usize,

    #[arg(long)]
    pub(crate) all_pages: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MedicationRecord {
    pub(crate) account: String,
    pub(crate) provider: String,
    pub(crate) patient_id: String,
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) status: Option<String>,
    pub(crate) intent: Option<String>,
    pub(crate) authored_on: String,
    pub(crate) dosage: Option<String>,
    pub(crate) prescriber: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DuplicateNameCandidate {
    pub(crate) name: String,
    pub(crate) count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MedsReconcileOutput {
    pub(crate) status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) patient_id: Option<String>,
    pub(crate) accounts_used: Vec<AccountUsage>,
    pub(crate) accounts_skipped: Vec<SkippedAccount>,
    pub(crate) medications: Vec<MedicationRecord>,
    pub(crate) duplicate_name_candidates: Vec<DuplicateNameCandidate>,
}

pub(crate) fn run_meds(command: MedsSubcommand, context: &crate::state::ResolvedContext) -> Result<Value> {
    match command {
        MedsSubcommand::Reconcile(args) => render_output(run_reconcile_output(args, context)?),
    }
}

pub(crate) fn run_reconcile_output(
    args: MedsReconcileArgs,
    context: &crate::state::ResolvedContext,
) -> Result<MedsReconcileOutput> {
    let selection = open_patient_sessions(context, args.patient, args.all_accounts)?;
    let floor = resolve_since_floor(args.since.as_deref())?;

    let mut duplicates = BTreeMap::<String, usize>::new();
    let mut reconciled = Vec::new();

    for session in &selection.sessions {
        let medications = session
            .search_resource(
                "MedicationRequest",
                &[("_count".into(), args.limit.max(100).to_string())],
                args.all_pages,
            )?
            .map(|bundle| bundle_entries(&bundle))
            .unwrap_or_default();

        reconciled.extend(medications.into_iter().filter_map(|resource| {
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

            Some(MedicationRecord {
                account: session.account_name.clone(),
                provider: session.provider_name.clone(),
                patient_id: session.patient_id.clone(),
                id: first_string(&resource, &["/id"]),
                name,
                status: first_string(&resource, &["/status"]),
                intent: first_string(&resource, &["/intent"]),
                authored_on,
                dosage: resource
                    .get("dosageInstruction")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find_map(|dose| dose.get("text").and_then(Value::as_str))
                    .map(ToOwned::to_owned),
                prescriber: first_string(&resource, &["/requester/display"]),
            })
        }));
    }

    reconciled.sort_by(|left, right| right.authored_on.cmp(&left.authored_on));

    Ok(MedsReconcileOutput {
        status: "ok".into(),
        patient_id: single_patient_id(&selection),
        accounts_used: deserialize_account_usage(accounts_used_json(&selection))?,
        accounts_skipped: deserialize_skipped_accounts(selection.skipped_accounts)?,
        medications: reconciled,
        duplicate_name_candidates: duplicates
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(name, count)| DuplicateNameCandidate { name, count })
            .collect(),
    })
}

fn render_output(output: MedsReconcileOutput) -> Result<Value> {
    serde_json::to_value(output).map_err(|error| Error::Config(format!("failed to serialize meds output: {error}")))
}

fn deserialize_account_usage(value: Vec<Value>) -> Result<Vec<AccountUsage>> {
    serde_json::from_value(Value::Array(value))
        .map_err(|error| Error::Config(format!("failed to materialize account usage output: {error}")))
}

fn deserialize_skipped_accounts(value: Vec<Value>) -> Result<Vec<SkippedAccount>> {
    serde_json::from_value(Value::Array(value))
        .map_err(|error| Error::Config(format!("failed to materialize skipped account output: {error}")))
}
