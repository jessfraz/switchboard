use std::{
    collections::BTreeMap,
    env, fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use switchboard_core::{
    AuthKind, AuthRef, AuthScopeProfile, AuthSecretRefs, AuthStore, Error, ProviderKind, ResolvedAuth,
    ResolvedNamespace, ResolvedSecret, Result, SecretRef, SecretSource, SecretStore, WritePolicy,
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

        Self::from_source_with_base(
            &source,
            &format!("switchboard config at {}", path.display()),
            path.parent(),
        )
    }

    pub fn from_toml_str(source: &str) -> Result<Self> {
        Self::from_source_with_base(source, "switchboard config", None)
    }

    pub fn into_stores(self) -> (StaticNamespaceStore, StaticAuthStore, StaticSecretStore) {
        (self.namespaces, self.auth, self.secrets)
    }

    pub fn policy_engine(&self) -> ConfiguredPolicyEngine {
        ConfiguredPolicyEngine::new(self.write_policy)
    }

    fn from_source_with_base(source: &str, source_label: &str, base_dir: Option<&Path>) -> Result<Self> {
        let mut config: RawConfig = toml::from_str(source)
            .map_err(|error| Error::Config(format!("failed to parse {source_label}: {error}")))?;
        config.resolve_paths(base_dir, config_home_dir().as_deref());
        let secrets = build_secret_store(config.secret)?;
        let explicit_auth = build_auth_store(config.auth, &secrets)?;
        let (namespaces, implicit_auth) = build_namespace_store(config.namespace, &explicit_auth)?;
        let auth = StaticAuthStore::new(explicit_auth.list().into_iter().chain(implicit_auth));
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
            RawSecret::File { path } => SecretSource::File { path },
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
) -> Result<(StaticNamespaceStore, Vec<ResolvedAuth>)> {
    let mut namespaces = Vec::new();
    let mut implicit_auth = Vec::new();

    for (provider_key, aliases) in raw_namespaces {
        let provider_in_path = ProviderKind::from_identifier(&provider_key).ok_or_else(|| {
            Error::Config(format!(
                "unknown provider {provider_key:?} in namespace table [namespace.{provider_key}.*]"
            ))
        })?;

        for (alias, namespace) in aliases {
            let auth_scope_profile = namespace.auth_scope_profile;
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

            let auth_ref = match namespace.auth.as_deref() {
                Some(auth_ref) => {
                    let auth_ref = AuthRef::new(auth_ref)?;
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

                    auth_ref
                }
                None if provider_uses_implicit_cli_auth(&provider) => {
                    let auth_ref = default_cli_auth_ref(&provider, &alias)?;
                    match auth.get(&auth_ref) {
                        Some(auth_entry) if auth_entry.provider != provider => {
                            return Err(Error::Config(format!(
                                "namespace.{provider_key}.{alias} uses implicit auth ref {auth_ref}, which belongs to provider {}, not {provider}",
                                auth_entry.provider
                            )));
                        }
                        Some(_) => {}
                        None => {
                            implicit_auth.push(default_cli_auth(provider.clone(), &auth_ref, &alias)?);
                        }
                    }

                    auth_ref
                }
                None => {
                    return Err(Error::Config(format!(
                        "namespace.{provider_key}.{alias} must declare auth = \"...\""
                    )));
                }
            };

            namespaces.push(
                ResolvedNamespace::new(
                    format!("{provider_key}.{alias}"),
                    provider,
                    namespace.account,
                    auth_ref.as_str(),
                    namespace.default_read,
                    namespace.state_dir,
                )?
                .with_auth_scope_profile(auth_scope_profile)?,
            );
        }
    }

    if namespaces.is_empty() {
        return Err(Error::Config(
            "config must define at least one namespace under [namespace.<provider>.<name>]".into(),
        ));
    }

    Ok((StaticNamespaceStore::new(namespaces), implicit_auth))
}

fn provider_uses_implicit_cli_auth(provider: &ProviderKind) -> bool {
    matches!(provider, ProviderKind::MyChart | ProviderKind::Schwab)
}

fn default_cli_auth_ref(provider: &ProviderKind, alias: &str) -> Result<AuthRef> {
    AuthRef::new(format!("{provider}_{alias}"))
}

fn default_cli_auth(provider: ProviderKind, auth_ref: &AuthRef, alias: &str) -> Result<ResolvedAuth> {
    let (kind, secrets) = match provider {
        ProviderKind::MyChart => (
            AuthKind::MyChartCli,
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
        ),
        ProviderKind::Schwab => (
            AuthKind::SchwabCli,
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
        ),
        _ => {
            return Err(Error::Config(format!(
                "provider {provider} does not support implicit CLI auth"
            )))
        }
    };

    ResolvedAuth::new(auth_ref.as_str(), provider, kind, alias.to_owned(), secrets)
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
    File { path: PathBuf },
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
    #[serde(rename = "mychart_cli")]
    MyChartCli {
        provider: String,
        account: String,
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        portal_base_url: Option<String>,
        #[serde(default)]
        client_id: Option<String>,
        #[serde(default)]
        client_secret: Option<String>,
        #[serde(default)]
        redirect_uri: Option<String>,
        #[serde(default)]
        access_token: Option<String>,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default)]
        username: Option<String>,
    },
    #[serde(rename = "schwab_cli")]
    SchwabCli {
        provider: String,
        account: String,
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        market_data_base_url: Option<String>,
        #[serde(default)]
        authorize_url: Option<String>,
        #[serde(default)]
        token_url: Option<String>,
        #[serde(default)]
        client_id: Option<String>,
        #[serde(default)]
        client_secret: Option<String>,
        #[serde(default)]
        third_party_id: Option<String>,
        #[serde(default)]
        client_channel: Option<String>,
        #[serde(default)]
        client_app_id: Option<String>,
        #[serde(default)]
        client_function_id: Option<String>,
        #[serde(default)]
        resource_version: Option<String>,
        #[serde(default)]
        rrbus_pilot_rollout: Option<String>,
        #[serde(default)]
        redirect_uri: Option<String>,
        #[serde(default)]
        access_token: Option<String>,
        #[serde(default)]
        refresh_token: Option<String>,
    },
}

