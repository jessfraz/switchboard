use serde_json::{json, Value};
use switchboard_core::{
    Error, ExecutionTarget, NamespaceId, OperationEffect, PlannedAction, ProviderKind, ResolvedNamespace, Result,
    ToolArguments, ToolOutput, ToolRef, ToolRefKind, ToolRequest,
};

use crate::{
    cli::{CliCommandSpec, CliResponse},
    google::commands::{
        append_optional_flag, append_optional_value, append_repeatable_values, flag_enabled, GWS_BINARY,
        GWS_CALENDAR_CAPABILITY, GWS_CALENDAR_INSERT_CAPABILITY,
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

pub(crate) const CALENDAR_CREATE_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "google.calendar.create",
        kind: switchboard_core::ToolKind::Write,
        summary: "Create a calendar event",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GWS_BINARY,
    capability: &GWS_CALENDAR_INSERT_CAPABILITY,
    summarize: summarize_calendar_create,
    build_args: build_calendar_create_args,
    decode: decode_calendar_create,
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

fn build_calendar_create_args(action: &PlannedAction) -> Result<Vec<String>> {
    let title = required_action_arg(action, &["title", "summary"])?;
    let start = required_action_arg(action, &["start"])?;
    let end = required_action_arg(action, &["end"])?;
    let mut args = vec![
        "calendar".to_owned(),
        "+insert".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--summary".to_owned(),
        title.to_owned(),
        "--start".to_owned(),
        start.to_owned(),
        "--end".to_owned(),
        end.to_owned(),
    ];

    append_optional_value(&mut args, &action.args, "calendar");
    append_optional_value(&mut args, &action.args, "location");
    append_optional_value(&mut args, &action.args, "description");
    append_repeatable_values(&mut args, &action.args, "attendee");
    append_optional_flag(&mut args, &action.args, "meet")?;

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

fn decode_calendar_create(
    target: &ExecutionTarget,
    action: &PlannedAction,
    response: CliResponse,
) -> Result<ToolOutput> {
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
    let title = required_json_string(&value, "summary")?;
    let event = simplify_created_event(&value, action)?;
    let event_ref = calendar_event_ref(&action.namespace, &event)?;
    let mut effect = OperationEffect::new(true).with_ref(event_ref.clone());
    effect = effect.with_undo_summary(format!("Delete calendar event {title:?} from {}", action.namespace))?;

    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Created calendar event {title:?} for {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field("event", event)
    .with_ref(event_ref)
    .with_effect(effect);

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

fn simplify_created_event(event: &Value, action: &PlannedAction) -> Result<Value> {
    let event_id = required_json_string(event, "id")?;
    let title = required_json_string(event, "summary")?;
    let calendar = event
        .get("organizer")
        .and_then(|organizer| organizer.get("email"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            action
                .args
                .value("calendar")
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("primary")
                .to_owned()
        });

    Ok(json!({
        "event_id": event_id,
        "title": title,
        "calendar": calendar,
        "status": event.get("status").cloned().unwrap_or(Value::Null),
        "start": extract_event_time(event.get("start")),
        "end": extract_event_time(event.get("end")),
        "location": event.get("location").cloned().unwrap_or(Value::Null),
        "description": event.get("description").cloned().unwrap_or(Value::Null),
        "html_link": event.get("htmlLink").cloned().unwrap_or(Value::Null),
        "meet_link": event.get("hangoutLink").cloned().unwrap_or(Value::Null),
        "attendees": event.get("attendees").cloned().unwrap_or_else(|| json!([])),
    }))
}

fn extract_event_time(event_time: Option<&Value>) -> Value {
    event_time
        .and_then(|event_time| {
            event_time
                .get("dateTime")
                .cloned()
                .or_else(|| event_time.get("date").cloned())
        })
        .unwrap_or(Value::Null)
}

fn calendar_event_ref(namespace: &NamespaceId, event: &Value) -> Result<ToolRef> {
    let event_id = required_json_string(event, "event_id")?;
    let calendar = required_json_string(event, "calendar")?;
    let title = event.get("title").and_then(Value::as_str);
    let url = event.get("html_link").and_then(Value::as_str);

    let mut event_ref = ToolRef::new(
        ProviderKind::GoogleWorkspace,
        namespace.clone(),
        ToolRefKind::Event,
        event_id,
    )?
    .with_parent_id(calendar)?;

    if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
        event_ref = event_ref.with_label(title)?;
    }
    if let Some(url) = url.filter(|value| !value.trim().is_empty()) {
        event_ref = event_ref.with_web_url(url)?;
    }

    Ok(event_ref)
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

fn required_json_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Error::Execution(format!("missing required {key:?} field in Google Calendar response")))
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
