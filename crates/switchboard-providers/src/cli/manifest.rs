use std::collections::HashMap;

use serde::Deserialize;
use switchboard_core::{
    Error, ExecutionTarget, PlannedAction, ProviderKind, ResolvedNamespace, Result, ToolDescriptor,
    ToolExecutionSupport, ToolKind, ToolName, ToolOutput, ToolRequest, ToolSurface, ToolUndoSupport,
};

use crate::cli::command::{CliBinarySpec, CliCapabilityProbe, CliCommandSpec, CliExecutableSpec, CliResponse};

type CliSummarizeFn = fn(&ResolvedNamespace, &ToolRequest) -> Result<String>;
type CliBuildArgsFn = fn(&PlannedAction) -> Result<Vec<String>>;
type CliDecodeFn = fn(&ExecutionTarget, &PlannedAction, CliResponse) -> Result<ToolOutput>;

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
    pub(crate) fn from_embedded(manifest_json: &'static str, handlers: &'static [CliCommandHandler]) -> Result<Self> {
        let manifest: CliManifest = serde_json::from_str(manifest_json)
            .map_err(|error| Error::Config(format!("invalid CLI manifest: {error}")))?;

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

        let mut tools = Vec::with_capacity(manifest.commands.len());
        let mut commands = Vec::with_capacity(manifest.commands.len());
        for command in manifest.commands {
            let tool_name = ToolName::new(&command.name)?;
            let provider = tool_name.provider()?;
            if provider != manifest.provider {
                return Err(Error::Config(format!(
                    "tool {} belongs to provider {}, but manifest provider is {}",
                    command.name, provider, manifest.provider
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
                    let binary = binaries.get(&binary).cloned().ok_or_else(|| {
                        Error::Config(format!("tool {} references unknown binary {binary}", descriptor.name))
                    })?;
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
                        build_args,
                        decode,
                    })
                }
                CliManifestExecution::PlanningOnly => None,
            };

            tools.push(descriptor.clone());
            commands.push(CliCommandSpec {
                descriptor,
                summarize: handler.summarize,
                executable,
            });
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
