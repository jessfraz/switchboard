use serde_json::{json, Value};
use switchboard_core::{
    Error, ExecutionTarget, NamespaceId, PlannedAction, ProviderKind, ResolvedNamespace, Result, ToolOutput, ToolRef,
    ToolRefKind, ToolRequest,
};

use crate::{
    cli::{CliCommandSpec, CliResponse},
    google::commands::{
        append_optional_flag, append_optional_value, GWS_BINARY, GWS_GMAIL_READ_CAPABILITY, GWS_GMAIL_TRIAGE_CAPABILITY,
    },
};

pub(crate) const MAIL_SEARCH_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "google.mail.search",
        kind: switchboard_core::ToolKind::Read,
        summary: "Search Gmail",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GWS_BINARY,
    capability: &GWS_GMAIL_TRIAGE_CAPABILITY,
    summarize: summarize_mail_search,
    build_args: build_mail_search_args,
    decode: decode_mail_search,
};

pub(crate) const MAIL_READ_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "google.mail.read",
        kind: switchboard_core::ToolKind::Read,
        summary: "Read a Gmail message",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GWS_BINARY,
    capability: &GWS_GMAIL_READ_CAPABILITY,
    summarize: summarize_mail_read,
    build_args: build_mail_read_args,
    decode: decode_mail_read,
};

fn summarize_mail_search(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let query = required_arg(request, &["query"])?;
    Ok(format!("Search Gmail in {} for {query:?}", namespace.id))
}

fn summarize_mail_read(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let message_id = required_arg(request, &["gmail-message-id", "message-id", "id"])?;
    Ok(format!("Read Gmail message {message_id} in {}", namespace.id))
}

fn build_mail_search_args(action: &PlannedAction) -> Result<Vec<String>> {
    let query = required_action_arg(action, &["query"])?;
    let mut args = vec![
        "gmail".to_owned(),
        "+triage".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--query".to_owned(),
        query.to_owned(),
    ];

    append_optional_value(&mut args, &action.args, "max");
    append_optional_flag(&mut args, &action.args, "labels")?;

    Ok(args)
}

fn build_mail_read_args(action: &PlannedAction) -> Result<Vec<String>> {
    let message_id = required_action_arg(action, &["gmail-message-id", "message-id", "id"])?;
    Ok(vec![
        "gmail".to_owned(),
        "+read".to_owned(),
        "--id".to_owned(),
        message_id.to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
    ])
}

