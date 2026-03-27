use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::process::ExitCode;
use std::sync::Arc;

use switchboard_core::{
    DispatchOutcome, ExecutionMode, NamespaceId, ProviderKind, Result, Switchboard, ToolOutput, ToolRequest,
};
use switchboard_providers::default_registry;
use switchboard_store::{DefaultPolicyEngine, MemoryAuditSink, StaticNamespaceStore};

pub fn default_switchboard() -> Result<Switchboard> {
    let namespaces = Arc::new(StaticNamespaceStore::bootstrap()?);
    let policy = Arc::new(DefaultPolicyEngine);
    let audit = Arc::new(MemoryAuditSink::default());
    let adapters = default_registry();

    Ok(Switchboard::new(namespaces, policy, audit, adapters))
}

pub fn main_entry<I>(args: I) -> ExitCode
where
    I: IntoIterator<Item = String>,
{
    match run(args) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(RunError { message, json }) => {
            if json {
                println!("{}", render_json_error(&message));
            } else {
                eprintln!("{message}");
            }

            ExitCode::FAILURE
        }
    }
}

fn run<I>(args: I) -> std::result::Result<String, RunError>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let command = parse_command(&args)?;
    let switchboard = default_switchboard().map_err(|error| RunError {
        message: error.to_string(),
        json: command.json,
    })?;

    match command.kind {
        CommandKind::Help => Ok(render_help()),
        CommandKind::NamespaceList => {
            let namespaces = switchboard.list_namespaces();
            if command.json {
                Ok(render_namespaces_json(&namespaces))
            } else {
                Ok(render_namespaces_human(&namespaces))
            }
        }
        CommandKind::Tool(request) => {
            let outcome = switchboard.dispatch(request).map_err(|error| RunError {
                message: error.to_string(),
                json: command.json,
            })?;

            if command.json {
                Ok(render_dispatch_json(&outcome))
            } else {
                Ok(render_dispatch_human(&outcome))
            }
        }
    }
}

#[derive(Debug)]
struct Command {
    kind: CommandKind,
    json: bool,
}

#[derive(Debug)]
enum CommandKind {
    Help,
    NamespaceList,
    Tool(ToolRequest),
}

#[derive(Debug)]
struct RunError {
    message: String,
    json: bool,
}

fn parse_command(args: &[String]) -> std::result::Result<Command, RunError> {
    let mut positionals = args.iter().skip(1).cloned().collect::<Vec<_>>();
    if positionals.is_empty() {
        return Ok(Command {
            kind: CommandKind::Help,
            json: false,
        });
    }

    if positionals[0] == "help" || positionals[0] == "--help" || positionals[0] == "-h" {
        return Ok(Command {
            kind: CommandKind::Help,
            json: false,
        });
    }

    if positionals[0] == "ns" && positionals.get(1).map(String::as_str) == Some("list") {
        let json = positionals.iter().any(|value| value == "--json");
        return Ok(Command {
            kind: CommandKind::NamespaceList,
            json,
        });
    }

    let tool = positionals.remove(0);
    let mut namespace = None;
    let mut args_map = BTreeMap::new();
    let mut mode = ExecutionMode::Auto;
    let mut json = false;
    let mut index = 0;

    while index < positionals.len() {
        let current = &positionals[index];
        match current.as_str() {
            "--json" => {
                json = true;
                index += 1;
            }
            "--plan" => {
                mode = ExecutionMode::Plan;
                index += 1;
            }
            "--draft" => {
                mode = ExecutionMode::Draft;
                index += 1;
            }
            "--apply" => {
                mode = ExecutionMode::Apply;
                index += 1;
            }
            "--ns" => {
                let value = positionals.get(index + 1).ok_or_else(|| RunError {
                    message: "missing value for --ns".into(),
                    json,
                })?;
                namespace = Some(value.clone());
                index += 2;
            }
            _ if current.starts_with("--") => {
                let key = current.trim_start_matches("--");
                let value = positionals.get(index + 1).ok_or_else(|| RunError {
                    message: format!("missing value for {current}"),
                    json,
                })?;
                args_map.insert(key.to_string(), value.clone());
                index += 2;
            }
            _ => {
                return Err(RunError {
                    message: format!("unexpected argument: {current}"),
                    json,
                });
            }
        }
    }

    let namespace = namespace.ok_or_else(|| RunError {
        message: "tool commands require --ns <namespace>".into(),
        json,
    })?;

    let request = ToolRequest::new(tool, namespace, mode, args_map).map_err(|error| RunError {
        message: error.to_string(),
        json,
    })?;

    Ok(Command {
        kind: CommandKind::Tool(request),
        json,
    })
}

fn render_help() -> String {
    let lines = [
        "switchboard",
        "",
        "Usage:",
        "  switchboard ns list [--json]",
        "  switchboard <tool> --ns <namespace> [--plan|--draft|--apply] [--json] [--key value ...]",
        "",
        "Examples:",
        "  switchboard ns list",
        "  switchboard github.notifications.list --ns github.personal --json",
        "  switchboard google.mail.search --ns google.work --query 'from:finance newer_than:7d'",
        "  switchboard github.pull_request.comment --ns github.personal --repo owner/repo --number 123 --body 'needs tests' --draft",
    ];

    format!("{}\n", lines.join("\n"))
}

fn render_namespaces_human(namespaces: &[switchboard_core::ResolvedNamespace]) -> String {
    let mut output = String::from("Namespaces\n");
    for namespace in namespaces {
        let _ = writeln!(
            output,
            "- {} ({}, account={}, default_read={})",
            namespace.id, namespace.provider, namespace.account_label, namespace.default_read
        );
    }

    output
}

