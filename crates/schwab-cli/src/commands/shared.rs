use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use reqwest::Method;
use serde_json::Value;

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    Error, ResolvedContext, Result,
};

#[derive(Debug, clap::Args)]
pub(crate) struct JsonBodyArgs {
    #[arg(long, value_name = "JSON", conflicts_with = "body_file")]
    pub(crate) body: Option<String>,

    #[arg(long, value_name = "PATH", conflicts_with = "body")]
    pub(crate) body_file: Option<PathBuf>,
}

const DEFAULT_LATEST_WINDOW_DAYS: i64 = 30;
const SECONDS_PER_DAY: i64 = 86_400;
const MILLIS_PER_SECOND: i64 = 1_000;
const DEFAULT_LATEST_WINDOW_MILLIS: i64 = DEFAULT_LATEST_WINDOW_DAYS * SECONDS_PER_DAY * MILLIS_PER_SECOND;

pub(crate) fn load_json_body(args: &JsonBodyArgs) -> Result<Value> {
    let raw = match (&args.body, &args.body_file) {
        (Some(body), None) => body.clone(),
        (None, Some(path)) => fs::read_to_string(path)
            .map_err(|error| Error::Io(format!("failed to read JSON body file {}: {error}", path.display())))?,
        (None, None) => {
            return Err(Error::Arguments(
                "missing request body, pass --body or --body-file".into(),
            ))
        }
        (Some(_), Some(_)) => return Err(Error::Arguments("pass only one of --body or --body-file".into())),
    };

    serde_json::from_str(&raw).map_err(|error| Error::Arguments(format!("invalid JSON request body: {error}")))
}

pub(crate) fn optional_query(query: &mut Vec<(String, String)>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        query.push((key.to_owned(), value));
    }
}

pub(crate) fn optional_bool_query(query: &mut Vec<(String, String)>, key: &str, value: bool) {
    if value {
        query.push((key.to_owned(), "true".into()));
    }
}

pub(crate) fn resolve_latest_rfc3339_window(start: Option<String>, end: Option<String>) -> Result<(String, String)> {
    resolve_latest_rfc3339_window_at(start, end, current_time_millis()?)
}

pub(crate) fn resolve_account_id(
    client: &SchwabClient,
    context: &mut ResolvedContext,
    account: &str,
) -> Result<String> {
    let trimmed = account.trim();
    if trimmed.is_empty() {
        return Err(Error::Arguments("account value may not be empty".into()));
    }

    if let Some(hash) = context.account_hash_for_plain_text(trimmed) {
        return Ok(hash.to_owned());
    }

    if !looks_like_plain_account_number(trimmed) {
        return Ok(trimmed.to_owned());
    }

    let response = client.execute(RequestSpec {
        method: Method::GET,
        path: "/accounts/accountNumbers".into(),
        query: Vec::new(),
        headers: context.trader_headers(),
        body: RequestBody::None,
        auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
    })?;
    context.remember_account_numbers(&response)?;

    if let Some(hash) = context.account_hash_for_plain_text(trimmed) {
        return Ok(hash.to_owned());
    }

    Err(Error::Arguments(format!(
        "Schwab did not return a hashed account mapping for plain account number {trimmed}"
    )))
}

pub(crate) fn resolve_all_account_ids(client: &SchwabClient, context: &mut ResolvedContext) -> Result<Vec<String>> {
    let mut hashes = context
        .account_number_cache()
        .iter()
        .filter_map(|entry| entry.hash_value.clone())
        .collect::<Vec<_>>();

    if hashes.is_empty() {
        let response = client.execute(RequestSpec {
            method: Method::GET,
            path: "/accounts/accountNumbers".into(),
            query: Vec::new(),
            headers: context.trader_headers(),
            body: RequestBody::None,
            auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
        })?;
        context.remember_account_numbers(&response)?;
        hashes = context
            .account_number_cache()
            .iter()
            .filter_map(|entry| entry.hash_value.clone())
            .collect::<Vec<_>>();
    }

    if hashes.is_empty() {
        return Err(Error::Config(
            "Schwab did not return any account hashes for this session".into(),
        ));
    }

    Ok(hashes)
}

