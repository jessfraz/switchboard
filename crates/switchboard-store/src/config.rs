use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use switchboard_core::{
    AuthKind, AuthRef, AuthSecretRefs, AuthStore, Error, ProviderKind, ResolvedAuth, ResolvedNamespace, ResolvedSecret,
    Result, SecretRef, SecretSource, SecretStore, WritePolicy,
};

use crate::{ConfiguredPolicyEngine, StaticAuthStore, StaticNamespaceStore, StaticSecretStore};

#[derive(Clone, Debug)]
pub struct SwitchboardConfig {
    namespaces: StaticNamespaceStore,
    auth: StaticAuthStore,
    secrets: StaticSecretStore,
    write_policy: WritePolicy,
}

impl SwitchboardConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|error| {
            Error::Config(format!(
                "failed to read switchboard config from {}: {error}",
                path.display()
            ))
        })?;

        Self::from_source(&source, &format!("switchboard config at {}", path.display()))
    }

    pub fn from_toml_str(source: &str) -> Result<Self> {
        Self::from_source(source, "switchboard config")
    }

    pub fn into_stores(self) -> (StaticNamespaceStore, StaticAuthStore, StaticSecretStore) {
        (self.namespaces, self.auth, self.secrets)
    }

    pub fn policy_engine(&self) -> ConfiguredPolicyEngine {
        ConfiguredPolicyEngine::new(self.write_policy)
    }

    fn from_source(source: &str, source_label: &str) -> Result<Self> {
        let config: RawConfig = toml::from_str(source)
            .map_err(|error| Error::Config(format!("failed to parse {source_label}: {error}")))?;
        let secrets = build_secret_store(config.secret)?;
        let auth = build_auth_store(config.auth, &secrets)?;
        let namespaces = build_namespace_store(config.namespace, &auth)?;
        let write_policy = config.policy.write;

        Ok(Self {
            namespaces,
            auth,
            secrets,
            write_policy,
        })
    }
}

fn build_secret_store(raw_secrets: BTreeMap<String, RawSecret>) -> Result<StaticSecretStore> {
    let mut secrets = Vec::with_capacity(raw_secrets.len());

    for (secret_ref, raw) in raw_secrets {
        let source = match raw {
            RawSecret::Env { name } => SecretSource::Env { name },
            RawSecret::File { path } => SecretSource::File {
                path: PathBuf::from(path),
            },
            RawSecret::OnePasswordItem {
                account,
                item,
                field,
                vault,
            } => SecretSource::OnePasswordItem {
                account,
                item,
                field,
                vault,
            },
        };

        secrets.push(ResolvedSecret::new(secret_ref, source)?);
    }

    Ok(StaticSecretStore::new(secrets))
}

fn build_auth_store(raw_auth: BTreeMap<String, RawAuth>, secrets: &StaticSecretStore) -> Result<StaticAuthStore> {
    if raw_auth.is_empty() {
        return Err(Error::Config(
            "config must define at least one auth entry under [auth.<name>]".into(),
        ));
    }

    let mut auth_entries = Vec::with_capacity(raw_auth.len());

    for (auth_ref, raw) in raw_auth {
        let provider = ProviderKind::from_identifier(raw.provider()).ok_or_else(|| {
            Error::Config(format!(
                "auth.{auth_ref} declares unknown provider {:?}",
                raw.provider()
            ))
        })?;
        let kind = raw.kind();

        if kind.provider() != provider {
            return Err(Error::Config(format!(
                "auth.{auth_ref} declares auth kind {kind}, which belongs to provider {}, not {provider}",
                kind.provider()
            )));
        }

        let secret_refs = raw.secret_refs()?;
        for secret_ref in secret_refs.secret_refs() {
            if secrets.get(secret_ref).is_none() {
                return Err(Error::Config(format!(
                    "auth.{auth_ref} references missing secret ref {secret_ref}"
                )));
            }
        }

        auth_entries.push(ResolvedAuth::new(
            auth_ref,
            provider,
            kind,
            raw.account().to_owned(),
            secret_refs,
        )?);
    }

    Ok(StaticAuthStore::new(auth_entries))
}

