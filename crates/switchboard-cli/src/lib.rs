use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::Value as JsonValue;
use switchboard_core::{
    AggregateReadOutcome, AggregateReadRequest, ApprovalState, AuthStore, BackendKind, DispatchOutcome, ExecutionMode,
    NamespaceId, NamespaceStore, OperationEffect, OperationId, OperationOutcome, OperationRequest, ProviderKind,
    RegisteredTool, ResolvedNamespace, SecretResolver, SecretStore, StoredOperation, Switchboard, SwitchboardServices,
    ToolArgument, ToolKind, ToolName, ToolOutput, ToolRef, ToolRequest,
};
use switchboard_providers::default_registry;
use switchboard_store::{
    resolve_operation_store_path, LocalSecretResolver, MemoryAuditSink, SqliteOperationStore, SwitchboardConfig,
};

#[cfg(test)]
mod test_support;

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  switchboard ns list\n",
    "  switchboard tools list\n",
    "  switchboard tools describe google.cli.write\n",
    "  switchboard op list\n",
    "  switchboard github.notifications.list --ns github.personal --json\n",
    "  switchboard google.mail.search --ns google.work --query 'from:finance newer_than:7d'\n",
    "  switchboard google.calendar.list --ns google.work --ns google.personal --json\n",
    "  switchboard google.cli.read --ns google.work --json -- calendar +agenda --format json --today\n",
    "  switchboard github.pull_request.comment --ns github.personal --repo owner/repo --number 123 --body 'needs tests' --draft\n",
    "  switchboard op approve op_1234abcd --actor codex --note 'ship it'\n",
    "  switchboard op apply op_1234abcd --json\n"
);

fn load_switchboard(config_path: Option<&Path>) -> Result<Switchboard> {
    let config_path = resolve_config_path(config_path)?;
    let config = SwitchboardConfig::from_file(&config_path).context("failed to load switchboard config")?;
    let policy = config.policy_engine();
    let (namespaces, auth, secrets) = config.into_stores();
    let operations = SqliteOperationStore::open(resolve_operation_store_path(&config_path))
        .context("failed to open operation store")?;

    Ok(build_switchboard(
        Arc::new(namespaces),
        Arc::new(auth),
        Arc::new(secrets),
        Arc::new(LocalSecretResolver::default()),
        Arc::new(policy),
        Arc::new(operations),
    ))
}