fn decode_mail_search(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
    let CliResponse {
        program,
        version,
        stdout,
        stderr,
    } = response;
    let query = required_action_arg(action, &["query"])?;

    let value = if stdout.trim().is_empty() {
        json!({
            "messages": [],
            "query": query,
            "resultSizeEstimate": 0,
        })
    } else {
        serde_json::from_str(&stdout).map_err(|error| {
            Error::Execution(format!(
                "{} returned invalid JSON for {}: {error}",
                program.display(),
                action.tool
            ))
        })?
    };

    let raw_messages = value
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let messages = raw_messages.iter().map(simplify_search_message).collect::<Vec<_>>();
    let refs = raw_messages
        .iter()
        .filter_map(|message| mail_search_ref(&action.namespace, message))
        .collect::<Result<Vec<_>>>()?;
    let count = messages.len();

    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Found {count} Gmail messages for {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field("count", json!(count))
    .with_value_field("query", json!(query))
    .with_value_field("messages", Value::Array(messages))
    .with_refs(refs);

    if let Some(result_size_estimate) = value.get("resultSizeEstimate").cloned() {
        output = output.with_value_field("result_size_estimate", result_size_estimate);
    }
    if !stderr.trim().is_empty() {
        output = output.with_field("cli_stderr", stderr);
    }

    Ok(output)
}

fn decode_mail_read(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
    let CliResponse {
        program,
        version,
        stdout,
        stderr,
    } = response;
    let requested_message_id = required_action_arg(action, &["gmail-message-id", "message-id", "id"])?;
    let value: Value = serde_json::from_str(&stdout).map_err(|error| {
        Error::Execution(format!(
            "{} returned invalid JSON for {}: {error}",
            program.display(),
            action.tool
        ))
    })?;
    let message = simplify_read_message(requested_message_id, value);
    let refs = mail_read_refs(&action.namespace, &message)?;
    let subject = message
        .get("subject")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(requested_message_id);

    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Read Gmail message {subject:?} for {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field("message", message)
    .with_refs(refs);

    if !stderr.trim().is_empty() {
        output = output.with_field("cli_stderr", stderr);
    }

    Ok(output)
}

fn simplify_search_message(message: &Value) -> Value {
    json!({
        "gmail_message_id": message.get("id").cloned().unwrap_or(Value::Null),
        "from": message.get("from").cloned().unwrap_or(Value::Null),
        "subject": message.get("subject").cloned().unwrap_or(Value::Null),
        "date": message.get("date").cloned().unwrap_or(Value::Null),
        "labels": message.get("labels").cloned().unwrap_or(json!([])),
    })
}

fn simplify_read_message(requested_message_id: &str, message: Value) -> Value {
    json!({
        "gmail_message_id": requested_message_id,
        "gmail_thread_id": message.get("thread_id").cloned().unwrap_or(Value::Null),
        "rfc_message_id": message.get("message_id").cloned().unwrap_or(Value::Null),
        "references": message.get("references").cloned().unwrap_or(json!([])),
        "from": message.get("from").cloned().unwrap_or(Value::Null),
        "reply_to": message.get("reply_to").cloned().unwrap_or(Value::Null),
        "to": message.get("to").cloned().unwrap_or(json!([])),
        "cc": message.get("cc").cloned().unwrap_or(Value::Null),
        "subject": message.get("subject").cloned().unwrap_or(Value::Null),
        "date": message.get("date").cloned().unwrap_or(Value::Null),
        "body_text": message.get("body_text").cloned().unwrap_or(Value::Null),
        "body_html": message.get("body_html").cloned().unwrap_or(Value::Null),
    })
}

fn mail_search_ref(namespace: &NamespaceId, message: &Value) -> Option<Result<ToolRef>> {
    let gmail_message_id = message.get("id")?.as_str()?;
    let subject = message.get("subject").and_then(Value::as_str);

    Some(
        ToolRef::new(
            ProviderKind::GoogleWorkspace,
            namespace.clone(),
            ToolRefKind::Message,
            gmail_message_id,
        )
        .and_then(|tool_ref| match subject.filter(|value| !value.trim().is_empty()) {
            Some(subject) => tool_ref.with_label(subject),
            None => Ok(tool_ref),
        }),
    )
}

fn mail_read_refs(namespace: &NamespaceId, message: &Value) -> Result<Vec<ToolRef>> {
    let gmail_message_id = required_json_string(message, "gmail_message_id")?;
    let subject = message.get("subject").and_then(Value::as_str);
    let thread_id = message.get("gmail_thread_id").and_then(Value::as_str);

    let mut refs = Vec::new();
    let mut message_ref = ToolRef::new(
        ProviderKind::GoogleWorkspace,
        namespace.clone(),
        ToolRefKind::Message,
        gmail_message_id,
    )?;
    if let Some(thread_id) = thread_id.filter(|value| !value.trim().is_empty()) {
        message_ref = message_ref.with_parent_id(thread_id)?;
    }
    if let Some(subject) = subject.filter(|value| !value.trim().is_empty()) {
        message_ref = message_ref.with_label(subject)?;
    }
    refs.push(message_ref);

    if let Some(thread_id) = thread_id.filter(|value| !value.trim().is_empty()) {
        let mut thread_ref = ToolRef::new(
            ProviderKind::GoogleWorkspace,
            namespace.clone(),
            ToolRefKind::Thread,
            thread_id,
        )?;
        if let Some(subject) = subject.filter(|value| !value.trim().is_empty()) {
            thread_ref = thread_ref.with_label(subject)?;
        }
        refs.push(thread_ref);
    }

    Ok(refs)
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

fn required_json_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|candidate| !candidate.trim().is_empty())
        .ok_or_else(|| Error::Execution(format!("missing required JSON field {key:?} in Gmail response")))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;
    use switchboard_core::{
        AuthKind, AuthSecretRefs, BackendKind, ExecutionMode, ExecutionTarget, PlannedAction, PlanningTarget,
        ProviderKind, ResolvedAuth, ResolvedCredentials, ResolvedNamespace, SecretRef, ToolRequest,
    };

    use super::decode_mail_search;
    use crate::cli::CliResponse;

    #[test]
    fn mail_search_treats_empty_stdout_as_no_results() {
        let request = ToolRequest::new(
            "google.mail.search",
            "google.work",
            ExecutionMode::Auto,
            vec![switchboard_core::ToolArgument::option("query", "from:nobody").expect("option should build")],
        )
        .expect("request should build");
        let action = PlannedAction::new(
            &request,
            &planning_target(),
            switchboard_core::ToolKind::Read,
            "Search Gmail in google.work for \"from:nobody\"",
            BackendKind::Cli,
        );

        let output = decode_mail_search(
            &execution_target(),
            &action,
            CliResponse {
                program: PathBuf::from("/usr/bin/gws"),
                version: "gws 0.99.0-test".to_owned(),
                stdout: String::new(),
                stderr: "No messages found matching query: from:nobody".to_owned(),
            },
        )
        .expect("decode should succeed");

        assert_eq!(output.fields.get("count"), Some(&json!(0)));
        assert_eq!(output.fields.get("query"), Some(&json!("from:nobody")));
        assert!(output.refs.is_empty());
    }

    fn planning_target() -> PlanningTarget {
        PlanningTarget {
            namespace: ResolvedNamespace::new(
                "google.work",
                ProviderKind::GoogleWorkspace,
                "Google Workspace (work)",
                "google.work_auth",
                false,
                Some(PathBuf::from("/tmp/gws-work")),
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "google.work_auth",
                ProviderKind::GoogleWorkspace,
                AuthKind::GoogleOAuth,
                "jess@example.com",
                AuthSecretRefs::GoogleOAuth {
                    client_id: SecretRef::new("google.work_client_id").expect("secret ref should build"),
                    client_secret: SecretRef::new("google.work_client_secret").expect("secret ref should build"),
                    refresh_token: Some(SecretRef::new("google.work_refresh_token").expect("secret ref should build")),
                },
            )
            .expect("auth should build"),
        }
    }

    fn execution_target() -> ExecutionTarget {
        ExecutionTarget {
            namespace: planning_target().namespace,
            auth: planning_target().auth,
            credentials: ResolvedCredentials::GoogleOAuth {
                client_id: "client-id".to_owned().into(),
                client_secret: "client-secret".to_owned().into(),
                refresh_token: Some("refresh-token".to_owned().into()),
            },
        }
    }
}