fn looks_like_plain_account_number(value: &str) -> bool {
    value.len() >= 4 && value.chars().all(|ch| ch.is_ascii_digit())
}

fn resolve_latest_rfc3339_window_at(
    start: Option<String>,
    end: Option<String>,
    now_millis: i64,
) -> Result<(String, String)> {
    let start = normalize_optional_string(start);
    let end = normalize_optional_string(end);

    match (start, end) {
        (Some(start), Some(end)) => Ok((start, end)),
        (Some(start), None) => Ok((start, format_rfc3339_utc(now_millis))),
        (None, Some(end)) => {
            let end_millis = parse_rfc3339_millis(&end)?;
            Ok((
                format_rfc3339_utc(end_millis.saturating_sub(DEFAULT_LATEST_WINDOW_MILLIS)),
                end,
            ))
        }
        (None, None) => Ok((
            format_rfc3339_utc(now_millis.saturating_sub(DEFAULT_LATEST_WINDOW_MILLIS)),
            format_rfc3339_utc(now_millis),
        )),
    }
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn current_time_millis() -> Result<i64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::Io(format!("system clock is before the Unix epoch: {error}")))?;
    i64::try_from(now.as_millis())
        .map_err(|_| Error::Io("current system time does not fit into signed milliseconds".into()))
}

fn parse_rfc3339_millis(value: &str) -> Result<i64> {
    let trimmed = value.trim();
    let (date, time_and_offset) = trimmed
        .split_once('T')
        .ok_or_else(|| Error::Arguments(format!("invalid RFC3339 timestamp: {trimmed}")))?;
    let (year, month, day) = parse_date(date)?;
    let (time, offset_seconds) = split_time_and_offset(time_and_offset)?;
    let (hour, minute, second, millis) = parse_time(time)?;

    let days = days_from_civil(year, month, day);
    let local_seconds = days
        .saturating_mul(SECONDS_PER_DAY)
        .saturating_add(hour.saturating_mul(3_600))
        .saturating_add(minute.saturating_mul(60))
        .saturating_add(second);
    Ok(local_seconds
        .saturating_sub(offset_seconds)
        .saturating_mul(MILLIS_PER_SECOND)
        .saturating_add(millis))
}

fn parse_date(value: &str) -> Result<(i32, u32, u32)> {
    let mut parts = value.split('-');
    let year = parse_component::<i32>(parts.next(), "year", value)?;
    let month = parse_component::<u32>(parts.next(), "month", value)?;
    let day = parse_component::<u32>(parts.next(), "day", value)?;
    if parts.next().is_some() {
        return Err(Error::Arguments(format!("invalid RFC3339 date: {value}")));
    }
    Ok((year, month, day))
}

fn split_time_and_offset(value: &str) -> Result<(&str, i64)> {
    if let Some(time) = value.strip_suffix('Z') {
        return Ok((time, 0));
    }

    let sign_index = value
        .char_indices()
        .rev()
        .find_map(|(index, ch)| matches!(ch, '+' | '-').then_some(index))
        .ok_or_else(|| Error::Arguments(format!("invalid RFC3339 timezone offset: {value}")))?;
    let (time, offset) = value.split_at(sign_index);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let offset = &offset[1..];

    let (hours, minutes) = if let Some((hours, minutes)) = offset.split_once(':') {
        (hours, minutes)
    } else if offset.len() == 4 {
        (&offset[..2], &offset[2..])
    } else {
        return Err(Error::Arguments(format!("invalid RFC3339 timezone offset: {value}")));
    };

    let hours = parse_component::<i64>(Some(hours), "timezone hour", value)?;
    let minutes = parse_component::<i64>(Some(minutes), "timezone minute", value)?;
    Ok((time, i64::from(sign) * (hours * 3_600 + minutes * 60)))
}

