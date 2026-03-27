use std::{
    collections::BTreeMap,
    fmt::{self, Display},
};

use serde::Serialize;

use crate::error::{Error, Result};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ProviderKind {
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "google")]
    GoogleWorkspace,
    #[serde(rename = "slack")]
    Slack,
    #[serde(rename = "ramp")]
    Ramp,
    #[serde(rename = "imessage")]
    IMessage,
    #[serde(rename = "whatsapp")]
    WhatsApp,
}

impl ProviderKind {
    pub fn from_identifier(value: &str) -> Option<Self> {
        match value {
            "github" => Some(Self::GitHub),
            "google" => Some(Self::GoogleWorkspace),
            "slack" => Some(Self::Slack),
            "ramp" => Some(Self::Ramp),
            "imessage" => Some(Self::IMessage),
            "whatsapp" => Some(Self::WhatsApp),
            _ => None,
        }
    }

    pub fn from_tool_name(tool_name: &str) -> Option<Self> {
        let prefix = tool_name.split('.').next()?;
        Self::from_identifier(prefix)
    }
}

impl Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::GitHub => "github",
            Self::GoogleWorkspace => "google",
            Self::Slack => "slack",
            Self::Ramp => "ramp",
            Self::IMessage => "imessage",
            Self::WhatsApp => "whatsapp",
        };

        write!(f, "{value}")
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct NamespaceId(String);

impl NamespaceId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::InvalidArguments("namespace cannot be empty".into()));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for NamespaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AuthRef(String);

impl AuthRef {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::InvalidArguments("auth reference cannot be empty".into()));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for AuthRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if ProviderKind::from_tool_name(&value).is_none() {
            return Err(Error::InvalidToolName(value));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn provider(&self) -> Result<ProviderKind> {
        ProviderKind::from_tool_name(&self.0).ok_or_else(|| Error::InvalidToolName(self.0.clone()))
    }
}

impl Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Auto,
    Plan,
    Draft,
    Apply,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Cli,
    Api,
    Local,
    Bridge,
}

impl Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Cli => "cli",
            Self::Api => "api",
            Self::Local => "local",
            Self::Bridge => "bridge",
        };

        write!(f, "{value}")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedNamespace {
    pub id: NamespaceId,
    pub provider: ProviderKind,
    pub account_label: String,
    pub auth_ref: AuthRef,
    pub default_read: bool,
}

impl ResolvedNamespace {
    pub fn new(
        id: impl Into<String>,
        provider: ProviderKind,
        account_label: impl Into<String>,
        auth_ref: impl Into<String>,
        default_read: bool,
    ) -> Result<Self> {
        let account_label = account_label.into();
        if account_label.trim().is_empty() {
            return Err(Error::InvalidArguments("account label cannot be empty".into()));
        }

        Ok(Self {
            id: NamespaceId::new(id)?,
            provider,
            account_label,
            auth_ref: AuthRef::new(auth_ref)?,
            default_read,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolRequest {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub mode: ExecutionMode,
    pub args: BTreeMap<String, String>,
}

impl ToolRequest {
    pub fn new(
        tool: impl Into<String>,
        namespace: impl Into<String>,
        mode: ExecutionMode,
        args: BTreeMap<String, String>,
    ) -> Result<Self> {
        Ok(Self {
            tool: ToolName::new(tool)?,
            namespace: NamespaceId::new(namespace)?,
            mode,
            args,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlannedAction {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub kind: ToolKind,
    pub mode: ExecutionMode,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    pub approval_reason: Option<String>,
    pub args: BTreeMap<String, String>,
}

impl PlannedAction {
    pub fn new(request: &ToolRequest, kind: ToolKind, summary: impl Into<String>, backend: BackendKind) -> Self {
        Self {
            tool: request.tool.clone(),
            namespace: request.namespace.clone(),
            kind,
            mode: request.mode,
            summary: summary.into(),
            backend,
            approval_required: false,
            approval_reason: None,
            args: request.args.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolOutput {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub summary: String,
    pub fields: BTreeMap<String, String>,
}

impl ToolOutput {
    pub fn new(tool: ToolName, namespace: NamespaceId, summary: impl Into<String>) -> Self {
        Self {
            tool,
            namespace,
            summary: summary.into(),
            fields: BTreeMap::new(),
        }
    }

    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolDescriptor {
    pub name: &'static str,
    pub kind: ToolKind,
    pub summary: &'static str,
    pub backend: BackendKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Planned,
    Executed,
    Blocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuditEvent {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    pub outcome: AuditOutcome,
}

impl AuditEvent {
    pub fn from_plan(plan: &PlannedAction, outcome: AuditOutcome) -> Self {
        Self {
            tool: plan.tool.clone(),
            namespace: plan.namespace.clone(),
            summary: plan.summary.clone(),
            backend: plan.backend,
            approval_required: plan.approval_required,
            outcome,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    RequireApproval { reason: String },
    Deny { reason: String },
}
