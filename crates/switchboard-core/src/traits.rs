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
    fn get(&self, id: &NamespaceId) -> Option<ResolvedNamespace>;
    fn list(&self) -> Vec<ResolvedNamespace>;
}

pub trait AuthStore: Send + Sync {
    fn get(&self, id: &AuthRef) -> Option<ResolvedAuth>;
    fn list(&self) -> Vec<ResolvedAuth>;
}

pub trait SecretStore: Send + Sync {
    fn get(&self, id: &SecretRef) -> Option<ResolvedSecret>;
    fn list(&self) -> Vec<ResolvedSecret>;
}

pub trait SecretResolver: Send + Sync {
    fn resolve(&self, secret: &ResolvedSecret) -> Result<SecretString>;
}

pub trait PolicyEngine: Send + Sync {
    fn evaluate(&self, namespace: &ResolvedNamespace, plan: &PlannedAction) -> PolicyDecision;
}

pub trait AuditStore: Send + Sync {
    fn record(&self, event: &AuditEvent) -> Result<()>;
    fn get(&self, id: &AuditEventId) -> Option<StoredAuditEvent>;
    fn list(&self) -> Vec<StoredAuditEvent>;
}

pub trait OperationStore: Send + Sync {
    fn create(&self, plan: &PlannedAction) -> Result<StoredOperation>;
    fn mark_approved(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation>;
    fn mark_rejected(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation>;
    fn mark_applied(&self, id: &OperationId, output: &ToolOutput) -> Result<StoredOperation>;
    fn mark_failed(&self, id: &OperationId, reason: &str) -> Result<StoredOperation>;
    fn mark_compensated(&self, id: &OperationId) -> Result<StoredOperation>;
    fn get(&self, id: &OperationId) -> Option<StoredOperation>;
    fn list(&self) -> Vec<StoredOperation>;
}

pub trait Adapter: Send + Sync {
    fn provider(&self) -> ProviderKind;
    fn tools(&self) -> &'static [ToolDescriptor];
    fn plan(
        &self,
        target: &PlanningTarget,
        request: &ToolRequest,
        descriptor: &'static ToolDescriptor,
    ) -> Result<PlannedAction>;
    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput>;

    fn compensation_request(&self, _operation: &StoredOperation, _mode: ExecutionMode) -> Result<Option<ToolRequest>> {
        Ok(None)
    }

    fn find_tool(&self, name: &ToolName) -> Option<&'static ToolDescriptor> {
        self.tools()
            .iter()
            .find(|descriptor| descriptor.name.as_str() == name.as_str())
    }
}
