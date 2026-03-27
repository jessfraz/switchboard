use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

use switchboard_core::{
    AuditEvent, AuditEventId, AuditStore, AuthRef, AuthStore, Error, NamespaceId, NamespaceStore, OperationId,
    OperationStore, ResolvedAuth, ResolvedNamespace, ResolvedSecret, Result, SecretRef, SecretStore, StoredAuditEvent,
    StoredOperation, ToolOutput,
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

#[derive(Debug)]
pub struct MemoryAuditStore {
    next_id: AtomicU64,
    events: Mutex<Vec<StoredAuditEvent>>,
}

impl Default for MemoryAuditStore {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            events: Mutex::new(Vec::new()),
        }
    }
}

impl MemoryAuditStore {
    pub fn snapshot(&self) -> Vec<StoredAuditEvent> {
        match self.events.lock() {
            Ok(events) => events.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn next_audit_event_id(&self) -> Result<AuditEventId> {
        let value = self.next_id.fetch_add(1, Ordering::Relaxed);
        AuditEventId::new(format!("audit_{value:08}"))
    }
}

impl AuditStore for MemoryAuditStore {
    fn record(&self, event: &AuditEvent) -> Result<()> {
        let stored = StoredAuditEvent::from_event(self.next_audit_event_id()?, "memory", event);
        match self.events.lock() {
            Ok(mut events) => events.push(stored),
            Err(poisoned) => poisoned.into_inner().push(stored),
        }

        Ok(())
    }

    fn get(&self, id: &AuditEventId) -> Option<StoredAuditEvent> {
        match self.events.lock() {
            Ok(events) => events.iter().find(|event| event.id == *id).cloned(),
            Err(poisoned) => poisoned.into_inner().iter().find(|event| event.id == *id).cloned(),
        }
    }

    fn list(&self) -> Vec<StoredAuditEvent> {
        self.snapshot()
    }
}

#[derive(Debug)]
pub struct MemoryOperationStore {
    next_id: AtomicU64,
    operations: Mutex<BTreeMap<OperationId, StoredOperation>>,
}

impl Default for MemoryOperationStore {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            operations: Mutex::new(BTreeMap::new()),
        }
    }
}

impl MemoryOperationStore {
    pub fn snapshot(&self) -> Vec<StoredOperation> {
        match self.operations.lock() {
            Ok(operations) => operations.values().cloned().collect(),
            Err(poisoned) => poisoned.into_inner().values().cloned().collect(),
        }
    }

    fn next_operation_id(&self) -> Result<OperationId> {
        let value = self.next_id.fetch_add(1, Ordering::Relaxed);
        OperationId::new(format!("op_{value:08}"))
    }
}

impl OperationStore for MemoryOperationStore {
    fn create(&self, plan: &switchboard_core::PlannedAction) -> Result<StoredOperation> {
        let operation = StoredOperation::from_plan(self.next_operation_id()?, plan);

        match self.operations.lock() {
            Ok(mut operations) => {
                operations.insert(operation.id.clone(), operation.clone());
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(operation.id.clone(), operation.clone());
            }
        }

        Ok(operation)
    }

    fn mark_approved(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
        self.with_operation_mut(id, |operation| {
            operation.approve(actor, note.map(str::to_owned))?;
            Ok(operation.clone())
        })
    }

    fn mark_rejected(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
        self.with_operation_mut(id, |operation| {
            operation.reject(actor, note.map(str::to_owned))?;
            Ok(operation.clone())
        })
    }

    fn mark_applied(&self, id: &OperationId, output: &ToolOutput) -> Result<StoredOperation> {
        self.with_operation_mut(id, |operation| {
            operation.mark_applied(output.effect.clone());
            Ok(operation.clone())
        })
    }

    fn mark_failed(&self, id: &OperationId, reason: &str) -> Result<StoredOperation> {
        self.with_operation_mut(id, |operation| {
            operation.mark_failed(reason)?;
            Ok(operation.clone())
        })
    }

    fn mark_compensated(&self, id: &OperationId) -> Result<StoredOperation> {
        self.with_operation_mut(id, |operation| {
            operation.mark_compensated();
            Ok(operation.clone())
        })
    }

    fn get(&self, id: &OperationId) -> Option<StoredOperation> {
        match self.operations.lock() {
            Ok(operations) => operations.get(id).cloned(),
            Err(poisoned) => poisoned.into_inner().get(id).cloned(),
        }
    }

    fn list(&self) -> Vec<StoredOperation> {
        self.snapshot()
    }
}

