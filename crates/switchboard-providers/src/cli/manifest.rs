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
    cli::declarative::{
        CliArgsSegment, CliArgsTemplate, CliJsonEffectSpec, CliJsonFieldMapping, CliJsonProjection,
        CliJsonProjectionShape, CliJsonRefsSpec, CliProjectionTemplate, CliSummaryTemplate,
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

    let execution = match command.execution {
        CliManifestExecution::Executable { binary, capability } => Some((
            binaries
                .get(&binary)
                .cloned()
                .ok_or_else(|| Error::Config(format!("tool {} references unknown binary {binary}", descriptor.name)))?,
            capabilities.get(&capability).cloned().ok_or_else(|| {
                Error::Config(format!(
                    "tool {} references unknown capability {capability}",
                    descriptor.name
                ))
            })?,
        )),
        CliManifestExecution::PlanningOnly => None,
    };

    let (summarize, defaults) = build_manifest_summarize_strategy(
        &descriptor.name,
        command.strategy,
        execution.as_ref().map(|(binary, _)| binary.program.as_str()),
        handlers,
    )?;
    let executable = match execution {
        Some((binary, capability)) => Some(CliExecutableSpec {
            binary: binary.clone(),
            capability: capability.clone(),
            args: build_manifest_args_strategy(&descriptor.name, command.args, &defaults, handlers)?,
            decode: build_manifest_decode_strategy(
                &descriptor.name,
                binary.program.as_str(),
                command.decode,
                &defaults,
                handlers,
            )?,
        }),
        None => {
            if command.args.is_some() {
                return Err(Error::Config(format!(
                    "tool {} defines args but is planning_only",
                    descriptor.name
                )));
            }
            if command.decode.is_some() {
                return Err(Error::Config(format!(
                    "tool {} defines decode but is planning_only",
                    descriptor.name
                )));
            }
            None
        }
    };

    Ok(CliCommandSpec {
        descriptor,
        summarize,
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
    strategy: CliManifestStrategy,
    execution: CliManifestExecution,
    #[serde(default)]
    args: Option<CliManifestArgsStrategy>,
    #[serde(default)]
    decode: Option<CliManifestDecodeStrategy>,
    #[serde(default = "default_surface")]
    surface: ToolSurface,
    #[serde(default)]
    aggregate_read_supported: Option<bool>,
    #[serde(default = "default_undo_support")]
    undo_support: ToolUndoSupport,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CliManifestStrategy {
    Handler {
        id: String,
    },
    SummaryTemplate {
        template: String,
    },
    RawPassthrough {
        #[serde(default)]
        prefix: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CliManifestExecution {
    Executable { binary: String, capability: String },
    PlanningOnly,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CliManifestArgsStrategy {
    Handler {
        id: String,
    },
    Template {
        segments: Vec<CliManifestArgsSegment>,
    },
    RawPassthrough {
        #[serde(default)]
        prefix: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CliManifestArgsSegment {
    Literal {
        value: String,
    },
    RequiredPositional {
        aliases: Vec<String>,
    },
    OptionalPositional {
        aliases: Vec<String>,
    },
    Option {
        flag: String,
        aliases: Vec<String>,
        #[serde(default)]
        repeated: bool,
        #[serde(default)]
        required: bool,
    },
    Flag {
        flag: String,
        aliases: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CliManifestDecodeStrategy {
    Handler {
        id: String,
    },
    JsonProjection(Box<CliManifestJsonProjection>),
    RawPassthrough {
        #[serde(default)]
        prefix: Vec<String>,
    },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CliManifestJsonProjectionShape {
    Object,
    Array,
}

#[derive(Deserialize)]
struct CliManifestJsonField {
    name: String,
    #[serde(default)]
    pointer: Option<String>,
    #[serde(default)]
    item_pointer: Option<String>,
    #[serde(default)]
    arg: Option<Vec<String>>,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    literal: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct CliManifestJsonProjection {
    response_field: String,
    shape: CliManifestJsonProjectionShape,
    fields: Vec<CliManifestJsonField>,
    #[serde(default)]
    count_field: Option<String>,
    #[serde(default)]
    summary_template: Option<String>,
    #[serde(default)]
    refs: Option<CliManifestJsonRefs>,
    #[serde(default)]
    effect: Option<CliManifestJsonEffect>,
}

#[derive(Deserialize)]
struct CliManifestJsonRefs {
    kind: switchboard_core::ToolRefKind,
    id_field: String,
    #[serde(default)]
    parent_id_field: Option<String>,
    #[serde(default)]
    label_field: Option<String>,
    #[serde(default)]
    url_field: Option<String>,
}

#[derive(Deserialize)]
struct CliManifestJsonEffect {
    undoable: bool,
    #[serde(default)]
    use_output_refs: bool,
    #[serde(default)]
    summary_template: Option<String>,
}

fn default_surface() -> ToolSurface {
    ToolSurface::Curated
}

fn default_undo_support() -> ToolUndoSupport {
    ToolUndoSupport::None
}

enum CliManifestDefaultStrategies<'a> {
    Handler(&'a CliCommandHandler),
    RawPassthrough { prefix: Vec<String> },
    None,
}

fn build_manifest_summarize_strategy<'a>(
    tool: &str,
    strategy: CliManifestStrategy,
    program: Option<&str>,
    handlers: &'a HashMap<&'static str, &'static CliCommandHandler>,
) -> Result<(CliSummarizeStrategy, CliManifestDefaultStrategies<'a>)> {
    match strategy {
        CliManifestStrategy::Handler { id } => {
            let handler = resolve_handler(tool, &id, handlers)?;
            Ok((
                CliSummarizeStrategy::Handler(handler.summarize),
                CliManifestDefaultStrategies::Handler(handler),
            ))
        }
        CliManifestStrategy::SummaryTemplate { template } => Ok((
            CliSummarizeStrategy::Template(CliSummaryTemplate::parse(template)?),
            CliManifestDefaultStrategies::None,
        )),
        CliManifestStrategy::RawPassthrough { prefix } => Ok((
            CliSummarizeStrategy::RawInventory {
                program: program
                    .ok_or_else(|| Error::Config(format!("tool {tool} uses raw_passthrough but is not executable")))?
                    .to_owned(),
                prefix: prefix.clone(),
            },
            CliManifestDefaultStrategies::RawPassthrough { prefix },
        )),
    }
}

fn build_manifest_args_strategy(
    tool: &str,
    strategy: Option<CliManifestArgsStrategy>,
    defaults: &CliManifestDefaultStrategies<'_>,
    handlers: &HashMap<&'static str, &'static CliCommandHandler>,
) -> Result<CliArgsStrategy> {
    match strategy {
        Some(CliManifestArgsStrategy::Handler { id }) => {
            let handler = resolve_handler(tool, &id, handlers)?;
            let build_args = handler.build_args.ok_or_else(|| {
                Error::Config(format!(
                    "handler {} for tool {} is missing build_args",
                    handler.id, tool
                ))
            })?;
            Ok(CliArgsStrategy::Handler(build_args))
        }
        Some(CliManifestArgsStrategy::Template { segments }) => {
            let segments = segments
                .into_iter()
                .map(build_manifest_args_segment)
                .collect::<Result<Vec<_>>>()?;
            Ok(CliArgsStrategy::Template(CliArgsTemplate::new(segments)?))
        }
        Some(CliManifestArgsStrategy::RawPassthrough { prefix }) => Ok(CliArgsStrategy::RawInventory { prefix }),
        None => match defaults {
            CliManifestDefaultStrategies::Handler(handler) => {
                let build_args = handler.build_args.ok_or_else(|| {
                    Error::Config(format!(
                        "handler {} for tool {} is missing build_args",
                        handler.id, tool
                    ))
                })?;
                Ok(CliArgsStrategy::Handler(build_args))
            }
            CliManifestDefaultStrategies::RawPassthrough { prefix } => {
                Ok(CliArgsStrategy::RawInventory { prefix: prefix.clone() })
            }
            CliManifestDefaultStrategies::None => Err(Error::Config(format!(
                "tool {tool} is executable and uses summary_template, so it must define args explicitly"
            ))),
        },
    }
}

fn build_manifest_decode_strategy(
    tool: &str,
    program: &str,
    strategy: Option<CliManifestDecodeStrategy>,
    defaults: &CliManifestDefaultStrategies<'_>,
    handlers: &HashMap<&'static str, &'static CliCommandHandler>,
) -> Result<CliDecodeStrategy> {
    match strategy {
        Some(CliManifestDecodeStrategy::Handler { id }) => {
            let handler = resolve_handler(tool, &id, handlers)?;
            let decode = handler
                .decode
                .ok_or_else(|| Error::Config(format!("handler {} for tool {} is missing decode", handler.id, tool)))?;
            Ok(CliDecodeStrategy::Handler(decode))
        }
        Some(CliManifestDecodeStrategy::JsonProjection(projection)) => {
            let CliManifestJsonProjection {
                response_field,
                shape,
                fields,
                count_field,
                summary_template,
                refs,
                effect,
            } = *projection;
            let mappings = fields
                .into_iter()
                .map(build_manifest_json_field_mapping)
                .collect::<Result<Vec<_>>>()?;
            let shape = match shape {
                CliManifestJsonProjectionShape::Object => CliJsonProjectionShape::object(mappings)?,
                CliManifestJsonProjectionShape::Array => CliJsonProjectionShape::array(mappings)?,
            };
            let refs = refs.map(|refs| {
                CliJsonRefsSpec::new(
                    refs.kind,
                    refs.id_field,
                    refs.parent_id_field,
                    refs.label_field,
                    refs.url_field,
                )
            });
            let summary_template = summary_template.map(CliProjectionTemplate::parse).transpose()?;
            let effect = effect
                .map(|effect| {
                    Ok::<_, Error>(CliJsonEffectSpec::new(
                        effect.undoable,
                        effect.use_output_refs,
                        effect.summary_template.map(CliProjectionTemplate::parse).transpose()?,
                    ))
                })
                .transpose()?;
            Ok(CliDecodeStrategy::JsonProjection(CliJsonProjection::new(
                response_field,
                shape,
                count_field,
                summary_template,
                refs,
                effect,
            )?))
        }
        Some(CliManifestDecodeStrategy::RawPassthrough { prefix }) => Ok(CliDecodeStrategy::RawInventory {
            program: program.to_owned(),
            prefix,
        }),
        None => match defaults {
            CliManifestDefaultStrategies::Handler(handler) => {
                let decode = handler.decode.ok_or_else(|| {
                    Error::Config(format!("handler {} for tool {} is missing decode", handler.id, tool))
                })?;
                Ok(CliDecodeStrategy::Handler(decode))
            }
            CliManifestDefaultStrategies::RawPassthrough { prefix } => Ok(CliDecodeStrategy::RawInventory {
                program: program.to_owned(),
                prefix: prefix.clone(),
            }),
            CliManifestDefaultStrategies::None => Err(Error::Config(format!(
                "tool {tool} is executable and uses summary_template, so it must define decode explicitly"
            ))),
        },
    }
}

fn build_manifest_args_segment(segment: CliManifestArgsSegment) -> Result<CliArgsSegment> {
    match segment {
        CliManifestArgsSegment::Literal { value } => CliArgsSegment::literal(value),
        CliManifestArgsSegment::RequiredPositional { aliases } => CliArgsSegment::required_positional(aliases),
        CliManifestArgsSegment::OptionalPositional { aliases } => CliArgsSegment::optional_positional(aliases),
        CliManifestArgsSegment::Option {
            flag,
            aliases,
            repeated,
            required,
        } => CliArgsSegment::option(flag, aliases, repeated, required),
        CliManifestArgsSegment::Flag { flag, aliases } => CliArgsSegment::flag(flag, aliases),
    }
}

fn build_manifest_json_field_mapping(field: CliManifestJsonField) -> Result<CliJsonFieldMapping> {
    let CliManifestJsonField {
        name,
        pointer,
        item_pointer,
        arg,
        default,
        literal,
    } = field;

    match (pointer, item_pointer, arg, literal) {
        (Some(pointer), item_pointer, None, None) => {
            CliJsonFieldMapping::from_pointer_with_items(name, pointer, item_pointer)
        }
        (None, None, Some(aliases), None) => CliJsonFieldMapping::from_argument(name, aliases, default),
        (None, None, None, Some(value)) => {
            if default.is_some() {
                return Err(Error::Config(format!(
                    "json projection field {name} cannot define both literal and default"
                )));
            }
            CliJsonFieldMapping::from_literal(name, value)
        }
        (None, None, None, None) => Err(Error::Config(format!(
            "json projection field {name} must define exactly one of pointer, arg, or literal"
        ))),
        _ => Err(Error::Config(format!(
            "json projection field {name} mixes incompatible source definitions"
        ))),
    }
}

fn resolve_handler<'a>(
    tool: &str,
    id: &str,
    handlers: &'a HashMap<&'static str, &'static CliCommandHandler>,
) -> Result<&'a CliCommandHandler> {
    handlers
        .get(id)
        .copied()
        .ok_or_else(|| Error::Config(format!("manifest command {tool} references unknown handler {id}")))
}
