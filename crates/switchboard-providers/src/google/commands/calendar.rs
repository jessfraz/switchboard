use serde_json::{json, Value};
use switchboard_core::{
    Error, ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolArguments, ToolOutput, ToolRequest,
};

use crate::{
    cli::{CliCommandSpec, CliResponse},
    google::commands::{
        append_optional_flag, append_optional_value, flag_enabled, GWS_BINARY, GWS_CALENDAR_CAPABILITY,
    },
};

pub(crate) const CALENDAR_LIST_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "google.calendar.list",
        kind: switchboard_core::ToolKind::Read,
        summary: "List calendar events",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GWS_BINARY,
    capability: &GWS_CALENDAR_CAPABILITY,
    summarize: summarize_calendar_list,
    build_args: build_calendar_list_args,
    decode: decode_calendar_list,
};

fn summarize_calendar_list(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let time_scope = agenda_scope(&request.args)?;
    Ok(format!("List {time_scope} calendar events for {}", namespace.id))
}

fn build_calendar_list_args(action: &PlannedAction) -> Result<Vec<String>> {
    let mut args = vec![
        "calendar".to_owned(),
        "+agenda".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
    ];

    append_optional_flag(&mut args, &action.args, "today")?;
    append_optional_flag(&mut args, &action.args, "tomorrow")?;
    append_optional_flag(&mut args, &action.args, "week")?;
    append_optional_value(&mut args, &action.args, "days");
    append_optional_value(&mut args, &action.args, "calendar");
    append_optional_value(&mut args, &action.args, "timezone");

    Ok(args)
}

fn decode_calendar_list(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
    let CliResponse {
        program,
        version,
        stdout,
        stderr,
    } = response;
    let value: Value = serde_json::from_str(&stdout).map_err(|error| {
        Error::Execution(format!(
            "{} returned invalid JSON for {}: {error}",
            program.display(),
            action.tool
        ))
    })?;
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(simplify_google_event)
        .collect::<Vec<_>>();
    let count = value
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(events.len() as u64);

    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Listed {count} calendar events for {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_value_field("count", json!(count))
    .with_value_field("events", Value::Array(events));

    if let Some(time_min) = value.get("timeMin").cloned() {
        output = output.with_value_field("time_min", time_min);
    }
    if let Some(time_max) = value.get("timeMax").cloned() {
        output = output.with_value_field("time_max", time_max);
    }
    output = output.with_field("cli_version", version);
    if !stderr.trim().is_empty() {
        output = output.with_field("cli_stderr", stderr);
    }

    Ok(output)
}

fn simplify_google_event(event: Value) -> Value {
    json!({
        "start": event.get("start").cloned().unwrap_or(Value::Null),
        "end": event.get("end").cloned().unwrap_or(Value::Null),
        "title": event.get("summary").cloned().unwrap_or(Value::Null),
        "calendar": event.get("calendar").cloned().unwrap_or(Value::Null),
        "location": event.get("location").cloned().unwrap_or(Value::Null),
    })
}

fn agenda_scope(arguments: &ToolArguments) -> Result<&'static str> {
    if flag_enabled(arguments, "today")? {
        return Ok("today's");
    }
    if flag_enabled(arguments, "tomorrow")? {
        return Ok("tomorrow's");
    }
    if flag_enabled(arguments, "week")? {
        return Ok("this week's");
    }
    if arguments.value("days").is_some() {
        return Ok("upcoming");
    }

    Ok("upcoming")
}
