use serde_json::{json, Value};
use switchboard_core::{
    Error, ExecutionTarget, NamespaceId, PlannedAction, ProviderKind, ResolvedNamespace, Result, ToolOutput, ToolRef,
    ToolRefKind, ToolRequest,
};

use crate::{
    cli::{CliCommandSpec, CliResponse},
    github::commands::{GH_BINARY, GH_ISSUE_VIEW_CAPABILITY},
};

const READ_FIELDS: &str = "id,number,title,body,state,stateReason,author,assignees,labels,createdAt,updatedAt,url";

pub(crate) const ISSUE_READ_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "github.issue.read",
        kind: switchboard_core::ToolKind::Read,
        summary: "Read an issue",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GH_BINARY,
    capability: &GH_ISSUE_VIEW_CAPABILITY,
    summarize: summarize_issue_read,
    build_args: build_issue_read_args,
    decode: decode_issue_read,
};

fn summarize_issue_read(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let repo = required_arg(request, &["repo"])?;
    let number = required_arg(request, &["number"])?;
    Ok(format!("Read GitHub issue {repo}#{number} in {}", namespace.id))
}

fn build_issue_read_args(action: &PlannedAction) -> Result<Vec<String>> {
    let repo = required_action_arg(action, &["repo"])?;
    let number = required_action_arg(action, &["number"])?;

    Ok(vec![
        "issue".to_owned(),
        "view".to_owned(),
        number.to_owned(),
        "--repo".to_owned(),
        repo.to_owned(),
        "--json".to_owned(),
        READ_FIELDS.to_owned(),
    ])
}

fn decode_issue_read(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
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
    let repo = required_action_arg(action, &["repo"])?;
    let number = required_action_arg(action, &["number"])?;
    let issue = simplify_issue_detail(repo, number, value);
    let refs = issue_refs(&action.namespace, &issue)?;
    let title = issue
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(number);

    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Read GitHub issue {title:?} for {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field("issue", issue)
    .with_refs(refs);

    if !stderr.trim().is_empty() {
        output = output.with_field("cli_stderr", stderr);
    }

    Ok(output)
}

fn simplify_issue_detail(repo: &str, number: &str, issue: Value) -> Value {
    json!({
        "id": issue.get("id").cloned().unwrap_or(Value::Null),
        "number": issue.get("number").cloned().unwrap_or(json!(number)),
        "repository": repo,
        "title": issue.get("title").cloned().unwrap_or(Value::Null),
        "body": issue.get("body").cloned().unwrap_or(Value::Null),
        "state": issue.get("state").cloned().unwrap_or(Value::Null),
        "state_reason": issue.get("stateReason").cloned().unwrap_or(Value::Null),
        "author": issue
            .get("author")
            .and_then(|author| author.get("login"))
            .cloned()
            .unwrap_or(Value::Null),
        "assignees": extract_login_list(&issue, "assignees"),
        "labels": extract_name_list(&issue, "labels"),
        "created_at": issue.get("createdAt").cloned().unwrap_or(Value::Null),
        "updated_at": issue.get("updatedAt").cloned().unwrap_or(Value::Null),
        "url": issue.get("url").cloned().unwrap_or(Value::Null),
    })
}

fn issue_refs(namespace: &NamespaceId, issue: &Value) -> Result<Vec<ToolRef>> {
    let number = required_json_number_as_string(issue, "number")?;
    let repository = required_json_string(issue, "repository")?;
    let title = issue.get("title").and_then(Value::as_str);
    let url = issue.get("url").and_then(Value::as_str);

    let mut tool_ref = ToolRef::new(ProviderKind::GitHub, namespace.clone(), ToolRefKind::Issue, number)?
        .with_parent_id(repository)?;

    if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
        tool_ref = tool_ref.with_label(title)?;
    }
    if let Some(url) = url.filter(|value| !value.trim().is_empty()) {
        tool_ref = tool_ref.with_web_url(url)?;
    }

    Ok(vec![tool_ref])
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

fn extract_login_list(value: &Value, key: &str) -> Value {
    Value::Array(
        value
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| entry.get("login").cloned())
            .collect(),
    )
}

fn extract_name_list(value: &Value, key: &str) -> Value {
    Value::Array(
        value
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| entry.get("name").cloned())
            .collect(),
    )
}

fn required_json_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|candidate| !candidate.trim().is_empty())
        .ok_or_else(|| Error::Execution(format!("missing required JSON field {key:?} in GitHub response")))
}

fn required_json_number_as_string(value: &Value, key: &str) -> Result<String> {
    json_number_as_string(
        value
            .get(key)
            .ok_or_else(|| Error::Execution(format!("missing required JSON field {key:?} in GitHub response")))?,
    )
    .ok_or_else(|| {
        Error::Execution(format!(
            "missing required numeric JSON field {key:?} in GitHub response"
        ))
    })
}

fn json_number_as_string(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        _ => None,
    }
}
