use std::{
    fmt::{self, Display},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    types::{AuthRef, ProviderKind, ResolvedAuth, ResolvedCredentials},
};

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
            state_dir,
        })
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
