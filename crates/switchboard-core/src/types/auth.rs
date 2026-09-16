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
#[serde(try_from = "String")]
pub struct AuthRef(String);

impl TryFrom<String> for AuthRef {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

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
    #[serde(rename = "google_cli")]
    GoogleCli,
    #[serde(rename = "google_oauth")]
    GoogleOAuth,
    #[serde(rename = "google_oauth_file")]
    GoogleOAuthFile,
    #[serde(rename = "mychart_cli")]
    MyChartCli,
    #[serde(rename = "schwab_cli")]
    SchwabCli,
    #[serde(rename = "phone_cli")]
    PhoneCli,
}

impl AuthKind {
    pub fn from_identifier(value: &str) -> Option<Self> {
        match value {
            "gh_cli" | "github_cli" => Some(Self::GitHubCli),
            "github_token" => Some(Self::GitHubToken),
            "google_cli" => Some(Self::GoogleCli),
            "google_oauth" => Some(Self::GoogleOAuth),
            "google_oauth_file" => Some(Self::GoogleOAuthFile),
            "mychart_cli" => Some(Self::MyChartCli),
            "schwab_cli" => Some(Self::SchwabCli),
            "phone_cli" => Some(Self::PhoneCli),
            _ => None,
        }
    }

    pub fn provider(&self) -> ProviderKind {
        match self {
            Self::GitHubCli | Self::GitHubToken => ProviderKind::GitHub,
            Self::GoogleCli | Self::GoogleOAuth | Self::GoogleOAuthFile => ProviderKind::GoogleWorkspace,
            Self::MyChartCli => ProviderKind::MyChart,
            Self::SchwabCli => ProviderKind::Schwab,
            Self::PhoneCli => ProviderKind::Phone,
        }
    }
}

impl Display for AuthKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::GitHubCli => "gh_cli",
            Self::GitHubToken => "github_token",
            Self::GoogleCli => "google_cli",
            Self::GoogleOAuth => "google_oauth",
            Self::GoogleOAuthFile => "google_oauth_file",
            Self::MyChartCli => "mychart_cli",
            Self::SchwabCli => "schwab_cli",
            Self::PhoneCli => "phone_cli",
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
    #[serde(rename = "phone_cli")]
    PhoneCli {
        #[serde(skip_serializing_if = "Option::is_none")]
        api_key: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        api_secret: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model_api_key: Option<SecretRef>,
    },
    #[serde(rename = "none")]
    GitHubCli,
    #[serde(rename = "github_token")]
    GitHubToken { token: SecretRef },
    #[serde(rename = "google_cli")]
    GoogleCli,
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
    #[serde(rename = "schwab_cli")]
    SchwabCli {
        #[serde(skip_serializing_if = "Option::is_none")]
        base_url: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        market_data_base_url: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        authorize_url: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        token_url: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_id: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_secret: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        third_party_id: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_channel: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_app_id: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_function_id: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resource_version: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rrbus_pilot_rollout: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        redirect_uri: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        access_token: Option<SecretRef>,
        #[serde(skip_serializing_if = "Option::is_none")]
        refresh_token: Option<SecretRef>,
    },
}

impl AuthSecretRefs {
    pub fn kind(&self) -> AuthKind {
        match self {
            Self::PhoneCli { .. } => AuthKind::PhoneCli,
            Self::GitHubCli => AuthKind::GitHubCli,
            Self::GitHubToken { .. } => AuthKind::GitHubToken,
            Self::GoogleCli => AuthKind::GoogleCli,
            Self::GoogleOAuth { .. } => AuthKind::GoogleOAuth,
            Self::GoogleOAuthFile { .. } => AuthKind::GoogleOAuthFile,
            Self::MyChartCli { .. } => AuthKind::MyChartCli,
            Self::SchwabCli { .. } => AuthKind::SchwabCli,
        }
    }

