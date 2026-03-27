use std::{env, ffi::OsString, path::PathBuf};

use anyhow::{anyhow, bail, Result};
use clap::{Args, Parser, Subcommand};
use switchboard_core::{
    AggregateReadRequest, AuditEventId, ExecutionMode, OperationId, OperationRequest, ToolArgument, ToolName,
    ToolRequest,
};

const AFTER_HELP: &str = concat!(
    "Raw CLI Coverage:\n",
    "  Curated tools are the nice typed layer, not the limit.\n",
    "  By default, any discovered provider CLI command can run through these raw surfaces:\n",
    "    <provider>.cli.read   for read-only commands\n",
    "    <provider>.cli.write  for write commands\n",
    "  Namespace auth, policy, approval, audit, and undo metadata still apply.\n",
    "  Put switchboard flags before --, then pass native CLI argv after --.\n",
    "\n",
    "Examples:\n",
    "  switchboard ns list\n",
    "  switchboard tools list\n",
    "  switchboard tools describe google.cli.write\n",
    "  switchboard tools describe github.cli.read\n",
    "  switchboard audit list\n",
    "  switchboard op list\n",
    "  switchboard op list --pending\n",
    "  switchboard github.notifications.list --ns github.personal --json\n",
    "  switchboard google.mail.search --ns google.work --query 'from:finance newer_than:7d'\n",
    "  switchboard google.calendar.list --ns google.work --ns google.personal --json\n",
    "  switchboard google.cli.read --ns google.work --json -- calendar +agenda --format json --today\n",
    "  switchboard github.cli.write --ns github.personal -- --repo owner/repo issue comment 123 --body 'needs tests'\n",
    "  switchboard github.pull_request.comment --ns github.personal --repo owner/repo --number 123 --body 'needs tests' --draft\n",
    "  switchboard op approve op_1234abcd --actor codex --note 'ship it'\n",
    "  switchboard op approve op_1234abcd --actor codex --apply --json\n",
    "  switchboard op undo op_1234abcd --apply --json\n",
    "  switchboard op apply op_1234abcd --json\n"
);

#[derive(Debug, Parser)]
#[command(
    name = "switchboard",
    version,
    about = "Rust-first local automation plane",
    disable_help_subcommand = true,
    after_help = AFTER_HELP
)]
pub(crate) struct Cli {
    #[arg(long, global = true, env = "SWITCHBOARD_CONFIG", value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Commands,
}

