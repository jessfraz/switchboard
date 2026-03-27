use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Method;
use serde_json::{json, Value};

use crate::{
    api_client,
    client::JsonResponse,
    ensure_json_success, fetch_capability_summary, merge_bundle_pages, normalize_token,
    state::{MyChartAccountState, ResolvedContext},
    ApiResourceCapability, CapabilitySummary, Error, Result,
};

pub(crate) struct PatientSession {
    pub(crate) account_name: String,
    pub(crate) provider_name: String,
    pub(crate) client: crate::client::MyChartClient,
    pub(crate) capability: CapabilitySummary,
    pub(crate) access_token: String,
    pub(crate) patient_id: String,
}

pub(crate) struct PatientSessionSelection {
    pub(crate) sessions: Vec<PatientSession>,
    pub(crate) skipped_accounts: Vec<Value>,
}

impl PatientSession {
    pub(crate) fn read_resource(&self, resource_token: &str, id: &str) -> Result<Option<Value>> {
        let Some(resource) = self.capability.resolve_resource(resource_token) else {
            return Ok(None);
        };

        if resource.supports("read") {
            let response = self.client.execute_bearer_json(
                Method::GET,
                &format!("{}/{}", resource.resource_type, id),
                &[],
                &self.access_token,
                None,
            )?;
            ensure_json_success(&response)?;
            return Ok(Some(response.body));
        }

        if resource.supports("search-type") {
            return Ok(self
                .search_resource(resource_token, &[("_id".into(), id.to_owned())], false)?
                .and_then(|bundle| {
                    bundle_entries(&bundle)
                        .into_iter()
                        .find(|resource| first_string(resource, &["/id"]).as_deref() == Some(id))
                }));
        }

        Ok(None)
    }

    pub(crate) fn search_resource(
        &self,
        resource_token: &str,
        extra_query: &[(String, String)],
        all_pages: bool,
    ) -> Result<Option<Value>> {
        let Some(resource) = self.capability.resolve_resource(resource_token) else {
            return Ok(None);
        };
        if !resource.supports("search-type") {
            return Ok(None);
        }

        let patient_param = patient_search_param(&resource).ok_or_else(|| {
            Error::Config(format!(
                "{} is available but this endpoint did not advertise a patient or subject search parameter",
                resource.resource_type
            ))
        })?;

        let mut query = vec![(patient_param.into(), self.patient_id.clone())];
        query.extend(extra_query.iter().cloned());
        let response =
            self.client
                .execute_bearer_json(Method::GET, &resource.resource_type, &query, &self.access_token, None)?;
        ensure_json_success(&response)?;
        if all_pages {
            let (body, _) = merge_bundle_pages(&self.client, &response.body, &self.access_token)?;
            Ok(Some(body))
        } else {
            Ok(Some(response.body))
        }
    }

    pub(crate) fn resource(&self, resource_token: &str) -> Option<ApiResourceCapability> {
        self.capability.resolve_resource(resource_token)
    }

    pub(crate) fn fetch_url(&self, url: &str) -> Result<JsonResponse> {
        if url.starts_with("http://") || url.starts_with("https://") {
            self.client
                .execute_bearer_json_absolute(Method::GET, url, &self.access_token, None)
        } else {
            self.client
                .execute_bearer_json(Method::GET, url, &[], &self.access_token, None)
        }
    }
}

pub(crate) fn open_patient_session(
    context: &ResolvedContext,
    patient_override: Option<String>,
) -> Result<PatientSession> {
    let persisted = context.describe_account(None).map(|(_, state)| state);
    build_patient_session(
        context.account.clone(),
        persisted.as_ref(),
        context.api_base_url.clone(),
        context.access_token.clone(),
        patient_override.or_else(|| context.patient_id.clone()),
    )?
    .ok_or_else(|| {
        Error::Config("missing MyChart API session for the active account, connect and authenticate it first".into())
    })
}