    pub fn secret_refs(&self) -> Vec<&SecretRef> {
        match self {
            Self::PhoneCli {
                api_key,
                api_secret,
                model_api_key,
            } => [api_key.as_ref(), api_secret.as_ref(), model_api_key.as_ref()]
                .into_iter()
                .flatten()
                .collect(),
            Self::GitHubCli | Self::GoogleCli => Vec::new(),
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
            Self::SchwabCli {
                base_url,
                market_data_base_url,
                authorize_url,
                token_url,
                client_id,
                client_secret,
                third_party_id,
                client_channel,
                client_app_id,
                client_function_id,
                resource_version,
                rrbus_pilot_rollout,
                redirect_uri,
                access_token,
                refresh_token,
            } => [
                base_url.as_ref(),
                market_data_base_url.as_ref(),
                authorize_url.as_ref(),
                token_url.as_ref(),
                client_id.as_ref(),
                client_secret.as_ref(),
                third_party_id.as_ref(),
                client_channel.as_ref(),
                client_app_id.as_ref(),
                client_function_id.as_ref(),
                resource_version.as_ref(),
                rrbus_pilot_rollout.as_ref(),
                redirect_uri.as_ref(),
                access_token.as_ref(),
                refresh_token.as_ref(),
            ]
            .into_iter()
            .flatten()
            .collect(),
        }
    }
}

/// Authentication whose provider and kind are determined by its credential variant.
///
/// Contradictory provider and auth-kind fields cannot be constructed:
///
/// ```compile_fail
/// use switchboard_core::{AuthKind, AuthRef, AuthSecretRefs, ProviderKind, ResolvedAuth};
///
/// let auth = ResolvedAuth {
///     id: AuthRef::new("google_personal").expect("valid auth reference"),
///     provider: ProviderKind::GitHub,
///     kind: AuthKind::GoogleCli,
///     account_label: "personal".into(),
///     secrets: AuthSecretRefs::GoogleCli,
/// };
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAuth {
    id: AuthRef,
    account_label: String,
    secrets: AuthSecretRefs,
}

impl ResolvedAuth {
    pub fn new(id: impl Into<String>, account_label: impl Into<String>, secrets: AuthSecretRefs) -> Result<Self> {
        let account_label = account_label.into();
        crate::types::validate_non_empty("auth account label", &account_label)?;

        Ok(Self {
            id: AuthRef::new(id)?,
            account_label,
            secrets,
        })
    }

    pub fn id(&self) -> &AuthRef {
        &self.id
    }

    pub fn provider(&self) -> ProviderKind {
        self.kind().provider()
    }

    pub fn kind(&self) -> AuthKind {
        self.secrets.kind()
    }

    pub fn account_label(&self) -> &str {
        &self.account_label
    }

    pub fn secrets(&self) -> &AuthSecretRefs {
        &self.secrets
    }

    pub fn secret_refs(&self) -> Vec<&SecretRef> {
        self.secrets.secret_refs()
    }
}

impl Serialize for ResolvedAuth {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("ResolvedAuth", 5)?;
        state.serialize_field("id", self.id())?;
        state.serialize_field("provider", &self.provider())?;
        state.serialize_field("kind", &self.kind())?;
        state.serialize_field("account_label", self.account_label())?;
        state.serialize_field("secrets", self.secrets())?;
        state.end()
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
    PhoneCli {
        api_key: Option<SecretString>,
        api_secret: Option<SecretString>,
        model_api_key: Option<SecretString>,
    },
    GitHubCli,
    GitHubToken {
        token: SecretString,
    },
    GoogleCli,
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
    SchwabCli {
        base_url: Option<SecretString>,
        market_data_base_url: Option<SecretString>,
        authorize_url: Option<SecretString>,
        token_url: Option<SecretString>,
        client_id: Option<SecretString>,
        client_secret: Option<SecretString>,
        third_party_id: Option<SecretString>,
        client_channel: Option<SecretString>,
        client_app_id: Option<SecretString>,
        client_function_id: Option<SecretString>,
        resource_version: Option<SecretString>,
        rrbus_pilot_rollout: Option<SecretString>,
        redirect_uri: Option<SecretString>,
        access_token: Option<SecretString>,
        refresh_token: Option<SecretString>,
    },
}

#[cfg(test)]
mod tests {
    use crate::{AuthKind, AuthSecretRefs, ProviderKind, ResolvedAuth, SecretRef};

    #[test]
    fn google_cli_auth_is_distinct_and_requires_no_secrets() {
        assert_eq!(AuthKind::from_identifier("google_cli"), Some(AuthKind::GoogleCli));
        assert_eq!(AuthKind::GoogleCli.provider(), ProviderKind::GoogleWorkspace);
        assert_eq!(
            serde_json::to_string(&AuthKind::GoogleCli).expect("auth kind should serialize"),
            "\"google_cli\""
        );
        let auth = ResolvedAuth::new("google_personal", "personal@example.com", AuthSecretRefs::GoogleCli)
            .expect("CLI-managed Google auth should be valid");
        assert!(auth.secret_refs().is_empty());
        assert_eq!(auth.kind(), AuthKind::GoogleCli);
        assert_eq!(auth.provider(), ProviderKind::GoogleWorkspace);
        assert_eq!(AuthSecretRefs::GitHubCli.kind(), AuthKind::GitHubCli);
    }

