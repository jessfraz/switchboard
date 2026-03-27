use serde_json::{json, Value};
use switchboard_core::{
    Error, ExecutionTarget, NamespaceId, PlannedAction, ProviderKind, ResolvedNamespace, Result, ToolOutput, ToolRef,
    ToolRefKind, ToolRequest,
};

use crate::{
    cli::{CliCommandHandler, CliResponse},
    github::commands::{append_optional_flag, append_optional_value},
};

const SEARCH_FIELDS: &str = "id,number,title,state,isDraft,repository,author,createdAt,updatedAt,url";
const READ_FIELDS: &str =
    "id,number,title,body,state,isDraft,author,assignees,labels,baseRefName,headRefName,reviewDecision,mergeStateStatus,createdAt,updatedAt,url";

pub(crate) const PULL_REQUEST_SEARCH_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "pull_request_search",
    summarize: summarize_pull_request_search,
    build_args: Some(build_pull_request_search_args),
    decode: Some(decode_pull_request_search),
};

pub(crate) const PULL_REQUEST_READ_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "pull_request_read",
    summarize: summarize_pull_request_read,
    build_args: Some(build_pull_request_read_args),
    decode: Some(decode_pull_request_read),
};

fn summarize_pull_request_search(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let query = required_arg(request, &["query"])?;
    Ok(format!("Search GitHub pull requests in {} for {query:?}", namespace.id))
}

fn summarize_pull_request_read(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let repo = required_arg(request, &["repo"])?;
    let number = required_arg(request, &["number"])?;
    Ok(format!("Read GitHub pull request {repo}#{number} in {}", namespace.id))
}

fn build_pull_request_search_args(action: &PlannedAction) -> Result<Vec<String>> {
    let query = required_action_arg(action, &["query"])?;
    let mut args = vec![
        "search".to_owned(),
        "prs".to_owned(),
        query.to_owned(),
        "--json".to_owned(),
        SEARCH_FIELDS.to_owned(),
    ];

    append_optional_value(&mut args, &action.args, "limit");
    append_optional_value(&mut args, &action.args, "state");
    append_optional_value(&mut args, &action.args, "repo");
    append_optional_value(&mut args, &action.args, "owner");
    append_optional_value(&mut args, &action.args, "author");
    append_optional_value(&mut args, &action.args, "assignee");
    append_optional_value(&mut args, &action.args, "review-requested");
    append_optional_value(&mut args, &action.args, "sort");
    append_optional_value(&mut args, &action.args, "order");
    append_optional_value(&mut args, &action.args, "base");
    append_optional_value(&mut args, &action.args, "head");
    append_optional_flag(&mut args, &action.args, "draft")?;
    append_optional_flag(&mut args, &action.args, "merged")?;

    Ok(args)
}

fn build_pull_request_read_args(action: &PlannedAction) -> Result<Vec<String>> {
    let repo = required_action_arg(action, &["repo"])?;
    let number = required_action_arg(action, &["number"])?;

    Ok(vec![
        "pr".to_owned(),
        "view".to_owned(),
        number.to_owned(),
        "--repo".to_owned(),
        repo.to_owned(),
        "--json".to_owned(),
        READ_FIELDS.to_owned(),
    ])
}

