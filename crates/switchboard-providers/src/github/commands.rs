use serde_json::{json, Value};
use switchboard_core::{
    Error, ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolArguments, ToolOutput, ToolRequest,
};

use crate::cli::{CliBinarySpec, CliCapabilityProbe, CliCommandSpec, CliResponse};

pub(crate) const GH_BINARY: CliBinarySpec = CliBinarySpec {
    program: "gh",
    env_override: Some("SWITCHBOARD_GH_BIN"),
    version_args: &["--version"],
};

pub(crate) const GH_API_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gh_api",
    args: &["api", "--help"],
};

pub(crate) const NOTIFICATIONS_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "github.notifications.list",
        kind: switchboard_core::ToolKind::Read,
        summary: "List notifications for a GitHub namespace",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GH_BINARY,
    capability: &GH_API_CAPABILITY,
    summarize: summarize_notifications,
    build_args: build_notifications_args,
    decode: decode_notifications,
};

fn summarize_notifications(namespace: &ResolvedNamespace, _request: &ToolRequest) -> Result<String> {
    Ok(format!("List GitHub notifications for {}", namespace.id))
}

fn build_notifications_args(action: &PlannedAction) -> Result<Vec<String>> {
    let mut args = vec!["api".to_owned(), "notifications".to_owned()];
    append_query_bool(&mut args, &action.args, "all")?;
    append_query_bool(&mut args, &action.args, "participating")?;
    append_query_value(&mut args, &action.args, "since");
    append_query_value(&mut args, &action.args, "before");
    append_query_value(&mut args, &action.args, "per_page");
    Ok(args)
}

fn decode_notifications(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
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
    let notifications = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(simplify_notification)
        .collect::<Vec<_>>();
    let count = notifications.len();

    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Listed {count} GitHub notifications for {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field("count", json!(count))
    .with_value_field("notifications", Value::Array(notifications));

    if !stderr.trim().is_empty() {
        output = output.with_field("cli_stderr", stderr);
    }

    Ok(output)
}

fn simplify_notification(notification: Value) -> Value {
    json!({
        "id": notification.get("id").cloned().unwrap_or(Value::Null),
        "unread": notification.get("unread").cloned().unwrap_or(Value::Null),
        "reason": notification.get("reason").cloned().unwrap_or(Value::Null),
        "updated_at": notification.get("updated_at").cloned().unwrap_or(Value::Null),
        "title": notification
            .get("subject")
            .and_then(|subject| subject.get("title"))
            .cloned()
            .unwrap_or(Value::Null),
        "type": notification
            .get("subject")
            .and_then(|subject| subject.get("type"))
            .cloned()
            .unwrap_or(Value::Null),
        "repository": notification
            .get("repository")
            .and_then(|repository| repository.get("full_name"))
            .cloned()
            .unwrap_or(Value::Null),
        "url": notification.get("url").cloned().unwrap_or(Value::Null),
    })
}

fn append_query_bool(args: &mut Vec<String>, arguments: &ToolArguments, name: &str) -> Result<()> {
    if arguments.has_flag(name) {
        args.push("-F".to_owned());
        args.push(format!("{name}=true"));
        return Ok(());
    }

    if let Some(value) = arguments.value(name) {
        let value = parse_bool(name, value)?;
        args.push("-F".to_owned());
        args.push(format!("{name}={value}"));
    }

    Ok(())
}

fn append_query_value(args: &mut Vec<String>, arguments: &ToolArguments, name: &str) {
    if let Some(value) = arguments.value(name) {
        args.push("-F".to_owned());
        args.push(format!("{name}={value}"));
    }
}

fn parse_bool(name: &str, value: &str) -> Result<bool> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(Error::InvalidArguments(format!(
            "--{name} expects a boolean value, got {value:?}"
        ))),
    }
}