fn build_switchboard(
    namespaces: Arc<dyn NamespaceStore>,
    auth: Arc<dyn AuthStore>,
    secrets: Arc<dyn SecretStore>,
    secret_resolver: Arc<dyn SecretResolver>,
    policy: Arc<dyn switchboard_core::PolicyEngine>,
    operations: Arc<dyn switchboard_core::OperationStore>,
) -> Switchboard {
    let audit = Arc::new(MemoryAuditSink::default());
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
        Err(error) => {
            if json_requested {
                println!("{}", render_json_error(&error.to_string()));
            } else {
                eprintln!("{error:#}");
            }

            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<String> {
    let config_path = cli.config.clone();
    let json_requested = cli.json_requested();
    let switchboard = load_switchboard(config_path.as_deref());
    let switchboard = match switchboard {
        Ok(switchboard) => switchboard,
        Err(error) if json_requested => return Err(error),
        Err(error) => return Err(error.context("failed to initialize switchboard")),
    };

    match cli.command.into_runtime_command()? {
        CommandKind::NamespaceList => {
            let namespaces = switchboard.list_namespaces();
            if json_requested {
                render_json(&NamespaceListResponse { namespaces }, true)
            } else {
                Ok(render_namespaces_human(&namespaces))
            }
        }
        CommandKind::ToolCatalog(command) => run_tool_catalog_command(&switchboard, command),
        CommandKind::Operation(request) => {
            let outcome = switchboard.execute_operation(request)?;

            if json_requested {
                render_json_operation(&outcome)
            } else {
                Ok(render_operation_human(&outcome))
            }
        }
        CommandKind::StoredOperation(command) => run_stored_operation_command(&switchboard, command),
    }
}

fn run_tool_catalog_command(switchboard: &Switchboard, command: ToolCatalogRuntimeCommand) -> Result<String> {
    match command {
        ToolCatalogRuntimeCommand::List { json } => {
            let tools = switchboard.list_tools()?;
            if json {
                render_json(
                    &ToolCatalogListResponse {
                        status: "ok",
                        tools: tools.iter().map(ToolCatalogEntry::from).collect(),
                    },
                    true,
                )
            } else {
                Ok(render_tools_human(&tools))
            }
        }
        ToolCatalogRuntimeCommand::Describe { tool, json } => {
            let descriptor = switchboard
                .describe_tool(&tool)
                .context("failed to resolve tool metadata")?
                .ok_or_else(|| anyhow!("unknown tool: {tool}"))?;
            let namespaces = switchboard
                .list_namespaces()
                .into_iter()
                .filter(|namespace| namespace.provider == descriptor.provider)
                .collect::<Vec<_>>();
            let detail = ToolCatalogDetail::new(&descriptor, &namespaces);

            if json {
                render_json(
                    &ToolCatalogDetailResponse {
                        status: "ok",
                        tool: detail,
                    },
                    true,
                )
            } else {
                Ok(render_tool_detail_human(&detail))
            }
        }
    }
}

fn run_stored_operation_command(switchboard: &Switchboard, command: StoredOperationCommand) -> Result<String> {
    match command {
        StoredOperationCommand::List { json } => {
            let operations = switchboard.list_operations();
            if json {
                render_json(
                    &StoredOperationListResponse {
                        status: "ok",
                        operations: &operations,
                    },
                    true,
                )
            } else {
                Ok(render_operations_human(&operations))
            }
        }
        StoredOperationCommand::Show { id, json } => {
            let operation = switchboard
                .get_operation(&id)
                .ok_or_else(|| anyhow!("unknown operation id: {id}"))?;
            if json {
                render_json(
                    &StoredOperationResponse {
                        status: "ok",
                        operation: &operation,
                    },
                    true,
                )
            } else {
                Ok(render_stored_operation_human(&operation))
            }
        }
        StoredOperationCommand::Approve { id, actor, note, json } => {
            let operation = switchboard.approve_operation(&id, &actor, note.as_deref())?;
            if json {
                render_json(
                    &StoredOperationResponse {
                        status: "approved",
                        operation: &operation,
                    },
                    true,
                )
            } else {
                Ok(render_stored_operation_human(&operation))
            }
        }
        StoredOperationCommand::Reject { id, actor, note, json } => {
            let operation = switchboard.reject_operation(&id, &actor, note.as_deref())?;
            if json {
                render_json(
                    &StoredOperationResponse {
                        status: "rejected",
                        operation: &operation,
                    },
                    true,
                )
            } else {
                Ok(render_stored_operation_human(&operation))
            }
        }
        StoredOperationCommand::Apply { id, json } => {
            let output = switchboard.apply_operation(&id)?;
            if json {
                render_json_dispatch(&DispatchOutcome::Executed(output))
            } else {
                Ok(render_output_human(&output))
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
            Commands::Tools(tools) => tools.json_requested(),
            Commands::Op(operation) => operation.json_requested(),
            Commands::Tool(tokens) => contains_json_os_tokens(tokens),
        }
    }
}

#[derive(Debug, Subcommand)]
enum Commands {
    Ns(NamespaceCommand),
    Tools(ToolCatalogCommand),
    Op(OperationCommand),
    #[command(external_subcommand)]
    Tool(Vec<OsString>),
}

impl Commands {
    fn into_runtime_command(self) -> Result<CommandKind> {
        match self {
            Self::Ns(namespace) => Ok(namespace.into_runtime_command()),
            Self::Tools(tools) => tools.into_runtime_command(),
            Self::Op(operation) => operation.into_runtime_command(),
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

#[derive(Debug, Args)]
struct ToolCatalogCommand {
    #[command(subcommand)]
    command: ToolCatalogSubcommand,
}

impl ToolCatalogCommand {
    fn json_requested(&self) -> bool {
        match &self.command {
            ToolCatalogSubcommand::List(arguments) => arguments.json,
            ToolCatalogSubcommand::Describe(arguments) => arguments.json,
        }
    }

    fn into_runtime_command(self) -> Result<CommandKind> {
        let command = match self.command {
            ToolCatalogSubcommand::List(arguments) => ToolCatalogRuntimeCommand::List { json: arguments.json },
            ToolCatalogSubcommand::Describe(arguments) => ToolCatalogRuntimeCommand::Describe {
                tool: ToolName::new(arguments.tool)?,
                json: arguments.json,
            },
        };

        Ok(CommandKind::ToolCatalog(command))
    }
}

#[derive(Debug, Subcommand)]
enum ToolCatalogSubcommand {
    List(ToolCatalogListArgs),
    Describe(ToolCatalogDescribeArgs),
}

#[derive(Debug, Args)]
struct ToolCatalogListArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ToolCatalogDescribeArgs {
    tool: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct OperationCommand {
    #[command(subcommand)]
    command: OperationSubcommand,
}

impl OperationCommand {
    fn json_requested(&self) -> bool {
        match &self.command {
            OperationSubcommand::List(arguments) => arguments.json,
            OperationSubcommand::Show(arguments) => arguments.json,
            OperationSubcommand::Approve(arguments) => arguments.json,
            OperationSubcommand::Reject(arguments) => arguments.json,
            OperationSubcommand::Apply(arguments) => arguments.json,
        }
    }

    fn into_runtime_command(self) -> Result<CommandKind> {
        let command = match self.command {
            OperationSubcommand::List(arguments) => StoredOperationCommand::List { json: arguments.json },
            OperationSubcommand::Show(arguments) => StoredOperationCommand::Show {
                id: OperationId::new(arguments.id)?,
                json: arguments.json,
            },
            OperationSubcommand::Approve(arguments) => StoredOperationCommand::Approve {
                id: OperationId::new(arguments.id)?,
                actor: arguments.actor.unwrap_or_else(default_actor),
                note: arguments.note,
                json: arguments.json,
            },
            OperationSubcommand::Reject(arguments) => StoredOperationCommand::Reject {
                id: OperationId::new(arguments.id)?,
                actor: arguments.actor.unwrap_or_else(default_actor),
                note: arguments.note,
                json: arguments.json,
            },
            OperationSubcommand::Apply(arguments) => StoredOperationCommand::Apply {
                id: OperationId::new(arguments.id)?,
                json: arguments.json,
            },
        };

        Ok(CommandKind::StoredOperation(command))
    }
}

#[derive(Debug, Subcommand)]
enum OperationSubcommand {
    List(OperationListArgs),
    Show(OperationShowArgs),
    Approve(OperationDecisionArgs),
    Reject(OperationDecisionArgs),
    Apply(OperationApplyArgs),
}

#[derive(Debug, Args)]
struct OperationListArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct OperationShowArgs {
    id: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct OperationDecisionArgs {
    id: String,
    #[arg(long)]
    actor: Option<String>,
    #[arg(long)]
    note: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct OperationApplyArgs {
    id: String,
    #[arg(long)]
    json: bool,
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
    ToolCatalog(ToolCatalogRuntimeCommand),
    Operation(OperationRequest),
    StoredOperation(StoredOperationCommand),
}

#[derive(Debug)]
enum ToolCatalogRuntimeCommand {
    List { json: bool },
    Describe { tool: ToolName, json: bool },
}

#[derive(Debug)]
enum StoredOperationCommand {
    List {
        json: bool,
    },
    Show {
        id: OperationId,
        json: bool,
    },
    Approve {
        id: OperationId,
        actor: String,
        note: Option<String>,
        json: bool,
    },
    Reject {
        id: OperationId,
        actor: String,
        note: Option<String>,
        json: bool,
    },
    Apply {
        id: OperationId,
        json: bool,
    },
}

fn default_actor() -> String {
    env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "switchboard-user".to_owned())
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

fn parse_external_tool_invocation(tokens: Vec<OsString>) -> Result<OperationRequest> {
    let mut positionals = tokens.into_iter().map(os_string_to_string).collect::<Vec<_>>();
    let tool = positionals
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("missing tool name"))?;
    positionals.remove(0);
    let mut namespaces = Vec::new();
    let mut arguments = Vec::new();
    let mut mode = ExecutionMode::Auto;
    let mut passthrough_tail = None;
    let mut index = 0;

    while index < positionals.len() {
        let current = &positionals[index];
        match current.as_str() {
            "--" => {
                passthrough_tail = Some(positionals[(index + 1)..].to_vec());
                break;
            }
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
                let value = positionals
                    .get(index + 1)
                    .ok_or_else(|| anyhow!("missing value for --ns"))?;
                namespaces.push(value.clone());
                index += 2;
            }
            "--argv" | "--argv-json" => {
                let value = positionals
                    .get(index + 1)
                    .ok_or_else(|| anyhow!("missing value for {current}"))?;
                arguments.push(ToolArgument::option(current.trim_start_matches("--"), value.clone())?);
                index += 2;
            }
            _ if current.starts_with("--") && current.contains('=') => {
                let (name, value) =
                    split_inline_argument(current).ok_or_else(|| anyhow!("invalid argument syntax: {current}"))?;
                arguments.push(ToolArgument::option(name, value)?);
                index += 1;
            }
            _ if current.starts_with("--") => {
                let key = current.trim_start_matches("--");
                let next = positionals.get(index + 1);
                if next.is_none() || next.is_some_and(|value| value.starts_with("--")) {
                    arguments.push(ToolArgument::flag(key)?);
                    index += 1;
                } else {
                    let value = next.expect("checked above");
                    arguments.push(ToolArgument::option(key, value.clone())?);
                    index += 2;
                }
            }
            _ => {
                bail!("unexpected argument: {current}");
            }
        }
    }

    if let Some(argv) = passthrough_tail {
        if !is_raw_cli_tool_name(&tool) {
            bail!("-- passthrough is only supported for raw *.cli.read and *.cli.write tools");
        }
        if argv.is_empty() {
            bail!("raw CLI passthrough requires at least one argv token after --");
        }
        if arguments
            .iter()
            .any(|argument| matches!(argument.name(), "argv" | "argv-json"))
        {
            bail!("use either raw -- passthrough or --argv/--argv-json, not both");
        }

        for value in argv {
            arguments.push(ToolArgument::option("argv", value)?);
        }
    }

    if namespaces.is_empty() {
        bail!("tool commands require at least one --ns <namespace>");
    }

    if namespaces.len() == 1 {
        let request = ToolRequest::new(tool, namespaces.remove(0), mode, arguments.clone())?;

        return Ok(OperationRequest::single(request));
    }

    let request = AggregateReadRequest::new(tool, namespaces, mode, arguments)?;

    Ok(OperationRequest::aggregate_read(request))
}

fn split_inline_argument(argument: &str) -> Option<(&str, &str)> {
    let trimmed = argument.strip_prefix("--")?;
    let (name, value) = trimmed.split_once('=')?;
    Some((name, value))
}

fn is_raw_cli_tool_name(tool: &str) -> bool {
    tool.ends_with(".cli.read") || tool.ends_with(".cli.write")
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

fn render_tools_human(tools: &[RegisteredTool]) -> String {
    let mut output = String::from("Tools\n");
    for tool in tools {
        let mut qualifiers = vec![
            tool.provider.to_string(),
            render_tool_kind(tool.kind).to_owned(),
            tool.backend.to_string(),
        ];
        if is_raw_cli_tool_name(tool.name.as_str()) {
            qualifiers.push("raw".into());
        }
        output.push_str(&format!(
            "- {} [{}] {}\n",
            tool.name,
            qualifiers.join(", "),
            tool.summary
        ));
    }

    output
}

fn render_tool_detail_human(detail: &ToolCatalogDetail) -> String {
    let mut output = String::new();
    output.push_str(&format!("Tool: {}\n", detail.name));
    output.push_str(&format!("Provider: {}\n", detail.provider));
    output.push_str(&format!("Kind: {}\n", render_tool_kind(detail.kind)));
    output.push_str(&format!("Backend: {}\n", detail.backend));
    output.push_str(&format!("Summary: {}\n", detail.summary));
    output.push_str(&format!(
        "Aggregate reads: {}\n",
        if detail.aggregate_read_supported {
            "supported"
        } else {
            "not supported"
        }
    ));
    output.push_str("Namespaces:\n");
    if detail.available_namespaces.is_empty() {
        output.push_str("- none configured\n");
    } else {
        for namespace in &detail.available_namespaces {
            output.push_str(&format!("- {namespace}\n"));
        }
    }

    if !detail.notes.is_empty() {
        output.push_str("Notes:\n");
        for note in &detail.notes {
            output.push_str(&format!("- {note}\n"));
        }
    }

    if !detail.examples.is_empty() {
        output.push_str("Examples:\n");
        for example in &detail.examples {
            output.push_str(&format!("- {example}\n"));
        }
    }

    output
}

fn render_operations_human(operations: &[StoredOperation]) -> String {
    let mut output = String::from("Operations\n");
    for operation in operations {
        output.push_str(&format!(
            "- {} {} {} approval={} status={}\n",
            operation.id,
            operation.tool,
            operation.namespace,
            render_approval_state(operation.approval.state),
            render_operation_status(operation.status)
        ));
    }

    output
}

fn render_stored_operation_human(operation: &StoredOperation) -> String {
    let mut output = String::new();
    output.push_str(&format!("Operation: {}\n", operation.id));
    output.push_str(&format!("Tool: {}\n", operation.tool));
    output.push_str(&format!("Namespace: {}\n", operation.namespace));
    output.push_str(&format!("Summary: {}\n", operation.summary));
    output.push_str(&format!("Backend: {}\n", operation.backend));
    output.push_str(&format!("Status: {}\n", render_operation_status(operation.status)));
    output.push_str(&format!(
        "Approval: {}\n",
        render_approval_state(operation.approval.state)
    ));
    if let Some(reason) = &operation.approval_reason {
        output.push_str(&format!("Approval reason: {reason}\n"));
    }
    if let Some(actor) = &operation.approval.actor {
        output.push_str(&format!("Approval actor: {actor}\n"));
    }
    if let Some(note) = &operation.approval.note {
        output.push_str(&format!("Approval note: {note}\n"));
    }
    if let Some(reason) = &operation.failure_reason {
        output.push_str(&format!("Failure: {reason}\n"));
    }
    if let Some(effect) = &operation.effect {
        output.push_str(&render_effect_human(effect));
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

fn render_tool_kind(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Write => "write",
    }
}

fn curated_tool_examples(tool: &RegisteredTool, namespace: &str) -> Vec<String> {
    let mode_flag = match tool.kind {
        ToolKind::Read => "--json",
        ToolKind::Write => "--draft",
    };
    vec![format!("switchboard {} --ns {namespace} {mode_flag} ...", tool.name)]
}

fn raw_tool_examples(tool: &RegisteredTool, namespace: &str) -> Vec<String> {
    match (tool.provider.clone(), tool.kind) {
        (ProviderKind::GoogleWorkspace, ToolKind::Read) => vec![
            format!(
                "switchboard {} --ns {namespace} --json -- calendar +agenda --format json --today",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"gmail\",\"users\",\"messages\",\"list\",\"--query\",\"from:finance\",\"--format\",\"json\"]' --json",
                tool.name
            ),
        ],
        (ProviderKind::GoogleWorkspace, ToolKind::Write) => vec![
            format!(
                "switchboard {} --ns {namespace} --draft -- gmail users drafts create --params '{{\"userId\":\"me\"}}' --json '{{\"message\":{{\"raw\":\"SGVsbG8=\"}}}}' --format json",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"calendar\",\"events\",\"insert\",\"--summary\",\"Vet visit\",\"--start\",\"2026-04-01T09:00:00-07:00\",\"--end\",\"2026-04-01T10:00:00-07:00\",\"--format\",\"json\"]' --apply --json",
                tool.name
            ),
        ],
        (ProviderKind::GitHub, ToolKind::Read) => vec![
            format!(
                "switchboard {} --ns {namespace} --json -- repo view owner/repo --json name,visibility,defaultBranchRef",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"search\",\"prs\",\"--repo\",\"owner/repo\",\"--state\",\"open\",\"--json\",\"number,title\"]' --json",
                tool.name
            ),
        ],
        (ProviderKind::GitHub, ToolKind::Write) => vec![
            format!(
                "switchboard {} --ns {namespace} --draft -- pr comment 123 --body 'needs tests'",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"issue\",\"edit\",\"77\",\"--add-label\",\"triage\"]' --apply --json",
                tool.name
            ),
        ],
        (_, _) => vec![format!("switchboard {} --ns {namespace} -- ...", tool.name)],
    }
}

fn render_approval_state(state: ApprovalState) -> &'static str {
    match state {
        ApprovalState::NotRequired => "not_required",
        ApprovalState::Pending => "pending",
        ApprovalState::Approved => "approved",
        ApprovalState::Rejected => "rejected",
    }
}

fn render_operation_status(status: switchboard_core::OperationStatus) -> &'static str {
    match status {
        switchboard_core::OperationStatus::Planned => "planned",
        switchboard_core::OperationStatus::Applied => "applied",
        switchboard_core::OperationStatus::Failed => "failed",
        switchboard_core::OperationStatus::Compensated => "compensated",
    }
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

fn render_json_operation(outcome: &OperationOutcome) -> Result<String> {
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

fn render_json_dispatch(outcome: &DispatchOutcome) -> Result<String> {
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

fn render_json<T>(value: &T, _json: bool) -> Result<String>
where
    T: Serialize,
{
    serde_json::to_string_pretty(value).context("failed to serialize JSON output")
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
            anyhow!(
                "no switchboard config found. Pass --config <path>, set SWITCHBOARD_CONFIG, create ./switchboard.toml, or place config at $XDG_CONFIG_HOME/switchboard/config.toml or $HOME/.config/switchboard/config.toml"
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
struct ToolCatalogListResponse {
    status: &'static str,
    tools: Vec<ToolCatalogEntry>,
}

#[derive(Serialize)]
struct ToolCatalogDetailResponse {
    status: &'static str,
    tool: ToolCatalogDetail,
}

#[derive(Serialize)]
struct ToolCatalogEntry {
    name: ToolName,
    provider: ProviderKind,
    kind: ToolKind,
    backend: BackendKind,
    summary: String,
    raw: bool,
}

impl From<&RegisteredTool> for ToolCatalogEntry {
    fn from(tool: &RegisteredTool) -> Self {
        Self {
            name: tool.name.clone(),
            provider: tool.provider.clone(),
            kind: tool.kind,
            backend: tool.backend,
            summary: tool.summary.to_owned(),
            raw: is_raw_cli_tool_name(tool.name.as_str()),
        }
    }
}

#[derive(Serialize)]
struct ToolCatalogDetail {
    name: ToolName,
    provider: ProviderKind,
    kind: ToolKind,
    backend: BackendKind,
    summary: String,
    raw: bool,
    aggregate_read_supported: bool,
    available_namespaces: Vec<NamespaceId>,
    notes: Vec<String>,
    examples: Vec<String>,
}

impl ToolCatalogDetail {
    fn new(tool: &RegisteredTool, namespaces: &[ResolvedNamespace]) -> Self {
        let available_namespaces = namespaces
            .iter()
            .map(|namespace| namespace.id.clone())
            .collect::<Vec<_>>();
        let raw = is_raw_cli_tool_name(tool.name.as_str());
        let example_namespace = namespaces
            .first()
            .map(|namespace| namespace.id.to_string())
            .unwrap_or_else(|| format!("{}.default", tool.provider));
        let mut notes = vec![
            "policy, auth isolation, and audit still apply".to_owned(),
            "repeat --ns for aggregate reads, writes stay single-namespace".to_owned(),
        ];
        let examples = if raw {
            notes.push(
                "put switchboard flags before --, everything after -- is forwarded to the provider CLI unchanged"
                    .to_owned(),
            );
            notes.push("for scripted calls, --argv-json accepts one JSON array of argv tokens".to_owned());
            raw_tool_examples(tool, &example_namespace)
        } else {
            curated_tool_examples(tool, &example_namespace)
        };

        Self {
            name: tool.name.clone(),
            provider: tool.provider.clone(),
            kind: tool.kind,
            backend: tool.backend,
            summary: tool.summary.to_owned(),
            raw,
            aggregate_read_supported: tool.kind == ToolKind::Read,
            available_namespaces,
            notes,
            examples,
        }
    }
}

#[derive(Serialize)]
struct StoredOperationListResponse<'a> {
    status: &'static str,
    operations: &'a [StoredOperation],
}

#[derive(Serialize)]
struct StoredOperationResponse<'a> {
    status: &'static str,
    operation: &'a StoredOperation,
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
        env,
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        sync::MutexGuard,
    };

    use clap::Parser;
    use serde::{de::DeserializeOwned, Deserialize};
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
    const ALLOW_WRITES_CONFIG_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/allow-writes.toml"
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
    const GOOGLE_GMAIL_DRAFT_CREATE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-gmail-draft-create.json"
    ));
    const GOOGLE_CALENDAR_CREATE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-calendar-create.json"
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
    const GITHUB_REPO_VIEW_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-repo-view.json"
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

    #[derive(Debug, Deserialize)]
    struct JsonPlannedResponse {
        status: String,
        tool: String,
        namespace: String,
        summary: String,
        backend: String,
        approval_required: bool,
        operation_id: Option<String>,
        approval_reason: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct JsonExecutedResponse<TFields> {
        status: String,
        tool: String,
        namespace: String,
        summary: String,
        fields: TFields,
        #[serde(default)]
        refs: Vec<JsonToolRef>,
        operation_id: Option<String>,
        effect: Option<JsonOperationEffect>,
    }

    #[derive(Debug, Deserialize)]
    struct JsonAggregateReadResponse<TFields> {
        status: String,
        tool: String,
        namespaces: Vec<String>,
        results: Vec<JsonAggregateReadResult<TFields>>,
    }

    #[derive(Debug, Deserialize)]
    struct JsonAggregateReadResult<TFields> {
        namespace: String,
        outcome: JsonExecutedResponse<TFields>,
    }

    #[derive(Debug, Deserialize)]
    struct JsonStoredOperationEnvelope {
        status: String,
        operation: JsonStoredOperation,
    }

    #[derive(Debug, Deserialize)]
    struct JsonStoredOperation {
        approval: JsonOperationApproval,
    }

    #[derive(Debug, Deserialize)]
    struct JsonOperationApproval {
        state: String,
    }

    #[derive(Debug, Deserialize)]
    struct JsonToolCatalogList {
        status: String,
        tools: Vec<JsonToolCatalogEntry>,
    }

    #[derive(Debug, Deserialize)]
    struct JsonToolCatalogEntry {
        name: String,
        raw: bool,
    }

    #[derive(Debug, Deserialize)]
    struct JsonToolRef {
        kind: String,
        #[serde(default)]
        parent_id: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct JsonOperationEffect {
        undoable: bool,
    }

    #[derive(Debug, Deserialize)]
    struct RawGoogleReadFields {
        response: GoogleAgendaPayload,
    }

    #[derive(Debug, Deserialize)]
    struct GoogleAgendaPayload {
        count: usize,
        events: Vec<GoogleAgendaEvent>,
    }

    #[derive(Debug, Deserialize)]
    struct GoogleAgendaEvent {
        summary: String,
    }

    #[derive(Debug, Deserialize)]
    struct RawGoogleWriteFields {
        response: GoogleDraftPayload,
    }

    #[derive(Debug, Deserialize)]
    struct GoogleDraftPayload {
        id: String,
    }

    #[derive(Debug, Deserialize)]
    struct CountFields {
        count: usize,
    }

    #[derive(Debug, Deserialize)]
    struct GoogleMailSearchFields {
        count: usize,
    }

    #[derive(Debug, Deserialize)]
    struct GoogleMailReadFields {
        message: GoogleMailReadMessage,
    }

    #[derive(Debug, Deserialize)]
    struct GoogleMailReadMessage {
        gmail_message_id: String,
        gmail_thread_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct GitHubPullRequestSearchFields {
        count: usize,
    }

    #[derive(Debug, Deserialize)]
    struct GitHubIssueReadFields {
        issue: GitHubIssuePayload,
    }

    #[derive(Debug, Deserialize)]
    struct GitHubIssuePayload {
        number: usize,
    }

    fn parse_json<T: DeserializeOwned>(output: &str) -> T {
        serde_json::from_str(output).expect("output should be valid json")
    }

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
    fn operation_approval_flow_can_approve_and_apply_planned_writes() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();

        let draft = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "google.calendar.create",
            "--ns",
            "google.work",
            "--title",
            "Budget review",
            "--start",
            "2026-03-30T15:00:00-07:00",
            "--end",
            "2026-03-30T15:30:00-07:00",
            "--draft",
            "--json",
        ])
        .expect("cli should parse");

        let draft_output = run(draft).expect("draft should succeed");
        let draft_value: JsonPlannedResponse = parse_json(&draft_output);
        assert_eq!(draft_value.status, "planned");
        assert_eq!(draft_value.tool, "google.calendar.create");
        assert_eq!(draft_value.namespace, "google.work");
        assert_eq!(draft_value.backend, "cli");
        assert!(draft_value.summary.contains("calendar"));
        assert!(draft_value.approval_required);
        assert!(draft_value.approval_reason.is_some());
        let operation_id = draft_value.operation_id.expect("draft operation id should exist");

        let approve = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "op",
            "approve",
            &operation_id,
            "--actor",
            "codex",
            "--note",
            "looks good",
            "--json",
        ])
        .expect("approve cli should parse");

        let approve_output = run(approve).expect("approve should succeed");
        let approve_value: JsonStoredOperationEnvelope = parse_json(&approve_output);
        assert_eq!(approve_value.status, "approved");
        assert_eq!(approve_value.operation.approval.state, "approved");

        let apply = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "op",
            "apply",
            &operation_id,
            "--json",
        ])
        .expect("apply cli should parse");

        let apply_output = run(apply).expect("apply should succeed");
        let apply_value: JsonExecutedResponse<BTreeMap<String, serde_json::Value>> = parse_json(&apply_output);
        assert_eq!(apply_value.status, "executed");
        assert_eq!(apply_value.tool, "google.calendar.create");
        assert_eq!(apply_value.namespace, "google.work");
        assert!(apply_value.summary.contains("calendar"));
        assert_eq!(apply_value.operation_id.as_deref(), Some(operation_id.as_str()));
        assert_eq!(apply_value.effect.as_ref().map(|effect| effect.undoable), Some(true));
        assert_eq!(apply_value.refs[0].kind, "event");
        assert!(
            environment
                .gws_capture_contents()
                .contains("ARGV=calendar +insert --format json --summary Budget review"),
            "expected calendar insert command to run after approval"
        );
    }

    #[test]
    fn allow_policy_executes_writes_without_manual_approval() {
        let environment = TestEnvironment::with_config_template(ALLOW_WRITES_CONFIG_TEMPLATE);
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "google.calendar.create",
            "--ns",
            "google.work",
            "--title",
            "Budget review",
            "--start",
            "2026-03-30T15:00:00-07:00",
            "--end",
            "2026-03-30T15:30:00-07:00",
            "--apply",
            "--json",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("write should execute");
        let value: JsonExecutedResponse<BTreeMap<String, serde_json::Value>> = parse_json(&output);

        assert_eq!(value.status, "executed");
        assert_eq!(value.tool, "google.calendar.create");
        assert_eq!(value.namespace, "google.work");
        assert!(value.summary.contains("calendar"));
        assert_eq!(value.effect.as_ref().map(|effect| effect.undoable), Some(true));
        assert_eq!(value.refs[0].kind, "event");
    }

    #[test]
    fn tools_list_includes_curated_and_raw_tools() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from(["switchboard", "--config", &config_path, "tools", "list", "--json"])
            .expect("cli should parse");

        let output = run(cli).expect("tools list should succeed");
        let value: JsonToolCatalogList = parse_json(&output);

        assert_eq!(value.status, "ok");
        assert!(
            value
                .tools
                .iter()
                .any(|tool| tool.name == "google.mail.search" && !tool.raw),
            "expected curated google tool in catalog"
        );
        assert!(
            value
                .tools
                .iter()
                .any(|tool| tool.name == "google.cli.write" && tool.raw),
            "expected raw google write tool in catalog"
        );
        assert!(
            value
                .tools
                .iter()
                .any(|tool| tool.name == "github.cli.read" && tool.raw),
            "expected raw github read tool in catalog"
        );
    }

    #[test]
    fn tools_describe_raw_google_tool_explains_passthrough_usage() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "tools",
            "describe",
            "google.cli.write",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("tools describe should succeed");

        assert!(output.contains("Tool: google.cli.write"));
        assert!(output.contains("policy, auth isolation, and audit still apply"));
        assert!(output.contains("put switchboard flags before --"));
        assert!(output.contains("switchboard google.cli.write --ns google."));
    }

    #[test]
    fn raw_google_cli_write_accepts_argv_json_and_executes() {
        let environment = TestEnvironment::with_config_template(ALLOW_WRITES_CONFIG_TEMPLATE);
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "google.cli.write",
            "--ns",
            "google.work",
            "--argv-json",
            r#"["gmail","users","drafts","create","--params","{\"userId\":\"me\"}","--json","{\"message\":{\"raw\":\"SGVsbG8sIHdvcmxkIQ==\"}}","--format","json"]"#,
            "--apply",
            "--json",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("raw cli write should execute");
        let value: JsonExecutedResponse<RawGoogleWriteFields> = parse_json(&output);

        assert_eq!(value.status, "executed");
        assert_eq!(value.tool, "google.cli.write");
        assert_eq!(value.namespace, "google.work");
        assert!(value.summary.contains("gws"));
        assert_eq!(value.fields.response.id, "draft-1960work");
        assert!(
            environment
                .gws_capture_contents()
                .contains("ARGV=gmail users drafts create --params {\"userId\":\"me\"}"),
            "expected raw gws argv to reach the provider backend"
        );
    }

    #[test]
    fn raw_google_cli_read_supports_double_dash_passthrough() {
        let environment = TestEnvironment::new();
        let config_path = environment.path_string();
        let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "google.cli.read",
            "--ns",
            "google.work",
            "--json",
            "--",
            "calendar",
            "+agenda",
            "--format",
            "json",
            "--today",
        ])
        .expect("cli should parse");

        let output = run(cli).expect("raw cli read should execute");
        let value: JsonExecutedResponse<RawGoogleReadFields> = parse_json(&output);

        assert_eq!(value.status, "executed");
        assert_eq!(value.tool, "google.cli.read");
        assert_eq!(value.namespace, "google.work");
        assert!(value.summary.contains("gws"));
        assert_eq!(value.fields.response.count, 2);
        assert_eq!(value.fields.response.events[0].summary, "Standup");
        assert!(
            environment
                .gws_capture_contents()
                .contains("ARGV=calendar +agenda --format json --today"),
            "expected raw passthrough argv to reach gws unchanged"
        );
    }

    #[test]
    fn repeated_argv_accepts_dash_prefixed_passthrough_tokens() {
        let request = super::parse_external_tool_invocation(
            [
                "google.cli.read",
                "--ns",
                "google.work",
                "--argv",
                "calendar",
                "--argv",
                "+agenda",
                "--argv",
                "--format",
                "--argv",
                "json",
                "--argv",
                "--today",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
        )
        .expect("external tool invocation should parse");

        match request {
            OperationRequest::Single(request) => {
                let argv = request.args.values("argv").collect::<Vec<_>>();
                assert_eq!(argv, vec!["calendar", "+agenda", "--format", "json", "--today"]);
            }
            OperationRequest::AggregateRead(_) => panic!("expected single operation request"),
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
        let value: JsonExecutedResponse<GoogleMailSearchFields> = parse_json(&output);

        assert_eq!(value.status, "executed");
        assert_eq!(value.tool, "google.mail.search");
        assert_eq!(value.namespace, "google.work");
        assert_eq!(value.fields.count, 2);
        assert_eq!(value.refs[0].kind, "message");
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
        let value: JsonExecutedResponse<GoogleMailReadFields> = parse_json(&output);

        assert_eq!(value.status, "executed");
        assert_eq!(value.tool, "google.mail.read");
        assert_eq!(value.namespace, "google.work");
        assert_eq!(value.fields.message.gmail_message_id, "1960abc456work");
        assert_eq!(value.fields.message.gmail_thread_id, "1960thread123work");
        assert_eq!(value.refs[0].kind, "message");
        assert_eq!(value.refs[1].kind, "thread");
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
        let value: JsonExecutedResponse<GitHubPullRequestSearchFields> = parse_json(&output);

        assert_eq!(value.status, "executed");
        assert_eq!(value.tool, "github.pull_request.search");
        assert_eq!(value.namespace, "github.personal");
        assert!(value.summary.contains("GitHub"));
        assert_eq!(value.fields.count, 2);
        assert_eq!(value.refs[0].kind, "pull_request");
        assert_eq!(value.refs[0].parent_id.as_deref(), Some("openai/codex"));
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
        let value: JsonExecutedResponse<GitHubIssueReadFields> = parse_json(&output);

        assert_eq!(value.status, "executed");
        assert_eq!(value.tool, "github.issue.read");
        assert_eq!(value.namespace, "github.personal");
        assert!(value.summary.contains("GitHub"));
        assert_eq!(value.fields.issue.number, 77);
        assert_eq!(value.refs[0].kind, "issue");
        assert_eq!(value.refs[0].parent_id.as_deref(), Some("openai/codex"));
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
        let value: JsonAggregateReadResponse<CountFields> = parse_json(&output);

        assert_eq!(value.status, "aggregate_read");
        assert_eq!(value.tool, "google.calendar.list");
        assert_eq!(value.namespaces, vec!["google.work", "google.personal"]);
        assert_eq!(value.results[0].namespace, "google.work");
        assert_eq!(value.results[1].namespace, "google.personal");
        assert_eq!(value.results[0].outcome.status, "executed");
        assert_eq!(value.results[1].outcome.status, "executed");
        assert_eq!(value.results[0].outcome.fields.count, 2);
        assert_eq!(value.results[1].outcome.fields.count, 2);
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
        let error = error
            .downcast::<switchboard_core::Error>()
            .expect("expected typed switchboard error");
        match error {
            switchboard_core::Error::AggregateReadRequiresReadTool(tool) => {
                assert_eq!(tool.to_string(), "google.calendar.create");
            }
            other => panic!("unexpected error: {other:?}"),
        }
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
        let value: JsonExecutedResponse<CountFields> = parse_json(&output);

        assert_eq!(value.status, "executed");
        assert_eq!(value.tool, "google.calendar.list");
        assert_eq!(value.namespace, "google.work");
        assert!(value.summary.contains("calendar"));
        assert_eq!(value.fields.count, 2);
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

        assert_eq!(
            error,
            switchboard_core::Error::AggregateReadRequiresReadTool(
                ToolName::new("google.calendar.create").expect("tool should build")
            )
        );
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
            Self::with_config_template(BASIC_CONFIG_TEMPLATE)
        }

        fn with_config_template(config_template: &str) -> Self {
            let env_guard = lock_env();
            let directory = test_fixture_directory();
            fs::create_dir_all(&directory).expect("temp dir should be created");
            env::set_var("GOOGLE_WORKSPACE_CLI_CLIENT_ID", "google-work-client-id");
            env::set_var("GOOGLE_WORKSPACE_CLI_CLIENT_SECRET", "google-work-client-secret");
            let gws_script = TempScript::new("gws-test", &render_google_script_template());
            let gh_script = TempScript::new("gh-test", &render_github_script_template());
            env::set_var("SWITCHBOARD_GWS_BIN", gws_script.path());
            env::set_var("SWITCHBOARD_GH_BIN", gh_script.path());
            env::set_var("SWITCHBOARD_STATE_DIR", directory.join("state"));
            let oauth_path = directory.join("google-personal-oauth.json");
            fs::write(&oauth_path, GOOGLE_PERSONAL_OAUTH_JSON).expect("oauth fixture should be written");
            let path = directory.join("switchboard.toml");
            let contents = config_template.replace(
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
            env::remove_var("SWITCHBOARD_STATE_DIR");
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
            .replace("__GMAIL_DRAFT_CREATE_FIXTURE__", GOOGLE_GMAIL_DRAFT_CREATE_FIXTURE)
            .replace("__CALENDAR_CREATE_FIXTURE__", GOOGLE_CALENDAR_CREATE_FIXTURE)
    }

    fn render_github_script_template() -> String {
        GITHUB_SCRIPT_TEMPLATE
            .replace("__NOTIFICATIONS_FIXTURE__", GITHUB_NOTIFICATIONS_FIXTURE)
            .replace("__PR_SEARCH_FIXTURE__", GITHUB_PR_SEARCH_FIXTURE)
            .replace("__PR_READ_FIXTURE__", GITHUB_PR_READ_FIXTURE)
            .replace("__ISSUE_READ_FIXTURE__", GITHUB_ISSUE_READ_FIXTURE)
            .replace("__REPO_VIEW_FIXTURE__", GITHUB_REPO_VIEW_FIXTURE)
    }
}
