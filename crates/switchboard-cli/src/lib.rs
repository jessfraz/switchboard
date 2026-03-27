use std::{collections::BTreeMap, env, ffi::OsString, process::ExitCode, sync::Arc};

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use switchboard_core::{
    AggregateReadOutcome, AggregateReadRequest, BackendKind, DispatchOutcome, ExecutionMode, NamespaceId,
    OperationOutcome, OperationRequest, ResolvedNamespace, Result, Switchboard, ToolName, ToolOutput, ToolRequest,
};
use switchboard_providers::default_registry;
use switchboard_store::{DefaultPolicyEngine, MemoryAuditSink, StaticNamespaceStore};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  switchboard ns list\n",
    "  switchboard github.notifications.list --ns github.personal --json\n",
    "  switchboard google.mail.search --ns google.work --query 'from:finance newer_than:7d'\n",
    "  switchboard google.calendar.list --ns google.work --ns google.personal --json\n",
    "  switchboard github.pull_request.comment --ns github.personal --repo owner/repo --number 123 --body 'needs tests' --draft\n"
);

pub fn default_switchboard() -> Result<Switchboard> {
    let namespaces = Arc::new(StaticNamespaceStore::bootstrap()?);
    let policy = Arc::new(DefaultPolicyEngine);
    let audit = Arc::new(MemoryAuditSink::default());
    let adapters = default_registry();

    Ok(Switchboard::new(namespaces, policy, audit, adapters))
}

