use std::collections::BTreeMap;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{
    commands::shared::{
        accounts_used_json, bundle_entries, concept_text, first_string, iso_on_or_after, open_patient_sessions,
        resolve_since_floor, single_patient_id,
    },
    Result,
};

#[derive(Debug, Args)]
pub(crate) struct ClaimsCommand {
    #[command(subcommand)]
    pub(crate) command: ClaimsSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ClaimsSubcommand {
    Audit(ClaimsAuditArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ClaimsAuditArgs {
    #[arg(long)]
    patient: Option<String>,

    #[arg(long, alias = "all-providers")]
    all_accounts: bool,

    #[arg(long)]
    since: Option<String>,

    #[arg(long, default_value_t = 100)]
    limit: usize,

    #[arg(long)]
    all_pages: bool,
}

pub(crate) fn run_claims(command: ClaimsSubcommand, context: &crate::state::ResolvedContext) -> Result<Value> {
    match command {
        ClaimsSubcommand::Audit(args) => run_audit(args, context),
    }
}

fn run_audit(args: ClaimsAuditArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let selection = open_patient_sessions(context, args.patient, args.all_accounts)?;
    let floor = resolve_since_floor(args.since.as_deref())?;

    let mut duplicate_index = BTreeMap::<String, Vec<Value>>::new();
    let mut denied = Vec::new();
    let mut claims_out = Vec::new();

    for session in &selection.sessions {
        let claims = session
            .search_resource(
                "ExplanationOfBenefit",
                &[("_count".into(), args.limit.max(100).to_string())],
                args.all_pages,
            )?
            .map(|bundle| bundle_entries(&bundle))
            .unwrap_or_default();

        for resource in claims {
            let service_date = first_string(
                &resource,
                &[
                    "/billablePeriod/start",
                    "/created",
                    "/item/0/servicedDate",
                    "/item/0/servicedPeriod/start",
                    "/meta/lastUpdated",
                ],
            )
            .unwrap_or_else(|| "unknown".into());
            if floor
                .as_deref()
                .is_some_and(|floor| !iso_on_or_after(&service_date, floor))
            {
                continue;
            }

            let provider =
                first_string(&resource, &["/provider/display"]).unwrap_or_else(|| session.provider_name.clone());
            let claim_use = first_string(&resource, &["/use"]).unwrap_or_else(|| "claim".into());
            let outcome = first_string(&resource, &["/outcome"]).unwrap_or_else(|| "unknown".into());
            let total_amount = total_amount(&resource);
            let total_currency = total_currency(&resource);
            let service_labels = service_labels(&resource);
            let duplicate_key = format!(
                "{}|{}|{}|{}",
                provider.to_ascii_lowercase(),
                service_labels.join("|").to_ascii_lowercase(),
                service_date.chars().take(10).collect::<String>(),
                total_amount
                    .map(|value| format!("{value:.2}"))
                    .unwrap_or_else(|| "unknown".into())
            );

            let rendered = json!({
                "account": session.account_name.clone(),
                "provider_account": session.provider_name.clone(),
                "patient_id": session.patient_id.clone(),
                "id": first_string(&resource, &["/id"]),
                "status": first_string(&resource, &["/status"]),
                "use": claim_use,
                "outcome": outcome,
                "service_date": service_date,
                "provider": provider,
                "services": service_labels,
                "total_amount": total_amount,
                "currency": total_currency,
            });

            if looks_denied(&rendered) {
                denied.push(rendered.clone());
            }
            duplicate_index.entry(duplicate_key).or_default().push(rendered.clone());
            claims_out.push(rendered);
        }
    }

    claims_out.sort_by(|left, right| {
        right
            .get("service_date")
            .and_then(Value::as_str)
            .cmp(&left.get("service_date").and_then(Value::as_str))
    });

    let duplicate_candidates = duplicate_index
        .into_values()
        .filter(|group| group.len() > 1)
        .map(|group| {
            json!({
                "count": group.len(),
                "claims": group,
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "status": "ok",
        "patient_id": single_patient_id(&selection),
        "accounts_used": accounts_used_json(&selection),
        "accounts_skipped": selection.skipped_accounts,
        "claim_count": claims_out.len(),
        "claims": claims_out,
        "duplicate_charge_candidates": duplicate_candidates,
        "denied_or_problematic_claims": denied,
    }))
}

fn total_amount(resource: &Value) -> Option<f64> {
    resource
        .get("total")
        .and_then(Value::as_array)
        .and_then(|totals| totals.first())
        .and_then(|total| total.pointer("/amount/value"))
        .and_then(Value::as_f64)
        .or_else(|| resource.pointer("/payment/amount/value").and_then(Value::as_f64))
}

fn total_currency(resource: &Value) -> Option<String> {
    first_string(resource, &["/total/0/amount/currency", "/payment/amount/currency"])
}

fn service_labels(resource: &Value) -> Vec<String> {
    let services = resource
        .get("item")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.pointer("/productOrService").and_then(concept_text))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if services.is_empty() {
        vec!["unknown-service".into()]
    } else {
        services
    }
}

fn looks_denied(claim: &Value) -> bool {
    let status = claim
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let outcome = claim
        .get("outcome")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(status.as_str(), "cancelled" | "entered-in-error" | "draft")
        || outcome.contains("error")
        || outcome.contains("partial")
        || outcome.contains("denied")
}
