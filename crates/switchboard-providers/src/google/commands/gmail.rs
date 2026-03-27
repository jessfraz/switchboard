use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Map, Value};
use switchboard_core::{
    Error, ExecutionTarget, NamespaceId, OperationEffect, PlannedAction, ProviderKind, ResolvedNamespace, Result,
    ToolOutput, ToolRef, ToolRefKind, ToolRequest,
};

use crate::{
    cli::{CliCommandHandler, CliResponse},
    google::commands::{append_optional_flag, append_optional_value},
};

pub(crate) const MAIL_SEARCH_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "mail_search",
    summarize: summarize_mail_search,
    build_args: Some(build_mail_search_args),
    decode: Some(decode_mail_search),
};

pub(crate) const MAIL_READ_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "mail_read",
    summarize: summarize_mail_read,
    build_args: Some(build_mail_read_args),
    decode: Some(decode_mail_read),
};

pub(crate) const MAIL_DRAFT_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "mail_draft",
    summarize: summarize_mail_draft,
    build_args: Some(build_mail_draft_args),
    decode: Some(decode_mail_draft),
};

fn summarize_mail_search(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let query = required_arg(request, &["query"])?;
    Ok(format!("Search Gmail in {} for {query:?}", namespace.id))
}

fn summarize_mail_read(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let message_id = required_arg(request, &["gmail-message-id", "message-id", "id"])?;
    Ok(format!("Read Gmail message {message_id} in {}", namespace.id))
}