fn build_namespace_store(
    raw_namespaces: BTreeMap<String, BTreeMap<String, RawNamespace>>,
    auth: &StaticAuthStore,
) -> Result<StaticNamespaceStore> {
    let mut namespaces = Vec::new();

    for (provider_key, aliases) in raw_namespaces {
        let provider_in_path = ProviderKind::from_identifier(&provider_key).ok_or_else(|| {
            Error::Config(format!(
                "unknown provider {provider_key:?} in namespace table [namespace.{provider_key}.*]"
            ))
        })?;

        for (alias, namespace) in aliases {
            if alias.trim().is_empty() {
                return Err(Error::Config(format!(
                    "namespace.{provider_key} contains an empty namespace alias"
                )));
            }

            let provider = ProviderKind::from_identifier(&namespace.provider).ok_or_else(|| {
                Error::Config(format!(
                    "namespace.{provider_key}.{alias} declares unknown provider {:?}",
                    namespace.provider
                ))
            })?;

            if provider != provider_in_path {
                return Err(Error::Config(format!(
                    "namespace.{provider_key}.{alias} declares provider {provider}, but its namespace path uses {provider_in_path}"
                )));
            }

            let auth_ref = AuthRef::new(&namespace.auth)?;
            let auth_entry = auth.get(&auth_ref).ok_or_else(|| {
                Error::Config(format!(
                    "namespace.{provider_key}.{alias} references missing auth ref {auth_ref}"
                ))
            })?;

            if auth_entry.provider != provider {
                return Err(Error::Config(format!(
                    "namespace.{provider_key}.{alias} uses auth ref {auth_ref}, which belongs to provider {}, not {provider}",
                    auth_entry.provider
                )));
            }

            namespaces.push(ResolvedNamespace::new(
                format!("{provider_key}.{alias}"),
                provider,
                namespace.account,
                namespace.auth,
                namespace.default_read,
                namespace.state_dir.map(PathBuf::from),
            )?);
        }
    }

    if namespaces.is_empty() {
        return Err(Error::Config(
            "config must define at least one namespace under [namespace.<provider>.<name>]".into(),
        ));
    }

    Ok(StaticNamespaceStore::new(namespaces))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    secret: BTreeMap<String, RawSecret>,
    #[serde(default)]
    auth: BTreeMap<String, RawAuth>,
    #[serde(default)]
    namespace: BTreeMap<String, BTreeMap<String, RawNamespace>>,
    #[serde(default)]
    policy: RawPolicy,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    #[serde(default = "default_write_policy")]
    write: WritePolicy,
}

impl Default for RawPolicy {
    fn default() -> Self {
        Self {
            write: default_write_policy(),
        }
    }
}

