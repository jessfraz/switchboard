use std::{
    fmt::{self, Display},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    types::{AuthRef, ProviderKind, ResolvedAuth, ResolvedCredentials},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScopeProfile {
    #[default]
    Standard,
    WorkspaceAdmin,
}

impl AuthScopeProfile {
    fn is_standard(&self) -> bool {
        *self == Self::Standard
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
    pub id: crate::types::NamespaceId,
    pub provider: ProviderKind,
    pub account_label: String,
    pub auth_ref: AuthRef,
    pub default_read: bool,
    #[serde(skip_serializing_if = "AuthScopeProfile::is_standard")]
    pub auth_scope_profile: AuthScopeProfile,
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
            id: crate::types::NamespaceId::new(id)?,
            provider,
            account_label,
            auth_ref: AuthRef::new(auth_ref)?,
            default_read,
            auth_scope_profile: AuthScopeProfile::Standard,
            state_dir,
        })
    }

    pub fn with_auth_scope_profile(mut self, profile: AuthScopeProfile) -> Result<Self> {
        if profile == AuthScopeProfile::WorkspaceAdmin && self.provider != ProviderKind::GoogleWorkspace {
            return Err(Error::InvalidArguments(
                "workspace_admin auth scope profile requires a Google Workspace namespace".into(),
            ));
        }

        self.auth_scope_profile = profile;
        Ok(self)
    }
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

#[cfg(test)]
mod tests {
    use super::{AuthScopeProfile, ResolvedNamespace};
    use crate::types::ProviderKind;

    #[test]
    fn workspace_admin_scope_profile_rejects_non_google_namespaces() {
        let namespace = ResolvedNamespace::new(
            "github.work",
            ProviderKind::GitHub,
            "example",
            "github.work_auth",
            false,
            None,
        )
        .expect("namespace should build");

        let error = namespace
            .with_auth_scope_profile(AuthScopeProfile::WorkspaceAdmin)
            .expect_err("non-Google namespace should reject workspace admin scopes");

        assert!(error.to_string().contains("requires a Google Workspace namespace"));
    }
}