    #[test]
    fn auth_identity_follows_its_credential_variant() {
        let cases = [
            (AuthSecretRefs::GitHubCli, AuthKind::GitHubCli, ProviderKind::GitHub),
            (
                AuthSecretRefs::GitHubToken {
                    token: SecretRef::new("github_token").expect("valid token reference"),
                },
                AuthKind::GitHubToken,
                ProviderKind::GitHub,
            ),
            (
                AuthSecretRefs::GoogleCli,
                AuthKind::GoogleCli,
                ProviderKind::GoogleWorkspace,
            ),
            (
                AuthSecretRefs::GoogleOAuth {
                    client_id: SecretRef::new("client_id").expect("valid client ID reference"),
                    client_secret: SecretRef::new("client_secret").expect("valid client secret reference"),
                    refresh_token: None,
                },
                AuthKind::GoogleOAuth,
                ProviderKind::GoogleWorkspace,
            ),
            (
                AuthSecretRefs::GoogleOAuthFile {
                    credentials: SecretRef::new("credentials").expect("valid credentials reference"),
                },
                AuthKind::GoogleOAuthFile,
                ProviderKind::GoogleWorkspace,
            ),
            (
                AuthSecretRefs::MyChartCli {
                    base_url: None,
                    portal_base_url: None,
                    client_id: None,
                    client_secret: None,
                    redirect_uri: None,
                    access_token: None,
                    refresh_token: None,
                    username: None,
                },
                AuthKind::MyChartCli,
                ProviderKind::MyChart,
            ),
            (
                AuthSecretRefs::SchwabCli {
                    base_url: None,
                    market_data_base_url: None,
                    authorize_url: None,
                    token_url: None,
                    client_id: None,
                    client_secret: None,
                    third_party_id: None,
                    client_channel: None,
                    client_app_id: None,
                    client_function_id: None,
                    resource_version: None,
                    rrbus_pilot_rollout: None,
                    redirect_uri: None,
                    access_token: None,
                    refresh_token: None,
                },
                AuthKind::SchwabCli,
                ProviderKind::Schwab,
            ),
        ];

        for (secrets, kind, provider) in cases {
            let auth = ResolvedAuth::new("auth_ref", "account", secrets.clone()).expect("valid auth");
            assert_eq!(auth.id().as_str(), "auth_ref");
            assert_eq!(auth.account_label(), "account");
            assert_eq!(auth.kind(), kind);
            assert_eq!(auth.provider(), provider);
            assert_eq!(auth.secrets(), &secrets);
        }
    }

    #[test]
    fn auth_serialization_keeps_existing_fields_and_credential_tags() {
        let auth =
            ResolvedAuth::new("github_personal", "personal", AuthSecretRefs::GitHubCli).expect("valid GitHub auth");
        assert_eq!(
            serde_json::to_string(&auth).expect("auth should serialize"),
            r#"{"id":"github_personal","provider":"github","kind":"gh_cli","account_label":"personal","secrets":{"kind":"none"}}"#
        );

        let auth = ResolvedAuth::new(
            "google_personal",
            "personal",
            AuthSecretRefs::GoogleOAuth {
                client_id: SecretRef::new("client_id").expect("valid client ID reference"),
                client_secret: SecretRef::new("client_secret").expect("valid client secret reference"),
                refresh_token: None,
            },
        )
        .expect("valid Google auth");
        assert_eq!(
            serde_json::to_string(&auth).expect("auth should serialize"),
            r#"{"id":"google_personal","provider":"google","kind":"google_oauth","account_label":"personal","secrets":{"kind":"google_oauth","client_id":"client_id","client_secret":"client_secret"}}"#
        );
    }

    #[test]
    fn auth_constructor_preserves_identity_validation() {
        for invalid in ["", " ", "\n\t"] {
            assert!(ResolvedAuth::new(invalid, "account", AuthSecretRefs::GoogleCli).is_err());
            assert!(ResolvedAuth::new("auth_ref", invalid, AuthSecretRefs::GoogleCli).is_err());
        }
    }
}