fn default_write_policy() -> WritePolicy {
    WritePolicy::RequireApproval
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum RawSecret {
    #[serde(rename = "env")]
    Env { name: String },
    #[serde(rename = "file")]
    File { path: String },
    #[serde(rename = "onepassword_item", alias = "one_password_item")]
    OnePasswordItem {
        account: String,
        item: String,
        field: String,
        #[serde(default)]
        vault: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum RawAuth {
    #[serde(rename = "gh_cli")]
    GitHubCli { provider: String, account: String },
    #[serde(rename = "github_token")]
    GitHubToken {
        provider: String,
        account: String,
        token: String,
    },
    #[serde(rename = "google_oauth")]
    GoogleOAuth {
        provider: String,
        account: String,
        client_id: String,
        client_secret: String,
        #[serde(default)]
        refresh_token: Option<String>,
    },
    #[serde(rename = "google_oauth_file")]
    GoogleOAuthFile {
        provider: String,
        account: String,
        credentials: String,
    },
}

impl RawAuth {
    fn provider(&self) -> &str {
        match self {
            Self::GitHubCli { provider, .. }
            | Self::GitHubToken { provider, .. }
            | Self::GoogleOAuth { provider, .. }
            | Self::GoogleOAuthFile { provider, .. } => provider,
        }
    }

    fn account(&self) -> &str {
        match self {
            Self::GitHubCli { account, .. }
            | Self::GitHubToken { account, .. }
            | Self::GoogleOAuth { account, .. }
            | Self::GoogleOAuthFile { account, .. } => account,
        }
    }

    fn kind(&self) -> AuthKind {
        match self {
            Self::GitHubCli { .. } => AuthKind::GitHubCli,
            Self::GitHubToken { .. } => AuthKind::GitHubToken,
            Self::GoogleOAuth { .. } => AuthKind::GoogleOAuth,
            Self::GoogleOAuthFile { .. } => AuthKind::GoogleOAuthFile,
        }
    }

    fn secret_refs(&self) -> Result<AuthSecretRefs> {
        match self {
            Self::GitHubCli { .. } => Ok(AuthSecretRefs::None),
            Self::GitHubToken { token, .. } => Ok(AuthSecretRefs::GitHubToken {
                token: SecretRef::new(token)?,
            }),
            Self::GoogleOAuth {
                client_id,
                client_secret,
                refresh_token,
                ..
            } => Ok(AuthSecretRefs::GoogleOAuth {
                client_id: SecretRef::new(client_id)?,
                client_secret: SecretRef::new(client_secret)?,
                refresh_token: match refresh_token {
                    Some(refresh_token) => Some(SecretRef::new(refresh_token)?),
                    None => None,
                },
            }),
            Self::GoogleOAuthFile { credentials, .. } => Ok(AuthSecretRefs::GoogleOAuthFile {
                credentials: SecretRef::new(credentials)?,
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNamespace {
    provider: String,
    account: String,
    auth: String,
    #[serde(default)]
    default_read: bool,
    #[serde(default)]
    state_dir: Option<String>,
}

#[cfg(test)]
mod tests {
    use switchboard_core::{
        AuthKind, AuthRef, AuthStore, Error, NamespaceId, NamespaceStore, SecretStore, WritePolicy,
    };

    use super::SwitchboardConfig;

    const BASIC_CONFIG_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/basic.toml"
    ));
    const UNKNOWN_PROVIDER_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/unknown-provider.toml"
    ));
    const ALLOW_WRITES_CONFIG_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/allow-writes.toml"
    ));
    const PROVIDER_MISMATCH_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/provider-mismatch.toml"
    ));
    const EMPTY_AUTH_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/empty-auth.toml"
    ));
    const EMPTY_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/empty.toml"
    ));
    const MISSING_AUTH_REF_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/missing-auth-ref.toml"
    ));
    const MISSING_SECRET_REF_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/missing-secret-ref.toml"
    ));

    #[test]
    fn parses_readme_shape_config_into_namespace_auth_and_secret_stores() {
        let config = SwitchboardConfig::from_toml_str(&render_basic_config("/tmp/google-personal-oauth.json"))
            .expect("config should parse");
        let (namespaces, auth, secrets) = config.into_stores();
        let ids = namespaces
            .list()
            .into_iter()
            .map(|namespace| namespace.id.to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec![
                "github.personal",
                "github.personal_token",
                "google.personal",
                "google.work"
            ]
        );

        let google_work = namespaces
            .get(&NamespaceId::new("google.work").expect("namespace should parse"))
            .expect("google.work should exist");
        assert_eq!(google_work.auth_ref.as_str(), "google_work");

        let google_work_auth = auth
            .get(&AuthRef::new("google_work").expect("auth ref should parse"))
            .expect("google work auth should exist");
        assert_eq!(google_work_auth.kind, AuthKind::GoogleOAuth);
        assert_eq!(google_work_auth.secret_refs().len(), 2);

        let google_personal_auth = auth
            .get(&AuthRef::new("google_personal").expect("auth ref should parse"))
            .expect("google personal auth should exist");
        assert_eq!(google_personal_auth.kind, AuthKind::GoogleOAuthFile);
        assert_eq!(google_personal_auth.secret_refs().len(), 1);

        let github_token_auth = auth
            .get(&AuthRef::new("github_personal_token").expect("auth ref should parse"))
            .expect("github token auth should exist");
        assert_eq!(github_token_auth.kind, AuthKind::GitHubToken);
        assert_eq!(github_token_auth.secret_refs().len(), 1);

        assert_eq!(secrets.list().len(), 4);
    }

    #[test]
    fn parses_allow_write_policy() {
        let config = SwitchboardConfig::from_toml_str(&render_allow_writes_config("/tmp/google-personal-oauth.json"))
            .expect("config should parse");

        assert_eq!(config.policy_engine().write_policy(), WritePolicy::Allow);
    }

    #[test]
    fn rejects_unknown_providers() {
        let error =
            SwitchboardConfig::from_toml_str(UNKNOWN_PROVIDER_CONFIG).expect_err("unknown providers should fail");

        assert_eq!(
            error,
            Error::Config("auth.oracle_personal declares unknown provider \"oracle\"".into())
        );
    }

    #[test]
    fn rejects_namespace_provider_mismatch() {
        let error =
            SwitchboardConfig::from_toml_str(PROVIDER_MISMATCH_CONFIG).expect_err("provider mismatch should fail");

        assert_eq!(
            error,
            Error::Config(
                "namespace.github.personal declares provider google, but its namespace path uses github".into()
            )
        );
    }

    #[test]
    fn rejects_empty_auth_references() {
        let error = SwitchboardConfig::from_toml_str(EMPTY_AUTH_CONFIG).expect_err("empty auth refs should fail");

        assert_eq!(error, Error::InvalidArguments("auth reference cannot be empty".into()));
    }

    #[test]
    fn rejects_missing_auth_refs() {
        let error =
            SwitchboardConfig::from_toml_str(MISSING_AUTH_REF_CONFIG).expect_err("missing auth refs should fail");

        assert_eq!(
            error,
            Error::Config("namespace.google.personal references missing auth ref google_work".into())
        );
    }

    #[test]
    fn rejects_missing_secret_refs() {
        let error =
            SwitchboardConfig::from_toml_str(MISSING_SECRET_REF_CONFIG).expect_err("missing secret refs should fail");

        assert_eq!(
            error,
            Error::Config("auth.google_work references missing secret ref google_work_client_id".into())
        );
    }

    #[test]
    fn rejects_empty_config() {
        let error = SwitchboardConfig::from_toml_str(EMPTY_CONFIG).expect_err("empty config should fail");

        assert_eq!(
            error,
            Error::Config("config must define at least one auth entry under [auth.<name>]".into())
        );
    }

    fn render_basic_config(google_personal_oauth_path: &str) -> String {
        BASIC_CONFIG_TEMPLATE.replace("__GOOGLE_PERSONAL_OAUTH_PATH__", google_personal_oauth_path)
    }

    fn render_allow_writes_config(google_personal_oauth_path: &str) -> String {
        ALLOW_WRITES_CONFIG_TEMPLATE.replace("__GOOGLE_PERSONAL_OAUTH_PATH__", google_personal_oauth_path)
    }
}
