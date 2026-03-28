use std::{
    fmt::{self, Debug, Display},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    types::ProviderKind,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
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
    #[serde(rename = "mychart_cli")]
    MyChartCli,
}

impl AuthKind {
    pub fn from_identifier(value: &str) -> Option<Self> {
        match value {
            "gh_cli" | "github_cli" => Some(Self::GitHubCli),
            "github_token" => Some(Self::GitHubToken),
            "google_oauth" => Some(Self::GoogleOAuth),
            "google_oauth_file" => Some(Self::GoogleOAuthFile),
            "mychart_cli" => Some(Self::MyChartCli),
            _ => None,
        }
    }

    pub fn provider(&self) -> ProviderKind {
        match self {
            Self::GitHubCli | Self::GitHubToken => ProviderKind::GitHub,
            Self::GoogleOAuth | Self::GoogleOAuthFile => ProviderKind::GoogleWorkspace,
            Self::MyChartCli => ProviderKind::MyChart,
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
            Self::MyChartCli => "mychart_cli",
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
            Self::Env { name } => crate::types::validate_non_empty("environment variable name", name),
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
                crate::types::validate_non_empty("1Password account", account)?;
                crate::types::validate_non_empty("1Password item", item)?;
                crate::types::validate_non_empty("1Password field", field)?;
                if let Some(vault) = vault {
                    crate::types::validate_non_empty("1Password vault", vault)?;
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
    #[serde(rename = "mychart_cli")]
    MyChartCli {
        #[serde(skip_serializing_if = "Option::is_none")]
        base_url: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        portal_base_url: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_id: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_secret: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        redirect_uri: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        access_token: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        refresh_token: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        username: Option<SecretRef>,
    },
}

impl AuthSecretRefs {
    pub fn matches_kind(&self, kind: AuthKind) -> bool {
        matches!(
            (kind, self),
            (AuthKind::GitHubCli, Self::None)
                | (AuthKind::GitHubToken, Self::GitHubToken { .. })
                | (AuthKind::GoogleOAuth, Self::GoogleOAuth { .. })
                | (AuthKind::GoogleOAuthFile, Self::GoogleOAuthFile { .. })
                | (AuthKind::MyChartCli, Self::MyChartCli { .. })
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
            Self::MyChartCli {
                base_url,
                portal_base_url,
                client_id,
                client_secret,
                redirect_uri,
                access_token,
                refresh_token,
                username,
            } => [
                base_url.as_ref(),
                portal_base_url.as_ref(),
                client_id.as_ref(),
                client_secret.as_ref(),
                redirect_uri.as_ref(),
                access_token.as_ref(),
                refresh_token.as_ref(),
                username.as_ref(),
            ]
            .into_iter()
            .flatten()
            .collect(),
        }
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
    MyChartCli {
        base_url: Option<SecretString>,
        portal_base_url: Option<SecretString>,
        client_id: Option<SecretString>,
        client_secret: Option<SecretString>,
        redirect_uri: Option<SecretString>,
        access_token: Option<SecretString>,
        refresh_token: Option<SecretString>,
        username: Option<SecretString>,
    },
}
