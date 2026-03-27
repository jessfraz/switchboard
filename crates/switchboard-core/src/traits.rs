use crate::{
    error::Result,
    types::{
        AuditEvent, AuditEventId, AuthRef, ExecutionMode, ExecutionTarget, OperationId, PlannedAction, PlanningTarget,
        PolicyDecision, ProviderKind, ResolvedAuth, ResolvedNamespace, ResolvedSecret, SecretRef, SecretString,
        StoredAuditEvent, StoredOperation, ToolDescriptor, ToolName, ToolOutput, ToolRequest,
    },
    NamespaceId,
};

pub trait NamespaceStore: Send + Sync {
    /// Look up one namespace by its stable id.
    fn get(&self, id: &NamespaceId) -> Option<ResolvedNamespace>;
    /// List every configured namespace.
    fn list(&self) -> Vec<ResolvedNamespace>;
}

pub trait AuthStore: Send + Sync {
    /// Look up one auth entry by its stable id.
    fn get(&self, id: &AuthRef) -> Option<ResolvedAuth>;
    /// List every configured auth entry.
    fn list(&self) -> Vec<ResolvedAuth>;
}

pub trait SecretStore: Send + Sync {
    /// Look up one secret reference by its stable id.
    fn get(&self, id: &SecretRef) -> Option<ResolvedSecret>;
    /// List every configured secret reference.
    fn list(&self) -> Vec<ResolvedSecret>;
}

pub trait SecretResolver: Send + Sync {
    /// Resolve one configured secret into its runtime value.
    fn resolve(&self, secret: &ResolvedSecret) -> Result<SecretString>;
}

pub trait PolicyEngine: Send + Sync {
    /// Evaluate whether a planned action is allowed, denied, or needs approval.
    fn evaluate(&self, namespace: &ResolvedNamespace, plan: &PlannedAction) -> PolicyDecision;
}

pub trait AuditStore: Send + Sync {
    /// Append one immutable audit event.
    fn record(&self, event: &AuditEvent) -> Result<()>;
    /// Look up one audit event by id.
    fn get(&self, id: &AuditEventId) -> Option<StoredAuditEvent>;
    /// List audit events in store-defined order.
    fn list(&self) -> Vec<StoredAuditEvent>;
}

pub trait OperationStore: Send + Sync {
    /// Persist a newly planned write operation.
    fn create(&self, plan: &PlannedAction) -> Result<StoredOperation>;
    /// Mark a planned operation approved.
    fn mark_approved(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation>;
    /// Mark a planned operation rejected.
    fn mark_rejected(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation>;
    /// Mark an approved operation applied and capture its execution receipts.
    fn mark_applied(&self, id: &OperationId, output: &ToolOutput) -> Result<StoredOperation>;
    /// Mark an operation failed during apply.
    fn mark_failed(&self, id: &OperationId, reason: &str) -> Result<StoredOperation>;
    /// Mark an applied operation compensated by a later undo flow.
    fn mark_compensated(&self, id: &OperationId) -> Result<StoredOperation>;
    /// Look up one stored operation by id.
    fn get(&self, id: &OperationId) -> Option<StoredOperation>;
    /// List stored operations in store-defined order.
    fn list(&self) -> Vec<StoredOperation>;
}

pub trait Adapter: Send + Sync {
    /// Return the provider this adapter owns.
    fn provider(&self) -> ProviderKind;
    /// Return the tool catalog exposed by this adapter.
    fn tools(&self) -> &'static [ToolDescriptor];
    /// Turn one tool request into a planned action for this provider.
    fn plan(
        &self,
        target: &PlanningTarget,
        request: &ToolRequest,
        descriptor: &'static ToolDescriptor,
    ) -> Result<PlannedAction>;
    /// Execute one planned action against the provider backend.
    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput>;

    /// Build a compensating request for one previously applied operation, if this adapter can undo it.
    fn compensation_request(&self, _operation: &StoredOperation, _mode: ExecutionMode) -> Result<Option<ToolRequest>> {
        Ok(None)
    }

    /// Find one tool descriptor by name inside this adapter's catalog.
    fn find_tool(&self, name: &ToolName) -> Option<&'static ToolDescriptor> {
        self.tools()
            .iter()
            .find(|descriptor| descriptor.name.as_str() == name.as_str())
    }
}
