use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Method;
use serde_json::Value;

use crate::{
    api_client, ensure_json_success, fetch_capability_summary, merge_bundle_pages, normalize_token,
    state::ResolvedContext, ApiResourceCapability, CapabilitySummary, Error, Result,
};

pub(crate) struct PatientSession {
    pub(crate) client: crate::client::MyChartClient,
    pub(crate) capability: CapabilitySummary,
    pub(crate) access_token: String,
    pub(crate) patient_id: String,
}

impl PatientSession {
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
}

pub(crate) fn open_patient_session(
    context: &ResolvedContext,
    patient_override: Option<String>,
) -> Result<PatientSession> {
    let base_url = context.require_api_base_url()?;
    let access_token = context.require_access_token(None)?;
    let patient_id = patient_override.or_else(|| context.patient_id.clone()).ok_or_else(|| {
        Error::Config("missing patient id, use SMART auth that returns one or pass --patient explicitly".into())
    })?;
    let client = api_client(&base_url)?;
    let capability = fetch_capability_summary(&client)?;
    Ok(PatientSession {
        client,
        capability,
        access_token,
        patient_id,
    })
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
    let unix_days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or(0)
        .saturating_sub(days_ago);
    let (year, month, day) = civil_from_days(unix_days as i64);
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
