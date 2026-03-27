use std::{collections::BTreeMap, sync::Mutex};

use switchboard_core::{
    AuditEvent, AuditSink, AuthRef, AuthStore, NamespaceId, NamespaceStore, ResolvedAuth, ResolvedNamespace,
    ResolvedSecret, Result, SecretRef, SecretStore,
};

#[derive(Clone, Debug)]
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
}

impl NamespaceStore for StaticNamespaceStore {
    fn get(&self, id: &NamespaceId) -> Option<ResolvedNamespace> {
        self.namespaces.get(id).cloned()
    }

    fn list(&self) -> Vec<ResolvedNamespace> {
        self.namespaces.values().cloned().collect()
    }
}

#[derive(Clone, Debug)]
pub struct StaticAuthStore {
    auth: BTreeMap<AuthRef, ResolvedAuth>,
}

impl StaticAuthStore {
    pub fn new(auth: impl IntoIterator<Item = ResolvedAuth>) -> Self {
        let auth = auth.into_iter().map(|entry| (entry.id.clone(), entry)).collect();

        Self { auth }
    }
}

impl AuthStore for StaticAuthStore {
    fn get(&self, id: &AuthRef) -> Option<ResolvedAuth> {
        self.auth.get(id).cloned()
    }

    fn list(&self) -> Vec<ResolvedAuth> {
        self.auth.values().cloned().collect()
    }
}

#[derive(Clone, Debug)]
pub struct StaticSecretStore {
    secrets: BTreeMap<SecretRef, ResolvedSecret>,
}

impl StaticSecretStore {
    pub fn new(secrets: impl IntoIterator<Item = ResolvedSecret>) -> Self {
        let secrets = secrets.into_iter().map(|secret| (secret.id.clone(), secret)).collect();

        Self { secrets }
    }
}

impl SecretStore for StaticSecretStore {
    fn get(&self, id: &SecretRef) -> Option<ResolvedSecret> {
        self.secrets.get(id).cloned()
    }

    fn list(&self) -> Vec<ResolvedSecret> {
        self.secrets.values().cloned().collect()
    }
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