impl RawAuth {
    fn provider(&self) -> &str {
        match self {
            Self::GitHubCli { provider, .. }
            | Self::GitHubToken { provider, .. }
            | Self::GoogleOAuth { provider, .. }
            | Self::GoogleOAuthFile { provider, .. }
            | Self::MyChartCli { provider, .. }
            | Self::SchwabCli { provider, .. } => provider,
        }
    }

    fn account(&self) -> &str {
        match self {
            Self::GitHubCli { account, .. }
            | Self::GitHubToken { account, .. }
            | Self::GoogleOAuth { account, .. }
            | Self::GoogleOAuthFile { account, .. }
            | Self::MyChartCli { account, .. }
            | Self::SchwabCli { account, .. } => account,
        }
    }

    fn kind(&self) -> AuthKind {
        match self {
            Self::GitHubCli { .. } => AuthKind::GitHubCli,
            Self::GitHubToken { .. } => AuthKind::GitHubToken,
            Self::GoogleOAuth { .. } => AuthKind::GoogleOAuth,
            Self::GoogleOAuthFile { .. } => AuthKind::GoogleOAuthFile,
            Self::MyChartCli { .. } => AuthKind::MyChartCli,
            Self::SchwabCli { .. } => AuthKind::SchwabCli,
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
            Self::MyChartCli {
                base_url,
                portal_base_url,
                client_id,
                client_secret,
                redirect_uri,
                access_token,
                refresh_token,
                username,
                ..
            } => Ok(AuthSecretRefs::MyChartCli {
                base_url: option_secret_ref(base_url.as_deref())?,
                portal_base_url: option_secret_ref(portal_base_url.as_deref())?,
                client_id: option_secret_ref(client_id.as_deref())?,
                client_secret: option_secret_ref(client_secret.as_deref())?,
                redirect_uri: option_secret_ref(redirect_uri.as_deref())?,
                access_token: option_secret_ref(access_token.as_deref())?,
                refresh_token: option_secret_ref(refresh_token.as_deref())?,
                username: option_secret_ref(username.as_deref())?,
            }),
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
                ..
            } => Ok(AuthSecretRefs::SchwabCli {
                base_url: option_secret_ref(base_url.as_deref())?,
                market_data_base_url: option_secret_ref(market_data_base_url.as_deref())?,
                authorize_url: option_secret_ref(authorize_url.as_deref())?,
                token_url: option_secret_ref(token_url.as_deref())?,
                client_id: option_secret_ref(client_id.as_deref())?,
                client_secret: option_secret_ref(client_secret.as_deref())?,
                third_party_id: option_secret_ref(third_party_id.as_deref())?,
                client_channel: option_secret_ref(client_channel.as_deref())?,
                client_app_id: option_secret_ref(client_app_id.as_deref())?,
                client_function_id: option_secret_ref(client_function_id.as_deref())?,
                resource_version: option_secret_ref(resource_version.as_deref())?,
                rrbus_pilot_rollout: option_secret_ref(rrbus_pilot_rollout.as_deref())?,
                redirect_uri: option_secret_ref(redirect_uri.as_deref())?,
                access_token: option_secret_ref(access_token.as_deref())?,
                refresh_token: option_secret_ref(refresh_token.as_deref())?,
            }),
        }
    }
}

