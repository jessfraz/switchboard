use std::{collections::BTreeMap, sync::Mutex};

use switchboard_core::{
    AuditEvent, AuditSink, NamespaceId, NamespaceStore, PlannedAction, PolicyDecision, PolicyEngine, ProviderKind,
    ResolvedNamespace, Result, ToolKind,
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

    pub fn bootstrap() -> Result<Self> {
        Ok(Self::new([
            ResolvedNamespace::new("github.personal", ProviderKind::GitHub, "jessfraz", true)?,
            ResolvedNamespace::new("google.work", ProviderKind::GoogleWorkspace, "jess@company.com", true)?,
            ResolvedNamespace::new(
                "google.personal",
                ProviderKind::GoogleWorkspace,
                "jess@example.com",
                false,
            )?,
        ]))
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
