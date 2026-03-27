use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::Value as JsonValue;
use switchboard_core::{
    AggregateReadOutcome, AggregateReadRequest, AuthStore, BackendKind, DispatchOutcome, ExecutionMode, NamespaceId,
    NamespaceStore, OperationEffect, OperationOutcome, OperationRequest, ResolvedNamespace, Result, SecretResolver,
    SecretStore, Switchboard, SwitchboardServices, ToolArgument, ToolName, ToolOutput, ToolRef, ToolRequest,
};
use switchboard_providers::default_registry;
use switchboard_store::{
    DefaultPolicyEngine, LocalSecretResolver, MemoryAuditSink, MemoryOperationStore, SwitchboardConfig,
};

#[cfg(test)]
mod test_support;

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  switchboard ns list\n",
    "  switchboard github.notifications.list --ns github.personal --json\n",
    "  switchboard google.mail.search --ns google.work --query 'from:finance newer_than:7d'\n",
    "  switchboard google.calendar.list --ns google.work --ns google.personal --json\n",
    "  switchboard github.pull_request.comment --ns github.personal --repo owner/repo --number 123 --body 'needs tests' --draft\n"
);

fn load_switchboard(config_path: Option<&Path>) -> Result<Switchboard> {
    let config_path = resolve_config_path(config_path)?;
    let config = SwitchboardConfig::from_file(&config_path)?;
    let (namespaces, auth, secrets) = config.into_stores();

    Ok(build_switchboard(
        Arc::new(namespaces),
        Arc::new(auth),
        Arc::new(secrets),
        Arc::new(LocalSecretResolver::default()),
    ))
}

