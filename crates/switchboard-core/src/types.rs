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
    pub auth_ref: AuthRef,
    pub kind: ToolKind,
    pub mode: ExecutionMode,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    pub approval_reason: Option<String>,
    pub args: BTreeMap<String, String>,
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolOutput {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub summary: String,
    pub fields: BTreeMap<String, Value>,
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
        self.fields.insert(key.into(), Value::String(value.into()));
        self
    }

    pub fn with_value_field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.fields.insert(key.into(), value);
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
    pub auth_ref: AuthRef,
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
            auth_ref: plan.auth_ref.clone(),
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

fn validate_non_empty(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::InvalidArguments(format!("{label} cannot be empty")));
    }

    Ok(())
}