pub fn main_entry<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let json_requested = contains_flag(&args, "--json");
    let cli = match Cli::try_parse_from(args.clone()) {
        Ok(cli) => cli,
        Err(error) => return render_clap_error(error, json_requested),
    };

    match run(cli) {
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

fn run(cli: Cli) -> std::result::Result<String, RunError> {
    let json_requested = cli.json_requested();
    let switchboard = default_switchboard().map_err(|error| RunError {
        message: error.to_string(),
        json: json_requested,
    })?;

    match cli.command.into_runtime_command()? {
        CommandKind::NamespaceList => {
            let namespaces = switchboard.list_namespaces();
            if json_requested {
                render_json(&NamespaceListResponse { namespaces }, true)
            } else {
                Ok(render_namespaces_human(&namespaces))
            }
        }
        CommandKind::Operation(request) => {
            let outcome = switchboard.execute_operation(request).map_err(|error| RunError {
                message: error.to_string(),
                json: json_requested,
            })?;

            if json_requested {
                render_json_operation(&outcome)
            } else {
                Ok(render_operation_human(&outcome))
            }
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "switchboard",
    version,
    about = "Rust-first local automation plane",
    disable_help_subcommand = true,
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

impl Cli {
    fn json_requested(&self) -> bool {
        match &self.command {
            Commands::Ns(namespace) => namespace.json_requested(),
            Commands::Tool(tokens) => contains_json_os_tokens(tokens),
        }
    }
}

#[derive(Debug, Subcommand)]
enum Commands {
    Ns(NamespaceCommand),
    #[command(external_subcommand)]
    Tool(Vec<OsString>),
}

impl Commands {
    fn into_runtime_command(self) -> std::result::Result<CommandKind, RunError> {
        match self {
            Self::Ns(namespace) => Ok(namespace.into_runtime_command()),
            Self::Tool(tokens) => parse_external_tool_invocation(tokens).map(CommandKind::Operation),
        }
    }
}

#[derive(Debug, Args)]
struct NamespaceCommand {
    #[command(subcommand)]
    command: NamespaceSubcommand,
}

impl NamespaceCommand {
    fn json_requested(&self) -> bool {
        match &self.command {
            NamespaceSubcommand::List(arguments) => arguments.json,
        }
    }

    fn into_runtime_command(self) -> CommandKind {
        match self.command {
            NamespaceSubcommand::List(_) => CommandKind::NamespaceList,
        }
    }
}

#[derive(Debug, Subcommand)]
enum NamespaceSubcommand {
    List(ListNamespaceArgs),
}

#[derive(Debug, Args)]
struct ListNamespaceArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug)]
enum CommandKind {
    NamespaceList,
    Operation(OperationRequest),
}

#[derive(Debug)]
struct RunError {
    message: String,
    json: bool,
}

fn parse_external_tool_invocation(tokens: Vec<OsString>) -> std::result::Result<OperationRequest, RunError> {
    let mut positionals = tokens.into_iter().map(os_string_to_string).collect::<Vec<_>>();
    let json = contains_json_token(&positionals);
    let tool = positionals.first().cloned().ok_or_else(|| RunError {
        message: "missing tool name".into(),
        json,
    })?;
    positionals.remove(0);
    let mut namespaces = Vec::new();
    let mut args_map = BTreeMap::new();
    let mut mode = ExecutionMode::Auto;
    let mut index = 0;

    while index < positionals.len() {
        let current = &positionals[index];
        match current.as_str() {
            "--json" => index += 1,
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
                namespaces.push(value.clone());
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

    if namespaces.is_empty() {
        return Err(RunError {
            message: "tool commands require at least one --ns <namespace>".into(),
            json,
        });
    }

    if namespaces.len() == 1 {
        let request = ToolRequest::new(tool, namespaces.remove(0), mode, args_map).map_err(|error| RunError {
            message: error.to_string(),
            json,
        })?;

        return Ok(OperationRequest::single(request));
    }

    let request = AggregateReadRequest::new(tool, namespaces, mode, args_map).map_err(|error| RunError {
        message: error.to_string(),
        json,
    })?;

    Ok(OperationRequest::aggregate_read(request))
}

fn render_namespaces_human(namespaces: &[ResolvedNamespace]) -> String {
    let mut output = String::from("Namespaces\n");
    for namespace in namespaces {
        output.push_str(&format!(
            "- {} ({}, account={}, default_read={})\n",
            namespace.id, namespace.provider, namespace.account_label, namespace.default_read
        ));
    }

    output
}

fn render_operation_human(outcome: &OperationOutcome) -> String {
    match outcome {
        OperationOutcome::Single(outcome) => render_dispatch_human(outcome),
        OperationOutcome::AggregateRead(outcome) => render_aggregate_read_human(outcome),
    }
}

fn render_dispatch_human(outcome: &DispatchOutcome) -> String {
    match outcome {
        DispatchOutcome::Planned(plan) => {
            let mut output = String::new();
            output.push_str(&format!("Planned: {}\n", plan.summary));
            output.push_str(&format!("Tool: {}\n", plan.tool));
            output.push_str(&format!("Namespace: {}\n", plan.namespace));
            output.push_str(&format!("Backend: {}\n", plan.backend));
            output.push_str(&format!("Approval required: {}\n", plan.approval_required));
            if let Some(reason) = &plan.approval_reason {
                output.push_str(&format!("Approval reason: {reason}\n"));
            }
            output
        }
        DispatchOutcome::Executed(output) => render_output_human(output),
    }
}

fn render_output_human(output: &ToolOutput) -> String {
    let mut rendered = String::new();
    rendered.push_str(&format!("Executed: {}\n", output.summary));
    rendered.push_str(&format!("Tool: {}\n", output.tool));
    rendered.push_str(&format!("Namespace: {}\n", output.namespace));
    if !output.fields.is_empty() {
        rendered.push_str("Fields:\n");
        for (key, value) in &output.fields {
            rendered.push_str(&format!("- {key}: {value}\n"));
        }
    }

    rendered
}

fn render_aggregate_read_human(outcome: &AggregateReadOutcome) -> String {
    let namespaces = outcome
        .namespaces
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut output = format!("Aggregate read: {}\nNamespaces: {namespaces}\n", outcome.tool);

    for result in &outcome.results {
        output.push('\n');
        output.push_str(&format!("[{}]\n", result.namespace));

        let rendered = render_dispatch_human(&result.outcome);
        for line in rendered.lines() {
            output.push_str(&format!("  {line}\n"));
        }
    }

    output
}

fn render_json_operation(outcome: &OperationOutcome) -> std::result::Result<String, RunError> {
    match outcome {
        OperationOutcome::Single(outcome) => render_json_dispatch(outcome),
        OperationOutcome::AggregateRead(outcome) => render_json(
            &AggregateReadResponse {
                status: "aggregate_read",
                tool: &outcome.tool,
                namespaces: &outcome.namespaces,
                results: outcome
                    .results
                    .iter()
                    .map(|result| AggregateReadResultResponse {
                        namespace: &result.namespace,
                        outcome: DispatchResponse::from(&result.outcome),
                    })
                    .collect(),
            },
            true,
        ),
    }
}

fn render_json_dispatch(outcome: &DispatchOutcome) -> std::result::Result<String, RunError> {
    match outcome {
        DispatchOutcome::Planned(plan) => render_json(&DispatchResponse::from_plan(plan), true),
        DispatchOutcome::Executed(output) => render_json(&DispatchResponse::from_output(output), true),
    }
}

fn render_json_error(message: &str) -> String {
    match serde_json::to_string_pretty(&ErrorResponse {
        status: "error",
        error: message,
    }) {
        Ok(json) => json,
        Err(_) => "{\"status\":\"error\",\"error\":\"failed to serialize error\"}".into(),
    }
}

fn render_json<T>(value: &T, json: bool) -> std::result::Result<String, RunError>
where
    T: Serialize,
{
    serde_json::to_string_pretty(value).map_err(|error| RunError {
        message: format!("failed to serialize JSON output: {error}"),
        json,
    })
}

fn render_clap_error(error: clap::Error, json_requested: bool) -> ExitCode {
    match error.kind() {
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
            print!("{error}");
            ExitCode::SUCCESS
        }
        _ => {
            if json_requested {
                println!("{}", render_json_error(&error.to_string()));
            } else {
                eprint!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}

fn contains_flag(args: &[OsString], flag: &str) -> bool {
    args.iter().any(|value| value == flag)
}

fn contains_json_os_tokens(tokens: &[OsString]) -> bool {
    tokens.iter().any(|value| value == "--json")
}

fn contains_json_token(tokens: &[impl AsRef<str>]) -> bool {
    tokens.iter().any(|value| value.as_ref() == "--json")
}

fn os_string_to_string(value: OsString) -> String {
    match value.into_string() {
        Ok(value) => value,
        Err(value) => value.to_string_lossy().into_owned(),
    }
}

pub fn args_from_env() -> Vec<OsString> {
    env::args_os().collect()
}

#[derive(Serialize)]
struct NamespaceListResponse {
    namespaces: Vec<ResolvedNamespace>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum DispatchResponse<'a> {
    Planned {
        tool: &'a ToolName,
        namespace: &'a NamespaceId,
        summary: &'a str,
        backend: BackendKind,
        approval_required: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval_reason: Option<&'a str>,
    },
    Executed {
        tool: &'a ToolName,
        namespace: &'a NamespaceId,
        summary: &'a str,
        fields: &'a BTreeMap<String, String>,
    },
}

impl<'a> DispatchResponse<'a> {
    fn from(outcome: &'a DispatchOutcome) -> Self {
        match outcome {
            DispatchOutcome::Planned(plan) => Self::from_plan(plan),
            DispatchOutcome::Executed(output) => Self::from_output(output),
        }
    }

    fn from_plan(plan: &'a switchboard_core::PlannedAction) -> Self {
        Self::Planned {
            tool: &plan.tool,
            namespace: &plan.namespace,
            summary: &plan.summary,
            backend: plan.backend,
            approval_required: plan.approval_required,
            approval_reason: plan.approval_reason.as_deref(),
        }
    }

    fn from_output(output: &'a ToolOutput) -> Self {
        Self::Executed {
            tool: &output.tool,
            namespace: &output.namespace,
            summary: &output.summary,
            fields: &output.fields,
        }
    }
}

#[derive(Serialize)]
struct AggregateReadResponse<'a> {
    status: &'static str,
    tool: &'a ToolName,
    namespaces: &'a [NamespaceId],
    results: Vec<AggregateReadResultResponse<'a>>,
}

#[derive(Serialize)]
struct AggregateReadResultResponse<'a> {
    namespace: &'a NamespaceId,
    outcome: DispatchResponse<'a>,
}

#[derive(Serialize)]
struct ErrorResponse<'a> {
    status: &'static str,
    error: &'a str,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use clap::Parser;
    use switchboard_core::{
        AggregateReadRequest, DispatchOutcome, ExecutionMode, OperationOutcome, OperationRequest, ToolRequest,
    };

    use crate::{default_switchboard, run, Cli};

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

    #[test]
    fn flat_tool_invocation_still_parses_with_clap() {
        let cli = Cli::try_parse_from([
            "switchboard",
            "google.mail.search",
            "--ns",
            "google.work",
            "--query",
            "from:finance newer_than:7d",
            "--json",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("command should run");
        let value: serde_json::Value = serde_json::from_str(&output).expect("output should be valid json");

        assert_eq!(value["status"], "executed");
        assert_eq!(value["tool"], "google.mail.search");
        assert_eq!(value["namespace"], "google.work");
    }

    #[test]
    fn repeated_namespace_flags_become_aggregate_reads() {
        let cli = Cli::try_parse_from([
            "switchboard",
            "google.calendar.list",
            "--ns",
            "google.work",
            "--ns",
            "google.personal",
            "--json",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("command should run");
        let value: serde_json::Value = serde_json::from_str(&output).expect("output should be valid json");

        assert_eq!(value["status"], "aggregate_read");
        assert_eq!(value["tool"], "google.calendar.list");
        assert_eq!(value["namespaces"][0], "google.work");
        assert_eq!(value["namespaces"][1], "google.personal");
        assert_eq!(value["results"][0]["outcome"]["status"], "executed");
        assert_eq!(value["results"][1]["outcome"]["status"], "executed");
    }

    #[test]
    fn repeated_namespace_flags_reject_write_tools() {
        let cli = Cli::try_parse_from([
            "switchboard",
            "google.calendar.create",
            "--ns",
            "google.work",
            "--ns",
            "google.personal",
            "--json",
        ])
        .expect("cli should parse");

        let error = run(cli).expect_err("aggregate write should fail");
        assert!(error.json);
        assert!(error.message.contains("aggregate reads require a read tool"));
    }

    #[test]
    fn aggregate_read_operations_can_fan_out_across_calendar_namespaces() {
        let switchboard = default_switchboard().expect("switchboard should build");
        let request = AggregateReadRequest::new(
            "google.calendar.list",
            ["google.work", "google.personal"],
            ExecutionMode::Auto,
            BTreeMap::new(),
        )
        .expect("aggregate request should parse");

        let outcome = switchboard
            .execute_operation(OperationRequest::aggregate_read(request))
            .expect("aggregate read should succeed");

        match outcome {
            OperationOutcome::AggregateRead(outcome) => {
                assert_eq!(outcome.namespaces.len(), 2);
                assert_eq!(outcome.results.len(), 2);
                assert_eq!(outcome.results[0].namespace.to_string(), "google.work");
                assert_eq!(outcome.results[1].namespace.to_string(), "google.personal");
            }
            OperationOutcome::Single(_) => {
                panic!("aggregate read should not collapse into a single operation");
            }
        }
    }

    #[test]
    fn aggregate_reads_reject_write_tools() {
        let switchboard = default_switchboard().expect("switchboard should build");
        let request = AggregateReadRequest::new(
            "google.calendar.create",
            ["google.work", "google.personal"],
            ExecutionMode::Auto,
            BTreeMap::new(),
        )
        .expect("aggregate request should parse");

        let error = switchboard
            .execute_operation(OperationRequest::aggregate_read(request))
            .expect_err("aggregate write should fail");

        assert!(error.to_string().contains("aggregate reads require a read tool"));
    }
}
