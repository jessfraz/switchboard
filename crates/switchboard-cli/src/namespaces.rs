use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Result};
use serde::Serialize;
use switchboard_core::{
    AuthKind, AuthScopeProfile, AuthStore, BackendKind, NamespaceId, NamespaceStore, ProviderKind, ResolvedNamespace,
    SecretSource, SecretStore, WritePolicy,
};
use switchboard_providers::{provider_cli_metadata, CliTransportMetadata};
use switchboard_store::SwitchboardConfig;

use crate::{args::NamespaceSubcommand, output::render_json, resolve_config_selection, ConfigSelection, ConfigSource};

// Explicit fields, not flattened ResolvedNamespace/ResolvedAuth serialization:
// future runtime fields must not silently become discovery output.
#[derive(Serialize)]
struct NamespaceSummary {
    id: NamespaceId,
    provider: ProviderKind,
    account_label: String,
    auth_ref: switchboard_core::AuthRef,
    default_read: bool,
    #[serde(skip_serializing_if = "is_standard")]
    auth_scope_profile: AuthScopeProfile,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_dir: Option<PathBuf>,
    auth_kind: AuthKind,
    auth_account_label: String,
}

fn is_standard(profile: &AuthScopeProfile) -> bool {
    *profile == AuthScopeProfile::Standard
}

impl NamespaceSummary {
    fn new(namespace: ResolvedNamespace, auth: &switchboard_core::ResolvedAuth) -> Self {
        Self {
            id: namespace.id,
            provider: namespace.provider,
            account_label: namespace.account_label,
            auth_ref: namespace.auth_ref,
            default_read: namespace.default_read,
            auth_scope_profile: namespace.auth_scope_profile,
            state_dir: namespace.state_dir,
            auth_kind: auth.kind(),
            auth_account_label: auth.account_label().to_owned(),
        }
    }

    fn human(&self) -> String {
        format!(
            "{}: provider={} account={} auth={} default_read={}\n",
            self.id, self.provider, self.auth_account_label, self.auth_kind, self.default_read
        )
    }
}

#[derive(Serialize)]
struct NamespaceList {
    namespaces: Vec<NamespaceSummary>,
    config: ConfigSelection,
}

#[derive(Serialize)]
struct NamespaceDetail {
    #[serde(flatten)]
    identity: NamespaceSummary,
    // Standard is included here, even though legacy list rows omit it.
    auth: AuthMetadata,
    write_policy: WritePolicy,
    transport: TransportMetadata,
}

#[derive(Serialize)]
struct AuthMetadata {
    scope_profile: AuthScopeProfile,
    credential_source_kinds: BTreeSet<&'static str>,
    one_password: Option<OnePasswordMetadata>,
}

#[derive(Serialize)]
struct OnePasswordMetadata {
    configured_auth_mode: &'static str,
    timeout_seconds: u64,
    // Profiles are named authorities, never item locators or bootstrap paths.
    service_account_profiles: BTreeSet<String>,
    unprofiled_sources: bool,
}

#[derive(Serialize)]
struct TransportMetadata {
    backend: BackendKind,
    cli: Option<CliTransportMetadata>,
}

#[derive(Serialize)]
struct NamespaceShow {
    namespace: NamespaceDetail,
    config: ConfigSelection,
}

/// Validate configuration without echoing TOML source or secret locator values.
/// This is the only load needed for discovery: no runtime stores or resolver.
pub(crate) fn load_config(path: &Path) -> Result<SwitchboardConfig> {
    SwitchboardConfig::from_file(path).map_err(|_| {
        anyhow!(switchboard_core::Error::Config(
            "Namespace configuration could not be read or validated".into()
        ))
    })
}

