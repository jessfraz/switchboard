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
    NamespaceStore, OperationOutcome, OperationRequest, ResolvedNamespace, Result, SecretResolver, SecretStore,
    Switchboard, ToolName, ToolOutput, ToolRequest,
};
use switchboard_providers::default_registry;
use switchboard_store::{DefaultPolicyEngine, LocalSecretResolver, MemoryAuditSink, SwitchboardConfig};

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
    let adapters = default_registry();

    Switchboard::new(namespaces, auth, secrets, secret_resolver, policy, audit, adapters)
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
        approval_reason: Option<&'a str>,
    },
    Executed {
        tool: &'a ToolName,
        namespace: &'a NamespaceId,
        summary: &'a str,
        fields: &'a BTreeMap<String, JsonValue>,
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
    use std::{
        collections::BTreeMap,
        env, fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use clap::Parser;
    use serde_json::json;
    use switchboard_core::{
        AggregateReadRequest, DispatchOutcome, ExecutionMode, NamespaceId, OperationOutcome, OperationRequest,
        ToolName, ToolOutput, ToolRequest,
    };

    use crate::{run, select_config_path, Cli, ConfigPathCandidates};

    const BASIC_CONFIG_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/basic.toml"
    ));
    const GOOGLE_PERSONAL_OAUTH_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/secrets/google-personal-oauth.json"
    ));
    static TEST_ENV_COUNTER: AtomicU64 = AtomicU64::new(0);

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
            }
            DispatchOutcome::Executed(_) => {
                panic!("write requests should not execute yet");
            }
        }
    }

    #[test]
    fn read_requests_execute_into_stub_results() {
        let environment = TestEnvironment::new();
        let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
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
        directory: PathBuf,
        path: PathBuf,
    }

    impl TestEnvironment {
        fn new() -> Self {
            let directory = env::temp_dir().join(format!(
                "switchboard-test-{}-{}-{}",
                process::id(),
                TEST_ENV_COUNTER.fetch_add(1, Ordering::Relaxed),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time should be after unix epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&directory).expect("temp dir should be created");
            env::set_var("GOOGLE_WORKSPACE_CLI_CLIENT_ID", "google-work-client-id");
            env::set_var("GOOGLE_WORKSPACE_CLI_CLIENT_SECRET", "google-work-client-secret");
            let oauth_path = directory.join("google-personal-oauth.json");
            fs::write(&oauth_path, GOOGLE_PERSONAL_OAUTH_JSON).expect("oauth fixture should be written");
            let path = directory.join("switchboard.toml");
            let contents = BASIC_CONFIG_TEMPLATE.replace(
                "__GOOGLE_PERSONAL_OAUTH_PATH__",
                oauth_path.to_str().expect("oauth fixture path should be valid utf-8"),
            );
            fs::write(&path, contents).expect("config should be written");

            Self { directory, path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn path_string(&self) -> String {
            self.path.to_str().expect("temp path should be valid utf-8").to_owned()
        }
    }

    impl Drop for TestEnvironment {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}