impl MemoryOperationStore {
    fn with_operation_mut<T, F>(&self, id: &OperationId, mut update: F) -> Result<T>
    where
        F: FnMut(&mut StoredOperation) -> Result<T>,
    {
        match self.operations.lock() {
            Ok(mut operations) => {
                let operation = operations
                    .get_mut(id)
                    .ok_or_else(|| Error::Operation(format!("unknown operation id: {id}")))?;
                update(operation)
            }
            Err(poisoned) => {
                let mut operations = poisoned.into_inner();
                let operation = operations
                    .get_mut(id)
                    .ok_or_else(|| Error::Operation(format!("unknown operation id: {id}")))?;
                update(operation)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use switchboard_core::{
        ApprovalState, BackendKind, ExecutionMode, NamespaceId, OperationEffect, OperationStatus, OperationStore,
        PlannedAction, PlanningTarget, ProviderKind, ResolvedAuth, ResolvedNamespace, ToolArgument, ToolKind, ToolName,
        ToolOutput, ToolRef, ToolRefKind, ToolRequest,
    };

    use super::MemoryOperationStore;

    #[test]
    fn operation_store_tracks_planned_applied_and_compensated_lifecycle() {
        let store = MemoryOperationStore::default();
        let plan = planned_action();
        let created = store.create(&plan).expect("operation should be created");

        assert_eq!(created.status, OperationStatus::Planned);
        assert_eq!(created.approval.state, ApprovalState::Pending);

        let approved = store
            .mark_approved(&created.id, "codex", Some("ship it"))
            .expect("operation should be approved");
        assert_eq!(approved.approval.state, ApprovalState::Approved);
        assert_eq!(approved.approval.actor.as_deref(), Some("codex"));

        let output = ToolOutput::new(
            ToolName::new("google.calendar.create").expect("tool should build"),
            NamespaceId::new("google.personal").expect("namespace should build"),
            "Created personal calendar event",
        )
        .with_effect(
            OperationEffect::new(true)
                .with_ref(
                    ToolRef::new(
                        ProviderKind::GoogleWorkspace,
                        NamespaceId::new("google.personal").expect("namespace should build"),
                        ToolRefKind::Event,
                        "evt_123",
                    )
                    .expect("tool ref should build"),
                )
                .with_undo_summary("Delete the created calendar event")
                .expect("undo summary should build"),
        );

        let applied = store
            .mark_applied(&created.id, &output)
            .expect("operation should be applied");
        assert_eq!(applied.status, OperationStatus::Applied);
        assert_eq!(applied.effect.as_ref().map(|effect| effect.undoable), Some(true));

        let compensated = store
            .mark_compensated(&created.id)
            .expect("operation should be compensated");
        assert_eq!(compensated.status, OperationStatus::Compensated);
    }

    #[test]
    fn operation_store_tracks_failures() {
        let store = MemoryOperationStore::default();
        let created = store.create(&planned_action()).expect("operation should be created");

        store
            .mark_rejected(&created.id, "codex", Some("not safe"))
            .expect("operation should be rejected");

        let failed = store
            .mark_failed(&created.id, "provider returned 403")
            .expect("operation should be marked failed");

        assert_eq!(failed.status, OperationStatus::Failed);
        assert_eq!(failed.failure_reason.as_deref(), Some("provider returned 403"));
        assert_eq!(failed.approval.state, ApprovalState::Rejected);
    }

    fn planned_action() -> PlannedAction {
        let request = ToolRequest::new(
            "google.calendar.create",
            "google.personal",
            ExecutionMode::Draft,
            vec![
                ToolArgument::option("title", "Dog hotel pickup").expect("title should build"),
                ToolArgument::option("date", "2026-04-01").expect("date should build"),
            ],
        )
        .expect("request should build");
        let target = PlanningTarget {
            namespace: ResolvedNamespace::new(
                "google.personal",
                ProviderKind::GoogleWorkspace,
                "Google personal",
                "google.personal_auth",
                false,
                None,
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "google.personal_auth",
                ProviderKind::GoogleWorkspace,
                switchboard_core::AuthKind::GoogleOAuthFile,
                "me@gmail.com",
                switchboard_core::AuthSecretRefs::GoogleOAuthFile {
                    credentials: switchboard_core::SecretRef::new("google.personal_oauth")
                        .expect("secret ref should build"),
                },
            )
            .expect("auth should build"),
        };

        let mut plan = PlannedAction::new(
            &request,
            &target,
            ToolKind::Write,
            "Create personal calendar event",
            BackendKind::Cli,
        );
        plan.approval_required = true;
        plan.approval_reason = Some("write approval required in tests".into());
        plan
    }
}
