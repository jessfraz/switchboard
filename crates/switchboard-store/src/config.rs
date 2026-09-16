use std::{
    collections::BTreeMap,
    env, fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use switchboard_core::{
    AuthRef, AuthScopeProfile, AuthSecretRefs, AuthStore, Error, ProviderKind, ResolvedAuth, ResolvedNamespace,
    ResolvedSecret, Result, SecretRef, SecretSource, SecretStore, WritePolicy,
};

use crate::{
    resolve_operation_store_path, ConfiguredPolicyEngine, OnePasswordConfig, StaticAuthStore, StaticNamespaceStore,
    StaticSecretStore,
};

#[derive(Clone, Debug)]
pub struct SwitchboardConfig {
    pub one_password: OnePasswordConfig,
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

        Self::from_source(
            &source,
            &format!("switchboard config at {}", path.display()),
            Some(path),
        )
    }

    pub fn from_toml_str(source: &str) -> Result<Self> {
        Self::from_source(source, "switchboard config", None)
    }

    pub fn into_stores(self) -> (StaticNamespaceStore, StaticAuthStore, StaticSecretStore) {
        (self.namespaces, self.auth, self.secrets)
    }

    pub fn policy_engine(&self) -> ConfiguredPolicyEngine {
        ConfiguredPolicyEngine::new(self.write_policy)
    }

    fn from_source(source: &str, source_label: &str, config_path: Option<&Path>) -> Result<Self> {
        let mut config: RawConfig = toml::from_str(source)
            .map_err(|error| Error::Config(format!("failed to parse {source_label}: {error}")))?;
        if config.one_password.timeout_seconds == 0 {
            return Err(Error::Config(
                "one_password.timeout_seconds must be greater than zero".into(),
            ));
        }
        config.resolve_paths(config_path.and_then(Path::parent), config_home_dir().as_deref());
        let state_db = resolve_operation_store_path(config_path.unwrap_or_else(|| Path::new("switchboard.toml")));
        let state_root = state_db.parent().unwrap_or_else(|| Path::new("."));
        let secrets = build_secret_store(config.secret)?;
        let explicit_auth = build_auth_store(config.auth, &secrets)?;
        let (namespaces, implicit_auth) = build_namespace_store(config.namespace, &explicit_auth, state_root)?;
        let auth = StaticAuthStore::new(explicit_auth.list().into_iter().chain(implicit_auth));
        let write_policy = config.policy.write;

        Ok(Self {
            one_password: config.one_password,
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
        let secret_refs = raw.secret_refs()?;
        let kind = secret_refs.kind();

        if kind.provider() != provider {
            return Err(Error::Config(format!(
                "auth.{auth_ref} declares auth kind {kind}, which belongs to provider {}, not {provider}",
                kind.provider()
            )));
        }

        for secret_ref in secret_refs.secret_refs() {
            if secrets.get(secret_ref).is_none() {
                return Err(Error::Config(format!(
                    "auth.{auth_ref} references missing secret ref {secret_ref}"
                )));
            }
        }

        auth_entries.push(ResolvedAuth::new(auth_ref, raw.account().to_owned(), secret_refs)?);
    }

    Ok(StaticAuthStore::new(auth_entries))
}

fn build_namespace_store(
    raw_namespaces: BTreeMap<String, BTreeMap<String, RawNamespace>>,
    auth: &StaticAuthStore,
    state_root: &Path,
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

                    if auth_entry.provider() != provider {
                        return Err(Error::Config(format!(
                            "namespace.{provider_key}.{alias} uses auth ref {auth_ref}, which belongs to provider {}, not {provider}",
                            auth_entry.provider()
                        )));
                    }

                    auth_ref
                }
                None if provider_uses_implicit_cli_auth(&provider) => {
                    let auth_ref = default_cli_auth_ref(&provider, &alias)?;
                    match auth.get(&auth_ref) {
                        Some(auth_entry) if auth_entry.provider() != provider => {
                            return Err(Error::Config(format!(
                                "namespace.{provider_key}.{alias} uses implicit auth ref {auth_ref}, which belongs to provider {}, not {provider}",
                                auth_entry.provider()
                            )));
                        }
                        Some(_) => {}
                        None => {
                            let account = if provider == ProviderKind::GoogleWorkspace {
                                &namespace.account
                            } else {
                                &alias
                            };
                            implicit_auth.push(default_cli_auth(provider.clone(), &auth_ref, account)?);
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

            let state_dir = namespace.state_dir.or_else(|| {
                (provider == ProviderKind::GoogleWorkspace).then(|| {
                    state_root
                        .join("namespaces")
                        .join(namespace_state_name(&provider, &alias))
                })
            });
            namespaces.push(
                ResolvedNamespace::new(
                    format!("{provider_key}.{alias}"),
                    provider,
                    namespace.account,
                    auth_ref.as_str(),
                    namespace.default_read,
                    state_dir,
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
    matches!(
        provider,
        ProviderKind::GoogleWorkspace | ProviderKind::MyChart | ProviderKind::Schwab
    )
}

fn namespace_state_name(provider: &ProviderKind, alias: &str) -> String {
    let mut name = format!("{provider}.");
    // Escape bytes outside lowercase ASCII so aliases stay distinct on case-insensitive
    // filesystems, cannot introduce path components, and cannot end with a dot or space.
    for byte in alias.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_') {
            name.push(char::from(byte));
        } else {
            name.push('%');
            const HEX: &[u8; 16] = b"0123456789abcdef";
            name.push(char::from(HEX[usize::from(byte >> 4)]));
            name.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    name
}

fn default_cli_auth_ref(provider: &ProviderKind, alias: &str) -> Result<AuthRef> {
    AuthRef::new(format!("{provider}_{alias}"))
}

fn default_cli_auth(provider: ProviderKind, auth_ref: &AuthRef, account: &str) -> Result<ResolvedAuth> {
    let secrets = match provider {
        ProviderKind::GoogleWorkspace => AuthSecretRefs::GoogleCli,
        ProviderKind::MyChart => AuthSecretRefs::MyChartCli {
            base_url: None,
            portal_base_url: None,
            client_id: None,
            client_secret: None,
            redirect_uri: None,
            access_token: None,
            refresh_token: None,
            username: None,
        },
        ProviderKind::Schwab => AuthSecretRefs::SchwabCli {
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
        _ => {
            return Err(Error::Config(format!(
                "provider {provider} does not support implicit CLI auth"
            )))
        }
    };

    ResolvedAuth::new(auth_ref.as_str(), account.to_owned(), secrets)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    one_password: OnePasswordConfig,
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
    #[serde(rename = "phone_cli")]
    PhoneCli {
        provider: String,
        account: String,
        #[serde(default)]
        api_key: Option<String>,
        #[serde(default)]
        api_secret: Option<String>,
        #[serde(default)]
        model_api_key: Option<String>,
    },
    #[serde(rename = "gh_cli")]
    GitHubCli { provider: String, account: String },
    #[serde(rename = "github_token")]
    GitHubToken {
        provider: String,
        account: String,
        token: String,
    },
    #[serde(rename = "google_cli")]
    GoogleCli { provider: String, account: String },
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
            Self::PhoneCli { provider, .. } => provider,
            Self::GitHubCli { provider, .. }
            | Self::GitHubToken { provider, .. }
            | Self::GoogleCli { provider, .. }
            | Self::GoogleOAuth { provider, .. }
            | Self::GoogleOAuthFile { provider, .. }
            | Self::MyChartCli { provider, .. }
            | Self::SchwabCli { provider, .. } => provider,
        }
    }

    fn account(&self) -> &str {
        match self {
            Self::PhoneCli { account, .. } => account,
            Self::GitHubCli { account, .. }
            | Self::GitHubToken { account, .. }
            | Self::GoogleCli { account, .. }
            | Self::GoogleOAuth { account, .. }
            | Self::GoogleOAuthFile { account, .. }
            | Self::MyChartCli { account, .. }
            | Self::SchwabCli { account, .. } => account,
        }
    }

    fn secret_refs(&self) -> Result<AuthSecretRefs> {
        match self {
            Self::PhoneCli {
                api_key,
                api_secret,
                model_api_key,
                ..
            } => Ok(AuthSecretRefs::PhoneCli {
                api_key: option_secret_ref(api_key.as_deref())?,
                api_secret: option_secret_ref(api_secret.as_deref())?,
                model_api_key: option_secret_ref(model_api_key.as_deref())?,
            }),
            Self::GitHubCli { .. } => Ok(AuthSecretRefs::GitHubCli),
            Self::GoogleCli { .. } => Ok(AuthSecretRefs::GoogleCli),
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
    fn phone_namespace_resolves_explicit_auth_and_scoped_state() {
        let config = SwitchboardConfig::from_toml_str(
            r#"
[secret.phone_key]
kind = "env"
name = "PHONE_TEST_API_KEY"
[secret.phone_secret]
kind = "env"
name = "PHONE_TEST_API_SECRET"
[secret.phone_model_key]
kind = "env"
name = "PHONE_TEST_MODEL_API_KEY"
[auth.phone_personal]
provider = "phone"
kind = "phone_cli"
account = "personal"
api_key = "phone_key"
api_secret = "phone_secret"
model_api_key = "phone_model_key"
[namespace.phone.personal]
provider = "phone"
account = "personal"
auth = "phone_personal"
state_dir = "/tmp/phone-personal"
"#,
        )
        .expect("phone configuration parses");
        let (namespaces, auth, _) = config.into_stores();
        let namespace = namespaces
            .get(&NamespaceId::new("phone.personal").expect("namespace ID"))
            .expect("namespace");
        assert_eq!(
            namespace.state_dir,
            Some(std::path::PathBuf::from("/tmp/phone-personal"))
        );
        let credentials = auth.get(&namespace.auth_ref).expect("auth");
        assert_eq!(credentials.kind(), AuthKind::PhoneCli);
        assert_eq!(
            credentials.secrets(),
            &switchboard_core::AuthSecretRefs::PhoneCli {
                api_key: Some(SecretRef::new("phone_key").expect("key reference")),
                api_secret: Some(SecretRef::new("phone_secret").expect("secret reference")),
                model_api_key: Some(SecretRef::new("phone_model_key").expect("model reference")),
            }
        );
    }

    #[test]
    fn google_namespaces_without_auth_get_isolated_local_credentials() {
        let config = SwitchboardConfig::from_toml_str(
            r#"
[namespace.google.work]
provider = "google"
account = "work@example.com"

[namespace.google.personal]
provider = "google"
account = "personal@example.com"
"#,
        )
        .expect("Google namespaces should work without secret configuration");
        let (namespaces, auth, secrets) = config.into_stores();
        let mut paths = Vec::new();
        for (alias, account) in [("work", "work@example.com"), ("personal", "personal@example.com")] {
            let namespace = namespaces
                .get(&NamespaceId::new(format!("google.{alias}")).expect("namespace ID should be valid"))
                .expect("configured namespace should exist");
            let credentials = auth.get(&namespace.auth_ref).expect("implicit auth should exist");
            assert_eq!(credentials.kind().to_string(), "google_cli");
            assert_eq!(credentials.account_label(), account);
            assert!(credentials.secret_refs().is_empty());
            let state_dir = namespace.state_dir.expect("Google state directory should be derived");
            assert!(state_dir.ends_with(Path::new("namespaces").join(format!("google.{alias}"))));
            paths.push(state_dir);
        }
        assert_ne!(paths[0], paths[1]);
        assert!(secrets.list().is_empty());
    }

    #[test]
    fn google_namespace_default_state_stays_inside_root_for_unusual_aliases() {
        let config = SwitchboardConfig::from_toml_str(
            r#"
[namespace.google."../personal"]
provider = "google"
account = "one@example.com"
[namespace.google."%2e%2e%2fpersonal"]
provider = "google"
account = "two@example.com"
[namespace.google."Personal"]
provider = "google"
account = "three@example.com"
[namespace.google."personal"]
provider = "google"
account = "four@example.com"
[namespace.google."trailing."]
provider = "google"
account = "five@example.com"
[namespace.google.'C:\personal']
provider = "google"
account = "six@example.com"
[namespace.google."é"]
provider = "google"
account = "seven@example.com"
[namespace.google."e\u0301"]
provider = "google"
account = "eight@example.com"
"#,
        )
        .expect("unusual namespace aliases should be encoded safely");
        let (namespaces, _, _) = config.into_stores();
        let mut directory_names = std::collections::BTreeSet::new();
        for namespace in namespaces.list() {
            let path = namespace.state_dir.expect("Google state directory should be derived");
            assert_eq!(
                path.parent()
                    .expect("state path should have a parent")
                    .file_name()
                    .expect("parent should have a name"),
                "namespaces"
            );
            let name = path
                .file_name()
                .expect("state path should have a name")
                .to_str()
                .expect("encoded state name should be ASCII");
            assert!(!name.ends_with('.'));
            assert!(!name.contains(['/', '\\']));
            assert!(directory_names.insert(name.to_ascii_lowercase()));
        }
    }

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
        assert_eq!(google_work_auth.kind(), AuthKind::GoogleOAuth);
        assert_eq!(google_work_auth.secret_refs().len(), 2);

        let google_personal_auth = auth
            .get(&AuthRef::new("google_personal").expect("auth ref should parse"))
            .expect("google personal auth should exist");
        assert_eq!(google_personal_auth.kind(), AuthKind::GoogleOAuthFile);
        assert_eq!(google_personal_auth.secret_refs().len(), 1);

        let github_token_auth = auth
            .get(&AuthRef::new("github_personal_token").expect("auth ref should parse"))
            .expect("github token auth should exist");
        assert_eq!(github_token_auth.kind(), AuthKind::GitHubToken);
        assert_eq!(github_token_auth.secret_refs().len(), 1);

        let mychart_auth = auth
            .get(&AuthRef::new("mychart_ucla").expect("auth ref should parse"))
            .expect("mychart auth should exist");
        assert_eq!(mychart_auth.kind(), AuthKind::MyChartCli);
        assert!(mychart_auth.secret_refs().is_empty());

        let schwab_auth = auth
            .get(&AuthRef::new("schwab_personal").expect("auth ref should parse"))
            .expect("schwab auth should exist");
        assert_eq!(schwab_auth.kind(), AuthKind::SchwabCli);
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
    fn google_namespace_without_auth_uses_matching_explicit_default_auth_when_present() {
        let config = SwitchboardConfig::from_toml_str(MISSING_NAMESPACE_AUTH_CONFIG)
            .expect("matching explicit Google auth should load");
        let (namespaces, auth, _) = config.into_stores();
        let namespace = namespaces
            .get(&NamespaceId::new("google.personal").expect("namespace ID should be valid"))
            .expect("configured namespace should exist");
        let credentials = auth.get(&namespace.auth_ref).expect("matching auth should exist");
        assert_eq!(namespace.auth_ref.as_str(), "google_personal");
        assert_eq!(credentials.kind(), AuthKind::GoogleOAuthFile);
        assert_eq!(credentials.secret_refs().len(), 1);
    }

    #[test]
    fn github_namespace_still_requires_auth() {
        let error = SwitchboardConfig::from_toml_str(
            "[namespace.github.personal]\nprovider = \"github\"\naccount = \"example\"",
        )
        .expect_err("GitHub namespace should require explicit auth");
        assert_eq!(
            error,
            Error::Config("namespace.github.personal must declare auth = \"...\"".into())
        );
    }

    #[test]
    fn explicit_google_cli_auth_preserves_configured_namespace_state() {
        let config = SwitchboardConfig::from_toml_str(
            r#"
[auth.existing_login]
provider = "google"
kind = "google_cli"
account = "personal@example.com"
[namespace.google.personal]
provider = "google"
account = "personal@example.com"
auth = "existing_login"
state_dir = "/tmp/existing-gws-login"
"#,
        )
        .expect("explicit Google CLI auth should load");
        let (namespaces, auth, _) = config.into_stores();
        let namespace = namespaces
            .get(&NamespaceId::new("google.personal").expect("namespace ID should be valid"))
            .expect("configured namespace should exist");
        let credentials = auth.get(&namespace.auth_ref).expect("explicit auth should exist");
        assert_eq!(credentials.kind(), AuthKind::GoogleCli);
        assert_eq!(namespace.auth_ref.as_str(), "existing_login");
        assert_eq!(namespace.state_dir, Some(PathBuf::from("/tmp/existing-gws-login")));
        assert!(credentials.secret_refs().is_empty());
    }

    #[test]
    fn google_default_state_uses_the_config_operation_store_root() {
        let directory = temp_fixture_directory();
        fs::create_dir_all(&directory).expect("test directory should be created");
        for name in ["config.toml", "switchboard.toml"] {
            let config_path = directory.join(name);
            fs::write(
                &config_path,
                "[namespace.google.personal]\nprovider = \"google\"\naccount = \"personal@example.com\"",
            )
            .expect("test config should be written");
            let config = SwitchboardConfig::from_file(&config_path).expect("Google config should load from disk");
            let (namespaces, _, _) = config.into_stores();
            let namespace = namespaces
                .get(&NamespaceId::new("google.personal").expect("namespace ID should be valid"))
                .expect("configured namespace should exist");
            let state_db = crate::resolve_operation_store_path(&config_path);
            assert_eq!(
                namespace.state_dir,
                Some(
                    state_db
                        .parent()
                        .expect("operation store should have a parent")
                        .join("namespaces/google.personal")
                )
            );
        }
        fs::remove_dir_all(directory).expect("test directory should be removed");
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
        assert_eq!(auth_entry.kind(), AuthKind::MyChartCli);
        assert_eq!(auth_entry.account_label(), "ucla-overrides");
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
        assert_eq!(auth_entry.kind(), AuthKind::SchwabCli);
        assert_eq!(auth_entry.account_label(), "jessfraz-overrides");
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
