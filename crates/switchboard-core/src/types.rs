use std::{
    collections::BTreeMap,
    fmt::{self, Debug, Display},
    path::PathBuf,
};

use serde::Serialize;
use serde_json::Value;

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
pub struct SecretRef(String);

impl SecretRef {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::InvalidArguments("secret reference cannot be empty".into()));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum AuthKind {
    #[serde(rename = "gh_cli")]
    GitHubCli,
    #[serde(rename = "github_token")]
    GitHubToken,
    #[serde(rename = "google_oauth")]
    GoogleOAuth,
    #[serde(rename = "google_oauth_file")]
    GoogleOAuthFile,
}

impl AuthKind {
    pub fn from_identifier(value: &str) -> Option<Self> {
        match value {
            "gh_cli" | "github_cli" => Some(Self::GitHubCli),
            "github_token" => Some(Self::GitHubToken),
            "google_oauth" => Some(Self::GoogleOAuth),
            "google_oauth_file" => Some(Self::GoogleOAuthFile),
            _ => None,
        }
    }

    pub fn provider(&self) -> ProviderKind {
        match self {
            Self::GitHubCli | Self::GitHubToken => ProviderKind::GitHub,
            Self::GoogleOAuth | Self::GoogleOAuthFile => ProviderKind::GoogleWorkspace,
        }
    }
}

impl Display for AuthKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::GitHubCli => "gh_cli",
            Self::GitHubToken => "github_token",
            Self::GoogleOAuth => "google_oauth",
            Self::GoogleOAuthFile => "google_oauth_file",
        };

        write!(f, "{value}")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum SecretSource {
    #[serde(rename = "env")]
    Env { name: String },
    #[serde(rename = "file")]
    File { path: PathBuf },
    #[serde(rename = "onepassword_item")]
    OnePasswordItem {
        account: String,
        item: String,
        field: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        vault: Option<String>,
    },
}