fn summarize_mail_draft(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let recipients = required_repeatable_arg(request, "to")?;
    let recipient_summary = summarize_recipients(&recipients);
    let body_kind = render_body_kind(request.args.value("body-text"), request.args.value("body-html"))?;

    match request.args.value("subject") {
        Some(subject) => Ok(format!(
            "Draft Gmail {body_kind} email to {recipient_summary} with subject {subject:?} in {}",
            namespace.id
        )),
        None => Ok(format!(
            "Draft Gmail {body_kind} email to {recipient_summary} in {}",
            namespace.id
        )),
    }
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

fn build_mail_draft_args(action: &PlannedAction) -> Result<Vec<String>> {
    let raw_message = build_gmail_raw_message(action)?;
    let mut message = Map::from_iter([(String::from("raw"), Value::String(raw_message))]);
    if let Some(thread_id) = action.args.value("thread-id") {
        message.insert(String::from("threadId"), Value::String(thread_id.to_owned()));
    }

    Ok(vec![
        "gmail".to_owned(),
        "users".to_owned(),
        "drafts".to_owned(),
        "create".to_owned(),
        "--params".to_owned(),
        json!({ "userId": "me" }).to_string(),
        "--json".to_owned(),
        Value::Object(Map::from_iter([(String::from("message"), Value::Object(message))])).to_string(),
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

fn decode_mail_draft(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
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
    let draft = simplify_draft_message(action, &value)?;
    let refs = mail_draft_refs(&action.namespace, &draft)?;
    let subject = draft
        .get("subject")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("draft email");

    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!("Drafted Gmail message {subject:?} for {}", action.namespace),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field("draft", draft)
    .with_refs(refs.clone())
    .with_effect(OperationEffect::new(false).with_refs(refs));

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

fn simplify_draft_message(action: &PlannedAction, value: &Value) -> Result<Value> {
    let draft_id = required_json_string(value, "id")?;
    let message = value
        .get("message")
        .ok_or_else(|| Error::Execution("missing required JSON field \"message\" in Gmail draft response".into()))?;
    let gmail_message_id = required_nested_json_string(message, "id")?;
    let gmail_thread_id = message
        .get("threadId")
        .and_then(Value::as_str)
        .filter(|candidate| !candidate.trim().is_empty())
        .map(str::to_owned);
    let to = collect_repeatable_action_values(action, "to");
    let cc = collect_repeatable_action_values(action, "cc");
    let bcc = collect_repeatable_action_values(action, "bcc");

    Ok(json!({
        "draft_id": draft_id,
        "gmail_message_id": gmail_message_id,
        "gmail_thread_id": gmail_thread_id,
        "to": to,
        "cc": cc,
        "bcc": bcc,
        "subject": action.args.value("subject"),
        "label_ids": message.get("labelIds").cloned().unwrap_or_else(|| json!([])),
        "has_body_text": action.args.value("body-text").is_some(),
        "has_body_html": action.args.value("body-html").is_some(),
    }))
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

fn mail_draft_refs(namespace: &NamespaceId, draft: &Value) -> Result<Vec<ToolRef>> {
    let gmail_message_id = required_json_string(draft, "gmail_message_id")?;
    let subject = draft.get("subject").and_then(Value::as_str);
    let thread_id = draft.get("gmail_thread_id").and_then(Value::as_str);

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

fn build_gmail_raw_message(action: &PlannedAction) -> Result<String> {
    let to = required_repeatable_action_arg(action, "to")?;
    for recipient in &to {
        validate_header_value("to", recipient)?;
    }

    let cc = collect_repeatable_action_values(action, "cc");
    for recipient in &cc {
        validate_header_value("cc", recipient)?;
    }
    let bcc = collect_repeatable_action_values(action, "bcc");
    for recipient in &bcc {
        validate_header_value("bcc", recipient)?;
    }

    if let Some(from) = action.args.value("from") {
        validate_header_value("from", from)?;
    }
    if let Some(reply_to) = action.args.value("reply-to") {
        validate_header_value("reply-to", reply_to)?;
    }
    if let Some(subject) = action.args.value("subject") {
        validate_header_value("subject", subject)?;
    }
    if let Some(in_reply_to) = action.args.value("in-reply-to") {
        validate_header_value("in-reply-to", in_reply_to)?;
    }
    for reference in action.args.values("reference") {
        validate_header_value("reference", reference)?;
    }

    let body_text = action.args.value("body-text");
    let body_html = action.args.value("body-html");
    let content = render_body_part(body_text, body_html)?;
    let mut message = String::new();

    if let Some(from) = action.args.value("from") {
        message.push_str(&format!("From: {from}\r\n"));
    }
    message.push_str(&format!("To: {}\r\n", to.join(", ")));
    if !cc.is_empty() {
        message.push_str(&format!("Cc: {}\r\n", cc.join(", ")));
    }
    if !bcc.is_empty() {
        message.push_str(&format!("Bcc: {}\r\n", bcc.join(", ")));
    }
    if let Some(reply_to) = action.args.value("reply-to") {
        message.push_str(&format!("Reply-To: {reply_to}\r\n"));
    }
    if let Some(subject) = action.args.value("subject") {
        message.push_str(&format!("Subject: {subject}\r\n"));
    }
    if let Some(in_reply_to) = action.args.value("in-reply-to") {
        message.push_str(&format!("In-Reply-To: {in_reply_to}\r\n"));
    }
    let references = collect_repeatable_action_values(action, "reference");
    if !references.is_empty() {
        message.push_str(&format!("References: {}\r\n", references.join(" ")));
    }
    message.push_str("MIME-Version: 1.0\r\n");
    message.push_str(&content);

    Ok(URL_SAFE_NO_PAD.encode(message.as_bytes()))
}

fn render_body_part(body_text: Option<&str>, body_html: Option<&str>) -> Result<String> {
    match (body_text, body_html) {
        (Some(body_text), Some(body_html)) => {
            let boundary = "switchboard-alt-boundary";
            Ok(format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_text}\r\n--{boundary}\r\nContent-Type: text/html; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_html}\r\n--{boundary}--\r\n"
            ))
        }
        (Some(body_text), None) => Ok(format!(
            "Content-Type: text/plain; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_text}\r\n"
        )),
        (None, Some(body_html)) => Ok(format!(
            "Content-Type: text/html; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_html}\r\n"
        )),
        (None, None) => Err(Error::InvalidArguments(
            "google.mail.draft requires --body-text, --body-html, or both".into(),
        )),
    }
}

fn validate_header_value(name: &str, value: &str) -> Result<()> {
    if value.contains(['\r', '\n']) {
        return Err(Error::InvalidArguments(format!(
            "--{name} cannot contain carriage returns or newlines"
        )));
    }

    if value.trim().is_empty() {
        return Err(Error::InvalidArguments(format!("--{name} cannot be empty")));
    }

    Ok(())
}

fn summarize_recipients(recipients: &[String]) -> String {
    match recipients {
        [] => "nobody".to_owned(),
        [single] => single.clone(),
        [first, second] => format!("{first} and {second}"),
        [first, ..] => format!("{first} and {} others", recipients.len() - 1),
    }
}

fn render_body_kind(body_text: Option<&str>, body_html: Option<&str>) -> Result<&'static str> {
    match (body_text, body_html) {
        (Some(_), Some(_)) => Ok("multipart"),
        (Some(_), None) => Ok("text"),
        (None, Some(_)) => Ok("html"),
        (None, None) => Err(Error::InvalidArguments(
            "google.mail.draft requires --body-text, --body-html, or both".into(),
        )),
    }
}

fn collect_repeatable_action_values(action: &PlannedAction, key: &str) -> Vec<String> {
    action.args.values(key).map(str::to_owned).collect()
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

fn required_repeatable_arg(request: &ToolRequest, key: &str) -> Result<Vec<String>> {
    let values = request.args.values(key).map(str::to_owned).collect::<Vec<_>>();
    if values.is_empty() {
        return Err(Error::InvalidArguments(format!(
            "missing required argument --{key} for {}",
            request.tool
        )));
    }

    Ok(values)
}

fn required_repeatable_action_arg(action: &PlannedAction, key: &str) -> Result<Vec<String>> {
    let values = collect_repeatable_action_values(action, key);
    if values.is_empty() {
        return Err(Error::InvalidArguments(format!(
            "missing required argument --{key} for {}",
            action.tool
        )));
    }

    Ok(values)
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

fn required_nested_json_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|candidate| !candidate.trim().is_empty())
        .ok_or_else(|| Error::Execution(format!("missing required JSON field {key:?} in nested Gmail response")))
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