fn option_secret_ref(value: Option<&str>) -> Result<Option<SecretRef>> {
    value.map(SecretRef::new).transpose()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNamespace {
    provider: String,
    account: String,
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    default_read: bool,
    #[serde(default)]
    auth_scope_profile: AuthScopeProfile,
    #[serde(default)]
    state_dir: Option<PathBuf>,
}

impl RawConfig {
    fn resolve_paths(&mut self, base_dir: Option<&Path>, home_dir: Option<&Path>) {
        for secret in self.secret.values_mut() {
            if let RawSecret::File { path } = secret {
                *path = resolve_configured_path(path, base_dir, home_dir);
            }
        }

        for provider_namespaces in self.namespace.values_mut() {
            for namespace in provider_namespaces.values_mut() {
                if let Some(state_dir) = namespace.state_dir.as_mut() {
                    *state_dir = resolve_configured_path(state_dir, base_dir, home_dir);
                }
            }
        }
    }
}

fn config_home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn resolve_configured_path(path: &Path, base_dir: Option<&Path>, home_dir: Option<&Path>) -> PathBuf {
    let expanded = expand_home_prefix(path, home_dir);
    if expanded.is_absolute() {
        expanded
    } else if let Some(base_dir) = base_dir {
        base_dir.join(expanded)
    } else {
        expanded
    }
}

fn expand_home_prefix(path: &Path, home_dir: Option<&Path>) -> PathBuf {
    let Some(home_dir) = home_dir else {
        return path.to_path_buf();
    };
    let mut components = path.components();
    match components.next() {
        Some(Component::Normal(component)) if component == "~" => {
            let remainder = components.as_path();
            if remainder.as_os_str().is_empty() {
                home_dir.to_path_buf()
            } else {
                home_dir.join(remainder)
            }
        }
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use switchboard_core::{
        AuthKind, AuthRef, AuthScopeProfile, AuthStore, Error, NamespaceId, NamespaceStore, SecretRef, SecretSource,
        SecretStore, WritePolicy,
    };

    use super::{resolve_configured_path, SwitchboardConfig};

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
    const MISSING_NAMESPACE_AUTH_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/missing-namespace-auth.toml"
    ));
    const MISSING_SECRET_REF_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/missing-secret-ref.toml"
    ));
    const MYCHART_EXPLICIT_DEFAULT_AUTH_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/mychart-explicit-default-auth.toml"
    ));
    const SCHWAB_EXPLICIT_DEFAULT_AUTH_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/schwab-explicit-default-auth.toml"
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
                "google.work",
                "mychart.ucla",
                "schwab.personal"
            ]
        );

        let google_work = namespaces
            .get(&NamespaceId::new("google.work").expect("namespace should parse"))
            .expect("google.work should exist");
        assert_eq!(google_work.auth_ref.as_str(), "google_work");
        assert_eq!(google_work.auth_scope_profile, AuthScopeProfile::WorkspaceAdmin);

        let google_personal = namespaces
            .get(&NamespaceId::new("google.personal").expect("namespace should parse"))
            .expect("google.personal should exist");
        assert_eq!(google_personal.auth_scope_profile, AuthScopeProfile::Standard);

        let mychart_ucla = namespaces
            .get(&NamespaceId::new("mychart.ucla").expect("namespace should parse"))
            .expect("mychart.ucla should exist");
        assert_eq!(mychart_ucla.auth_ref.as_str(), "mychart_ucla");

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

        let mychart_auth = auth
            .get(&AuthRef::new("mychart_ucla").expect("auth ref should parse"))
            .expect("mychart auth should exist");
        assert_eq!(mychart_auth.kind, AuthKind::MyChartCli);
        assert!(mychart_auth.secret_refs().is_empty());

        let schwab_auth = auth
            .get(&AuthRef::new("schwab_personal").expect("auth ref should parse"))
            .expect("schwab auth should exist");
        assert_eq!(schwab_auth.kind, AuthKind::SchwabCli);
        assert_eq!(schwab_auth.secret_refs().len(), 2);

        assert_eq!(secrets.list().len(), 6);
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
    fn rejects_missing_required_namespace_auth() {
        let error = SwitchboardConfig::from_toml_str(MISSING_NAMESPACE_AUTH_CONFIG)
            .expect_err("non-mychart namespaces should still require auth");

        assert_eq!(
            error,
            Error::Config("namespace.google.personal must declare auth = \"...\"".into())
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
    fn mychart_namespace_without_auth_uses_matching_explicit_default_auth_when_present() {
        let config =
            SwitchboardConfig::from_toml_str(MYCHART_EXPLICIT_DEFAULT_AUTH_CONFIG).expect("config should parse");
        let (namespaces, auth, _secrets) = config.into_stores();
        let namespace = namespaces
            .get(&NamespaceId::new("mychart.ucla").expect("namespace should parse"))
            .expect("mychart.ucla should exist");
        let auth_entry = auth
            .get(&AuthRef::new("mychart_ucla").expect("auth ref should parse"))
            .expect("mychart_ucla auth should exist");

        assert_eq!(namespace.auth_ref.as_str(), "mychart_ucla");
        assert_eq!(auth_entry.kind, AuthKind::MyChartCli);
        assert_eq!(auth_entry.account_label, "ucla-overrides");
        assert_eq!(auth_entry.secret_refs().len(), 1);
    }

    #[test]
    fn schwab_namespace_without_auth_uses_matching_explicit_default_auth_when_present() {
        let config =
            SwitchboardConfig::from_toml_str(SCHWAB_EXPLICIT_DEFAULT_AUTH_CONFIG).expect("config should parse");
        let (namespaces, auth, _secrets) = config.into_stores();
        let namespace = namespaces
            .get(&NamespaceId::new("schwab.personal").expect("namespace should parse"))
            .expect("schwab.personal should exist");
        let auth_entry = auth
            .get(&AuthRef::new("schwab_personal").expect("auth ref should parse"))
            .expect("schwab_personal auth should exist");

        assert_eq!(namespace.auth_ref.as_str(), "schwab_personal");
        assert_eq!(auth_entry.kind, AuthKind::SchwabCli);
        assert_eq!(auth_entry.account_label, "jessfraz-overrides");
        assert_eq!(auth_entry.secret_refs().len(), 1);
    }

    #[test]
    fn rejects_empty_config() {
        let error = SwitchboardConfig::from_toml_str(EMPTY_CONFIG).expect_err("empty config should fail");

        assert_eq!(
            error,
            Error::Config("config must define at least one namespace under [namespace.<provider>.<name>]".into())
        );
    }

    #[test]
    fn from_file_resolves_relative_secret_paths_and_state_dirs_against_config_directory() {
        let temp_dir = temp_fixture_directory();
        let config_dir = temp_dir.join("xdg").join("switchboard");
        fs::create_dir_all(config_dir.join("secrets")).expect("config dir should exist");
        let config_path = config_dir.join("config.toml");
        let config_contents = BASIC_CONFIG_TEMPLATE
            .replace("__GOOGLE_PERSONAL_OAUTH_PATH__", "secrets/google-personal-oauth.json")
            .replace("/tmp/switchboard-google-work", "state/google-work")
            .replace("/tmp/switchboard-google-personal", "state/google-personal")
            .replace("/tmp/switchboard-mychart-ucla", "state/mychart-ucla")
            .replace("/tmp/switchboard-schwab-personal", "state/schwab-personal");
        fs::write(&config_path, config_contents).expect("config should write");

        let config = SwitchboardConfig::from_file(&config_path).expect("config should parse from file");
        let (namespaces, _auth, secrets) = config.into_stores();

        let google_personal_secret = secrets
            .get(&SecretRef::new("google_personal_oauth").expect("secret ref should parse"))
            .expect("google personal secret should exist");
        assert_eq!(
            google_personal_secret.source,
            SecretSource::File {
                path: config_dir.join("secrets").join("google-personal-oauth.json"),
            }
        );

        let google_personal = namespaces
            .get(&NamespaceId::new("google.personal").expect("namespace should parse"))
            .expect("google.personal should exist");
        assert_eq!(
            google_personal.state_dir,
            Some(config_dir.join("state").join("google-personal"))
        );

        let mychart_ucla = namespaces
            .get(&NamespaceId::new("mychart.ucla").expect("namespace should parse"))
            .expect("mychart.ucla should exist");
        assert_eq!(
            mychart_ucla.state_dir,
            Some(config_dir.join("state").join("mychart-ucla"))
        );

        let schwab_personal = namespaces
            .get(&NamespaceId::new("schwab.personal").expect("namespace should parse"))
            .expect("schwab.personal should exist");
        assert_eq!(
            schwab_personal.state_dir,
            Some(config_dir.join("state").join("schwab-personal"))
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn configured_paths_expand_home_and_then_resolve_relative_paths() {
        let home_dir = Path::new("/home/alice");
        let base_dir = Path::new("/configs/switchboard");

        assert_eq!(
            resolve_configured_path(Path::new("~/state/google"), Some(base_dir), Some(home_dir)),
            PathBuf::from("/home/alice/state/google")
        );
        assert_eq!(
            resolve_configured_path(Path::new("state/google"), Some(base_dir), Some(home_dir)),
            PathBuf::from("/configs/switchboard/state/google")
        );
    }

    fn render_basic_config(google_personal_oauth_path: &str) -> String {
        BASIC_CONFIG_TEMPLATE.replace("__GOOGLE_PERSONAL_OAUTH_PATH__", google_personal_oauth_path)
    }

    fn render_allow_writes_config(google_personal_oauth_path: &str) -> String {
        ALLOW_WRITES_CONFIG_TEMPLATE.replace("__GOOGLE_PERSONAL_OAUTH_PATH__", google_personal_oauth_path)
    }

    fn temp_fixture_directory() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("switchboard-store-config-test-{}-{stamp}", std::process::id()))
    }
}