impl SecretSource {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Env { name } => validate_non_empty("environment variable name", name),
            Self::File { path } => {
                if path.as_os_str().is_empty() {
                    return Err(Error::InvalidArguments("secret file path cannot be empty".into()));
                }

                Ok(())
            }
            Self::OnePasswordItem {
                account,
                item,
                field,
                vault,
            } => {
                validate_non_empty("1Password account", account)?;
                validate_non_empty("1Password item", item)?;
                validate_non_empty("1Password field", field)?;
                if let Some(vault) = vault {
                    validate_non_empty("1Password vault", vault)?;
                }

                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedSecret {
    pub id: SecretRef,
    pub source: SecretSource,
}

impl ResolvedSecret {
    pub fn new(id: impl Into<String>, source: SecretSource) -> Result<Self> {
        source.validate()?;

        Ok(Self {
            id: SecretRef::new(id)?,
            source,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum AuthSecretRefs {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "github_token")]
    GitHubToken { token: SecretRef },
    #[serde(rename = "google_oauth")]
    GoogleOAuth {
        client_id: SecretRef,
        client_secret: SecretRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        refresh_token: Option<SecretRef>,
    },
    #[serde(rename = "google_oauth_file")]
    GoogleOAuthFile { credentials: SecretRef },
}

impl AuthSecretRefs {
    pub fn matches_kind(&self, kind: AuthKind) -> bool {
        matches!(
            (kind, self),
            (AuthKind::GitHubCli, Self::None)
                | (AuthKind::GitHubToken, Self::GitHubToken { .. })
                | (AuthKind::GoogleOAuth, Self::GoogleOAuth { .. })
                | (AuthKind::GoogleOAuthFile, Self::GoogleOAuthFile { .. })
        )
    }

    pub fn secret_refs(&self) -> Vec<&SecretRef> {
        match self {
            Self::None => Vec::new(),
            Self::GitHubToken { token } => vec![token],
            Self::GoogleOAuth {
                client_id,
                client_secret,
                refresh_token,
            } => {
                let mut refs = vec![client_id, client_secret];
                if let Some(refresh_token) = refresh_token.as_ref() {
                    refs.push(refresh_token);
                }

                refs
            }
            Self::GoogleOAuthFile { credentials } => vec![credentials],
        }
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_non_empty("operation id", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_dir: Option<PathBuf>,
}

impl ResolvedNamespace {
    pub fn new(
        id: impl Into<String>,
        provider: ProviderKind,
        account_label: impl Into<String>,
        auth_ref: impl Into<String>,
        default_read: bool,
        state_dir: Option<PathBuf>,
    ) -> Result<Self> {
        let account_label = account_label.into();
        if account_label.trim().is_empty() {
            return Err(Error::InvalidArguments("account label cannot be empty".into()));
        }

        if state_dir.as_ref().is_some_and(|path| path.as_os_str().is_empty()) {
            return Err(Error::InvalidArguments("namespace state_dir cannot be empty".into()));
        }

        Ok(Self {
            id: NamespaceId::new(id)?,
            provider,
            account_label,
            auth_ref: AuthRef::new(auth_ref)?,
            default_read,
            state_dir,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedAuth {
    pub id: AuthRef,
    pub provider: ProviderKind,
    pub kind: AuthKind,
    pub account_label: String,
    pub secrets: AuthSecretRefs,
}

impl ResolvedAuth {
    pub fn new(
        id: impl Into<String>,
        provider: ProviderKind,
        kind: AuthKind,
        account_label: impl Into<String>,
        secrets: AuthSecretRefs,
    ) -> Result<Self> {
        let account_label = account_label.into();
        if account_label.trim().is_empty() {
            return Err(Error::InvalidArguments("auth account label cannot be empty".into()));
        }

        if !secrets.matches_kind(kind) {
            return Err(Error::InvalidArguments(format!(
                "auth kind {kind} does not accept the configured secret references"
            )));
        }

        Ok(Self {
            id: AuthRef::new(id)?,
            provider,
            kind,
            account_label,
            secrets,
        })
    }

    pub fn secret_refs(&self) -> Vec<&SecretRef> {
        self.secrets.secret_refs()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SecretString(String);

impl SecretString {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedCredentials {
    GitHubCli,
    GitHubToken {
        token: SecretString,
    },
    GoogleOAuth {
        client_id: SecretString,
        client_secret: SecretString,
        refresh_token: Option<SecretString>,
    },
    GoogleOAuthFile {
        credentials: SecretString,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningTarget {
    pub namespace: ResolvedNamespace,
    pub auth: ResolvedAuth,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionTarget {
    pub namespace: ResolvedNamespace,
    pub auth: ResolvedAuth,
    pub credentials: ResolvedCredentials,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolArgument {
    Flag { name: String },
    Option { name: String, value: String },
}

impl ToolArgument {
    pub fn flag(name: impl Into<String>) -> Result<Self> {
        let name = validate_argument_name(name.into())?;
        Ok(Self::Flag { name })
    }

    pub fn option(name: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        let name = validate_argument_name(name.into())?;
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
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

    pub fn value<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        self.values(name).last()
    }

    pub fn values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0.iter().filter_map(move |argument| match argument {
            ToolArgument::Option { name: candidate, value } if candidate == name => Some(value.as_str()),
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
    pub auth_ref: AuthRef,
    pub kind: ToolKind,
    pub mode: ExecutionMode,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    pub approval_reason: Option<String>,
    pub args: ToolArguments,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
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
        }
    }

    pub fn with_operation_id(mut self, operation_id: OperationId) -> Self {
        self.operation_id = Some(operation_id);
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
        validate_non_empty("tool ref id", &id)?;

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
        validate_non_empty("tool ref parent id", &parent_id)?;
        self.parent_id = Some(parent_id);
        Ok(self)
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Result<Self> {
        let label = label.into();
        validate_non_empty("tool ref label", &label)?;
        self.label = Some(label);
        Ok(self)
    }

    pub fn with_web_url(mut self, web_url: impl Into<String>) -> Result<Self> {
        let web_url = web_url.into();
        validate_non_empty("tool ref web url", &web_url)?;
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

fn validate_argument_name(name: String) -> Result<String> {
    if name.trim().is_empty() {
        return Err(Error::InvalidArguments("tool argument name cannot be empty".into()));
    }

    Ok(name)
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
    Failed,
    Compensated,
    Blocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuditEvent {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub auth_ref: AuthRef,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    pub outcome: AuditOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
}

impl AuditEvent {
    pub fn from_plan(plan: &PlannedAction, outcome: AuditOutcome) -> Self {
        Self {
            tool: plan.tool.clone(),
            namespace: plan.namespace.clone(),
            auth_ref: plan.auth_ref.clone(),
            summary: plan.summary.clone(),
            backend: plan.backend,
            approval_required: plan.approval_required,
            outcome,
            operation_id: plan.operation_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperationEffect {
    pub refs: Vec<ToolRef>,
    pub undoable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_summary: Option<String>,
}

impl OperationEffect {
    pub fn new(undoable: bool) -> Self {
        Self {
            refs: Vec::new(),
            undoable,
            undo_summary: None,
        }
    }

    pub fn with_ref(mut self, tool_ref: ToolRef) -> Self {
        self.refs.push(tool_ref);
        self
    }

    pub fn with_refs(mut self, tool_refs: impl IntoIterator<Item = ToolRef>) -> Self {
        self.refs.extend(tool_refs);
        self
    }

    pub fn with_undo_summary(mut self, undo_summary: impl Into<String>) -> Result<Self> {
        let undo_summary = undo_summary.into();
        validate_non_empty("undo summary", &undo_summary)?;
        self.undo_summary = Some(undo_summary);
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Planned,
    Applied,
    Failed,
    Compensated,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StoredOperation {
    pub id: OperationId,
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub auth_ref: AuthRef,
    pub kind: ToolKind,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    pub status: OperationStatus,
    pub args: ToolArguments,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<OperationEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

impl StoredOperation {
    pub fn from_plan(id: OperationId, plan: &PlannedAction) -> Self {
        Self {
            id,
            tool: plan.tool.clone(),
            namespace: plan.namespace.clone(),
            auth_ref: plan.auth_ref.clone(),
            kind: plan.kind,
            summary: plan.summary.clone(),
            backend: plan.backend,
            approval_required: plan.approval_required,
            status: OperationStatus::Planned,
            args: plan.args.clone(),
            effect: None,
            failure_reason: None,
        }
    }

    pub fn mark_applied(&mut self, effect: Option<OperationEffect>) {
        self.status = OperationStatus::Applied;
        self.effect = effect;
        self.failure_reason = None;
    }

    pub fn mark_failed(&mut self, failure_reason: impl Into<String>) -> Result<()> {
        let failure_reason = failure_reason.into();
        validate_non_empty("operation failure reason", &failure_reason)?;
        self.status = OperationStatus::Failed;
        self.failure_reason = Some(failure_reason);
        Ok(())
    }

    pub fn mark_compensated(&mut self) {
        self.status = OperationStatus::Compensated;
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    RequireApproval { reason: String },
    Deny { reason: String },
}

fn validate_non_empty(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::InvalidArguments(format!("{label} cannot be empty")));
    }

    Ok(())
}