fn decode_pull_request_search(
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
    let raw_pull_requests = value.as_array().cloned().unwrap_or_default();
    let pull_requests = raw_pull_requests
        .iter()
        .map(simplify_pull_request_search_result)
        .collect::<Vec<_>>();
    let refs = raw_pull_requests
        .iter()
        .filter_map(|pull_request| pull_request_ref(&action.namespace, pull_request))
        .collect::<Result<Vec<_>>>()?;
    let count = pull_requests.len();

    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Found {count} GitHub pull requests for {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field("count", json!(count))
    .with_value_field("pull_requests", Value::Array(pull_requests))
    .with_refs(refs);

    if !stderr.trim().is_empty() {
        output = output.with_field("cli_stderr", stderr);
    }

    Ok(output)
}

fn decode_pull_request_read(
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
    let repo = required_action_arg(action, &["repo"])?;
    let number = required_action_arg(action, &["number"])?;
    let pull_request = simplify_pull_request_detail(repo, number, value);
    let refs = pull_request_detail_refs(&action.namespace, &pull_request)?;
    let title = pull_request
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(number);

    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Read GitHub pull request {title:?} for {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field("pull_request", pull_request)
    .with_refs(refs);

    if !stderr.trim().is_empty() {
        output = output.with_field("cli_stderr", stderr);
    }

    Ok(output)
}

fn simplify_pull_request_search_result(pull_request: &Value) -> Value {
    json!({
        "id": pull_request.get("id").cloned().unwrap_or(Value::Null),
        "number": pull_request.get("number").cloned().unwrap_or(Value::Null),
        "title": pull_request.get("title").cloned().unwrap_or(Value::Null),
        "state": pull_request.get("state").cloned().unwrap_or(Value::Null),
        "is_draft": pull_request.get("isDraft").cloned().unwrap_or(Value::Null),
        "repository": extract_repository_name(pull_request).unwrap_or(Value::Null),
        "author": pull_request
            .get("author")
            .and_then(|author| author.get("login"))
            .cloned()
            .unwrap_or(Value::Null),
        "created_at": pull_request.get("createdAt").cloned().unwrap_or(Value::Null),
        "updated_at": pull_request.get("updatedAt").cloned().unwrap_or(Value::Null),
        "url": pull_request.get("url").cloned().unwrap_or(Value::Null),
    })
}

fn simplify_pull_request_detail(repo: &str, number: &str, pull_request: Value) -> Value {
    json!({
        "id": pull_request.get("id").cloned().unwrap_or(Value::Null),
        "number": pull_request
            .get("number")
            .cloned()
            .unwrap_or(json!(number)),
        "repository": repo,
        "title": pull_request.get("title").cloned().unwrap_or(Value::Null),
        "body": pull_request.get("body").cloned().unwrap_or(Value::Null),
        "state": pull_request.get("state").cloned().unwrap_or(Value::Null),
        "is_draft": pull_request.get("isDraft").cloned().unwrap_or(Value::Null),
        "author": pull_request
            .get("author")
            .and_then(|author| author.get("login"))
            .cloned()
            .unwrap_or(Value::Null),
        "assignees": extract_login_list(&pull_request, "assignees"),
        "labels": extract_name_list(&pull_request, "labels"),
        "base_ref_name": pull_request.get("baseRefName").cloned().unwrap_or(Value::Null),
        "head_ref_name": pull_request.get("headRefName").cloned().unwrap_or(Value::Null),
        "review_decision": pull_request.get("reviewDecision").cloned().unwrap_or(Value::Null),
        "merge_state_status": pull_request.get("mergeStateStatus").cloned().unwrap_or(Value::Null),
        "created_at": pull_request.get("createdAt").cloned().unwrap_or(Value::Null),
        "updated_at": pull_request.get("updatedAt").cloned().unwrap_or(Value::Null),
        "url": pull_request.get("url").cloned().unwrap_or(Value::Null),
    })
}

fn pull_request_ref(namespace: &NamespaceId, pull_request: &Value) -> Option<Result<ToolRef>> {
    let number = json_number_as_string(pull_request.get("number")?)?;
    let repository = extract_repository_name(pull_request)?.as_str()?.to_owned();
    let title = pull_request.get("title").and_then(Value::as_str);
    let url = pull_request.get("url").and_then(Value::as_str);

    Some(build_pull_request_ref(namespace, &number, &repository, title, url))
}

fn pull_request_detail_refs(namespace: &NamespaceId, pull_request: &Value) -> Result<Vec<ToolRef>> {
    let number = required_json_number_as_string(pull_request, "number")?;
    let repository = required_json_string(pull_request, "repository")?;
    let title = pull_request.get("title").and_then(Value::as_str);
    let url = pull_request.get("url").and_then(Value::as_str);

    Ok(vec![build_pull_request_ref(
        namespace, &number, repository, title, url,
    )?])
}

fn build_pull_request_ref(
    namespace: &NamespaceId,
    number: &str,
    repository: &str,
    title: Option<&str>,
    url: Option<&str>,
) -> Result<ToolRef> {
    let mut tool_ref = ToolRef::new(
        ProviderKind::GitHub,
        namespace.clone(),
        ToolRefKind::PullRequest,
        number,
    )?
    .with_parent_id(repository)?;

    if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
        tool_ref = tool_ref.with_label(title)?;
    }
    if let Some(url) = url.filter(|value| !value.trim().is_empty()) {
        tool_ref = tool_ref.with_web_url(url)?;
    }

    Ok(tool_ref)
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

fn extract_repository_name(value: &Value) -> Option<Value> {
    value
        .get("repository")
        .and_then(|repository| repository.get("nameWithOwner"))
        .cloned()
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