impl Cli {
    pub(crate) fn json_requested(&self) -> bool {
        match &self.command {
            Commands::Ns(namespace) => namespace.json_requested(),
            Commands::Tools(tools) => tools.json_requested(),
            Commands::Audit(audit) => audit.json_requested(),
            Commands::Op(operation) => operation.json_requested(),
            Commands::Tool(tokens) => contains_json_os_tokens(tokens),
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    Ns(NamespaceCommand),
    Tools(ToolCatalogCommand),
    Audit(AuditCommand),
    Op(OperationCommand),
    #[command(external_subcommand)]
    Tool(Vec<OsString>),
}

impl Commands {
    pub(crate) fn into_runtime_command(self) -> Result<CommandKind> {
        match self {
            Self::Ns(namespace) => Ok(namespace.into_runtime_command()),
            Self::Tools(tools) => tools.into_runtime_command(),
            Self::Audit(audit) => audit.into_runtime_command(),
            Self::Op(operation) => operation.into_runtime_command(),
            Self::Tool(tokens) => parse_external_tool_invocation(tokens).map(CommandKind::Operation),
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct NamespaceCommand {
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
pub(crate) struct ToolCatalogCommand {
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
pub(crate) struct AuditCommand {
    #[command(subcommand)]
    command: AuditSubcommand,
}

impl AuditCommand {
    fn json_requested(&self) -> bool {
        match &self.command {
            AuditSubcommand::List(arguments) => arguments.json,
            AuditSubcommand::Show(arguments) => arguments.json,
        }
    }

    fn into_runtime_command(self) -> Result<CommandKind> {
        let command = match self.command {
            AuditSubcommand::List(arguments) => AuditRuntimeCommand::List {
                operation_id: arguments.operation_id.map(OperationId::new).transpose()?,
                json: arguments.json,
            },
            AuditSubcommand::Show(arguments) => AuditRuntimeCommand::Show {
                selector: parse_audit_selector(&arguments.selector)?,
                json: arguments.json,
            },
        };

        Ok(CommandKind::Audit(command))
    }
}

#[derive(Debug, Subcommand)]
enum AuditSubcommand {
    List(AuditListArgs),
    Show(AuditShowArgs),
}

#[derive(Debug, Args)]
struct AuditListArgs {
    #[arg(long = "operation-id")]
    operation_id: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AuditShowArgs {
    selector: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct OperationCommand {
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
            OperationSubcommand::Undo(arguments) => arguments.json,
        }
    }

    fn into_runtime_command(self) -> Result<CommandKind> {
        let command = match self.command {
            OperationSubcommand::List(arguments) => StoredOperationCommand::List {
                pending_only: arguments.pending,
                json: arguments.json,
            },
            OperationSubcommand::Show(arguments) => StoredOperationCommand::Show {
                id: OperationId::new(arguments.id)?,
                json: arguments.json,
            },
            OperationSubcommand::Approve(arguments) => StoredOperationCommand::Approve {
                id: OperationId::new(arguments.id)?,
                actor: arguments.actor.unwrap_or_else(default_actor),
                note: arguments.note,
                apply: arguments.apply,
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
            OperationSubcommand::Undo(arguments) => StoredOperationCommand::Undo {
                id: OperationId::new(arguments.id)?,
                mode: operation_write_mode(arguments.apply),
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
    Approve(OperationApproveArgs),
    Reject(OperationRejectArgs),
    Apply(OperationApplyArgs),
    Undo(OperationUndoArgs),
}

#[derive(Debug, Args)]
struct OperationListArgs {
    #[arg(long)]
    pending: bool,
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
struct OperationApproveArgs {
    id: String,
    #[arg(long)]
    actor: Option<String>,
    #[arg(long)]
    note: Option<String>,
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct OperationRejectArgs {
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

#[derive(Debug, Args)]
struct OperationUndoArgs {
    id: String,
    #[arg(long)]
    apply: bool,
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
pub(crate) enum CommandKind {
    NamespaceList,
    ToolCatalog(ToolCatalogRuntimeCommand),
    Audit(AuditRuntimeCommand),
    Operation(OperationRequest),
    StoredOperation(StoredOperationCommand),
}

#[derive(Debug)]
pub(crate) enum ToolCatalogRuntimeCommand {
    List { json: bool },
    Describe { tool: ToolName, json: bool },
}

#[derive(Debug)]
pub(crate) enum AuditRuntimeCommand {
    List {
        operation_id: Option<OperationId>,
        json: bool,
    },
    Show {
        selector: AuditSelector,
        json: bool,
    },
}

#[derive(Debug)]
pub(crate) enum AuditSelector {
    EventId(AuditEventId),
    OperationId(OperationId),
}

#[derive(Debug)]
pub(crate) enum StoredOperationCommand {
    List {
        pending_only: bool,
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
        apply: bool,
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
    Undo {
        id: OperationId,
        mode: ExecutionMode,
        json: bool,
    },
}

pub(crate) fn default_actor() -> String {
    env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "switchboard-user".to_owned())
}

fn operation_write_mode(apply: bool) -> ExecutionMode {
    if apply {
        ExecutionMode::Apply
    } else {
        ExecutionMode::Draft
    }
}

fn parse_audit_selector(value: &str) -> Result<AuditSelector> {
    if value.starts_with("audit_") {
        return Ok(AuditSelector::EventId(AuditEventId::new(value.to_owned())?));
    }
    if value.starts_with("op_") {
        return Ok(AuditSelector::OperationId(OperationId::new(value.to_owned())?));
    }

    bail!("audit selector must be an audit_* event id or op_* operation id");
}

pub(crate) fn parse_external_tool_invocation(tokens: Vec<OsString>) -> Result<OperationRequest> {
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
            bail!("-- passthrough is only supported for raw *.cli.* tools");
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
    tool.split('.').nth(1) == Some("cli")
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