fn render_dispatch_human(outcome: &DispatchOutcome) -> String {
    match outcome {
        DispatchOutcome::Planned(plan) => {
            let mut output = String::new();
            let _ = writeln!(output, "Planned: {}", plan.summary);
            let _ = writeln!(output, "Tool: {}", plan.tool);
            let _ = writeln!(output, "Namespace: {}", plan.namespace);
            let _ = writeln!(output, "Backend: {}", plan.backend);
            let _ = writeln!(output, "Approval required: {}", plan.approval_required);
            if let Some(reason) = &plan.approval_reason {
                let _ = writeln!(output, "Approval reason: {reason}");
            }
            output
        }
        DispatchOutcome::Executed(output) => render_output_human(output),
    }
}

fn render_output_human(output: &ToolOutput) -> String {
    let mut rendered = String::new();
    let _ = writeln!(rendered, "Executed: {}", output.summary);
    let _ = writeln!(rendered, "Tool: {}", output.tool);
    let _ = writeln!(rendered, "Namespace: {}", output.namespace);
    if !output.fields.is_empty() {
        let _ = writeln!(rendered, "Fields:");
        for (key, value) in &output.fields {
            let _ = writeln!(rendered, "- {key}: {value}");
        }
    }

    rendered
}

fn render_namespaces_json(namespaces: &[switchboard_core::ResolvedNamespace]) -> String {
    let items = namespaces
        .iter()
        .map(|namespace| {
            format!(
                "{{\"id\":{},\"provider\":{},\"account_label\":{},\"default_read\":{}}}",
                json_string(namespace.id.as_str()),
                json_string(&namespace.provider.to_string()),
                json_string(&namespace.account_label),
                namespace.default_read
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    format!("{{\"namespaces\":[{items}]}}")
}

fn render_dispatch_json(outcome: &DispatchOutcome) -> String {
    match outcome {
        DispatchOutcome::Planned(plan) => {
            let approval_reason = match &plan.approval_reason {
                Some(reason) => json_string(reason),
                None => "null".into(),
            };

            format!(
                concat!(
                    "{{",
                    "\"status\":\"planned\",",
                    "\"tool\":{},",
                    "\"namespace\":{},",
                    "\"summary\":{},",
                    "\"backend\":{},",
                    "\"approval_required\":{},",
                    "\"approval_reason\":{}",
                    "}}"
                ),
                json_string(plan.tool.as_str()),
                json_string(plan.namespace.as_str()),
                json_string(&plan.summary),
                json_string(&plan.backend.to_string()),
                plan.approval_required,
                approval_reason
            )
        }
        DispatchOutcome::Executed(output) => {
            let fields = output
                .fields
                .iter()
                .map(|(key, value)| format!("{}:{}", json_string(key), json_string(value)))
                .collect::<Vec<_>>()
                .join(",");

            format!(
                concat!(
                    "{{",
                    "\"status\":\"executed\",",
                    "\"tool\":{},",
                    "\"namespace\":{},",
                    "\"summary\":{},",
                    "\"fields\":{{{}}}",
                    "}}"
                ),
                json_string(output.tool.as_str()),
                json_string(output.namespace.as_str()),
                json_string(&output.summary),
                fields
            )
        }
    }
}

fn render_json_error(message: &str) -> String {
    format!("{{\"status\":\"error\",\"error\":{}}}", json_string(message))
}

fn json_string(value: &str) -> String {
    let mut escaped = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

pub fn detect_provider(namespace: &NamespaceId) -> Option<ProviderKind> {
    let value = namespace.as_str();
    if value.starts_with("github.") {
        Some(ProviderKind::GitHub)
    } else if value.starts_with("google.") {
        Some(ProviderKind::GoogleWorkspace)
    } else {
        None
    }
}

pub fn args_from_env() -> Vec<String> {
    env::args().collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use switchboard_core::{DispatchOutcome, ExecutionMode, ToolRequest};

    use crate::default_switchboard;

    #[test]
    fn bootstrap_namespaces_match_current_examples() {
        let switchboard = default_switchboard().expect("switchboard should build");
        let namespaces = switchboard.list_namespaces();
        let ids = namespaces
            .into_iter()
            .map(|namespace| namespace.id.to_string())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["github.personal", "google.personal", "google.work"]);
    }

    #[test]
    fn write_requests_default_to_planning_until_approval_exists() {
        let switchboard = default_switchboard().expect("switchboard should build");
        let request = ToolRequest::new(
            "github.pull_request.comment",
            "github.personal",
            ExecutionMode::Auto,
            BTreeMap::from([
                ("repo".into(), "owner/repo".into()),
                ("number".into(), "42".into()),
                ("body".into(), "Needs a regression test".into()),
            ]),
        )
        .expect("request should parse");

        let outcome = switchboard.dispatch(request).expect("dispatch should succeed");
        match outcome {
            DispatchOutcome::Planned(plan) => {
                assert!(plan.approval_required);
                assert_eq!(plan.backend.to_string(), "cli");
            }
            DispatchOutcome::Executed(_) => {
                panic!("write requests should not execute yet");
            }
        }
    }

    #[test]
    fn read_requests_execute_into_stub_results() {
        let switchboard = default_switchboard().expect("switchboard should build");
        let request = ToolRequest::new(
            "google.mail.search",
            "google.work",
            ExecutionMode::Auto,
            BTreeMap::from([("query".into(), "from:finance".into())]),
        )
        .expect("request should parse");

        let outcome = switchboard.dispatch(request).expect("dispatch should succeed");
        match outcome {
            DispatchOutcome::Executed(output) => {
                assert_eq!(output.fields.get("status").map(String::as_str), Some("stub"));
            }
            DispatchOutcome::Planned(_) => {
                panic!("read requests should execute by default");
            }
        }
    }
}
