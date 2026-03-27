use std::{collections::BTreeMap, fs, path::Path, sync::Mutex};

use serde::Deserialize;
use switchboard_core::{
    AuditEvent, AuditSink, Error, NamespaceId, NamespaceStore, PlannedAction, PolicyDecision, PolicyEngine,
    ProviderKind, ResolvedNamespace, Result, ToolKind,
};

#[derive(Debug)]
pub struct StaticNamespaceStore {
    namespaces: BTreeMap<NamespaceId, ResolvedNamespace>,
}

impl StaticNamespaceStore {
    pub fn new(namespaces: impl IntoIterator<Item = ResolvedNamespace>) -> Self {
        let namespaces = namespaces
            .into_iter()
            .map(|namespace| (namespace.id.clone(), namespace))
            .collect();

        Self { namespaces }
    }

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

    pub fn bootstrap() -> Result<Self> {
        Ok(Self::new([
            ResolvedNamespace::new("github.personal", ProviderKind::GitHub, "jessfraz", "gh", true)?,
            ResolvedNamespace::new(
                "google.work",
                ProviderKind::GoogleWorkspace,
                "jess@company.com",
                "oauth",
                true,
            )?,
            ResolvedNamespace::new(
                "google.personal",
                ProviderKind::GoogleWorkspace,
                "jess@example.com",
                "oauth",
                false,
            )?,
        ]))
    }

    fn from_source(source: &str, source_label: &str) -> Result<Self> {
        let config: RawConfig = toml::from_str(source)
            .map_err(|error| Error::Config(format!("failed to parse {source_label}: {error}")))?;
        let mut namespaces = Vec::new();

        for (provider_key, aliases) in config.namespace {
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

                namespaces.push(ResolvedNamespace::new(
                    format!("{provider_key}.{alias}"),
                    provider,
                    namespace.account,
                    namespace.auth,
                    namespace.default_read,
                )?);
            }
        }

        if namespaces.is_empty() {
            return Err(Error::Config(
                "config must define at least one namespace under [namespace.<provider>.<name>]".into(),
            ));
        }

        Ok(Self::new(namespaces))
    }
}

impl NamespaceStore for StaticNamespaceStore {
    fn get(&self, id: &NamespaceId) -> Option<ResolvedNamespace> {
        self.namespaces.get(id).cloned()
    }

    fn list(&self) -> Vec<ResolvedNamespace> {
        self.namespaces.values().cloned().collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    namespace: BTreeMap<String, BTreeMap<String, RawNamespace>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNamespace {
    provider: String,
    account: String,
    auth: String,
    #[serde(default)]
    default_read: bool,
}

#[derive(Default, Debug)]
pub struct MemoryAuditSink {
    events: Mutex<Vec<AuditEvent>>,
}

impl MemoryAuditSink {
    pub fn snapshot(&self) -> Vec<AuditEvent> {
        match self.events.lock() {
            Ok(events) => events.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl AuditSink for MemoryAuditSink {
    fn record(&self, event: &AuditEvent) -> Result<()> {
        match self.events.lock() {
            Ok(mut events) => events.push(event.clone()),
            Err(poisoned) => poisoned.into_inner().push(event.clone()),
        }

        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct DefaultPolicyEngine;

impl PolicyEngine for DefaultPolicyEngine {
    fn evaluate(&self, _namespace: &ResolvedNamespace, plan: &PlannedAction) -> PolicyDecision {
        match plan.kind {
            ToolKind::Read => PolicyDecision::Allow,
            ToolKind::Write => PolicyDecision::RequireApproval {
                reason: format!("{} stays draft-first until approval UX is wired", plan.tool),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use switchboard_core::{NamespaceId, NamespaceStore};

    use super::StaticNamespaceStore;

    const BASIC_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/basic.toml"
    ));
    const UNKNOWN_PROVIDER_CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config/unknown-provider.toml"
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

    #[test]
    fn parses_readme_shape_config_into_namespaces() {
        let store = StaticNamespaceStore::from_toml_str(BASIC_CONFIG).expect("config should parse");
        let ids = store
            .list()
            .into_iter()
            .map(|namespace| namespace.id.to_string())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["github.personal", "google.personal", "google.work"]);

        let google_work = store
            .get(&NamespaceId::new("google.work").expect("namespace id should parse"))
            .expect("google.work should exist");
        assert_eq!(google_work.account_label, "jess@company.com");
        assert_eq!(google_work.auth_ref.as_str(), "oauth");
        assert!(google_work.default_read);
    }

    #[test]
    fn rejects_unknown_provider_namespaces() {
        let error =
            StaticNamespaceStore::from_toml_str(UNKNOWN_PROVIDER_CONFIG).expect_err("unknown providers should fail");

        assert!(error.to_string().contains("unknown provider"));
    }

    #[test]
    fn rejects_provider_mismatches_between_path_and_entry() {
        let error =
            StaticNamespaceStore::from_toml_str(PROVIDER_MISMATCH_CONFIG).expect_err("provider mismatch should fail");

        assert!(error
            .to_string()
            .contains("namespace.github.personal declares provider google"));
    }

    #[test]
    fn rejects_empty_auth_references() {
        let error = StaticNamespaceStore::from_toml_str(EMPTY_AUTH_CONFIG).expect_err("empty auth refs should fail");

        assert!(error.to_string().contains("auth reference cannot be empty"));
    }

    #[test]
    fn rejects_configs_without_namespaces() {
        let error = StaticNamespaceStore::from_toml_str(EMPTY_CONFIG).expect_err("empty config should fail");

        assert!(error.to_string().contains("must define at least one namespace"));
    }
}