pub(crate) fn run(
    config_path: Option<&Path>,
    source: Option<ConfigSource>,
    command: NamespaceSubcommand,
    full: bool,
) -> Result<String> {
    // Validate arguments before loading; no fuzzy provider or account matching.
    let provider = match &command {
        NamespaceSubcommand::List(arguments) => arguments
            .provider
            .as_ref()
            .map(|provider| {
                ProviderKind::from_identifier(provider)
                    .filter(|kind| kind.to_string() == *provider)
                    .ok_or_else(|| anyhow!("unknown provider identifier"))
            })
            .transpose()?,
        NamespaceSubcommand::Show(_) => None,
    };
    let config = resolve_config_selection(config_path, source.unwrap_or(ConfigSource::Flag))?;
    let loaded = load_config(&config.path)?;
    let write_policy = loaded.policy_engine().write_policy();
    let one_password = loaded.one_password.clone();
    let (namespaces, auth, secrets) = loaded.into_stores();
    let identities = namespaces.list();
    match command {
        NamespaceSubcommand::List(arguments) => {
            let mut matches = Vec::new();
            for namespace in identities {
                let auth = auth
                    .get(&namespace.auth_ref)
                    .ok_or_else(|| anyhow!("namespace auth is missing"))?;
                if provider
                    .as_ref()
                    .is_some_and(|provider| *provider != namespace.provider)
                    || arguments
                        .account
                        .as_ref()
                        .is_some_and(|account| account != auth.account_label())
                    || (arguments.default_read && !namespace.default_read)
                {
                    continue;
                }
                matches.push(NamespaceSummary::new(namespace, &auth));
            }
            if arguments.json {
                render_json(
                    &NamespaceList {
                        namespaces: matches,
                        config,
                    },
                    full,
                )
            } else {
                let mut text = format!(
                    "{:<24} {:<10} {:<16} {:<30} {}\n",
                    "NAMESPACE", "PROVIDER", "AUTH", "ACCOUNT", "DEFAULT_READ"
                );
                for namespace in matches {
                    text.push_str(&format!(
                        "{:<24} {:<10} {:<16} {:<30} {}\n",
                        namespace.id.to_string(),
                        namespace.provider.to_string(),
                        namespace.auth_kind.to_string(),
                        namespace.auth_account_label,
                        namespace.default_read
                    ));
                }
                text.push_str("Configured identities; credentials not checked.\n");
                Ok(text)
            }
        }
        NamespaceSubcommand::Show(arguments) => {
            let id = NamespaceId::new(arguments.namespace)?;
            let namespace = identities
                .into_iter()
                .find(|namespace| namespace.id == id)
                .ok_or_else(|| anyhow!(switchboard_core::Error::UnknownNamespace(id.to_string())))?;
            let auth = auth
                .get(&namespace.auth_ref)
                .ok_or_else(|| anyhow!("namespace auth is missing"))?;
            let mut source_kinds = BTreeSet::new();
            let mut profiles = BTreeSet::new();
            let mut unprofiled_sources = false;
            for reference in auth.secret_refs() {
                let secret = secrets
                    .get(reference)
                    .ok_or_else(|| anyhow!("auth credential source is missing"))?;
                source_kinds.insert(match secret.source {
                    SecretSource::Env { .. } => "env",
                    SecretSource::File { .. } => "file",
                    SecretSource::OnePasswordItem { auth_profile, .. } => {
                        match auth_profile {
                            Some(profile) => {
                                profiles.insert(profile);
                            }
                            None => unprofiled_sources = true,
                        }
                        "onepassword_item"
                    }
                });
            }
            let password_metadata = source_kinds
                .contains("onepassword_item")
                .then_some(OnePasswordMetadata {
                    configured_auth_mode: one_password.auth_mode.as_str(),
                    timeout_seconds: one_password.timeout_seconds,
                    service_account_profiles: profiles,
                    unprofiled_sources,
                });
            let local = namespace.provider == ProviderKind::Messages;
            let transport = TransportMetadata {
                backend: if local { BackendKind::Local } else { BackendKind::Cli },
                cli: if local {
                    None
                } else {
                    Some(provider_cli_metadata(namespace.provider.clone())?)
                },
            };
            let detail = NamespaceDetail {
                auth: AuthMetadata {
                    scope_profile: namespace.auth_scope_profile,
                    credential_source_kinds: source_kinds,
                    one_password: password_metadata,
                },
                identity: NamespaceSummary::new(namespace, &auth),
                write_policy,
                transport,
            };
            if arguments.json {
                render_json(
                    &NamespaceShow {
                        namespace: detail,
                        config,
                    },
                    full,
                )
            } else {
                let mut text = detail.identity.human();
                text.push_str(&format!("Config: {} ({:?})\nNamespace label: {}\nAuth reference: {}\nScope profile: {:?}\nWrite policy (global): {:?}\nState directory: {}\nTransport: {:?}", config.path.display(), config.selected_by, detail.identity.account_label, detail.identity.auth_ref, detail.auth.scope_profile, detail.write_policy, detail.identity.state_dir.as_deref().map(|path| path.display().to_string()).unwrap_or_else(|| "not configured".into()), detail.transport.backend));
                if let Some(cli) = &detail.transport.cli {
                    text.push_str(&format!(
                        " via {} (override: {})",
                        cli.program,
                        cli.override_variable.as_deref().unwrap_or("none")
                    ));
                }
                text.push_str(&format!(
                    "\nCredential sources: {:?}\n",
                    detail.auth.credential_source_kinds
                ));
                if let Some(password) = &detail.auth.one_password {
                    text.push_str(&format!("1Password: configured mode={}, timeout={}s, service-account profiles={:?}, unprofiled sources={}\nNamed profiles use scoped service-account credentials without desktop fallback.\n", password.configured_auth_mode, password.timeout_seconds, password.service_account_profiles, password.unprofiled_sources));
                }
                text.push_str("Configuration only; CLI availability, credentials, identity, and granted permissions were not checked.\n");
                Ok(text)
            }
        }
    }
}
