use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use switchboard_core::{
    Error, ProviderKind, Result, ToolDescriptor, ToolExecutionSupport, ToolKind, ToolName, ToolSurface, ToolUndoSupport,
};

use crate::{
    cli::command::{
        CliArgsStrategy, CliBinarySpec, CliCapabilityProbe, CliCommandSpec, CliDecodeStrategy, CliExecutableSpec,
        CliSummarizeStrategy,
    },
    cli::declarative::{
        CliArgsSegment, CliArgsTemplate, CliComputedJsonValue, CliJsonArgumentField, CliJsonArgumentTemplate,
        CliJsonArgumentValue, CliJsonEffectSpec, CliJsonFieldMapping, CliJsonProjection, CliJsonProjectionConfig,
        CliJsonProjectionShape, CliJsonRefsSpec, CliProjectionTemplate, CliSummaryTemplate,
    },
    inventory::{CliInventory, CliInventoryCommand, CliInventoryNodeKind, CliOperationKind},
};

pub(crate) struct CliProviderCatalog {
    tools: Vec<ToolDescriptor>,
    commands: Vec<CliCommandSpec>,
}

impl CliProviderCatalog {
    pub(crate) fn from_embedded(manifest_json: &str, inventory: &CliInventory) -> Result<Self> {
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
        let mut tools = Vec::new();
        let mut commands = Vec::new();
        let mut registered_names = HashSet::new();

        for command in manifest.commands {
            let spec = build_manifest_command(command, manifest.provider.clone(), &binaries, &capabilities)?;
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

pub fn validate_manifest_json(manifest_json: &str, inventory: &CliInventory) -> Result<()> {
    CliProviderCatalog::from_embedded(manifest_json, inventory).map(|_| ())
}

fn build_manifest_command(
    command: CliManifestCommand,
    provider: ProviderKind,
    binaries: &HashMap<String, CliBinarySpec>,
    capabilities: &HashMap<String, CliCapabilityProbe>,
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

    let execution = match command.execution {
        CliManifestExecution::Executable { binary, capability } => Some((
            binaries
                .get(&binary)
                .cloned()
                .ok_or_else(|| Error::Config(format!("tool {} references unknown binary {binary}", tool_name)))?,
            capabilities.get(&capability).cloned().ok_or_else(|| {
                Error::Config(format!(
                    "tool {} references unknown capability {capability}",
                    tool_name
                ))
            })?,
        )),
        CliManifestExecution::PlanningOnly => None,
    };

    let (summarize, defaults) = build_manifest_summarize_strategy(
        &descriptor.name,
        command.strategy,
        execution.as_ref().map(|(binary, _)| binary.program.as_str()),
    )?;
    let executable = match execution {
        Some((binary, capability)) => Some(CliExecutableSpec {
            binary: binary.clone(),
            capability: capability.clone(),
            args: build_manifest_args_strategy(tool_name.as_str(), command.args, &defaults)?,
            decode: build_manifest_decode_strategy(
                tool_name.as_str(),
                binary.program.as_str(),
                command.decode,
                &defaults,
            )?,
        }),
        None => {
            if command.args.is_some() {
                return Err(Error::Config(format!(
                    "tool {} defines args but is planning_only",
                    tool_name
                )));
            }
            if command.decode.is_some() {
                return Err(Error::Config(format!(
                    "tool {} defines decode but is planning_only",
                    tool_name
                )));
            }
            None
        }
    };

    let arguments = executable
        .as_ref()
        .map(|executable| executable.args.argument_specs())
        .transpose()?
        .unwrap_or_default();
    let descriptor = ToolDescriptor::new(
        command.name,
        command.kind,
        command.summary,
        switchboard_core::BackendKind::Cli,
    )?
    .with_surface(command.surface)
    .with_aggregate_read_supported(aggregate_read_supported)
    .with_execution_support(execution_support)
    .with_undo_support(command.undo_support)
    .with_arguments(arguments);

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
        .with_undo_support(ToolUndoSupport::None)
        .with_arguments(vec![
            crate::cli::command::CliArgsStrategy::RawInventory {
                prefix: command.path.clone(),
            }
            .argument_specs()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Config(format!("inventory raw tool {} is missing argv metadata", command.command)))?,
        ]);
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
#[serde(deny_unknown_fields)]
struct CliManifest {
    provider: ProviderKind,
    binaries: Vec<CliManifestBinary>,
    capabilities: Vec<CliManifestCapability>,
    commands: Vec<CliManifestCommand>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CliManifestBinary {
    id: String,
    program: String,
    #[serde(default)]
    env_override: Option<String>,
    version_args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CliManifestCapability {
    id: String,
    args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CliManifestStrategy {
    SummaryTemplate {
        template: String,
    },
    RawPassthrough {
        #[serde(default)]
        prefix: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CliManifestExecution {
    Executable { binary: String, capability: String },
    PlanningOnly,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CliManifestArgsStrategy {
    Template {
        segments: Vec<CliManifestArgsSegment>,
    },
    RawPassthrough {
        #[serde(default)]
        prefix: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
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
    Json {
        value: CliManifestJsonArgValue,
    },
    Option {
        flag: String,
        aliases: Vec<String>,
        #[serde(default)]
        repeated: bool,
        #[serde(default)]
        required: bool,
    },
    KeyValueOption {
        flag: String,
        key: String,
        aliases: Vec<String>,
        #[serde(default)]
        repeated: bool,
        #[serde(default)]
        required: bool,
        #[serde(default)]
        boolish: bool,
    },
    Flag {
        flag: String,
        aliases: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CliManifestDecodeStrategy {
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
#[serde(deny_unknown_fields)]
struct CliManifestJsonField {
    name: String,
    #[serde(default)]
    pointer: Option<String>,
    #[serde(default)]
    item_pointer: Option<String>,
    #[serde(default)]
    arg: Option<Vec<String>>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    arg_present: Option<Vec<String>>,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    literal: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CliManifestJsonArgValue {
    Literal {
        value: serde_json::Value,
    },
    Argument {
        aliases: Vec<String>,
        #[serde(default)]
        required: bool,
        #[serde(default)]
        repeated: bool,
        #[serde(default)]
        default: Option<serde_json::Value>,
    },
    Object {
        fields: Vec<CliManifestJsonArgField>,
    },
    Array {
        items: Vec<CliManifestJsonArgValue>,
    },
    Computed {
        id: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CliManifestJsonArgField {
    name: String,
    value: CliManifestJsonArgValue,
    #[serde(default)]
    omit_if_null: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CliManifestJsonProjection {
    response_field: String,
    #[serde(default)]
    source_pointer: Option<String>,
    shape: CliManifestJsonProjectionShape,
    fields: Vec<CliManifestJsonField>,
    #[serde(default)]
    count_field: Option<String>,
    #[serde(default)]
    extra_fields: Vec<CliManifestJsonField>,
    #[serde(default)]
    summary_template: Option<String>,
    #[serde(default)]
    refs: Option<CliManifestJsonRefsConfig>,
    #[serde(default)]
    effect: Option<CliManifestJsonEffect>,
    #[serde(default)]
    empty_stdout_json: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(untagged)]
enum CliManifestJsonRefsConfig {
    One(CliManifestJsonRefs),
    Many(Vec<CliManifestJsonRefs>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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

enum CliManifestDefaultStrategies {
    RawPassthrough { prefix: Vec<String> },
    None,
}

fn build_manifest_summarize_strategy(
    tool: &str,
    strategy: CliManifestStrategy,
    program: Option<&str>,
) -> Result<(CliSummarizeStrategy, CliManifestDefaultStrategies)> {
    match strategy {
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
    defaults: &CliManifestDefaultStrategies,
) -> Result<CliArgsStrategy> {
    match strategy {
        Some(CliManifestArgsStrategy::Template { segments }) => {
            let segments = segments
                .into_iter()
                .map(build_manifest_args_segment)
                .collect::<Result<Vec<_>>>()?;
            Ok(CliArgsStrategy::Template(CliArgsTemplate::new(segments)?))
        }
        Some(CliManifestArgsStrategy::RawPassthrough { prefix }) => Ok(CliArgsStrategy::RawInventory { prefix }),
        None => match defaults {
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
    defaults: &CliManifestDefaultStrategies,
) -> Result<CliDecodeStrategy> {
    match strategy {
        Some(CliManifestDecodeStrategy::JsonProjection(projection)) => {
            let CliManifestJsonProjection {
                response_field,
                source_pointer,
                shape,
                fields,
                count_field,
                extra_fields,
                summary_template,
                refs,
                effect,
                empty_stdout_json,
            } = *projection;
            let mappings = fields
                .into_iter()
                .map(build_manifest_json_field_mapping)
                .collect::<Result<Vec<_>>>()?;
            let extra_fields = extra_fields
                .into_iter()
                .map(build_manifest_json_field_mapping)
                .collect::<Result<Vec<_>>>()?;
            let shape = match shape {
                CliManifestJsonProjectionShape::Object => CliJsonProjectionShape::object(mappings)?,
                CliManifestJsonProjectionShape::Array => CliJsonProjectionShape::array(mappings)?,
            };
            let refs = refs.map(build_manifest_json_refs).transpose()?.unwrap_or_default();
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
                CliJsonProjectionConfig {
                    response_field,
                    source_pointer,
                    shape,
                    count_field,
                    extra_fields,
                    summary_template,
                    refs,
                    effect,
                    empty_stdout_json,
                },
            )?))
        }
        Some(CliManifestDecodeStrategy::RawPassthrough { prefix }) => Ok(CliDecodeStrategy::RawInventory {
            program: program.to_owned(),
            prefix,
        }),
        None => match defaults {
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
        CliManifestArgsSegment::Json { value } => Ok(CliArgsSegment::json(CliJsonArgumentTemplate::new(
            build_manifest_json_arg_value(value)?,
        ))),
        CliManifestArgsSegment::Option {
            flag,
            aliases,
            repeated,
            required,
        } => CliArgsSegment::option(flag, aliases, repeated, required),
        CliManifestArgsSegment::KeyValueOption {
            flag,
            key,
            aliases,
            repeated,
            required,
            boolish,
        } => CliArgsSegment::key_value_option(flag, key, aliases, repeated, required, boolish),
        CliManifestArgsSegment::Flag { flag, aliases } => CliArgsSegment::flag(flag, aliases),
    }
}

fn build_manifest_json_arg_value(value: CliManifestJsonArgValue) -> Result<CliJsonArgumentValue> {
    match value {
        CliManifestJsonArgValue::Literal { value } => Ok(CliJsonArgumentValue::Literal(value)),
        CliManifestJsonArgValue::Argument {
            aliases,
            required,
            repeated,
            default,
        } => Ok(CliJsonArgumentValue::Argument {
            aliases: validate_manifest_aliases("json argument source", aliases)?,
            required,
            repeated,
            default,
        }),
        CliManifestJsonArgValue::Object { fields } => Ok(CliJsonArgumentValue::Object {
            fields: fields
                .into_iter()
                .map(|field| {
                    CliJsonArgumentField::new(
                        field.name,
                        build_manifest_json_arg_value(field.value)?,
                        field.omit_if_null,
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        }),
        CliManifestJsonArgValue::Array { items } => Ok(CliJsonArgumentValue::Array {
            items: items
                .into_iter()
                .map(build_manifest_json_arg_value)
                .collect::<Result<Vec<_>>>()?,
        }),
        CliManifestJsonArgValue::Computed { id } => {
            Ok(CliJsonArgumentValue::Computed(build_manifest_computed_json_value(&id)?))
        }
    }
}

fn build_manifest_computed_json_value(id: &str) -> Result<CliComputedJsonValue> {
    match id {
        "gmail_raw_message" => Ok(CliComputedJsonValue::GmailRawMessage),
        _ => Err(Error::Config(format!("unknown computed json value {id}"))),
    }
}

fn build_manifest_json_refs(config: CliManifestJsonRefsConfig) -> Result<Vec<CliJsonRefsSpec>> {
    let refs = match config {
        CliManifestJsonRefsConfig::One(refs) => vec![refs],
        CliManifestJsonRefsConfig::Many(refs) => refs,
    };

    refs.into_iter()
        .map(|refs| {
            Ok(CliJsonRefsSpec::new(
                refs.kind,
                refs.id_field,
                refs.parent_id_field,
                refs.label_field,
                refs.url_field,
            ))
        })
        .collect()
}

fn build_manifest_json_field_mapping(field: CliManifestJsonField) -> Result<CliJsonFieldMapping> {
    let CliManifestJsonField {
        name,
        pointer,
        item_pointer,
        arg,
        args,
        arg_present,
        default,
        literal,
    } = field;

    let source_count = usize::from(pointer.is_some())
        + usize::from(arg.is_some())
        + usize::from(args.is_some())
        + usize::from(arg_present.is_some())
        + usize::from(literal.is_some());
    if source_count != 1 {
        return Err(Error::Config(format!(
            "json projection field {name} must define exactly one source"
        )));
    }

    if item_pointer.is_some() && pointer.is_none() {
        return Err(Error::Config(format!(
            "json projection field {name} cannot define item_pointer without pointer"
        )));
    }
    if default.is_some() && arg.is_none() {
        return Err(Error::Config(format!(
            "json projection field {name} can only define default with arg"
        )));
    }

    if let Some(pointer) = pointer {
        return CliJsonFieldMapping::from_pointer_with_items(name, pointer, item_pointer);
    }
    if let Some(aliases) = arg {
        return CliJsonFieldMapping::from_argument(name, aliases, default);
    }
    if let Some(aliases) = args {
        return CliJsonFieldMapping::from_argument_values(name, aliases);
    }
    if let Some(aliases) = arg_present {
        return CliJsonFieldMapping::from_argument_presence(name, aliases);
    }
    if let Some(value) = literal {
        return CliJsonFieldMapping::from_literal(name, value);
    }

    Err(Error::Config(format!(
        "json projection field {name} must define exactly one source"
    )))
}

fn validate_manifest_aliases(context: &str, aliases: Vec<String>) -> Result<Vec<String>> {
    let aliases = aliases
        .into_iter()
        .map(|alias| alias.trim().to_owned())
        .filter(|alias| !alias.is_empty())
        .collect::<Vec<_>>();
    if aliases.is_empty() {
        return Err(Error::Config(format!("{context} must name at least one argument")));
    }

    Ok(aliases)
}
