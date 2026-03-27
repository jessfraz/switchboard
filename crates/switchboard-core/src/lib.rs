use std::collections::{BTreeMap, HashMap};
use std::error::Error as StdError;
use std::fmt::{self, Display};
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProviderKind {
    GitHub,
    GoogleWorkspace,
    Slack,
    Ramp,
    IMessage,
    WhatsApp,
}

impl ProviderKind {
    pub fn from_tool_name(tool_name: &str) -> Option<Self> {
        let prefix = tool_name.split('.').next()?;
        match prefix {
            "github" => Some(Self::GitHub),
            "google" => Some(Self::GoogleWorkspace),
            "slack" => Some(Self::Slack),
            "ramp" => Some(Self::Ramp),
            "imessage" => Some(Self::IMessage),
            "whatsapp" => Some(Self::WhatsApp),
            _ => None,
        }
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolKind {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionMode {
    Auto,
    Plan,
    Draft,
    Apply,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedNamespace {
    pub id: NamespaceId,
    pub provider: ProviderKind,
    pub account_label: String,
    pub default_read: bool,
}

impl ResolvedNamespace {
    pub fn new(
        id: impl Into<String>,
        provider: ProviderKind,
        account_label: impl Into<String>,
        default_read: bool,
    ) -> Result<Self> {
        Ok(Self {
            id: NamespaceId::new(id)?,
            provider,
            account_label: account_label.into(),
            default_read,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDescriptor {
    pub name: &'static str,
    pub kind: ToolKind,
    pub summary: &'static str,
    pub backend: BackendKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditOutcome {
    Planned,
    Executed,
    Blocked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyDecision {
    Allow,
    RequireApproval { reason: String },
    Deny { reason: String },
}

pub trait NamespaceStore: Send + Sync {
    fn get(&self, id: &NamespaceId) -> Option<ResolvedNamespace>;
    fn list(&self) -> Vec<ResolvedNamespace>;
}

pub trait PolicyEngine: Send + Sync {
    fn evaluate(&self, namespace: &ResolvedNamespace, plan: &PlannedAction) -> PolicyDecision;
}

pub trait AuditSink: Send + Sync {
    fn record(&self, event: &AuditEvent) -> Result<()>;
}

pub trait Adapter: Send + Sync {
    fn provider(&self) -> ProviderKind;
    fn tools(&self) -> &'static [ToolDescriptor];
    fn plan(
        &self,
        namespace: &ResolvedNamespace,
        request: &ToolRequest,
        descriptor: &'static ToolDescriptor,
    ) -> Result<PlannedAction>;
    fn execute(&self, action: &PlannedAction) -> Result<ToolOutput>;

    fn find_tool(&self, name: &ToolName) -> Option<&'static ToolDescriptor> {
        self.tools().iter().find(|descriptor| descriptor.name == name.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DispatchOutcome {
    Planned(PlannedAction),
    Executed(ToolOutput),
}

#[derive(Default)]
pub struct AdapterRegistry {
    adapters: HashMap<ProviderKind, Arc<dyn Adapter>>,
}

impl AdapterRegistry {
    pub fn register(&mut self, adapter: Arc<dyn Adapter>) {
        self.adapters.insert(adapter.provider(), adapter);
    }

    pub fn get(&self, provider: &ProviderKind) -> Option<Arc<dyn Adapter>> {
        self.adapters.get(provider).cloned()
    }
}

pub struct Switchboard {
    namespaces: Arc<dyn NamespaceStore>,
    policy: Arc<dyn PolicyEngine>,
    audit: Arc<dyn AuditSink>,
    adapters: AdapterRegistry,
}

impl Switchboard {
    pub fn new(
        namespaces: Arc<dyn NamespaceStore>,
        policy: Arc<dyn PolicyEngine>,
        audit: Arc<dyn AuditSink>,
        adapters: AdapterRegistry,
    ) -> Self {
        Self {
            namespaces,
            policy,
            audit,
            adapters,
        }
    }

    pub fn list_namespaces(&self) -> Vec<ResolvedNamespace> {
        self.namespaces.list()
    }

    pub fn dispatch(&self, request: ToolRequest) -> Result<DispatchOutcome> {
        let namespace = self
            .namespaces
            .get(&request.namespace)
            .ok_or_else(|| Error::UnknownNamespace(request.namespace.to_string()))?;
        let requested_provider = request.tool.provider()?;

        if namespace.provider != requested_provider {
            return Err(Error::ProviderMismatch {
                namespace: namespace.id.to_string(),
                namespace_provider: namespace.provider,
                requested_provider,
            });
        }

        let adapter = self
            .adapters
            .get(&namespace.provider)
            .ok_or_else(|| Error::MissingAdapter(namespace.provider.clone()))?;
        let descriptor = adapter
            .find_tool(&request.tool)
            .ok_or_else(|| Error::UnsupportedTool(request.tool.to_string()))?;
        let mut plan = adapter.plan(&namespace, &request, descriptor)?;

        match self.policy.evaluate(&namespace, &plan) {
            PolicyDecision::Allow => {}
            PolicyDecision::RequireApproval { reason } => {
                plan.approval_required = true;
                plan.approval_reason = Some(reason);
            }
            PolicyDecision::Deny { reason } => {
                let _ = self.audit.record(&AuditEvent::from_plan(&plan, AuditOutcome::Blocked));
                return Err(Error::PolicyDenied(reason));
            }
        }

        match descriptor.kind {
            ToolKind::Read => self.finish_read(adapter.as_ref(), plan),
            ToolKind::Write => self.finish_write(adapter.as_ref(), plan),
        }
    }

    fn finish_read(&self, adapter: &dyn Adapter, plan: PlannedAction) -> Result<DispatchOutcome> {
        match plan.mode {
            ExecutionMode::Plan | ExecutionMode::Draft => {
                self.audit
                    .record(&AuditEvent::from_plan(&plan, AuditOutcome::Planned))?;
                Ok(DispatchOutcome::Planned(plan))
            }
            ExecutionMode::Auto | ExecutionMode::Apply => {
                let output = adapter.execute(&plan)?;
                self.audit
                    .record(&AuditEvent::from_plan(&plan, AuditOutcome::Executed))?;
                Ok(DispatchOutcome::Executed(output))
            }
        }
    }

    fn finish_write(&self, adapter: &dyn Adapter, plan: PlannedAction) -> Result<DispatchOutcome> {
        let should_apply = matches!(plan.mode, ExecutionMode::Apply) && !plan.approval_required;

        if should_apply {
            let output = adapter.execute(&plan)?;
            self.audit
                .record(&AuditEvent::from_plan(&plan, AuditOutcome::Executed))?;
            return Ok(DispatchOutcome::Executed(output));
        }

        self.audit
            .record(&AuditEvent::from_plan(&plan, AuditOutcome::Planned))?;
        Ok(DispatchOutcome::Planned(plan))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    Audit(String),
    InvalidArguments(String),
    InvalidToolName(String),
    MissingAdapter(ProviderKind),
    PolicyDenied(String),
    ProviderMismatch {
        namespace: String,
        namespace_provider: ProviderKind,
        requested_provider: ProviderKind,
    },
    UnknownNamespace(String),
    UnsupportedTool(String),
    NotImplemented(String),
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audit(message) => write!(f, "audit failure: {message}"),
            Self::InvalidArguments(message) => write!(f, "invalid arguments: {message}"),
            Self::InvalidToolName(tool) => write!(f, "invalid tool name: {tool}"),
            Self::MissingAdapter(provider) => write!(f, "missing adapter for provider: {provider}"),
            Self::PolicyDenied(reason) => write!(f, "policy denied request: {reason}"),
            Self::ProviderMismatch {
                namespace,
                namespace_provider,
                requested_provider,
            } => write!(
                f,
                "namespace {namespace} belongs to provider {namespace_provider}, requested tool targets {requested_provider}"
            ),
            Self::UnknownNamespace(namespace) => write!(f, "unknown namespace: {namespace}"),
            Self::UnsupportedTool(tool) => write!(f, "unsupported tool: {tool}"),
            Self::NotImplemented(message) => write!(f, "not implemented: {message}"),
        }
    }
}

impl StdError for Error {}
