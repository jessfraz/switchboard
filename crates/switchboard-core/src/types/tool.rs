use std::collections::BTreeMap;
use std::fmt::{self, Display};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::{
    BackendKind, NamespaceId, OperationEffect, OperationId, PlanningTarget, ProviderKind, ToolName,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Auto,
    Plan,
    Draft,
    Apply,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolArgument {
    Flag { name: String },
    Option { name: String, value: String },
}

impl ToolArgument {
    pub fn flag(name: impl Into<String>) -> Result<Self> {
        let name = crate::types::validate_argument_name(name.into())?;
        Ok(Self::Flag { name })
    }

    pub fn option(name: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        let name = crate::types::validate_argument_name(name.into())?;
        Ok(Self::Option {
            name,
            value: value.into(),
        })
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Flag { name } | Self::Option { name, .. } => name,
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            Self::Flag { .. } => None,
            Self::Option { value, .. } => Some(value),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolArguments(Vec<ToolArgument>);

impl ToolArguments {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(arguments: Vec<ToolArgument>) -> Self {
        Self(arguments)
    }

    pub fn has_flag(&self, name: &str) -> bool {
        self.0
            .iter()
            .any(|argument| matches!(argument, ToolArgument::Flag { name: candidate } if candidate == name))
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.values(name).last()
    }

    pub fn values<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a str> + 'a {
        let name = name.to_owned();
        self.0.iter().filter_map(move |argument| match argument {
            ToolArgument::Option { name: candidate, value } if candidate == &name => Some(value.as_str()),
            _ => None,
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &ToolArgument> {
        self.0.iter()
    }
}

impl From<Vec<ToolArgument>> for ToolArguments {
    fn from(value: Vec<ToolArgument>) -> Self {
        Self::new(value)
    }
}

impl From<BTreeMap<String, String>> for ToolArguments {
    fn from(value: BTreeMap<String, String>) -> Self {
        let arguments = value
            .into_iter()
            .map(|(name, value)| ToolArgument::Option { name, value })
            .collect();
        Self::new(arguments)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolRequest {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub mode: ExecutionMode,
    pub args: ToolArguments,
}

impl ToolRequest {
    pub fn new(
        tool: impl Into<String>,
        namespace: impl Into<String>,
        mode: ExecutionMode,
        args: impl Into<ToolArguments>,
    ) -> Result<Self> {
        Ok(Self {
            tool: ToolName::new(tool)?,
            namespace: NamespaceId::new(namespace)?,
            mode,
            args: args.into(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlannedAction {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub auth_ref: crate::types::AuthRef,
    pub kind: ToolKind,
    pub mode: ExecutionMode,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    pub approval_reason: Option<String>,
    pub args: ToolArguments,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensates_operation_id: Option<OperationId>,
}

impl PlannedAction {
    pub fn new(
        request: &ToolRequest,
        target: &PlanningTarget,
        kind: ToolKind,
        summary: impl Into<String>,
        backend: BackendKind,
    ) -> Self {
        Self {
            tool: request.tool.clone(),
            namespace: target.namespace.id.clone(),
            auth_ref: target.auth.id.clone(),
            kind,
            mode: request.mode,
            summary: summary.into(),
            backend,
            approval_required: false,
            approval_reason: None,
            args: request.args.clone(),
            operation_id: None,
            compensates_operation_id: None,
        }
    }

    pub fn with_operation_id(mut self, operation_id: OperationId) -> Self {
        self.operation_id = Some(operation_id);
        self
    }

    pub fn with_compensates_operation_id(mut self, operation_id: OperationId) -> Self {
        self.compensates_operation_id = Some(operation_id);
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRefKind {
    Message,
    Thread,
    Event,
    Notification,
    PullRequest,
    Issue,
    Repository,
}

impl Display for ToolRefKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Message => "message",
            Self::Thread => "thread",
            Self::Event => "event",
            Self::Notification => "notification",
            Self::PullRequest => "pull_request",
            Self::Issue => "issue",
            Self::Repository => "repository",
        };

        write!(f, "{value}")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolRef {
    pub provider: ProviderKind,
    pub namespace: NamespaceId,
    pub kind: ToolRefKind,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
}

impl ToolRef {
    pub fn new(
        provider: ProviderKind,
        namespace: NamespaceId,
        kind: ToolRefKind,
        id: impl Into<String>,
    ) -> Result<Self> {
        let id = id.into();
        crate::types::validate_non_empty("tool ref id", &id)?;

        Ok(Self {
            provider,
            namespace,
            kind,
            id,
            parent_id: None,
            label: None,
            web_url: None,
        })
    }

    pub fn with_parent_id(mut self, parent_id: impl Into<String>) -> Result<Self> {
        let parent_id = parent_id.into();
        crate::types::validate_non_empty("tool ref parent id", &parent_id)?;
        self.parent_id = Some(parent_id);
        Ok(self)
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Result<Self> {
        let label = label.into();
        crate::types::validate_non_empty("tool ref label", &label)?;
        self.label = Some(label);
        Ok(self)
    }

    pub fn with_web_url(mut self, web_url: impl Into<String>) -> Result<Self> {
        let web_url = web_url.into();
        crate::types::validate_non_empty("tool ref web url", &web_url)?;
        self.web_url = Some(web_url);
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolOutput {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub summary: String,
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<ToolRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<OperationEffect>,
}

impl ToolOutput {
    pub fn new(tool: ToolName, namespace: NamespaceId, summary: impl Into<String>) -> Self {
        Self {
            tool,
            namespace,
            summary: summary.into(),
            fields: BTreeMap::new(),
            refs: Vec::new(),
            operation_id: None,
            effect: None,
        }
    }

    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), Value::String(value.into()));
        self
    }

    pub fn with_value_field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.fields.insert(key.into(), value);
        self
    }

    pub fn with_ref(mut self, tool_ref: ToolRef) -> Self {
        self.refs.push(tool_ref);
        self
    }

    pub fn with_refs(mut self, tool_refs: impl IntoIterator<Item = ToolRef>) -> Self {
        self.refs.extend(tool_refs);
        self
    }

    pub fn with_operation_id(mut self, operation_id: OperationId) -> Self {
        self.operation_id = Some(operation_id);
        self
    }

    pub fn with_effect(mut self, effect: OperationEffect) -> Self {
        self.effect = Some(effect);
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSurface {
    Curated,
    Raw,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionSupport {
    PlanningOnly,
    Executable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolUndoSupport {
    None,
    CompensatingAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolArgumentTransport {
    Positional,
    Option,
    KeyValueOption,
    Flag,
    JsonField,
    PassthroughArgv,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolArgumentValueKind {
    String,
    Boolean,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolArgumentSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub transport: ToolArgumentTransport,
    pub value_kind: ToolArgumentValueKind,
    pub required: bool,
    pub repeated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_flag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_key: Option<String>,
}

impl ToolArgumentSpec {
    pub fn new(
        name: impl Into<String>,
        transport: ToolArgumentTransport,
        value_kind: ToolArgumentValueKind,
    ) -> Result<Self> {
        let name = crate::types::validate_argument_name(name.into())?;
        Ok(Self {
            name,
            aliases: Vec::new(),
            transport,
            value_kind,
            required: false,
            repeated: false,
            forwarded_flag: None,
            forwarded_key: None,
        })
    }

    pub fn with_aliases(mut self, aliases: Vec<String>) -> Result<Self> {
        self.aliases = crate::types::normalize_argument_aliases(&self.name, aliases)?;
        Ok(self)
    }

    pub fn with_required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn with_repeated(mut self, repeated: bool) -> Self {
        self.repeated = repeated;
        self
    }

    pub fn with_forwarding(mut self, forwarded_flag: Option<String>, forwarded_key: Option<String>) -> Result<Self> {
        if let Some(flag) = forwarded_flag.as_ref() {
            crate::types::validate_non_empty("tool argument forwarded_flag", flag)?;
        }
        if let Some(key) = forwarded_key.as_ref() {
            crate::types::validate_non_empty("tool argument forwarded_key", key)?;
        }

        self.forwarded_flag = forwarded_flag;
        self.forwarded_key = forwarded_key;
        Ok(self)
    }

    pub fn merge_with(&mut self, other: &Self) -> Result<()> {
        if self.transport != other.transport
            || self.value_kind != other.value_kind
            || self.forwarded_flag != other.forwarded_flag
            || self.forwarded_key != other.forwarded_key
        {
            return Err(Error::Config(format!(
                "conflicting argument metadata for {}",
                self.name
            )));
        }

        self.required |= other.required;
        self.repeated |= other.repeated;
        self.aliases = crate::types::normalize_argument_aliases(
            &self.name,
            self.aliases.iter().chain(other.aliases.iter()).cloned().collect(),
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub kind: ToolKind,
    pub summary: String,
    pub backend: BackendKind,
    pub surface: ToolSurface,
    pub aggregate_read_supported: bool,
    pub execution_support: ToolExecutionSupport,
    pub undo_support: ToolUndoSupport,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<ToolArgumentSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegisteredTool {
    pub name: ToolName,
    pub provider: ProviderKind,
    pub kind: ToolKind,
    pub summary: String,
    pub backend: BackendKind,
    pub surface: ToolSurface,
    pub aggregate_read_supported: bool,
    pub execution_support: ToolExecutionSupport,
    pub undo_support: ToolUndoSupport,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<ToolArgumentSpec>,
}

impl ToolDescriptor {
    pub fn new(
        name: impl Into<String>,
        kind: ToolKind,
        summary: impl Into<String>,
        backend: BackendKind,
    ) -> Result<Self> {
        let name = name.into();
        let summary = summary.into();
        ToolName::new(&name)?;
        crate::types::validate_non_empty("tool summary", &summary)?;

        Ok(Self {
            name,
            kind,
            summary,
            backend,
            surface: ToolSurface::Curated,
            aggregate_read_supported: kind == ToolKind::Read,
            execution_support: ToolExecutionSupport::Executable,
            undo_support: ToolUndoSupport::None,
            arguments: Vec::new(),
        })
    }

    pub fn with_surface(mut self, surface: ToolSurface) -> Self {
        self.surface = surface;
        self
    }

    pub fn with_aggregate_read_supported(mut self, aggregate_read_supported: bool) -> Self {
        self.aggregate_read_supported = aggregate_read_supported;
        self
    }

    pub fn with_execution_support(mut self, execution_support: ToolExecutionSupport) -> Self {
        self.execution_support = execution_support;
        self
    }

    pub fn with_undo_support(mut self, undo_support: ToolUndoSupport) -> Self {
        self.undo_support = undo_support;
        self
    }

    pub fn with_arguments(mut self, arguments: Vec<ToolArgumentSpec>) -> Self {
        self.arguments = arguments;
        self
    }
}

impl RegisteredTool {
    pub fn from_descriptor(descriptor: &ToolDescriptor) -> Result<Self> {
        let name = ToolName::new(&descriptor.name)?;
        let provider = name.provider()?;

        Ok(Self {
            name,
            provider,
            kind: descriptor.kind,
            summary: descriptor.summary.clone(),
            backend: descriptor.backend,
            surface: descriptor.surface,
            aggregate_read_supported: descriptor.aggregate_read_supported,
            execution_support: descriptor.execution_support,
            undo_support: descriptor.undo_support,
            arguments: descriptor.arguments.clone(),
        })
    }
}
