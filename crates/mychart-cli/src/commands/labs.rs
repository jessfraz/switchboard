use std::{collections::BTreeMap, ffi::OsString};

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{
    commands::shared::{
        accounts_used_json, bundle_entries, concept_text, interpretation_summary, iso_on_or_after,
        normalize_match_text, observation_effective_date, open_patient_sessions, resolve_since_floor,
        single_patient_id, value_summary,
    },
    DynamicArgs, Error, Result,
};

#[derive(Debug, Args)]
pub(crate) struct LabsCommand {
    #[command(subcommand)]
    pub(crate) command: LabsSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LabsSubcommand {
    Trend(LabsTrendArgs),
    Abnormals(LabsAbnormalsArgs),
    #[command(external_subcommand)]
    Shorthand(Vec<OsString>),
}

#[derive(Debug, Args, Clone)]
pub(crate) struct LabsTrendArgs {
    #[arg(long = "query", value_name = "TEXT")]
    query: Vec<String>,

    #[arg(long)]
    patient: Option<String>,

    #[arg(long, alias = "all-providers")]
    all_accounts: bool,

    #[arg(long)]
    since: Option<String>,

    #[arg(long)]
    spark: bool,

    #[arg(long, default_value_t = 100)]
    limit: usize,

    #[arg(long)]
    all_pages: bool,
}

#[derive(Debug, Args)]
pub(crate) struct LabsAbnormalsArgs {
    #[arg(long)]
    patient: Option<String>,

    #[arg(long, alias = "all-providers")]
    all_accounts: bool,

    #[arg(long)]
    since: Option<String>,

    #[arg(long)]
    new: bool,

    #[arg(long, default_value_t = 50)]
    limit: usize,