pub(crate) fn open_patient_sessions(
    context: &ResolvedContext,
    patient_override: Option<String>,
    all_accounts: bool,
) -> Result<PatientSessionSelection> {
    if !all_accounts {
        return Ok(PatientSessionSelection {
            sessions: vec![open_patient_session(context, patient_override)?],
            skipped_accounts: Vec::new(),
        });
    }

    if patient_override.is_some() {
        return Err(Error::Arguments(
            "--patient cannot be combined with --all-accounts/--all-providers because patient ids are provider-specific"
                .into(),
        ));
    }

    let mut sessions = Vec::new();
    let mut skipped_accounts = Vec::new();
    let accounts = context.list_accounts();
    if accounts.is_empty() {
        return Ok(PatientSessionSelection {
            sessions: vec![open_patient_session(context, None)?],
            skipped_accounts,
        });
    }

    for (account_name, account_state) in accounts {
        match build_patient_session(
            account_name.clone(),
            Some(&account_state),
            account_state.api_base_url.clone(),
            account_state.access_token.clone(),
            account_state.patient_id.clone(),
        ) {
            Ok(Some(session)) => sessions.push(session),
            Ok(None) => skipped_accounts.push(json!({
                "account": account_name,
                "provider": provider_name(&account_name, Some(&account_state)),
                "reason": skipped_account_reason(&account_state),
            })),
            Err(error) => skipped_accounts.push(json!({
                "account": account_name,
                "provider": provider_name(&account_name, Some(&account_state)),
                "reason": error.to_string(),
            })),
        }
    }

    if sessions.is_empty() {
        return Err(Error::Config(
            "none of the saved MyChart accounts were ready for API use, authenticate them first".into(),
        ));
    }

    Ok(PatientSessionSelection {
        sessions,
        skipped_accounts,
    })
}

pub(crate) fn accounts_used_json(selection: &PatientSessionSelection) -> Vec<Value> {
    selection
        .sessions
        .iter()
        .map(|session| {
            json!({
                "account": session.account_name.clone(),
                "provider": session.provider_name.clone(),
                "patient_id": session.patient_id.clone(),
            })
        })
        .collect()
}

pub(crate) fn single_patient_id(selection: &PatientSessionSelection) -> Option<String> {
    (selection.sessions.len() == 1).then(|| selection.sessions[0].patient_id.clone())
}

fn build_patient_session(
    account_name: String,
    account_state: Option<&MyChartAccountState>,
    base_url: Option<String>,
    access_token: Option<String>,
    patient_id: Option<String>,
) -> Result<Option<PatientSession>> {
    let Some(base_url) = base_url else {
        return Ok(None);
    };
    let Some(access_token) = access_token else {
        return Ok(None);
    };
    let Some(patient_id) = patient_id else {
        return Ok(None);
    };

    let client = api_client(&base_url)?;
    let capability = fetch_capability_summary(&client)?;
    Ok(Some(PatientSession {
        account_name: account_name.clone(),
        provider_name: provider_name(&account_name, account_state),
        client,
        capability,
        access_token,
        patient_id,
    }))
}

fn provider_name(account_name: &str, account_state: Option<&MyChartAccountState>) -> String {
    account_state
        .and_then(|account_state| {
            account_state
                .discovery
                .as_ref()
                .and_then(|discovery| discovery.brand_name.clone())
                .or_else(|| {
                    account_state
                        .discovery
                        .as_ref()
                        .and_then(|discovery| discovery.managing_organization_name.clone())
                })
        })
        .unwrap_or_else(|| account_name.to_owned())
}

fn skipped_account_reason(account_state: &MyChartAccountState) -> &'static str {
    if account_state.api_base_url.is_none() {
        "missing base url"
    } else if account_state.access_token.is_none() {
        "missing access token"
    } else if account_state.patient_id.is_none() {
        "missing patient id"
    } else {
        "account is not ready for API use"
    }
}

pub(crate) fn bundle_entries(body: &Value) -> Vec<Value> {
    body.get("entry")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("resource").cloned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) fn resolve_since_floor(input: Option<&str>) -> Result<Option<String>> {
    input
        .map(|input| {
            if looks_like_iso_date(input) {
                Ok(input.to_owned())
            } else {
                parse_relative_period(input).map(days_ago_iso_date)
            }
        })
        .transpose()
}

pub(crate) fn current_utc_date_string() -> String {
    days_ago_iso_date(0)
}

pub(crate) fn resolve_until_ceiling(input: Option<&str>, default_relative_days: u64) -> Result<String> {
    match input {
        Some(input) if looks_like_iso_date(input) => Ok(input.to_owned()),
        Some(input) => parse_relative_period(input).map(days_from_now_iso_date),
        None => Ok(days_from_now_iso_date(default_relative_days)),
    }
}

pub(crate) fn iso_on_or_after(candidate: &str, floor: &str) -> bool {
    candidate.chars().take(10).collect::<String>().as_str() >= floor
}

pub(crate) fn first_string(value: &Value, paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

pub(crate) fn concept_text(value: &Value) -> Option<String> {
    value
        .get("text")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("coding")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find_map(|coding| {
                    coding
                        .get("display")
                        .and_then(Value::as_str)
                        .or_else(|| coding.get("code").and_then(Value::as_str))
                })
                .map(ToOwned::to_owned)
        })
}

