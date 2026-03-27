use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use switchboard_core::{
    Error, ProviderKind, Result, ToolDescriptor, ToolExecutionSupport, ToolKind, ToolName, ToolSurface, ToolUndoSupport,
};

use crate::{
    cli::command::{
        CliArgsStrategy, CliBinarySpec, CliBuildArgsFn, CliCapabilityProbe, CliCommandSpec, CliDecodeFn,
        CliDecodeStrategy, CliExecutableSpec, CliSummarizeFn, CliSummarizeStrategy,
    },
    inventory::{CliInventory, CliInventoryCommand, CliInventoryNodeKind, CliOperationKind},
};

#[derive(Clone, Copy)]
pub(crate) struct CliCommandHandler {
    pub id: &'static str,
    pub summarize: CliSummarizeFn,
    pub build_args: Option<CliBuildArgsFn>,
    pub decode: Option<CliDecodeFn>,
}

pub(crate) struct CliProviderCatalog {
    tools: Vec<ToolDescriptor>,
    commands: Vec<CliCommandSpec>,
}

impl CliProviderCatalog {
    pub(crate) fn from_embedded(
        manifest_json: &'static str,
        inventory: &CliInventory,
        handlers: &'static [CliCommandHandler],
    ) -> Result<Self> {
        let manifest: CliManifest = serde_json::from_str(manifest_json)
            .map_err(|error| Error::Config(format!("invalid CLI manifest: {error}")))?;

        if manifest.provider != inventory.provider {
            return Err(Error::Config(format!(
                "manifest provider {} does not match embedded inventory provider {}",
                manifest.provider, inventory.provider
            )));
        }

        let binaries = manifest
            .binaries
            .into_iter()
            .map(|binary| {
                (
                    binary.id.clone(),
                    CliBinarySpec {
                        program: binary.program,
                        env_override: binary.env_override,
                        version_args: binary.version_args,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let capabilities = manifest
            .capabilities
            .into_iter()
            .map(|capability| {
                (
                    capability.id.clone(),
                    CliCapabilityProbe {
                        id: capability.id,
                        args: capability.args,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let handlers = handlers
            .iter()
            .map(|handler| (handler.id, handler))
            .collect::<HashMap<_, _>>();

        let mut tools = Vec::new();
        let mut commands = Vec::new();
        let mut registered_names = HashSet::new();

        for command in manifest.commands {
            let spec = build_manifest_command(command, manifest.provider.clone(), &binaries, &capabilities, &handlers)?;
            registered_names.insert(spec.descriptor.name.clone());
            tools.push(spec.descriptor.clone());
            commands.push(spec);
        }

        let default_binary = binaries.values().next().cloned().ok_or_else(|| {
            Error::Config(format!(
                "provider {} manifest does not define any binaries",
                manifest.provider
            ))
        })?;

        for inventory_command in inventory
            .commands
            .iter()
            .filter(|command| command.node_kind == CliInventoryNodeKind::Operation)
        {
            let spec = build_inventory_raw_command(inventory, inventory_command, &default_binary)?;
            if registered_names.insert(spec.descriptor.name.clone()) {
                tools.push(spec.descriptor.clone());
                commands.push(spec);
            }
        }

        Ok(Self { tools, commands })
    }

    pub(crate) fn tools(&self) -> &[ToolDescriptor] {
        &self.tools
    }

    pub(crate) fn find_command(&self, tool: &str) -> Option<&CliCommandSpec> {
        self.commands.iter().find(|command| command.name() == tool)
    }
}

fn build_manifest_command(
    command: CliManifestCommand,
    provider: ProviderKind,
    binaries: &HashMap<String, CliBinarySpec>,
    capabilities: &HashMap<String, CliCapabilityProbe>,
    handlers: &HashMap<&'static str, &'static CliCommandHandler>,
) -> Result<CliCommandSpec> {
    let tool_name = ToolName::new(&command.name)?;
    let tool_provider = tool_name.provider()?;
    if tool_provider != provider {
        return Err(Error::Config(format!(
            "tool {} belongs to provider {}, but manifest provider is {}",
            command.name, tool_provider, provider
        )));
    }

    let handler = handlers.get(command.handler.as_str()).ok_or_else(|| {
        Error::Config(format!(
            "manifest command {} references unknown handler {}",
            command.name, command.handler
        ))
    })?;
    let aggregate_read_supported = command
        .aggregate_read_supported
        .unwrap_or(command.kind == ToolKind::Read);
    if aggregate_read_supported && command.kind != ToolKind::Read {
        return Err(Error::Config(format!(
            "tool {} cannot support aggregate reads because it is a write tool",
            command.name
        )));
    }

    let execution_support = match command.execution {
        CliManifestExecution::Executable { .. } => ToolExecutionSupport::Executable,
        CliManifestExecution::PlanningOnly => ToolExecutionSupport::PlanningOnly,
    };

    let descriptor = ToolDescriptor::new(
        command.name,
        command.kind,
        command.summary,
        switchboard_core::BackendKind::Cli,
    )?
    .with_surface(command.surface)
    .with_aggregate_read_supported(aggregate_read_supported)
    .with_execution_support(execution_support)
    .with_undo_support(command.undo_support);

    let executable = match command.execution {
        CliManifestExecution::Executable { binary, capability } => {
            let binary = binaries
                .get(&binary)
                .cloned()
                .ok_or_else(|| Error::Config(format!("tool {} references unknown binary {binary}", descriptor.name)))?;
            let capability = capabilities.get(&capability).cloned().ok_or_else(|| {
                Error::Config(format!(
                    "tool {} references unknown capability {capability}",
                    descriptor.name
                ))
            })?;
            let build_args = handler.build_args.ok_or_else(|| {
                Error::Config(format!(
                    "handler {} for tool {} is missing build_args",
                    handler.id, descriptor.name
                ))
            })?;
            let decode = handler.decode.ok_or_else(|| {
                Error::Config(format!(
                    "handler {} for tool {} is missing decode",
                    handler.id, descriptor.name
                ))
            })?;

            Some(CliExecutableSpec {
                binary,
                capability,
                args: CliArgsStrategy::Handler(build_args),
                decode: CliDecodeStrategy::Handler(decode),
            })
        }
        CliManifestExecution::PlanningOnly => None,
    };

    Ok(CliCommandSpec {
        descriptor,
        summarize: CliSummarizeStrategy::Handler(handler.summarize),
        executable,
    })
}

fn build_inventory_raw_command(
    inventory: &CliInventory,
    command: &CliInventoryCommand,
    binary: &CliBinarySpec,
) -> Result<CliCommandSpec> {
    let tool_name = inventory_tool_name(&inventory.provider, &command.path);
    let kind = inventory_tool_kind(command.operation_kind);
    let summary = format!("Run raw {} command {}", inventory.program, command.command);
    let descriptor = ToolDescriptor::new(tool_name, kind, summary, switchboard_core::BackendKind::Cli)?
        .with_surface(ToolSurface::Raw)
        .with_aggregate_read_supported(kind == ToolKind::Read)
        .with_execution_support(ToolExecutionSupport::Executable)
        .with_undo_support(ToolUndoSupport::None);
    let capability = CliCapabilityProbe {
        id: format!("inventory:{}", command.command),
        args: command.help_args.clone(),
    };

    Ok(CliCommandSpec {
        descriptor,
        summarize: CliSummarizeStrategy::RawInventory {
            program: inventory.program.clone(),
            prefix: command.path.clone(),
        },
        executable: Some(CliExecutableSpec {
            binary: binary.clone(),
            capability,
            args: CliArgsStrategy::RawInventory {
                prefix: command.path.clone(),
            },
            decode: CliDecodeStrategy::RawInventory {
                program: inventory.program.clone(),
                prefix: command.path.clone(),
            },
        }),
    })
}

fn inventory_tool_name(provider: &ProviderKind, path: &[String]) -> String {
    if path.is_empty() {
        return format!("{provider}.cli.command");
    }

    format!("{provider}.cli.{}", path.join("."))
}

fn inventory_tool_kind(operation_kind: CliOperationKind) -> ToolKind {
    match operation_kind {
        CliOperationKind::Read => ToolKind::Read,
        CliOperationKind::Write | CliOperationKind::Unknown => ToolKind::Write,
    }
}

#[derive(Deserialize)]
struct CliManifest {
    provider: ProviderKind,
    binaries: Vec<CliManifestBinary>,
    capabilities: Vec<CliManifestCapability>,
    commands: Vec<CliManifestCommand>,
}

#[derive(Deserialize)]
struct CliManifestBinary {
    id: String,
    program: String,
    #[serde(default)]
    env_override: Option<String>,
    version_args: Vec<String>,
}

#[derive(Deserialize)]
struct CliManifestCapability {
    id: String,
    args: Vec<String>,
}

#[derive(Deserialize)]
struct CliManifestCommand {
    name: String,
    kind: ToolKind,
    summary: String,
    handler: String,
    execution: CliManifestExecution,
    #[serde(default = "default_surface")]
    surface: ToolSurface,
    #[serde(default)]
    aggregate_read_supported: Option<bool>,
    #[serde(default = "default_undo_support")]
    undo_support: ToolUndoSupport,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CliManifestExecution {
    Executable { binary: String, capability: String },
    PlanningOnly,
}

fn default_surface() -> ToolSurface {
    ToolSurface::Curated
}

fn default_undo_support() -> ToolUndoSupport {
    ToolUndoSupport::None
}