fn build_switchboard(
    namespaces: Arc<dyn NamespaceStore>,
    auth: Arc<dyn AuthStore>,
    secrets: Arc<dyn SecretStore>,
    secret_resolver: Arc<dyn SecretResolver>,
) -> Switchboard {
    let policy = Arc::new(DefaultPolicyEngine);
    let audit = Arc::new(MemoryAuditSink::default());
    let operations = Arc::new(MemoryOperationStore::default());
    let adapters = default_registry();

    Switchboard::new(
        SwitchboardServices {
            namespaces,
            auth,
            secrets,
            secret_resolver,
            policy,
            audit,
            operations,
        },
        adapters,
    )
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
    let config_path = cli.config.clone();
    let json_requested = cli.json_requested();
    let switchboard = load_switchboard(config_path.as_deref()).map_err(|error| RunError {
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
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

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

#[derive(Debug, Default)]
struct ConfigPathCandidates {
    explicit: Option<PathBuf>,
    env: Option<PathBuf>,
    cwd: Option<PathBuf>,
    appdata: Option<PathBuf>,
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
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
    let mut arguments = Vec::new();
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
            _ if current.starts_with("--") && current.contains('=') => {
                let (name, value) = split_inline_argument(current).ok_or_else(|| RunError {
                    message: format!("invalid argument syntax: {current}"),
                    json,
                })?;
                arguments.push(ToolArgument::option(name, value).map_err(|error| RunError {
                    message: error.to_string(),
                    json,
                })?);
                index += 1;
            }
            _ if current.starts_with("--") => {
                let key = current.trim_start_matches("--");
                let next = positionals.get(index + 1);
                if next.is_none() || next.is_some_and(|value| value.starts_with("--")) {
                    arguments.push(ToolArgument::flag(key).map_err(|error| RunError {
                        message: error.to_string(),
                        json,
                    })?);
                    index += 1;
                } else {
                    let value = next.expect("checked above");
                    arguments.push(ToolArgument::option(key, value.clone()).map_err(|error| RunError {
                        message: error.to_string(),
                        json,
                    })?);
                    index += 2;
                }
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
        let request =
            ToolRequest::new(tool, namespaces.remove(0), mode, arguments.clone()).map_err(|error| RunError {
                message: error.to_string(),
                json,
            })?;

        return Ok(OperationRequest::single(request));
    }

    let request = AggregateReadRequest::new(tool, namespaces, mode, arguments).map_err(|error| RunError {
        message: error.to_string(),
        json,
    })?;

    Ok(OperationRequest::aggregate_read(request))
}

fn split_inline_argument(argument: &str) -> Option<(&str, &str)> {
    let trimmed = argument.strip_prefix("--")?;
    let (name, value) = trimmed.split_once('=')?;
    Some((name, value))
}

fn render_namespaces_human(namespaces: &[ResolvedNamespace]) -> String {
    let mut output = String::from("Namespaces\n");
    for namespace in namespaces {
        let state_dir = namespace
            .state_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".into());
        output.push_str(&format!(
            "- {} ({}, account={}, auth={}, default_read={}, state_dir={})\n",
            namespace.id,
            namespace.provider,
            namespace.account_label,
            namespace.auth_ref,
            namespace.default_read,
            state_dir
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
            if let Some(operation_id) = &plan.operation_id {
                output.push_str(&format!("Operation ID: {operation_id}\n"));
            }
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
    if let Some(operation_id) = &output.operation_id {
        rendered.push_str(&format!("Operation ID: {operation_id}\n"));
    }
    if !output.fields.is_empty() {
        rendered.push_str("Fields:\n");
        for (key, value) in &output.fields {
            match value {
                JsonValue::String(value) => rendered.push_str(&format!("- {key}: {value}\n")),
                _ => {
                    rendered.push_str(&format!("- {key}:\n"));
                    let formatted =
                        serde_json::to_string_pretty(value).unwrap_or_else(|_| "<failed to render field>".to_owned());
                    for line in formatted.lines() {
                        rendered.push_str(&format!("  {line}\n"));
                    }
                }
            }
        }
    }
    if !output.refs.is_empty() {
        rendered.push_str("Refs:\n");
        for tool_ref in &output.refs {
            rendered.push_str(&format!("- {}\n", render_ref_human(tool_ref)));
        }
    }
    if let Some(effect) = &output.effect {
        rendered.push_str(&render_effect_human(effect));
    }

    rendered
}

fn render_ref_human(tool_ref: &ToolRef) -> String {
    let mut rendered = format!("{}:{} id={}", tool_ref.provider, tool_ref.kind, tool_ref.id);
    if let Some(parent_id) = &tool_ref.parent_id {
        rendered.push_str(&format!(" parent={parent_id}"));
    }
    if let Some(label) = &tool_ref.label {
        rendered.push_str(&format!(" label={label:?}"));
    }
    if let Some(web_url) = &tool_ref.web_url {
        rendered.push_str(&format!(" url={web_url}"));
    }

    rendered
}

fn render_effect_human(effect: &OperationEffect) -> String {
    let mut rendered = String::from("Effect:\n");
    rendered.push_str(&format!("- undoable: {}\n", effect.undoable));
    if let Some(undo_summary) = &effect.undo_summary {
        rendered.push_str(&format!("- undo_summary: {undo_summary}\n"));
    }
    if !effect.refs.is_empty() {
        rendered.push_str("- refs:\n");
        for tool_ref in &effect.refs {
            rendered.push_str(&format!("  - {}\n", render_ref_human(tool_ref)));
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

fn resolve_config_path(config_path: Option<&Path>) -> Result<PathBuf> {
    let candidates = ConfigPathCandidates {
        explicit: config_path.map(Path::to_path_buf),
        env: env::var_os("SWITCHBOARD_CONFIG").map(PathBuf::from),
        cwd: existing_file(PathBuf::from("switchboard.toml")),
        appdata: env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("switchboard").join("config.toml"))
            .filter(|path| path.is_file()),
        xdg: env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .map(|path| path.join("switchboard").join("config.toml"))
            .filter(|path| path.is_file()),
        home: env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| path.join(".config").join("switchboard").join("config.toml"))
            .filter(|path| path.is_file()),
    };

    select_config_path(candidates)
}

fn select_config_path(candidates: ConfigPathCandidates) -> Result<PathBuf> {
    candidates
        .explicit
        .or(candidates.env)
        .or(candidates.cwd)
        .or(candidates.appdata)
        .or(candidates.xdg)
        .or(candidates.home)
        .ok_or_else(|| {
            switchboard_core::Error::Config(
                "no switchboard config found. Pass --config <path>, set SWITCHBOARD_CONFIG, create ./switchboard.toml, or place config at $XDG_CONFIG_HOME/switchboard/config.toml or $HOME/.config/switchboard/config.toml".into(),
            )
        })
}

fn existing_file(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
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
        operation_id: Option<&'a switchboard_core::OperationId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval_reason: Option<&'a str>,
    },
    Executed {
        tool: &'a ToolName,
        namespace: &'a NamespaceId,
        summary: &'a str,
        fields: &'a BTreeMap<String, JsonValue>,
        refs: &'a [ToolRef],
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<&'a switchboard_core::OperationId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effect: Option<&'a OperationEffect>,
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
            operation_id: plan.operation_id.as_ref(),
            approval_reason: plan.approval_reason.as_deref(),
        }
    }

    fn from_output(output: &'a ToolOutput) -> Self {
        Self::Executed {
            tool: &output.tool,
            namespace: &output.namespace,
            summary: &output.summary,
            fields: &output.fields,
            refs: &output.refs,
            operation_id: output.operation_id.as_ref(),
            effect: output.effect.as_ref(),
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
    use std::{
        collections::BTreeMap,
        env, fs,
        path::{Path, PathBuf},
        sync::MutexGuard,
    };

    use clap::Parser;
    use serde_json::json;
    use switchboard_core::{
        AggregateReadRequest, DispatchOutcome, ExecutionMode, NamespaceId, OperationOutcome, OperationRequest,
        ToolName, ToolOutput, ToolRequest,
    };

    use crate::{
        run, select_config_path,
        test_support::{lock_env, TempScript},
        Cli, ConfigPathCandidates,
    };

    const BASIC_CONFIG_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/basic.toml"
    ));
    const GOOGLE_CALENDAR_AGENDA_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-calendar-agenda.json"
    ));
    const GOOGLE_GMAIL_TRIAGE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-gmail-triage.json"
    ));
    const GOOGLE_GMAIL_READ_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-gmail-read.json"
    ));
    const GITHUB_NOTIFICATIONS_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-notifications.json"
    ));
    const GITHUB_PR_SEARCH_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-pull-request-search.json"
    ));
    const GITHUB_PR_READ_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-pull-request-read.json"
    ));
    const GITHUB_ISSUE_READ_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-issue-read.json"
    ));
    const GOOGLE_SCRIPT_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/scripts/gws-test.sh"
    ));
    const GITHUB_SCRIPT_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/scripts/gh-test.sh"
    ));
    const GOOGLE_PERSONAL_OAUTH_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/secrets/google-personal-oauth.json"
    ));

    #[test]
    fn configured_namespaces_match_current_examples() {
        let environment = TestEnvironment::new();
        let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
        let namespaces = switchboard.list_namespaces();
        let ids = namespaces
            .into_iter()
            .map(|namespace| namespace.id.to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec![
                "github.personal",
                "github.personal_token",
                "google.personal",
                "google.work"
            ]
        );
    }

    #[test]
    fn write_requests_default_to_planning_until_approval_exists() {
        let environment = TestEnvironment::new();
        let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
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
                assert!(plan.operation_id.is_some());
            }
            DispatchOutcome::Executed(_) => {
                panic!("write requests should not execute yet");
            }
        }
    }

    #[test]
    fn unwired_read_requests_execute_into_stub_results() {
        let environment = TestEnvironment::new();
        let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
        let request = ToolRequest::new(
            "google.drive.search",
            "google.work",
            ExecutionMode::Auto,
            BTreeMap::from([("query".into(), "from:finance".into())]),
        )
        .expect("request should parse");

        let outcome = switchboard.dispatch(request).expect("dispatch should succeed");
        match outcome {
            DispatchOutcome::Executed(output) => {
                assert_eq!(
                    output.fields.get("status").and_then(serde_json::Value::as_str),
                    Some("stub")
                );
            }
            DispatchOutcome::Planned(_) => {
                panic!("read requests should execute by default");
            }
        }
    }

    #[test]
    fn flat_tool_invocation_still_parses_with_clap() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
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
        assert_eq!(value["fields"]["count"], 2);
        assert_eq!(value["refs"][0]["kind"], "message");
    }

    #[test]
    fn gmail_read_returns_stable_message_and_thread_refs() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "google.mail.read",
            "--ns",
            "google.work",
            "--message-id",
            "1960abc456work",
            "--json",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("command should run");
        let value: serde_json::Value = serde_json::from_str(&output).expect("output should be valid json");

        assert_eq!(value["status"], "executed");
        assert_eq!(value["tool"], "google.mail.read");
        assert_eq!(value["fields"]["message"]["gmail_message_id"], "1960abc456work");
        assert_eq!(value["fields"]["message"]["gmail_thread_id"], "1960thread123work");
        assert_eq!(value["refs"][0]["kind"], "message");
        assert_eq!(value["refs"][1]["kind"], "thread");
    }

    #[test]
    fn github_pull_request_search_returns_stable_refs() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "github.pull_request.search",
            "--ns",
            "github.personal",
            "--query",
            "is:open review-requested:@me",
            "--repo",
            "openai/codex",
            "--limit",
            "10",
            "--json",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("command should run");
        let value: serde_json::Value = serde_json::from_str(&output).expect("output should be valid json");

        assert_eq!(value["status"], "executed");
        assert_eq!(value["tool"], "github.pull_request.search");
        assert_eq!(value["fields"]["count"], 2);
        assert_eq!(value["refs"][0]["kind"], "pull_request");
        assert_eq!(value["refs"][0]["parent_id"], "openai/codex");
    }

    #[test]
    fn github_issue_read_returns_stable_issue_refs() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "github.issue.read",
            "--ns",
            "github.personal",
            "--repo",
            "openai/codex",
            "--number",
            "77",
            "--json",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("command should run");
        let value: serde_json::Value = serde_json::from_str(&output).expect("output should be valid json");

        assert_eq!(value["status"], "executed");
        assert_eq!(value["tool"], "github.issue.read");
        assert_eq!(value["fields"]["issue"]["number"], 77);
        assert_eq!(value["refs"][0]["kind"], "issue");
        assert_eq!(value["refs"][0]["parent_id"], "openai/codex");
    }

    #[test]
    fn repeated_namespace_flags_become_aggregate_reads() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
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
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
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
    fn valueless_flags_flow_through_to_real_cli_backends() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "google.calendar.list",
            "--ns",
            "google.work",
            "--today",
            "--json",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("command should run");
        let value: serde_json::Value = serde_json::from_str(&output).expect("output should be valid json");

        assert_eq!(value["status"], "executed");
        assert_eq!(value["tool"], "google.calendar.list");
        assert_eq!(value["namespace"], "google.work");
        assert_eq!(value["fields"]["count"], 2);
        assert!(
            environment
                .gws_capture_contents()
                .contains("ARGV=calendar +agenda --format json --today"),
            "expected --today to reach gws"
        );
    }

    #[test]
    fn aggregate_read_operations_can_fan_out_across_calendar_namespaces() {
        let environment = TestEnvironment::new();
        let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
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
        let environment = TestEnvironment::new();
        let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
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

    #[test]
    fn human_output_renders_structured_fields_without_flattening_them_into_nonsense() {
        let output = ToolOutput::new(
            ToolName::new("google.calendar.list").expect("tool should build"),
            NamespaceId::new("google.work").expect("namespace should build"),
            "agenda summary",
        )
        .with_field("status", "ok")
        .with_value_field(
            "events",
            json!([
                {
                    "id": "event-123",
                    "title": "Vet visit",
                }
            ]),
        );

        let rendered = super::render_output_human(&output);

        assert!(rendered.contains("- status: ok"));
        assert!(rendered.contains("- events:"));
        assert!(rendered.contains("\"title\": \"Vet visit\""));
    }

    #[test]
    fn human_output_renders_refs_without_hiding_them_in_json_soup() {
        let output = ToolOutput::new(
            ToolName::new("google.mail.read").expect("tool should build"),
            NamespaceId::new("google.work").expect("namespace should build"),
            "read gmail message",
        )
        .with_ref(
            switchboard_core::ToolRef::new(
                switchboard_core::ProviderKind::GoogleWorkspace,
                NamespaceId::new("google.work").expect("namespace should build"),
                switchboard_core::ToolRefKind::Message,
                "1960abc456work",
            )
            .expect("tool ref should build")
            .with_label("Booking details for June stay")
            .expect("tool ref label should build"),
        );

        let rendered = super::render_output_human(&output);

        assert!(rendered.contains("Refs:"));
        assert!(rendered.contains("google:message id=1960abc456work"));
    }

    #[test]
    fn config_path_selection_prefers_explicit_paths_first() {
        let selected = select_config_path(ConfigPathCandidates {
            explicit: Some(PathBuf::from("/explicit.toml")),
            env: Some(PathBuf::from("/env.toml")),
            cwd: Some(PathBuf::from("/cwd.toml")),
            ..ConfigPathCandidates::default()
        })
        .expect("an explicit path should win");

        assert_eq!(selected, PathBuf::from("/explicit.toml"));
    }

    #[test]
    fn config_path_selection_falls_back_in_documented_order() {
        let selected = select_config_path(ConfigPathCandidates {
            cwd: Some(PathBuf::from("/cwd.toml")),
            home: Some(PathBuf::from("/home.toml")),
            ..ConfigPathCandidates::default()
        })
        .expect("a discovered config should be selected");

        assert_eq!(selected, PathBuf::from("/cwd.toml"));
    }

    struct TestEnvironment {
        _env_guard: MutexGuard<'static, ()>,
        directory: PathBuf,
        path: PathBuf,
        _gws_script: TempScript,
        _gh_script: TempScript,
    }

    impl TestEnvironment {
        fn new() -> Self {
            let env_guard = lock_env();
            let directory = test_fixture_directory();
            fs::create_dir_all(&directory).expect("temp dir should be created");
            env::set_var("GOOGLE_WORKSPACE_CLI_CLIENT_ID", "google-work-client-id");
            env::set_var("GOOGLE_WORKSPACE_CLI_CLIENT_SECRET", "google-work-client-secret");
            let gws_script = TempScript::new("gws-test", &render_google_script_template());
            let gh_script = TempScript::new("gh-test", &render_github_script_template());
            env::set_var("SWITCHBOARD_GWS_BIN", gws_script.path());
            env::set_var("SWITCHBOARD_GH_BIN", gh_script.path());
            let oauth_path = directory.join("google-personal-oauth.json");
            fs::write(&oauth_path, GOOGLE_PERSONAL_OAUTH_JSON).expect("oauth fixture should be written");
            let path = directory.join("switchboard.toml");
            let contents = BASIC_CONFIG_TEMPLATE.replace(
                "__GOOGLE_PERSONAL_OAUTH_PATH__",
                oauth_path.to_str().expect("oauth fixture path should be valid utf-8"),
            );
            fs::write(&path, contents).expect("config should be written");

            Self {
                _env_guard: env_guard,
                directory,
                path,
                _gws_script: gws_script,
                _gh_script: gh_script,
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn path_string(&self) -> String {
            self.path.to_str().expect("temp path should be valid utf-8").to_owned()
        }

        fn gws_capture_contents(&self) -> String {
            self._gws_script.capture_contents()
        }
    }

    impl Drop for TestEnvironment {
        fn drop(&mut self) {
            env::remove_var("GOOGLE_WORKSPACE_CLI_CLIENT_ID");
            env::remove_var("GOOGLE_WORKSPACE_CLI_CLIENT_SECRET");
            env::remove_var("SWITCHBOARD_GWS_BIN");
            env::remove_var("SWITCHBOARD_GH_BIN");
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn test_fixture_directory() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        env::temp_dir().join(format!("switchboard-test-{}-{stamp}", std::process::id()))
    }

    fn render_google_script_template() -> String {
        GOOGLE_SCRIPT_TEMPLATE
            .replace("__AGENDA_FIXTURE__", GOOGLE_CALENDAR_AGENDA_FIXTURE)
            .replace("__GMAIL_TRIAGE_FIXTURE__", GOOGLE_GMAIL_TRIAGE_FIXTURE)
            .replace("__GMAIL_READ_FIXTURE__", GOOGLE_GMAIL_READ_FIXTURE)
    }

    fn render_github_script_template() -> String {
        GITHUB_SCRIPT_TEMPLATE
            .replace("__NOTIFICATIONS_FIXTURE__", GITHUB_NOTIFICATIONS_FIXTURE)
            .replace("__PR_SEARCH_FIXTURE__", GITHUB_PR_SEARCH_FIXTURE)
            .replace("__PR_READ_FIXTURE__", GITHUB_PR_READ_FIXTURE)
            .replace("__ISSUE_READ_FIXTURE__", GITHUB_ISSUE_READ_FIXTURE)
    }
}