pub(crate) fn normalize_match_text(input: &str) -> String {
    normalize_token(input)
}

pub(crate) fn value_summary(resource: &Value) -> Option<String> {
    if let Some(quantity) = resource.get("valueQuantity") {
        let value = quantity.get("value").and_then(Value::as_f64)?;
        let unit = quantity.get("unit").and_then(Value::as_str).unwrap_or_default();
        return Some(if unit.is_empty() {
            trim_float(value)
        } else {
            format!("{} {}", trim_float(value), unit)
        });
    }
    first_string(
        resource,
        &[
            "/valueString",
            "/valueCodeableConcept/text",
            "/valueCodeableConcept/coding/0/display",
            "/valueCodeableConcept/coding/0/code",
            "/valueInteger",
            "/valueBoolean",
        ],
    )
}

pub(crate) fn interpretation_summary(resource: &Value) -> Option<String> {
    resource
        .get("interpretation")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(concept_text)
}

pub(crate) fn observation_effective_date(resource: &Value) -> Option<String> {
    first_string(
        resource,
        &[
            "/effectiveDateTime",
            "/issued",
            "/effectivePeriod/start",
            "/meta/lastUpdated",
        ],
    )
}

pub(crate) fn resource_timestamp(resource: &Value) -> Option<String> {
    match resource.get("resourceType").and_then(Value::as_str) {
        Some("Appointment") => first_string(resource, &["/start", "/created", "/meta/lastUpdated"]),
        Some("Encounter") => first_string(resource, &["/period/start", "/meta/lastUpdated"]),
        Some("Observation") => observation_effective_date(resource),
        Some("DiagnosticReport") => first_string(
            resource,
            &[
                "/effectiveDateTime",
                "/issued",
                "/effectivePeriod/start",
                "/meta/lastUpdated",
            ],
        ),
        Some("MedicationRequest") => first_string(resource, &["/authoredOn", "/meta/lastUpdated"]),
        Some("DocumentReference") => first_string(resource, &["/date", "/meta/lastUpdated"]),
        Some("ExplanationOfBenefit") => {
            first_string(resource, &["/billablePeriod/start", "/created", "/meta/lastUpdated"])
        }
        _ => first_string(resource, &["/meta/lastUpdated"]),
    }
}

fn patient_search_param(resource: &ApiResourceCapability) -> Option<&'static str> {
    if resource
        .search_params
        .iter()
        .any(|parameter| parameter.name == "patient")
    {
        Some("patient")
    } else if resource
        .search_params
        .iter()
        .any(|parameter| parameter.name == "subject")
    {
        Some("subject")
    } else {
        None
    }
}

fn looks_like_iso_date(input: &str) -> bool {
    input.len() == 10
        && input.chars().enumerate().all(|(index, character)| match index {
            4 | 7 => character == '-',
            _ => character.is_ascii_digit(),
        })
}

fn parse_relative_period(input: &str) -> Result<u64> {
    let trimmed = input.trim().to_ascii_lowercase();
    let (amount, unit) = trimmed
        .chars()
        .last()
        .map(|unit| (&trimmed[..trimmed.len().saturating_sub(1)], unit))
        .ok_or_else(|| Error::Arguments("empty relative period".into()))?;
    let amount = amount.parse::<u64>().map_err(|_| {
        Error::Arguments(format!(
            "invalid relative period {input:?}, use YYYY-MM-DD or a shorthand like 30d, 12w, 6m, or 2y"
        ))
    })?;

    let days = match unit {
        'd' => amount,
        'w' => amount.saturating_mul(7),
        'm' => amount.saturating_mul(30),
        'y' => amount.saturating_mul(365),
        _ => {
            return Err(Error::Arguments(format!(
                "invalid relative period unit in {input:?}, use d, w, m, or y"
            )))
        }
    };

    Ok(days)
}

fn days_ago_iso_date(days_ago: u64) -> String {
    shifted_iso_date(-(days_ago as i64))
}

fn days_from_now_iso_date(days_from_now: u64) -> String {
    shifted_iso_date(days_from_now as i64)
}

fn shifted_iso_date(day_offset: i64) -> String {
    let unix_days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let shifted_days = unix_days.saturating_add(day_offset).max(0);
    let (year, month, day) = civil_from_days(shifted_days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_from_days(unix_days: i64) -> (i32, u32, u32) {
    let z = unix_days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn trim_float(value: f64) -> String {
    let mut rendered = format!("{value}");
    if rendered.contains('.') {
        while rendered.ends_with('0') {
            rendered.pop();
        }
        if rendered.ends_with('.') {
            rendered.pop();
        }
    }
    rendered
}