fn parse_time(value: &str) -> Result<(i64, i64, i64, i64)> {
    let mut parts = value.split(':');
    let hour = parse_component::<i64>(parts.next(), "hour", value)?;
    let minute = parse_component::<i64>(parts.next(), "minute", value)?;
    let second_and_fraction = parts
        .next()
        .ok_or_else(|| Error::Arguments(format!("invalid RFC3339 time: {value}")))?;
    if parts.next().is_some() {
        return Err(Error::Arguments(format!("invalid RFC3339 time: {value}")));
    }

    let (second, millis) = if let Some((second, fraction)) = second_and_fraction.split_once('.') {
        (
            parse_component::<i64>(Some(second), "second", value)?,
            parse_fractional_millis(fraction, value)?,
        )
    } else {
        (parse_component::<i64>(Some(second_and_fraction), "second", value)?, 0)
    };

    Ok((hour, minute, second, millis))
}

fn parse_fractional_millis(value: &str, raw: &str) -> Result<i64> {
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(Error::Arguments(format!("invalid RFC3339 fractional seconds: {raw}")));
    }

    let mut millis = value.chars().take(3).collect::<String>();
    while millis.len() < 3 {
        millis.push('0');
    }
    millis
        .parse::<i64>()
        .map_err(|error| Error::Arguments(format!("invalid RFC3339 fractional seconds {raw}: {error}")))
}

fn parse_component<T>(value: Option<&str>, label: &str, raw: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = value.ok_or_else(|| Error::Arguments(format!("missing {label} in RFC3339 timestamp: {raw}")))?;
    value
        .parse::<T>()
        .map_err(|error| Error::Arguments(format!("invalid {label} in RFC3339 timestamp {raw}: {error}")))
}

fn format_rfc3339_utc(millis: i64) -> String {
    let seconds = millis.div_euclid(MILLIS_PER_SECOND);
    let milliseconds = millis.rem_euclid(MILLIS_PER_SECOND);
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let seconds_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milliseconds:03}Z")
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);

    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::{format_rfc3339_utc, parse_rfc3339_millis, resolve_latest_rfc3339_window_at};

    #[test]
    fn resolve_latest_window_defaults_to_last_thirty_days() {
        let (start, end) =
            resolve_latest_rfc3339_window_at(None, None, 1_775_564_340_123).expect("window should resolve");
        assert_eq!(start, "2026-03-08T12:19:00.123Z");
        assert_eq!(end, "2026-04-07T12:19:00.123Z");
    }

    #[test]
    fn resolve_latest_window_uses_now_for_missing_end() {
        let (start, end) =
            resolve_latest_rfc3339_window_at(Some("2026-04-01T00:00:00.000Z".into()), None, 1_775_564_340_123)
                .expect("window should resolve");
        assert_eq!(start, "2026-04-01T00:00:00.000Z");
        assert_eq!(end, "2026-04-07T12:19:00.123Z");
    }

    #[test]
    fn resolve_latest_window_backfills_missing_start_from_end() {
        let (start, end) =
            resolve_latest_rfc3339_window_at(None, Some("2026-04-07T23:59:59.000Z".into()), 1_775_564_340_123)
                .expect("window should resolve");
        assert_eq!(start, "2026-03-08T23:59:59.000Z");
        assert_eq!(end, "2026-04-07T23:59:59.000Z");
    }

    #[test]
    fn parse_and_format_rfc3339_support_offsets() {
        let millis = parse_rfc3339_millis("2026-04-07T16:59:00.123-07:00").expect("timestamp should parse");
        assert_eq!(format_rfc3339_utc(millis), "2026-04-07T23:59:00.123Z");

        let millis = parse_rfc3339_millis("2026-04-07T23:59:00+0000").expect("timestamp should parse");
        assert_eq!(format_rfc3339_utc(millis), "2026-04-07T23:59:00.000Z");
    }
}