    #[arg(long)]
    all_pages: bool,
}

pub(crate) fn run_labs(command: LabsSubcommand, context: &crate::state::ResolvedContext) -> Result<Value> {
    match command {
        LabsSubcommand::Trend(args) => run_trend(args, context),
        LabsSubcommand::Abnormals(args) => run_abnormals(args, context),
        LabsSubcommand::Shorthand(tokens) => run_trend(parse_shorthand(tokens)?, context),
    }
}

fn run_trend(args: LabsTrendArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let selection = open_patient_sessions(context, args.patient, args.all_accounts)?;
    let floor = resolve_since_floor(args.since.as_deref())?;
    let mut query = vec![("_count".into(), args.limit.max(100).to_string())];
    if selection.sessions.iter().any(|session| {
        session.resource("Observation").is_some_and(|resource| {
            resource
                .search_params
                .iter()
                .any(|parameter| parameter.name == "category")
        })
    }) {
        query.push(("category".into(), "laboratory".into()));
    }
    if let Some(floor) = floor.as_deref() {
        if selection.sessions.iter().any(|session| {
            session
                .resource("Observation")
                .is_some_and(|resource| resource.search_params.iter().any(|parameter| parameter.name == "date"))
        }) {
            query.push(("date".into(), format!("ge{floor}")));
        }
    }

    let analytes = args
        .query
        .iter()
        .map(|query| normalize_match_text(query))
        .collect::<Vec<_>>();
    let mut series = BTreeMap::<String, Vec<Value>>::new();
    for session in &selection.sessions {
        let observations = session
            .search_resource("Observation", &query, args.all_pages)?
            .map(|bundle| bundle_entries(&bundle))
            .unwrap_or_default();

        for resource in observations {
            let label = observation_label(&resource);
            let normalized_label = normalize_match_text(&label);
            if !analytes.is_empty() && !analytes.iter().any(|query| normalized_label.contains(query)) {
                continue;
            }
            let observed_at = observation_effective_date(&resource).unwrap_or_else(|| "unknown".into());
            if floor
                .as_deref()
                .is_some_and(|floor| !iso_on_or_after(&observed_at, floor))
            {
                continue;
            }
            let value = value_summary(&resource);
            let numeric_value = resource.pointer("/valueQuantity/value").and_then(Value::as_f64);
            series.entry(label.clone()).or_default().push(json!({
                "account": session.account_name.clone(),
                "provider": session.provider_name.clone(),
                "patient_id": session.patient_id.clone(),
                "id": resource.get("id").and_then(Value::as_str),
                "observed_at": observed_at,
                "value": value,
                "numeric_value": numeric_value,
                "interpretation": interpretation_summary(&resource),
            }));
        }
    }

    let mut output = series
        .into_iter()
        .map(|(label, mut points)| {
            points.sort_by(|left, right| {
                left.get("observed_at")
                    .and_then(Value::as_str)
                    .cmp(&right.get("observed_at").and_then(Value::as_str))
            });
            let spark = if args.spark {
                build_sparkline(&points)
            } else {
                String::new()
            };
            json!({
                "label": label,
                "point_count": points.len(),
                "spark": if args.spark { Value::String(spark) } else { Value::Null },
                "points": points,
            })
        })
        .collect::<Vec<_>>();

    output.sort_by(|left, right| {
        left.get("label")
            .and_then(Value::as_str)
            .cmp(&right.get("label").and_then(Value::as_str))
    });

    Ok(json!({
        "status": "ok",
        "patient_id": single_patient_id(&selection),
        "accounts_used": accounts_used_json(&selection),
        "accounts_skipped": selection.skipped_accounts,
        "queries": args.query,
        "series": output,
    }))
}

fn run_abnormals(args: LabsAbnormalsArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let selection = open_patient_sessions(context, args.patient, args.all_accounts)?;
    let floor = resolve_since_floor(args.since.as_deref())?;
    let mut query = vec![("_count".into(), args.limit.max(100).to_string())];
    if selection.sessions.iter().any(|session| {
        session.resource("Observation").is_some_and(|resource| {
            resource
                .search_params
                .iter()
                .any(|parameter| parameter.name == "category")
        })
    }) {
        query.push(("category".into(), "laboratory".into()));
    }

    let mut grouped = BTreeMap::<String, Value>::new();
    let mut abnormal_results = Vec::new();
    for session in &selection.sessions {
        let observations = session
            .search_resource("Observation", &query, args.all_pages)?
            .map(|bundle| bundle_entries(&bundle))
            .unwrap_or_default();
        abnormal_results.extend(observations.into_iter().filter_map(|resource| {
            let observed_at = observation_effective_date(&resource)?;
            if floor
                .as_deref()
                .is_some_and(|floor| !iso_on_or_after(&observed_at, floor))
            {
                return None;
            }
            let interpretation = interpretation_summary(&resource)?;
            if !looks_abnormal(&interpretation) {
                return None;
            }
            let label = observation_label(&resource);
            Some(json!({
                "account": session.account_name.clone(),
                "provider": session.provider_name.clone(),
                "patient_id": session.patient_id.clone(),
                "id": resource.get("id").and_then(Value::as_str),
                "label": label,
                "observed_at": observed_at,
                "value": value_summary(&resource),
                "interpretation": interpretation,
            }))
        }));
    }

    abnormal_results.sort_by(|left, right| {
        right
            .get("observed_at")
            .and_then(Value::as_str)
            .cmp(&left.get("observed_at").and_then(Value::as_str))
    });

    if args.new {
        for abnormal in abnormal_results {
            if let Some(label) = abnormal.get("label").and_then(Value::as_str) {
                grouped.entry(label.to_owned()).or_insert(abnormal);
            }
        }
        abnormal_results = grouped.into_values().collect();
    }

    abnormal_results.truncate(args.limit);

    Ok(json!({
        "status": "ok",
        "patient_id": single_patient_id(&selection),
        "accounts_used": accounts_used_json(&selection),
        "accounts_skipped": selection.skipped_accounts,
        "abnormals": abnormal_results,
    }))
}

fn parse_shorthand(tokens: Vec<OsString>) -> Result<LabsTrendArgs> {
    let tokens = tokens
        .into_iter()
        .map(|token| token.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut parsed = DynamicArgs::parse(&tokens)?;
    let patient = parsed.take_optional_single("patient")?;
    let all_accounts = parsed.take_flag("all-accounts") || parsed.take_flag("all-providers");
    let since = parsed.take_optional_single("since")?;
    let limit = parsed
        .take_optional_single("limit")?
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| Error::Arguments(format!("invalid --limit value {value:?}, expected a positive integer")))
        })
        .transpose()?
        .unwrap_or(100);
    let spark = parsed.take_flag("spark");
    let all_pages = parsed.take_flag("all-pages");
    let queries = std::mem::take(&mut parsed.positionals);
    if !parsed.options.is_empty() || !parsed.flags.is_empty() {
        return Err(Error::Arguments(
            "unsupported labs shorthand flags, use `mychart labs trend --help` for the boring grown-up form".into(),
        ));
    }

    Ok(LabsTrendArgs {
        query: queries,
        patient,
        all_accounts,
        since,
        spark,
        limit,
        all_pages,
    })
}

fn observation_label(resource: &Value) -> String {
    resource
        .pointer("/code")
        .and_then(concept_text)
        .unwrap_or_else(|| "Unknown lab".into())
}

fn looks_abnormal(interpretation: &str) -> bool {
    matches!(
        normalize_match_text(interpretation).as_str(),
        "h" | "hh" | "l" | "ll" | "a" | "aa" | "abnormal" | "critical" | "high" | "low"
    ) || interpretation.to_ascii_lowercase().contains("abnormal")
        || interpretation.to_ascii_lowercase().contains("high")
        || interpretation.to_ascii_lowercase().contains("low")
}

fn build_sparkline(points: &[Value]) -> String {
    const BLOCKS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let values = points
        .iter()
        .filter_map(|point| point.get("numeric_value").and_then(Value::as_f64))
        .collect::<Vec<_>>();
    if values.is_empty() {
        return String::new();
    }
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if (max - min).abs() < f64::EPSILON {
        return std::iter::repeat(BLOCKS[3]).take(values.len()).collect();
    }

    values
        .into_iter()
        .map(|value| {
            let normalized = ((value - min) / (max - min) * (BLOCKS.len() - 1) as f64).round() as usize;
            BLOCKS[normalized.min(BLOCKS.len() - 1)]
        })
        .collect()
}
