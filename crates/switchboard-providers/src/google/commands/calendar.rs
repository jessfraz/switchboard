use serde_json::{json, Value};
use switchboard_core::{
    Error, ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolArguments, ToolOutput, ToolRequest,
};

use crate::{
    cli::{CliCommandHandler, CliResponse},
    google::commands::{append_optional_flag, append_optional_value, flag_enabled},
};

pub(crate) const CALENDAR_LIST_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "calendar_list",
    summarize: summarize_calendar_list,
    build_args: Some(build_calendar_list_args),
    decode: Some(decode_calendar_list),
};

pub(crate) const CALENDAR_CREATE_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "calendar_create",
    summarize: summarize_calendar_create,
    build_args: None,
    decode: None,
};

pub(crate) const CALENDAR_DELETE_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "calendar_delete",
    summarize: summarize_calendar_delete,
    build_args: Some(build_calendar_delete_args),
    decode: Some(decode_calendar_delete),
};

fn summarize_calendar_list(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let time_scope = agenda_scope(&request.args)?;
    Ok(format!("List {time_scope} calendar events for {}", namespace.id))
}

fn summarize_calendar_create(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let title = required_arg(request, &["title", "summary"])?;
    let start = required_arg(request, &["start"])?;
    let verb = match request.mode {
        switchboard_core::ExecutionMode::Plan | switchboard_core::ExecutionMode::Draft => "Draft",
        switchboard_core::ExecutionMode::Auto | switchboard_core::ExecutionMode::Apply => "Create",
    };
    Ok(format!(
        "{verb} calendar event {title:?} starting at {start} for {}",
        namespace.id
    ))
}

fn summarize_calendar_delete(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let event_id = required_arg(request, &["event-id"])?;
    let calendar = request.args.value("calendar").unwrap_or("primary");
    let verb = match request.mode {
        switchboard_core::ExecutionMode::Plan | switchboard_core::ExecutionMode::Draft => "Draft delete of",
        switchboard_core::ExecutionMode::Auto | switchboard_core::ExecutionMode::Apply => "Delete",
    };
    Ok(format!(
        "{verb} calendar event {event_id:?} from calendar {calendar:?} for {}",
        namespace.id
    ))
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

fn build_calendar_delete_args(action: &PlannedAction) -> Result<Vec<String>> {
    let event_id = required_action_arg(action, &["event-id"])?;
    let calendar = action.args.value("calendar").unwrap_or("primary");
    let mut args = vec![
        "calendar".to_owned(),
        "events".to_owned(),
        "delete".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--params".to_owned(),
        json!({
            "calendarId": calendar,
            "eventId": event_id,
        })
        .to_string(),
    ];

    append_optional_value(&mut args, &action.args, "send-updates");

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

fn decode_calendar_delete(
    target: &ExecutionTarget,
    action: &PlannedAction,
    response: CliResponse,
) -> Result<ToolOutput> {
    let CliResponse {
        version,
        stdout,
        stderr,
        ..
    } = response;
    let event_id = required_action_arg(action, &["event-id"])?;
    let calendar = action.args.value("calendar").unwrap_or("primary");
    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Deleted calendar event {event_id:?} from {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field(
        "event",
        json!({
            "event_id": event_id,
            "calendar": calendar,
        }),
    );

    if !stdout.trim().is_empty() {
        if let Ok(value) = serde_json::from_str::<Value>(&stdout) {
            output = output.with_value_field("response", value);
        } else {
            output = output.with_field("stdout_text", stdout);
        }
    }
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

fn required_arg<'a>(request: &'a ToolRequest, keys: &[&'a str]) -> Result<&'a str> {
    keys.iter().find_map(|key| request.args.value(key)).ok_or_else(|| {
        Error::InvalidArguments(format!(
            "missing required argument {} for {}",
            render_arg_keys(keys),
            request.tool
        ))
    })
}

fn required_action_arg<'a>(action: &'a PlannedAction, keys: &[&'a str]) -> Result<&'a str> {
    keys.iter().find_map(|key| action.args.value(key)).ok_or_else(|| {
        Error::InvalidArguments(format!(
            "missing required argument {} for {}",
            render_arg_keys(keys),
            action.tool
        ))
    })
}

fn render_arg_keys(keys: &[&str]) -> String {
    keys.iter()
        .map(|key| format!("--{key}"))
        .collect::<Vec<_>>()
        .join(" or ")
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
