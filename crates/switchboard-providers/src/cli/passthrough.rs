use serde_json::{json, Value};
use switchboard_core::{
    Error, ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolArguments, ToolOutput, ToolRequest,
};

use crate::cli::{command::CliResponse, executor::CliStdioMode};

pub(crate) fn summarize_prefixed_passthrough(
    namespace: &ResolvedNamespace,
    request: &ToolRequest,
    program: &str,
    prefix: &[String],
) -> Result<String> {
    let argv = merge_prefixed_argv(prefix, parse_passthrough_argv(&request.args)?);
    Ok(format!(
        "Run raw {program} command for {}: {}",
        namespace.id,
        summarize_argv(&argv)
    ))
}

pub(crate) fn build_prefixed_passthrough_args(action: &PlannedAction, prefix: &[String]) -> Result<Vec<String>> {
    Ok(merge_prefixed_argv(prefix, parse_passthrough_argv(&action.args)?))
}

pub(crate) fn prefixed_passthrough_stdio_mode(action: &PlannedAction, prefix: &[String]) -> Result<CliStdioMode> {
    let argv = merge_prefixed_argv(prefix, parse_passthrough_argv(&action.args)?);
    Ok(if requires_interactive_stdio(&argv) {
        CliStdioMode::Inherit
    } else {
        CliStdioMode::Capture
    })
}

pub(crate) fn decode_prefixed_passthrough(
    target: &ExecutionTarget,
    action: &PlannedAction,
    response: CliResponse,
    program: &str,
    prefix: &[String],
) -> Result<ToolOutput> {
    let CliResponse {
        version,
        stdout,
        stderr,
        ..
    } = response;
    let argv = merge_prefixed_argv(prefix, parse_passthrough_argv(&action.args)?);
    let mut output = ToolOutput::new(
        action.tool.clone(),
        action.namespace.clone(),
        format!(
            "Ran raw {program} command for {}: {}",
            action.namespace,
            summarize_argv(&argv)
        ),
    )
    .with_field("status", "ok")
    .with_field("backend", action.backend.to_string())
    .with_field("auth", target.auth.id.to_string())
    .with_field("cli_version", version)
    .with_value_field("argv", json!(argv));

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

pub(crate) fn parse_passthrough_argv(arguments: &ToolArguments) -> Result<Vec<String>> {
    let repeated = arguments.values("argv").map(ToOwned::to_owned).collect::<Vec<_>>();
    let inline = arguments.value("argv-json");

    if !repeated.is_empty() && inline.is_some() {
        return Err(Error::InvalidArguments(
            "use either repeated --argv values or one --argv-json array, not both".into(),
        ));
    }

    if let Some(inline) = inline {
        let value: Value = serde_json::from_str(inline).map_err(|error| {
            Error::InvalidArguments(format!("--argv-json must be a JSON array of strings: {error}"))
        })?;
        let array = value
            .as_array()
            .ok_or_else(|| Error::InvalidArguments("--argv-json must be a JSON array of strings".into()))?;
        let argv = array
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| Error::InvalidArguments("--argv-json must contain only string values".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        if argv.is_empty() {
            return Err(Error::InvalidArguments(
                "raw CLI passthrough requires at least one argv token".into(),
            ));
        }
        return Ok(argv);
    }

    if repeated.is_empty() {
        return Err(Error::InvalidArguments(
            "raw CLI passthrough requires either repeated --argv values or --argv-json".into(),
        ));
    }

    Ok(repeated)
}

fn summarize_argv(argv: &[String]) -> String {
    let preview_len = argv.len().min(6);
    let mut summary = argv[..preview_len].join(" ");
    if argv.len() > preview_len {
        summary.push_str(" ...");
    }
    summary
}

fn merge_prefixed_argv(prefix: &[String], argv: Vec<String>) -> Vec<String> {
    prefix.iter().cloned().chain(argv).collect()
}

fn requires_interactive_stdio(argv: &[String]) -> bool {
    argv.windows(2)
        .any(|window| window[0] == "auth" && window[1].starts_with("login"))
}

#[cfg(test)]
mod tests {
    use switchboard_core::{BackendKind, ExecutionMode, PlannedAction, ToolArgument, ToolKind, ToolRequest};

    use crate::cli::{executor::CliStdioMode, passthrough::prefixed_passthrough_stdio_mode};

    fn passthrough_action(argv: serde_json::Value) -> PlannedAction {
        let request = ToolRequest::new(
            "google.cli.write",
            "google.work",
            ExecutionMode::Apply,
            vec![ToolArgument::option("argv-json", argv.to_string()).expect("argv-json should build")],
        )
        .expect("request should build");

        PlannedAction {
            tool: request.tool.clone(),
            namespace: request.namespace.clone(),
            auth_ref: switchboard_core::AuthRef::new("google_work").expect("auth ref should build"),
            kind: ToolKind::Write,
            mode: request.mode,
            summary: "raw".into(),
            backend: BackendKind::Cli,
            approval_required: true,
            approval_reason: Some("approval".into()),
            args: request.args,
            operation_id: None,
            compensates_operation_id: None,
        }
    }

    #[test]
    fn auth_login_passthrough_uses_inherited_stdio() {
        let action = passthrough_action(serde_json::json!(["auth", "login"]));

        let mode = prefixed_passthrough_stdio_mode(&action, &[]).expect("stdio mode should resolve");

        assert_eq!(mode, CliStdioMode::Inherit);
    }

    #[test]
    fn portal_auth_login_password_uses_inherited_stdio() {
        let action = passthrough_action(serde_json::json!(["portal", "auth", "login-password"]));

        let mode = prefixed_passthrough_stdio_mode(&action, &[]).expect("stdio mode should resolve");

        assert_eq!(mode, CliStdioMode::Inherit);
    }

    #[test]
    fn json_passthrough_keeps_captured_stdio() {
        let action = passthrough_action(serde_json::json!([
            "gmail", "users", "messages", "list", "--format", "json"
        ]));

        let mode = prefixed_passthrough_stdio_mode(&action, &[]).expect("stdio mode should resolve");

        assert_eq!(mode, CliStdioMode::Capture);
    }
}
